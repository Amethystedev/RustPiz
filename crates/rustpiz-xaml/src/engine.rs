use rustpiz_theme::expr::{EvalError, Template, Value};
use rustpiz_theme::selector::{ElementMatcher, Matcher};
use rustpiz_theme::CompiledRule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

/// Nombre d'échecs d'écriture au-delà duquel tout est restauré.
pub const MAX_WRITE_FAILURES: u32 = 64;

/// Profondeur maximale des recalculs en cascade des variables.
const MAX_REFRESH_DEPTH: u32 = 8;

/// Accès à l'arbre visuel. Toutes les méthodes s'exécutent sur le thread XAML.
pub trait XamlBackend {
    type Element: Clone;
    type Key: Clone + Eq + Hash;
    type Property: Clone + PartialEq;
    type Value;
    type Original;
    type Token;

    fn key(&self, element: &Self::Element) -> Self::Key;
    fn class_name(&self, element: &Self::Element) -> String;
    fn element_name(&self, element: &Self::Element) -> String;
    fn parent(&self, element: &Self::Element) -> Option<Self::Element>;
    /// Position 0-based parmi les enfants visuels du parent.
    fn child_index(&self, element: &Self::Element) -> Option<u32>;
    fn local_value_equals(
        &mut self,
        element: &Self::Element,
        class: &str,
        property: &str,
        expected: &str,
    ) -> bool;
    /// `None` si le groupe n'existe pas ; `Some("")` s'il n'a pas d'état courant.
    fn visual_state(&mut self, owner: &Self::Element, group: &str) -> Option<String>;
    fn resolve_property(
        &mut self,
        element: &Self::Element,
        class: &str,
        property: &str,
    ) -> Result<Self::Property, String>;
    fn parse_value(
        &mut self,
        element: &Self::Element,
        class: &str,
        property: &str,
        value: &str,
        is_xaml: bool,
    ) -> Result<Self::Value, String>;
    fn read_original(
        &mut self,
        element: &Self::Element,
        property: &Self::Property,
    ) -> Result<Self::Original, String>;
    fn set_value(
        &mut self,
        element: &Self::Element,
        property: &Self::Property,
        value: &Self::Value,
        initial: bool,
    ) -> Result<(), String>;
    fn restore(
        &mut self,
        element: &Self::Element,
        property: &Self::Property,
        original: Self::Original,
    ) -> Result<(), String>;
    fn read_primitive(
        &mut self,
        element: &Self::Element,
        property: &Self::Property,
    ) -> Option<Value>;
    fn watch_property(
        &mut self,
        element: &Self::Element,
        property: &Self::Property,
        key: &Self::Key,
    ) -> Option<Self::Token>;
    fn watch_group(
        &mut self,
        owner: &Self::Element,
        group: &str,
        key: &Self::Key,
    ) -> Option<Self::Token>;
    fn unwatch(&mut self, token: Self::Token);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Applique réellement les valeurs.
    Apply,
    /// Parcourt et compile les valeurs sans rien écrire.
    DryRun,
}

/// Compteurs d'une règle, pour détecter les sélecteurs devenus obsolètes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleStat {
    pub source_index: usize,
    pub target: String,
    pub matched: u32,
    pub applied: u32,
    pub failed: u32,
    pub last_error: Option<String>,
}

/// Élément portant le groupe d'états d'une règle, s'il y en a un.
type GroupOwner<B> = Option<(<B as XamlBackend>::Element, String)>;

#[derive(Clone)]
struct Spec {
    value: String,
    is_xaml: bool,
    template: Option<Rc<Template>>,
    rule: usize,
}

struct Entry<B: XamlBackend> {
    name: String,
    property: B::Property,
    owner: Option<(B::Element, String)>,
    specs: HashMap<String, Spec>,
    original: Option<B::Original>,
    /// Dernière valeur rendue et écrite (ou simulée en dry-run).
    applied: Option<String>,
    last_rule: usize,
    token: Option<B::Token>,
}

struct CaptureSlot<B: XamlBackend> {
    variable: String,
    property: B::Property,
    token: Option<B::Token>,
}

