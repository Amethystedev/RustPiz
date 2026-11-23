//! Implémentation de `XamlBackend` sur `Windows.UI.Xaml`, et moteur par thread.

use crate::bindings::{
    Application, Binding, BindingExpressionBase, CoreDispatcherPriority, DependencyObject,
    DependencyProperty, DependencyPropertyChangedCallback, DispatchedHandler, FrameworkElement,
    IPropertyValue, PropertyType, PropertyValue, ResourceDictionary, Setter, Style, TypeKind,
    TypeName, VisualStateChangedEventArgs, VisualStateChangedEventHandler, VisualStateGroup,
    VisualStateManager, VisualTreeHelper, XamlBindingHelper, XamlReader,
};
use crate::log;
use rustpiz_protocol::RuntimeConfig;
use rustpiz_theme::expr::Value;
use rustpiz_theme::resource::{ResourceKind, ResourceTheme, ResourceVariable};
use rustpiz_xaml::{Engine, Mode, XamlBackend};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use windows_core::{IInspectable, IUnknown, Interface, Ref, Result as WinResult, HSTRING};

const FALLBACK_TYPE: &str = "Windows.UI.Xaml.FrameworkElement";
const RECTANGLE_TYPE: &str = "Windows.UI.Xaml.Shapes.Rectangle";
/// Échecs XAML détaillés dans `runtime.log` par moteur (thread et session) :
/// au-delà, une seule ligne signale la limite (journal borné).
const MAX_LOGGED_FAILURES: u32 = 20;

thread_local! {
    /// Vrai pendant nos propres écritures, pour ignorer leurs notifications.
    static MODIFYING: Cell<bool> = const { Cell::new(false) };
    static ENGINE: RefCell<Option<(u64, Engine<WinBackend>)>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static HANDLES: RefCell<HashMap<u64, usize>> = RefCell::new(HashMap::new());
    static RESOURCES: RefCell<Option<ResourceJournal>> = const { RefCell::new(None) };
}

/// Rend un objet XAML transportable vers un délégué exécuté sur son thread.
struct UiThreadOnly<T>(T);
unsafe impl<T> Send for UiThreadOnly<T> {}
unsafe impl<T> Sync for UiThreadOnly<T> {}

impl<T> UiThreadOnly<T> {
    /// Passe par une méthode pour que la closure capture l'enveloppe entière.
    fn get(&self) -> &T {
        &self.0
    }
}

enum Event {
    Added(u64, FrameworkElement),
    Removed(u64),
    State(usize, String),
    Property(usize, DependencyProperty),
}

/// Valeur prête à être écrite.
pub enum Parsed {
    Clear,
    Object(Option<IInspectable>),
}

/// Valeur à remettre lors du rollback.
pub enum Original {
    Unset,
    Null,
    Value(IInspectable),
    Binding(Binding),
}

pub enum Token {
    Property(DependencyObject, DependencyProperty, i64),
    Group(VisualStateGroup, i64),
}

/// Une référence nulle remonte en erreur de code 0 avec windows-rs.
fn nullable<T>(result: WinResult<T>) -> WinResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(error),
    }
}