struct Tracked<B: XamlBackend> {
    element: B::Element,
    class: String,
    entries: Vec<Entry<B>>,
    captures: Vec<CaptureSlot<B>>,
    group_tokens: Vec<B::Token>,
}

struct CaptureRecord<K> {
    key: K,
    sequence: u64,
    value: Option<Value>,
}

struct RuntimeRule {
    rule: CompiledRule,
    templates: Vec<Option<Rc<Template>>>,
}

/// État du moteur pour un thread XAML.
pub struct Engine<B: XamlBackend> {
    backend: B,
    rules: Vec<RuntimeRule>,
    mode: Mode,
    tracked: HashMap<B::Key, Tracked<B>>,
    captures: HashMap<String, Vec<CaptureRecord<B::Key>>>,
    stats: Vec<RuleStat>,
    write_failures: u32,
    tripped: bool,
    stopped: bool,
    sequence: u64,
    refresh_depth: u32,
}

impl<B: XamlBackend> Engine<B> {
    pub fn new(backend: B, rules: Vec<CompiledRule>, mode: Mode) -> Self {
        let stats = rules
            .iter()
            .map(|rule| RuleStat {
                source_index: rule.source_index,
                target: rule.selector.raw.clone(),
                ..RuleStat::default()
            })
            .collect();
        let rules = rules
            .into_iter()
            .map(|rule| {
                let templates = rule
                    .values
                    .iter()
                    .map(|value| {
                        value
                            .is_dynamic()
                            .then(|| Template::parse(&value.value).ok().map(Rc::new))
                            .flatten()
                    })
                    .collect();
                RuntimeRule { rule, templates }
            })
            .collect();
        Self {
            backend,
            rules,
            mode,
            tracked: HashMap::new(),
            captures: HashMap::new(),
            stats,
            write_failures: 0,
            tripped: false,
            stopped: false,
            sequence: 0,
            refresh_depth: 0,
        }
    }

    pub fn stats(&self) -> &[RuleStat] {
        &self.stats
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Vrai si le disjoncteur a tout restauré après trop d'échecs.
    pub fn tripped(&self) -> bool {
        self.tripped
    }

    pub fn write_failures(&self) -> u32 {
        self.write_failures
    }

    pub fn is_tracked(&self, key: &B::Key) -> bool {
        self.tracked.contains_key(key)
    }

    pub fn tracked_count(&self) -> usize {
        self.tracked.len()
    }

    fn matches_element(
        &mut self,
        element: &B::Element,
        matcher: &ElementMatcher,
        class: Option<&str>,
    ) -> bool {
        let owned;
        let class = match class {
            Some(class) => class,
            None => {
                owned = self.backend.class_name(element);
                &owned
            }
        };
        if matcher.type_name != class {
            return false;
        }
        if let Some(name) = &matcher.name {
            if *name != self.backend.element_name(element) {
                return false;
            }
        }
        if matcher.index != 0 && self.backend.child_index(element) != Some(matcher.index - 1) {
            return false;
        }
        for (property, expected) in &matcher.properties {
            if !self
                .backend
                .local_value_equals(element, class, property, expected)
            {
                return false;
            }
        }
        true
    }

    /// Teste les ancêtres, avec retour arrière pour `*`.
    fn matches_ancestors(
        &mut self,
        ancestors: &[Matcher],
        current: Option<B::Element>,
        owner: &mut Option<(B::Element, String)>,
    ) -> bool {
        let Some((first, rest)) = ancestors.split_first() else {
            return true;
        };
        match first {
            Matcher::Root => current.is_none(),
            Matcher::Wildcard => {
                let mut candidate = current;
                loop {
                    if self.matches_ancestors(rest, candidate.clone(), owner) {
                        return true;
                    }
                    match candidate {
                        Some(element) => candidate = self.backend.parent(&element),
                        None => return false,
                    }
                }
            }
            Matcher::Element(matcher) => {
                let Some(element) = current else {
                    return false;
                };
                if !self.matches_element(&element, matcher, None) {
                    return false;
                }
                let parent = self.backend.parent(&element);
                if !self.matches_ancestors(rest, parent, owner) {
                    return false;
                }
                if let Some(group) = &matcher.state_group {
                    *owner = Some((element, group.clone()));
                }
                true
            }
        }
    }

    /// Cherche les règles qui visent l'élément.
    fn find_matches(&mut self, element: &B::Element, class: &str) -> Vec<(usize, GroupOwner<B>)> {
        let mut matched = Vec::new();
        for index in 0..self.rules.len() {
            let leaf = self.rules[index].rule.selector.leaf.clone();
            if !self.matches_element(element, &leaf, Some(class)) {
                continue;
            }
            let ancestors = self.rules[index].rule.selector.ancestors.clone();
            let parent = self.backend.parent(element);
            let mut owner = None;
            if !self.matches_ancestors(&ancestors, parent, &mut owner) {
                continue;
            }
            if let Some(group) = &leaf.state_group {
                owner = Some((element.clone(), group.clone()));
            }
            matched.push((index, owner));
        }
        matched
    }

    fn record_error(&mut self, rule: usize, message: String) {
        let stat = &mut self.stats[rule];
        stat.failed += 1;
        stat.last_error = Some(message);
    }

    /// À appeler pour chaque élément ajouté à l'arbre visuel.
    pub fn on_element_added(&mut self, element: B::Element) {
        if self.stopped {
            return;
        }
        let key = self.backend.key(&element);
        if self.tracked.contains_key(&key) {
            self.release(&key, true);
        }

        let class = self.backend.class_name(&element);
        let matched = self.find_matches(&element, &class);
        if matched.is_empty() {
            return;
        }

        let mut tracked = Tracked::<B> {
            element: element.clone(),
            class: class.clone(),
            entries: Vec::new(),
            captures: Vec::new(),
            group_tokens: Vec::new(),
        };

        let mut watched_groups: Vec<(B::Key, String)> = Vec::new();
        for (rule_index, owner) in matched {
            self.stats[rule_index].matched += 1;
            if let Some((owner_element, group)) = &owner {
                let owner_key = self.backend.key(owner_element);
                if self.mode == Mode::Apply
                    && !watched_groups
                        .iter()
                        .any(|(k, g)| *k == owner_key && g == group)
                {
                    watched_groups.push((owner_key, group.clone()));
                    if let Some(token) = self.backend.watch_group(owner_element, group, &key) {
                        tracked.group_tokens.push(token);
                    }
                }
            }
            self.collect_rule(&element, &class, rule_index, owner, &mut tracked);
        }

        self.tracked.insert(key.clone(), tracked);
        self.apply_element(&key);
    }

    /// Fusionne une règle dans l'état de l'élément. La dernière règle l'emporte.
    fn collect_rule(
        &mut self,
        element: &B::Element,
        class: &str,
        rule_index: usize,
        owner: Option<(B::Element, String)>,
        tracked: &mut Tracked<B>,
    ) {
        let owner_id = owner
            .as_ref()
            .map(|(e, g)| (self.backend.key(e), g.clone()));

        for value_index in 0..self.rules[rule_index].rule.values.len() {
            let value = self.rules[rule_index].rule.values[value_index].clone();
            let template = self.rules[rule_index].templates[value_index].clone();
            let position = tracked.entries.iter().position(|entry| {
                entry.name == value.property
                    && entry
                        .owner
                        .as_ref()
                        .map(|(e, g)| (self.backend.key(e), g.clone()))
                        == owner_id
            });
            let position = match position {
                Some(position) => position,
                None => match self
                    .backend
                    .resolve_property(element, class, &value.property)
                {
                    Ok(property) => {
                        tracked.entries.push(Entry {
                            name: value.property.clone(),
                            property,
                            owner: owner.clone(),
                            specs: HashMap::new(),
                            original: None,
                            applied: None,
                            last_rule: rule_index,
                            token: None,
                        });
                        tracked.entries.len() - 1
                    }
                    Err(message) => {
                        self.record_error(
                            rule_index,
                            format!("propriété {} : {message}", value.property),
                        );
                        continue;
                    }
                },
            };
            tracked.entries[position].last_rule = rule_index;
            tracked.entries[position].specs.insert(
                value.state.clone(),
                Spec {
                    value: value.value.clone(),
                    is_xaml: value.is_xaml,
                    template,
                    rule: rule_index,
                },
            );
        }

        for capture_index in 0..self.rules[rule_index].rule.captures.len() {
            let capture = self.rules[rule_index].rule.captures[capture_index].clone();
            match self
                .backend
                .resolve_property(element, class, &capture.property)
            {
                Ok(property) => tracked.captures.push(CaptureSlot {
                    variable: capture.variable,
                    property,
                    token: None,
                }),
                Err(message) => self.record_error(
                    rule_index,
                    format!("capture {} : {message}", capture.property),
                ),
            }
        }
    }

    /// Transaction : si une écriture échoue, ce passage est annulé.
    fn apply_element(&mut self, key: &B::Key) {
        let count = self.tracked.get(key).map_or(0, |t| t.entries.len());
        let mut written = Vec::new();
        for index in 0..count {
            match self.apply_entry(key, index, true) {
                Ok(true) => written.push(index),
                Ok(false) => {}
                Err(()) => {
                    for &done in written.iter().rev() {
                        self.restore_entry(key, done);
                    }
                    self.note_write_failure();
                    return;
                }
            }
        }

        if self.mode == Mode::Apply {
            self.watch_entries(key);
        }
        self.register_captures(key);
    }

    fn watch_entries(&mut self, key: &B::Key) {
        let Some(tracked) = self.tracked.get_mut(key) else {
            return;
        };
        for entry in &mut tracked.entries {
            if entry.token.is_none() && entry.applied.is_some() {
                entry.token = self
                    .backend
                    .watch_property(&tracked.element, &entry.property, key);
            }
        }
    }

    fn register_captures(&mut self, key: &B::Key) {
        let Some(tracked) = self.tracked.get_mut(key) else {
            return;
        };
        if tracked.captures.is_empty() {
            return;
        }
        for slot in &mut tracked.captures {
            if self.mode == Mode::Apply {
                slot.token = self
                    .backend
                    .watch_property(&tracked.element, &slot.property, key);
            }
            self.sequence += 1;
            self.captures
                .entry(slot.variable.clone())
                .or_default()
                .push(CaptureRecord {
                    key: key.clone(),
                    sequence: self.sequence,
                    value: None,
                });
        }
        self.update_captures(key);
    }

    /// Relit les propriétés capturées et propage les changements.
    fn update_captures(&mut self, key: &B::Key) {
        let Some(tracked) = self.tracked.get(key) else {
            return;
        };
        let mut changed = false;
        for slot in &tracked.captures {
            let value = self
                .backend
                .read_primitive(&tracked.element, &slot.property);
            if let Some(record) = self
                .captures
                .get_mut(&slot.variable)
                .and_then(|records| records.iter_mut().find(|r| r.key == *key))
            {
                if record.value != value {
                    record.value = value;
                    changed = true;
                }
            }
        }
        if changed {
            self.refresh_dynamic();
        }
    }

    /// Recalcule les valeurs dynamiques après un changement de variable.
    fn refresh_dynamic(&mut self) {
        if self.refresh_depth >= MAX_REFRESH_DEPTH || self.stopped {
            return;
        }
        self.refresh_depth += 1;
        let targets: Vec<(B::Key, usize)> = self
            .tracked
            .iter()
            .flat_map(|(key, tracked)| {
                tracked
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.specs.values().any(|s| s.template.is_some()))
                    .map(move |(i, _)| (key.clone(), i))
            })
            .collect();
        for (key, index) in targets {
            self.apply_or_fail(&key, index);
        }
        self.refresh_depth -= 1;
    }