fn text(error: windows_core::Error) -> String {
    format!("{:08X} {}", error.code().0, error.message())
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Construit le dictionnaire XAML à une seule règle de style.
fn style_document(class: &str, setter: &str) -> String {
    let head = "<ResourceDictionary \
        xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\" \
        xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" \
        xmlns:d=\"http://schemas.microsoft.com/expression/blend/2008\" \
        xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
        xmlns:muxc=\"using:Microsoft.UI.Xaml.Controls\"";
    let (extra, target) = match class.rsplit_once('.') {
        Some((namespace, name)) => (
            format!(" xmlns:rh=\"using:{}\"", escape_attribute(namespace)),
            format!("rh:{}", escape_attribute(name)),
        ),
        None => (String::new(), escape_attribute(class)),
    };
    format!("{head}{extra}><Style TargetType=\"{target}\">{setter}</Style></ResourceDictionary>")
}

fn load_setter(class: &str, setter: &str) -> WinResult<Setter> {
    let document = HSTRING::from(style_document(class, setter));
    let dictionary: ResourceDictionary = XamlReader::Load(&document)?.cast()?;
    let style: Style = dictionary.First()?.Current()?.Value()?.cast()?;
    style.Setters()?.GetAt(0)?.cast()
}

/// Essaie le type réel, puis `FrameworkElement` pour les types internes.
fn load_setter_with_fallback(class: &str, setter: &str) -> Result<Setter, String> {
    load_setter(class, setter)
        .or_else(|_| load_setter(FALLBACK_TYPE, setter))
        .map_err(text)
}

fn setter_markup(property: &str, value: &str, is_xaml: bool) -> String {
    let property = escape_attribute(property);
    if is_xaml && value.is_empty() {
        format!("<Setter Property=\"{property}\" Value=\"{{x:Null}}\" />")
    } else if is_xaml {
        format!("<Setter Property=\"{property}\"><Setter.Value>{value}</Setter.Value></Setter>")
    } else {
        format!(
            "<Setter Property=\"{property}\" Value=\"{}\" />",
            escape_attribute(value)
        )
    }
}

fn identity(object: &impl Interface) -> usize {
    object
        .cast::<IUnknown>()
        .map_or(0, |unknown| unknown.as_raw() as usize)
}

fn as_primitive(value: &IInspectable) -> Option<Value> {
    let property: IPropertyValue = value.cast().ok()?;
    let kind = property.Type().ok()?;
    let number = |n: f64| Some(Value::Num(n));
    match kind {
        PropertyType::String => property.GetString().ok().map(|s| Value::Str(s.to_string())),
        PropertyType::Boolean => number(f64::from(u8::from(property.GetBoolean().ok()?))),
        PropertyType::Double => number(property.GetDouble().ok()?),
        PropertyType::Single => number(f64::from(property.GetSingle().ok()?)),
        PropertyType::Int32 => number(f64::from(property.GetInt32().ok()?)),
        PropertyType::UInt32 => number(f64::from(property.GetUInt32().ok()?)),
        PropertyType::Int64 => number(property.GetInt64().ok()? as f64),
        PropertyType::UInt8 => number(f64::from(property.GetUInt8().ok()?)),
        _ => None,
    }
}

/// Accès réel à XAML pour un thread.
pub struct WinBackend {
    properties: HashMap<(String, String), DependencyProperty>,
    values: HashMap<(String, String, String), IInspectable>,
    unset: Option<IInspectable>,
    /// Échecs déjà détaillés dans le journal (borné à [`MAX_LOGGED_FAILURES`]).
    logged_failures: u32,
}

impl WinBackend {
    fn new() -> Self {
        Self {
            properties: HashMap::new(),
            values: HashMap::new(),
            unset: DependencyProperty::UnsetValue().ok(),
            logged_failures: 0,
        }
    }

    /// Nom d'une propriété déjà résolue (recherche inverse dans le cache).
    fn property_name(&self, property: &DependencyProperty) -> String {
        self.properties
            .iter()
            .find(|(_, dp)| *dp == property)
            .map_or_else(|| "?".to_owned(), |((_, name), _)| name.clone())
    }

    /// Détaille un échec XAML dans `runtime.log`, en nombre borné.
    fn log_failure(&mut self, step: &str, element: &FrameworkElement, property: &str, message: &str) {
        self.logged_failures = self.logged_failures.saturating_add(1);
        if self.logged_failures > MAX_LOGGED_FAILURES {
            if self.logged_failures == MAX_LOGGED_FAILURES + 1 {
                log::write(format!(
                    "session {} : plus de {MAX_LOGGED_FAILURES} échecs XAML sur ce thread, \
                     les suivants ne sont plus détaillés (compteurs dans le rapport d'état)",
                    crate::control::generation()
                ));
            }
            return;
        }
        let name = element
            .Name()
            .map_or_else(|_| String::new(), |n| n.to_string());
        let name = if name.is_empty() { String::new() } else { format!("#{name}") };
        let message: String = message.chars().take(300).collect();
        log::write(format!(
            "session {} : échec {step} — élément {}{name}, propriété {property} : {message}",
            crate::control::generation(),
            class_of(element)
        ));
    }

    fn write(
        element: &DependencyObject,
        property: &DependencyProperty,
        value: &Parsed,
    ) -> WinResult<()> {
        let previous = MODIFYING.replace(true);
        let result = match value {
            Parsed::Clear => element.ClearValue(property),
            Parsed::Object(Some(object)) => element.SetValue(property, object),
            Parsed::Object(None) => element.SetValue(property, None::<&IInspectable>),
        };
        MODIFYING.set(previous);
        result
    }

    fn find_group(owner: &FrameworkElement, group: &str) -> Option<VisualStateGroup> {
        // Ces deux éléments plantent explorer quand on lit leurs groupes.
        let class = class_of(owner);
        let parent = parent_of(owner).map(|p| class_of(&p)).unwrap_or_default();
        if (class == "Taskbar.TaskListButtonPanel" && parent == "Taskbar.SearchBoxLaunchListButton")
            || (class == "SearchUx.SearchUI.SearchButtonRootGrid"
                && parent == "SearchUx.SearchUI.SearchPillButton")
        {
            return None;
        }
        let groups = VisualStateManager::GetVisualStateGroups(owner).ok()?;
        (0..groups.Size().ok()?)
            .filter_map(|index| groups.GetAt(index).ok())
            .find(|candidate| candidate.Name().is_ok_and(|name| name == group))
    }
}

fn class_of(element: &FrameworkElement) -> String {
    element
        .cast::<IInspectable>()
        .and_then(|i| i.GetRuntimeClassName())
        .map_or_else(|_| String::new(), |name| name.to_string())
}

fn parent_of(element: &FrameworkElement) -> Option<FrameworkElement> {
    VisualTreeHelper::GetParent(element).ok()?.cast().ok()
}

impl XamlBackend for WinBackend {
    type Element = FrameworkElement;
    type Key = usize;
    type Property = DependencyProperty;
    type Value = Parsed;
    type Original = Original;
    type Token = Token;

    fn key(&self, element: &FrameworkElement) -> usize {
        identity(element)
    }

    fn class_name(&self, element: &FrameworkElement) -> String {
        class_of(element)
    }

    fn element_name(&self, element: &FrameworkElement) -> String {
        element
            .Name()
            .map_or_else(|_| String::new(), |name| name.to_string())
    }

    fn parent(&self, element: &FrameworkElement) -> Option<FrameworkElement> {
        parent_of(element)
    }

    fn child_index(&self, element: &FrameworkElement) -> Option<u32> {
        let parent = VisualTreeHelper::GetParent(element).ok()?;
        let count = VisualTreeHelper::GetChildrenCount(&parent).ok()?;
        let own = identity(element);
        (0..count)
            .find(|&i| VisualTreeHelper::GetChild(&parent, i).is_ok_and(|c| identity(&c) == own))
            .map(|i| i as u32)
    }

    fn local_value_equals(
        &mut self,
        element: &FrameworkElement,
        class: &str,
        property: &str,
        expected: &str,
    ) -> bool {
        let Ok(dp) = self.resolve_property(element, class, property) else {
            return false;
        };
        let Ok(Parsed::Object(Some(wanted))) =
            self.parse_value(element, class, property, expected, false)
        else {
            return false;
        };
        let Ok(object) = element.cast::<DependencyObject>() else {
            return false;
        };
        let Ok(Some(actual)) = nullable(object.ReadLocalValue(&dp)) else {
            return false;
        };
        if self.unset.as_ref() == Some(&actual) {
            return false;
        }
        matches!((as_primitive(&actual), as_primitive(&wanted)), (Some(a), Some(b)) if a == b)
    }

    fn visual_state(&mut self, owner: &FrameworkElement, group: &str) -> Option<String> {
        let group = Self::find_group(owner, group)?;
        let state = nullable(group.CurrentState()).ok()?;
        Some(
            state
                .and_then(|s| s.Name().ok())
                .map_or_else(String::new, |n| n.to_string()),
        )
    }

    fn resolve_property(
        &mut self,
        element: &FrameworkElement,
        class: &str,
        property: &str,
    ) -> Result<DependencyProperty, String> {
        let key = (class.to_owned(), property.to_owned());
        if let Some(found) = self.properties.get(&key) {
            return Ok(found.clone());
        }
        let resolved = load_setter_with_fallback(class, &setter_markup(property, "", true))
            .and_then(|setter| setter.Property().map_err(text));
        match resolved {
            Ok(resolved) => {
                self.properties.insert(key, resolved.clone());
                Ok(resolved)
            }
            Err(message) => {
                self.log_failure("de résolution (Setter)", element, property, &message);
                Err(message)
            }
        }
    }

    fn parse_value(
        &mut self,
        element: &FrameworkElement,
        class: &str,
        property: &str,
        value: &str,
        is_xaml: bool,
    ) -> Result<Parsed, String> {
        if is_xaml && value.is_empty() {
            return Ok(Parsed::Clear);
        }
        // Les objets XAML (pinceaux…) sont propres à chaque élément.
        let key = (class.to_owned(), property.to_owned(), value.to_owned());
        if !is_xaml {
            if let Some(cached) = self.values.get(&key) {
                return Ok(Parsed::Object(Some(cached.clone())));
            }
        }
        let parsed = load_setter_with_fallback(class, &setter_markup(property, value, is_xaml))
            .and_then(|setter| nullable(setter.Value()).map_err(text));
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(message) => {
                let shown: String = value.chars().take(120).collect();
                self.log_failure(
                    "d'analyse de la valeur",
                    element,
                    property,
                    &format!("{message} (valeur : {shown})"),
                );
                return Err(message);
            }
        };
        if let (false, Some(object)) = (is_xaml, &parsed) {
            if object.cast::<DependencyObject>().is_err() {
                self.values.insert(key, object.clone());
            }
        }
        Ok(Parsed::Object(parsed))
    }

    fn read_original(
        &mut self,
        element: &FrameworkElement,
        property: &DependencyProperty,
    ) -> Result<Original, String> {
        let object: DependencyObject = element.cast().map_err(text)?;
        let local = match nullable(object.ReadLocalValue(property)).map_err(text) {
            Ok(local) => local,
            Err(message) => {
                let name = self.property_name(property);
                self.log_failure("de lecture de l'origine", element, &name, &message);
                return Err(message);
            }
        };
        let bound = local
            .as_ref()
            .is_none_or(|value| value.cast::<BindingExpressionBase>().is_ok());
        if !bound {
            let value = local.ok_or("valeur locale absente")?;
            return Ok(if self.unset.as_ref() == Some(&value) {
                Original::Unset
            } else {
                Original::Value(value)
            });
        }
        if let Ok(Some(expression)) = nullable(element.GetBindingExpression(property)) {
            if let Ok(binding) = expression.ParentBinding() {
                return Ok(Original::Binding(binding));
            }
        }
        // TemplateBinding : on remet sa valeur courante (voir agent.md, limites).
        Ok(
            match nullable(object.GetAnimationBaseValue(property)).map_err(text)? {
                Some(value) => Original::Value(value),
                None => Original::Null,
            },
        )
    }

    fn set_value(
        &mut self,
        element: &FrameworkElement,
        property: &DependencyProperty,
        value: &Parsed,
        initial: bool,
    ) -> Result<(), String> {
        let object: DependencyObject = element.cast().map_err(text)?;
        let fill = self
            .properties
            .get(&(RECTANGLE_TYPE.to_owned(), "Fill".to_owned()));
        // Écrit trop tôt, ce Fill plante explorer ou est écrasé par
        // TaskbarBackground::OnApplyTemplate : l'écriture est différée.
        if initial
            && fill == Some(property)
            && class_of(element) == RECTANGLE_TYPE
            && element.Name().is_ok_and(|name| name == "BackgroundFill")
        {
            if let Parsed::Object(content) = value {
                return defer_set(&object, property, content.clone());
            }
        }
        Self::write(&object, property, value).map_err(text).inspect_err(|message| {
            let name = self.property_name(property);
            self.log_failure("de SetValue", element, &name, message);
        })
    }

    fn restore(
        &mut self,
        element: &FrameworkElement,
        property: &DependencyProperty,
        original: Original,
    ) -> Result<(), String> {
        let object: DependencyObject = element.cast().map_err(text)?;
        let result = match original {
            Original::Unset => Self::write(&object, property, &Parsed::Clear),
            Original::Null => Self::write(&object, property, &Parsed::Object(None)),
            Original::Value(value) => Self::write(&object, property, &Parsed::Object(Some(value))),
            Original::Binding(binding) => {
                let previous = MODIFYING.replace(true);
                let outcome = element.SetBinding(property, &binding);
                MODIFYING.set(previous);
                outcome
            }
        };
        result.map_err(text)
    }

    fn read_primitive(
        &mut self,
        element: &FrameworkElement,
        property: &DependencyProperty,
    ) -> Option<Value> {
        let object: DependencyObject = element.cast().ok()?;
        as_primitive(&object.GetValue(property).ok()?)
    }

    fn watch_property(
        &mut self,
        element: &FrameworkElement,
        property: &DependencyProperty,
        key: &usize,
    ) -> Option<Token> {
        let object: DependencyObject = element.cast().ok()?;
        let key = *key;
        let callback = DependencyPropertyChangedCallback::new(
            move |_sender: Ref<DependencyObject>, changed: Ref<DependencyProperty>| {
                if !MODIFYING.get() {
                    if let Ok(changed) = changed.ok() {
                        dispatch(Event::Property(key, changed.clone()));
                    }
                }
                Ok(())
            },
        );
        let token = object
            .RegisterPropertyChangedCallback(property, &callback)
            .ok()?;
        Some(Token::Property(object, property.clone(), token))
    }

    fn watch_group(&mut self, owner: &FrameworkElement, group: &str, key: &usize) -> Option<Token> {
        let found = Self::find_group(owner, group)?;
        let (key, name) = (*key, group.to_owned());
        let handler = VisualStateChangedEventHandler::new(
            move |_sender: Ref<IInspectable>, _args: Ref<VisualStateChangedEventArgs>| {
                dispatch(Event::State(key, name.clone()));
                Ok(())
            },
        );
        let token = found.CurrentStateChanged(&handler).ok()?;
        Some(Token::Group(found, token))
    }

    fn unwatch(&mut self, token: Token) {
        match token {
            Token::Property(object, property, id) => {
                let _ = object.UnregisterPropertyChangedCallback(&property, id);
            }
            Token::Group(group, id) => {
                let _ = group.RemoveCurrentStateChanged(id);
            }
        }
    }
}

fn defer_set(
    object: &DependencyObject,
    property: &DependencyProperty,
    value: Option<IInspectable>,
) -> Result<(), String> {
    let generation = crate::control::generation();
    let captured = UiThreadOnly((object.clone(), property.clone(), value));
    let handler = DispatchedHandler::new(move || {
        if crate::control::generation() == generation && crate::control::is_active() {
            let (object, property, value) = captured.get();
            if let Err(error) = WinBackend::write(object, property, &Parsed::Object(value.clone())) {
                log::write(format!(
                    "session {generation} : écriture différée de BackgroundFill.Fill en échec : {}",
                    text(error)
                ));
            }
        }
        Ok(())
    });
    object
        .Dispatcher()
        .and_then(|d| d.RunAsync(CoreDispatcherPriority::High, &handler))
        .map(|_| ())
        .map_err(text)
        .inspect_err(|message| {
            log::write(format!(
                "session {generation} : écriture différée de BackgroundFill.Fill non planifiée : {message}"
            ));
        })
}

// ---- Variables de ressources ----

/// Ce qu'il faut défaire pour rendre les ressources d'origine.
struct ResourceJournal {
    resources: ResourceDictionary,
    /// Dictionnaire fusionné pour les clés `@Dark` et `@Light`.
    themed: Option<ResourceDictionary>,
    originals: Vec<(IInspectable, IInspectable)>,
}