    fn apply_or_fail(&mut self, key: &B::Key, index: usize) {
        match self.apply_entry(key, index, false) {
            Ok(_) => {
                if self.mode == Mode::Apply {
                    self.watch_entries(key);
                }
            }
            Err(()) => self.note_write_failure(),
        }
    }

    /// Chemin des clés de la racine jusqu'à l'élément.
    fn path_to_root(&self, element: &B::Element) -> Vec<B::Key> {
        let mut path = vec![self.backend.key(element)];
        let mut current = self.backend.parent(element);
        while let Some(element) = current {
            path.push(self.backend.key(&element));
            current = self.backend.parent(&element);
        }
        path.reverse();
        path
    }

    /// Choisit la capture la plus proche. À égalité, la plus récente.
    fn lookup_variable(&self, name: &str, consumer: &[B::Key]) -> Option<Value> {
        let records = self.captures.get(name)?;
        let mut best: Option<(usize, u64, &Value)> = None;
        for record in records {
            let Some(value) = &record.value else {
                continue;
            };
            let Some(tracked) = self.tracked.get(&record.key) else {
                continue;
            };
            let path = self.path_to_root(&tracked.element);
            let shared = path
                .iter()
                .zip(consumer)
                .take_while(|(a, b)| a == b)
                .count();
            if best.is_none_or(|(s, n, _)| (shared, record.sequence) > (s, n)) {
                best = Some((shared, record.sequence, value));
            }
        }
        best.map(|(_, _, value)| value.clone())
    }