fn boxed(text: &str) -> WinResult<IInspectable> {
    PropertyValue::CreateString(&HSTRING::from(text))
}

/// Analyse une valeur XAML en réutilisant le mécanisme des `Setter`.
fn parse_xaml_resource(value: &str) -> Result<Option<IInspectable>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let setter = load_setter_with_fallback(FALLBACK_TYPE, &setter_markup("Tag", value, true))?;
    nullable(setter.Value()).map_err(text)
}

/// Convertit le texte vers le type de la ressource remplacée.
fn convert_to_existing_type(existing: &IInspectable, value: &str) -> WinResult<IInspectable> {
    let class = existing.GetRuntimeClassName()?.to_string();
    let inner = class
        .strip_prefix("Windows.Foundation.IReference`1<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(&class);
    let target = TypeName {
        Name: HSTRING::from(inner),
        Kind: TypeKind::Primitive,
    };
    XamlBindingHelper::ConvertValue(&target, &boxed(value)?)
}

fn resource_value(
    resources: &ResourceDictionary,
    variable: &ResourceVariable,
    existing: Option<&IInspectable>,
) -> Result<Option<IInspectable>, String> {
    match (variable.kind, existing) {
        (ResourceKind::Xaml, _) => parse_xaml_resource(&variable.value),
        (ResourceKind::ThemeReference, _) => boxed(&variable.value)
            .and_then(|key| resources.Lookup(&key))
            .map(Some)
            .map_err(text),
        (ResourceKind::String, Some(existing)) => {
            convert_to_existing_type(existing, &variable.value)
                .map(Some)
                .map_err(text)
        }
        (ResourceKind::String, None) => boxed(&variable.value).map(Some).map_err(text),
    }
}

/// Applique une variable. Vrai si elle vise le dictionnaire clair ou sombre.
fn apply_resource(
    resources: &ResourceDictionary,
    dark: &ResourceDictionary,
    light: &ResourceDictionary,
    variable: &ResourceVariable,
    seen: &mut HashSet<(String, u8)>,
    originals: &mut Vec<(IInspectable, IInspectable)>,
) -> Result<bool, String> {
    let key = boxed(&variable.key).map_err(text)?;
    let slot = match variable.theme {
        ResourceTheme::None => 0,
        ResourceTheme::Dark => 1,
        ResourceTheme::Light => 2,
    };
    // La liste est parcourue à l'envers : la dernière définition l'emporte.
    if !seen.insert((variable.key.clone(), slot)) {
        return Ok(false);
    }
    let target = match variable.theme {
        ResourceTheme::Dark => dark,
        ResourceTheme::Light => light,
        ResourceTheme::None => {
            if !resources.HasKey(&key).map_err(text)? {
                return Err("ressource inconnue".to_owned());
            }
            let existing = resources.Lookup(&key).map_err(text)?;
            let value = resource_value(resources, variable, Some(&existing))?;
            resources.Insert(&key, value.as_ref()).map_err(text)?;
            originals.push((key, existing));
            return Ok(false);
        }
    };
    let value = resource_value(resources, variable, None)?;
    target.Insert(&key, value.as_ref()).map_err(text)?;
    Ok(true)
}

fn merge_resources(variables: &[ResourceVariable]) -> WinResult<ResourceJournal> {
    let resources = Application::Current()?.Resources()?;
    let (dark, light) = (ResourceDictionary::new()?, ResourceDictionary::new()?);
    let mut seen = HashSet::new();
    let mut originals = Vec::new();
    let mut has_themed = false;
    for variable in variables.iter().rev() {
        match apply_resource(
            &resources,
            &dark,
            &light,
            variable,
            &mut seen,
            &mut originals,
        ) {
            Ok(themed) => has_themed |= themed,
            Err(message) => log::write(format!("ressource {} : {message}", variable.key)),
        }
    }
    let mut themed = None;
    if has_themed {
        let dictionary = ResourceDictionary::new()?;
        let theme_dictionaries = dictionary.ThemeDictionaries()?;
        theme_dictionaries.Insert(&boxed("Dark")?, &dark.cast::<IInspectable>()?)?;
        theme_dictionaries.Insert(&boxed("Light")?, &light.cast::<IInspectable>()?)?;
        resources.MergedDictionaries()?.Append(&dictionary)?;
        themed = Some(dictionary);
    }
    Ok(ResourceJournal {
        resources,
        themed,
        originals,
    })
}

/// Fusionne les ressources sur ce thread, ou les rétablit si XAML les a remplacées.
fn ensure_resources(variables: &[ResourceVariable]) {
    if variables.is_empty() {
        return;
    }
    RESOURCES.with(|cell| {
        let mut slot = cell.borrow_mut();
        let current = Application::Current().and_then(|a| a.Resources()).ok();
        let replaced = slot
            .as_ref()
            .is_none_or(|j| current.as_ref() != Some(&j.resources));
        if replaced {
            match merge_resources(variables) {
                Ok(journal) => *slot = Some(journal),
                Err(error) => log::write(format!("variables de ressources : {error}")),
            }
            return;
        }
        if let Some(ResourceJournal {
            resources,
            themed: Some(dictionary),
            ..
        }) = slot.as_ref()
        {
            if let Ok(merged) = resources.MergedDictionaries() {
                let mut index = 0;
                if matches!(merged.IndexOf(dictionary, &mut index), Ok(false)) {
                    let _ = merged.Append(dictionary);
                }
            }
        }
    });
}

/// Rend les valeurs d'origine des ressources de ce thread.
fn restore_resources() {
    let Some(journal) = RESOURCES.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    for (key, original) in &journal.originals {
        if let Err(error) = journal.resources.Insert(key, original) {
            log::write(format!("restauration d'une ressource : {error}"));
        }
    }
    if let Some(dictionary) = &journal.themed {
        if let Ok(merged) = journal.resources.MergedDictionaries() {
            let mut index = 0;
            if matches!(merged.IndexOf(dictionary, &mut index), Ok(true)) {
                let _ = merged.RemoveAt(index);
            }
        }
    }
}

/// Enfile un événement puis traite la file si le moteur est libre.
fn dispatch(event: Event) {
    QUEUE.with(|queue| queue.borrow_mut().push_back(event));
    drain();
}

fn drain() {
    if !crate::control::is_active() {
        return;
    }
    let Some(config) = crate::control::config() else {
        return;
    };
    let generation = crate::control::generation();
    ENGINE.with(|cell| {
        // Occupé : l'appel en cours dépilera cet événement.
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        if slot.as_ref().is_none_or(|(g, _)| *g != generation) {
            if let Some((old, _)) = slot.as_ref() {
                // Moteur d'une session précédente jamais restauré (thread
                // injoignable au moment du rollback) : ses valeurs restent.
                log::write(format!(
                    "session {generation} : moteur de la session {old} encore présent sur ce thread \
                     (rollback manqué) ; il est abandonné sans restauration"
                ));
            }
            *slot = Some((generation, new_engine(&config)));
            log::write(format!(
                "session {generation} : application du thème {:?} commencée sur le thread XAML {} \
                 ({} règles, mode {:?})",
                config.theme.name,
                unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                config.theme.rules.len(),
                config.mode
            ));
        }
        let Some((_, engine)) = slot.as_mut() else {
            return;
        };
        while let Some(event) = QUEUE.with(|queue| queue.borrow_mut().pop_front()) {
            // Avant chaque élément : les styles peuvent référencer ces ressources.
            if config.mode == Mode::Apply && matches!(event, Event::Added(..)) {
                ensure_resources(&config.theme.resources);
            }
            process(engine, event);
        }
    });
}

fn new_engine(config: &Arc<RuntimeConfig>) -> Engine<WinBackend> {
    Engine::new(WinBackend::new(), config.theme.rules.clone(), config.mode)
}

fn process(engine: &mut Engine<WinBackend>, event: Event) {
    match event {
        Event::Added(handle, element) => {
            let key = identity(&element);
            engine.on_element_added(element);
            if engine.is_tracked(&key) {
                HANDLES.with(|map| map.borrow_mut().insert(handle, key));
            }
        }
        Event::Removed(handle) => {
            if let Some(key) = HANDLES.with(|map| map.borrow_mut().remove(&handle)) {
                engine.on_element_removed(&key);
            }
        }
        Event::State(key, group) => engine.on_state_changed(&key, &group),
        Event::Property(key, property) => engine.on_property_changed(&key, &property),
    }
}

/// Élément ajouté à l'arbre visuel, sur le thread XAML.
pub fn element_added(handle: u64, element: FrameworkElement) {
    dispatch(Event::Added(handle, element));
}

/// Élément retiré de l'arbre visuel, sur le thread XAML.
pub fn element_removed(handle: u64) {
    dispatch(Event::Removed(handle));
}

fn fill_report(engine: &Engine<WinBackend>, report: &mut rustpiz_protocol::StatusReport) {
    report.dry_run = engine.mode() == Mode::DryRun;
    report.ui_threads = 1;
    report.tracked_elements = engine.tracked_count() as u32;
    report.write_failures = engine.write_failures();
    report.tripped = engine.tripped();
    report.rules = engine.stats().to_vec();
}

/// Rapport du thread courant. `None` si le moteur est occupé ou absent.
pub fn thread_report() -> Option<rustpiz_protocol::StatusReport> {
    ENGINE.with(|cell| {
        let slot = cell.try_borrow().ok()?;
        let (_, engine) = slot.as_ref()?;
        let mut report = rustpiz_protocol::StatusReport::default();
        fill_report(engine, &mut report);
        Some(report)
    })
}

/// Restaure tout sur le thread courant et détruit son moteur.
pub fn thread_rollback() -> Option<rustpiz_protocol::StatusReport> {
    ENGINE.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            log::write("rollback impossible : moteur occupé sur ce thread");
            return Some(rustpiz_protocol::StatusReport {
                unresponsive_threads: 1,
                ..Default::default()
            });
        };
        let (_, mut engine) = slot.take()?;
        let mut report = rustpiz_protocol::StatusReport::default();
        fill_report(&engine, &mut report);
        engine.rollback_all();
        restore_resources();
        QUEUE.with(|queue| queue.borrow_mut().clear());
        HANDLES.with(|map| map.borrow_mut().clear());
        Some(report)
    })
}