    /// Applique une entrée. `Ok(true)` si une écriture a eu lieu.
    fn apply_entry(&mut self, key: &B::Key, index: usize, initial: bool) -> Result<bool, ()> {
        let Some(tracked) = self.tracked.get(key) else {
            return Ok(false);
        };
        let entry = &tracked.entries[index];
        let element = tracked.element.clone();
        let class = tracked.class.clone();
        let state = match &entry.owner {
            Some((owner, group)) => self.backend.visual_state(owner, group).unwrap_or_default(),
            None => String::new(),
        };
        let Some(spec) = entry
            .specs
            .get(&state)
            .or_else(|| entry.specs.get(""))
            .cloned()
        else {
            self.restore_entry(key, index);
            return Ok(false);
        };
        let name = entry.name.clone();
        let property = entry.property.clone();
        let previous = entry.applied.clone();

        let rendered = match &spec.template {
            None => spec.value.clone(),
            Some(template) => {
                let path = self.path_to_root(&element);
                let result = template.render(&mut |variable| self.lookup_variable(variable, &path));
                match result {
                    Ok(text) => text,
                    Err(reason) => {
                        if !matches!(reason, EvalError::Skip | EvalError::Undefined(_)) {
                            self.record_error(spec.rule, format!("{name} : {reason:?}"));
                        }
                        self.restore_entry(key, index);
                        return Ok(false);
                    }
                }
            }
        };
        if previous.as_deref() == Some(rendered.as_str()) {
            return Ok(false);
        }

        let value = match self
            .backend
            .parse_value(&element, &class, &name, &rendered, spec.is_xaml)
        {
            Ok(value) => value,
            Err(message) => {
                self.record_error(spec.rule, format!("valeur de {name} : {message}"));
                return Ok(false);
            }
        };

        if self.mode == Mode::DryRun {
            self.stats[spec.rule].applied += 1;
            if let Some(entry) = self.entry_mut(key, index) {
                entry.applied = Some(rendered);
            }
            return Ok(false);
        }

        let needs_original = self
            .entry_mut(key, index)
            .is_some_and(|e| e.original.is_none());
        if needs_original {
            match self.backend.read_original(&element, &property) {
                Ok(original) => {
                    if let Some(entry) = self.entry_mut(key, index) {
                        entry.original = Some(original);
                    }
                }
                Err(message) => {
                    self.record_error(spec.rule, format!("lecture de {name} : {message}"));
                    return Err(());
                }
            }
        }

        if let Err(message) = self.backend.set_value(&element, &property, &value, initial) {
            self.record_error(spec.rule, format!("écriture de {name} : {message}"));
            return Err(());
        }
        self.stats[spec.rule].applied += 1;
        if let Some(entry) = self.entry_mut(key, index) {
            entry.applied = Some(rendered);
        }
        Ok(true)
    }

    fn entry_mut(&mut self, key: &B::Key, index: usize) -> Option<&mut Entry<B>> {
        self.tracked.get_mut(key)?.entries.get_mut(index)
    }

    /// Remet la valeur d'origine d'une entrée, si elle a été écrite.
    fn restore_entry(&mut self, key: &B::Key, index: usize) {
        let Some(tracked) = self.tracked.get_mut(key) else {
            return;
        };
        let element = tracked.element.clone();
        let entry = &mut tracked.entries[index];
        entry.applied = None;
        let rule = entry.last_rule;
        let property = entry.property.clone();
        let name = entry.name.clone();
        let original = entry.original.take();
        if let Some(original) = original {
            if let Err(message) = self.backend.restore(&element, &property, original) {
                self.record_error(rule, format!("restauration de {name} : {message}"));
            }
        }
    }

    fn note_write_failure(&mut self) {
        self.write_failures += 1;
        if self.write_failures >= MAX_WRITE_FAILURES && !self.tripped {
            self.tripped = true;
            self.rollback_all();
        }
    }

    /// Signale un changement d'état visuel.
    pub fn on_state_changed(&mut self, key: &B::Key, group: &str) {
        if self.stopped {
            return;
        }
        let count = self
            .tracked
            .get(key)
            .map_or(0, |tracked| tracked.entries.len());
        for index in 0..count {
            let concerned = self
                .tracked
                .get(key)
                .is_some_and(|t| matches!(&t.entries[index].owner, Some((_, g)) if g == group));
            if concerned {
                self.apply_or_fail(key, index);
            }
        }
        self.update_captures(key);
    }

    /// Signale qu'une propriété surveillée a changé hors du moteur.
    ///
    /// La nouvelle valeur devient l'origine, puis le style est réappliqué.
    pub fn on_property_changed(&mut self, key: &B::Key, property: &B::Property) {
        if self.stopped {
            return;
        }
        let Some(tracked) = self.tracked.get(key) else {
            return;
        };
        let element = tracked.element.clone();
        let positions: Vec<usize> = tracked
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.property == *property && e.applied.is_some())
            .map(|(i, _)| i)
            .collect();
        for index in positions {
            match self.backend.read_original(&element, property) {
                Ok(original) => {
                    if let Some(entry) = self.entry_mut(key, index) {
                        entry.original = Some(original);
                        entry.applied = None;
                    }
                }
                Err(_) => continue,
            }
            self.apply_or_fail(key, index);
        }
        self.update_captures(key);
    }

    /// Oublie un élément retiré de l'arbre, sans y écrire.
    pub fn on_element_removed(&mut self, key: &B::Key) {
        if self.tracked.contains_key(key) {
            self.release(key, false);
        }
    }

    /// Cesse de suivre un élément, avec ou sans restauration.
    fn release(&mut self, key: &B::Key, restore: bool) {
        if restore {
            let count = self.tracked.get(key).map_or(0, |t| t.entries.len());
            for index in (0..count).rev() {
                self.restore_entry(key, index);
            }
        }
        let Some(tracked) = self.tracked.remove(key) else {
            return;
        };
        let had_captures = !tracked.captures.is_empty();
        for token in tracked.group_tokens {
            self.backend.unwatch(token);
        }
        for entry in tracked.entries {
            if let Some(token) = entry.token {
                self.backend.unwatch(token);
            }
        }
        for slot in tracked.captures {
            if let Some(token) = slot.token {
                self.backend.unwatch(token);
            }
            if let Some(records) = self.captures.get_mut(&slot.variable) {
                records.retain(|record| record.key != *key);
            }
        }
        if had_captures {
            self.refresh_dynamic();
        }
    }

    /// Restaure tout et arrête le moteur. Idempotent.
    pub fn rollback_all(&mut self) {
        self.stopped = true;
        let keys: Vec<B::Key> = self.tracked.keys().cloned().collect();
        for key in keys {
            self.release(&key, true);
        }
        self.captures.clear();
    }
}
