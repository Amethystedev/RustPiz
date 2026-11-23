//! Compilation d'un thème : constantes, découpage des cibles, analyse des règles.

use crate::expr::Template;
use crate::model::Theme;
use crate::resource::{parse_resource_variable, ResourceKind, ResourceVariable};
use crate::rule::{parse_rule, CaptureRule, Rule, ValueRule};
use crate::selector::{parse_targets, Selector};
use crate::validate::Diagnostic;
use serde::{Deserialize, Serialize};

/// Règle prête à être appliquée pour une cible simple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledRule {
    /// Indice dans `control_styles`, pour les rapports.
    pub source_index: usize,
    pub selector: Selector,
    pub values: Vec<ValueRule>,
    pub captures: Vec<CaptureRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledTheme {
    pub name: String,
    pub rules: Vec<CompiledRule>,
    #[serde(default)]
    pub resources: Vec<ResourceVariable>,
}

/// Constantes triées du nom le plus long au plus court, la plus récente d'abord.
#[derive(Default)]
struct Constants(Vec<(String, String)>);

impl Constants {
    fn expand(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(pos) = rest.find('$') {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 1..];
            match self
                .0
                .iter()
                .find(|(name, _)| after.starts_with(name.as_str()))
            {
                Some((name, value)) => {
                    out.push_str(value);
                    rest = &after[name.len()..];
                }
                None => {
                    out.push('$');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn define(&mut self, entry: &str) -> Result<(), String> {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| "'=' manquant dans la constante".to_owned())?;
        let key = key.trim();
        if key.is_empty() {
            return Err("nom de constante vide".to_owned());
        }
        let value = self.expand(value.trim());
        let position = self.0.partition_point(|(name, _)| name.len() > key.len());
        self.0.insert(position, (key.to_owned(), value));
        Ok(())
    }
}

fn escape_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Remplace `<WindhawkBlur …/>` par un `AcrylicBrush` équivalent au mieux.
///
/// Le vrai flou demande un pinceau de composition, prévu à l'étape 6.
fn approximate_blur(value: &str) -> Result<Option<String>, String> {
    if !value.trim_start().starts_with("<WindhawkBlur") {
        return Ok(None);
    }
    let document = roxmltree::Document::parse(value).map_err(|e| e.to_string())?;
    let root = document.root_element();
    let tint = root.attribute("TintColor").unwrap_or("#00000000");
    let opacity = root.attribute("TintOpacity").unwrap_or("1");
    let fallback = root.attribute("FallbackColor").unwrap_or(tint);
    Ok(Some(format!(
        "<AcrylicBrush TintColor=\"{}\" TintOpacity=\"{}\" FallbackColor=\"{}\" />",
        escape_attribute(tint),
        escape_attribute(opacity),
        escape_attribute(fallback),
    )))
}

/// Compile un thème. Les règles invalides sont écartées et signalées.
pub fn compile(theme: &Theme, extra_constants: &[String]) -> (CompiledTheme, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut constants = Constants::default();

    let constant_sources = theme
        .style_constants
        .iter()
        .map(|c| ("style_constants", c))
        .chain(
            extra_constants
                .iter()
                .map(|c| ("profil.style_constants", c)),
        );
    for (index, (origin, entry)) in constant_sources.enumerate() {
        if entry.starts_with("//") {
            continue;
        }
        if let Err(message) = constants.define(entry) {
            diagnostics.push(Diagnostic::error(
                "constant-syntax",
                format!("{origin}[{index}]"),
                message,
            ));
        }
    }

    let mut compiled = CompiledTheme {
        name: theme.meta.name.clone(),
        rules: Vec::new(),
        resources: Vec::new(),
    };

    for (index, entry) in theme.theme_resource_variables.iter().enumerate() {
        if entry.starts_with("//") {
            continue;
        }
        let location = format!("theme_resource_variables[{index}]");
        match parse_resource_variable(entry, &|value| constants.expand(value)) {
            Err(message) => {
                diagnostics.push(Diagnostic::error("resource-syntax", location, message))
            }
            Ok(mut variable) => {
                if variable.kind == ResourceKind::Xaml {
                    match approximate_blur(&variable.value) {
                        Ok(Some(acrylic)) => {
                            variable.value = acrylic;
                            diagnostics.push(Diagnostic::warning(
                                "blur-approximated",
                                location,
                                "WindhawkBlur n'est pas encore implémenté : remplacé par AcrylicBrush (rendu approché)",
                            ));
                        }
                        Ok(None) => {}
                        Err(message) => {
                            diagnostics.push(Diagnostic::error("xaml-syntax", location, message));
                            continue;
                        }
                    }
                }
                compiled.resources.push(variable);
            }
        }
    }

    for (index, control) in theme.control_styles.iter().enumerate() {
        if control.target.starts_with("//") {
            continue;
        }
        let location = format!("control_styles[{index}]");
        let mut values = Vec::new();
        let mut captures = Vec::new();

        for (style_index, style) in control.styles.iter().enumerate() {
            if style.starts_with("//") {
                continue;
            }
            let style_location = format!("{location}.styles[{style_index}]");
            let expanded = constants.expand(style);
            match parse_rule(&expanded) {
                Err(message) => {
                    diagnostics.push(Diagnostic::error("style-syntax", style_location, message))
                }
                Ok(Rule::Capture(capture)) => captures.push(capture),
                Ok(Rule::Value(mut rule)) => {
                    if rule.is_dynamic() {
                        if let Err(message) = Template::parse(&rule.value) {
                            diagnostics.push(Diagnostic::error(
                                "expression-syntax",
                                style_location,
                                message,
                            ));
                            continue;
                        }
                    }
                    if rule.is_xaml {
                        match approximate_blur(&rule.value) {
                            Ok(Some(acrylic)) => {
                                rule.value = acrylic;
                                diagnostics.push(Diagnostic::warning(
                                    "blur-approximated",
                                    style_location.clone(),
                                    "WindhawkBlur n'est pas encore implémenté : remplacé par AcrylicBrush (rendu approché)",
                                ));
                            }
                            Ok(None) => {}
                            Err(message) => {
                                diagnostics.push(Diagnostic::error(
                                    "xaml-syntax",
                                    style_location,
                                    message,
                                ));
                                continue;
                            }
                        }
                    }
                    values.push(rule);
                }
            }
        }

        if values.is_empty() && captures.is_empty() {
            continue;
        }

        for parsed in parse_targets(&control.target) {
            match parsed {
                Ok(selector) => compiled.rules.push(CompiledRule {
                    source_index: index,
                    selector,
                    values: values.clone(),
                    captures: captures.clone(),
                }),
                Err((single, error)) => diagnostics.push(Diagnostic::error(
                    "selector-syntax",
                    location.clone(),
                    format!("cible {single:?} : {error}"),
                )),
            }
        }
    }

    (compiled, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ControlStyle;

    fn theme(constants: &[&str], target: &str, styles: &[&str]) -> Theme {
        Theme {
            schema: 1,
            style_constants: constants.iter().map(ToString::to_string).collect(),
            theme_resource_variables: vec![],
            meta: Default::default(),
            compat: Default::default(),
            control_styles: vec![ControlStyle {
                target: target.to_owned(),
                styles: styles.iter().map(ToString::to_string).collect(),
            }],
        }
    }

    #[test]
    fn longest_constant_wins_and_constants_can_chain() {
        let t = theme(
            &["A=1", "AB=2", "C=$AB"],
            "Grid",
            &["X=$AB", "Y=$A", "Z=$C"],
        );
        let (compiled, diagnostics) = compile(&t, &[]);
        assert!(diagnostics.is_empty());
        let values: Vec<_> = compiled.rules[0]
            .values
            .iter()
            .map(|v| v.value.as_str())
            .collect();
        assert_eq!(values, ["2", "1", "2"]);
    }

    #[test]
    fn later_definition_and_profile_override() {
        let t = theme(&["A=1"], "Grid", &["X=$A"]);
        let (compiled, _) = compile(&t, &["A=9".to_owned()]);
        assert_eq!(compiled.rules[0].values[0].value, "9");
    }

    #[test]
    fn comments_are_ignored() {
        let t = theme(&["// nope"], "Grid", &["// A=1", "B=2"]);
        let (compiled, diagnostics) = compile(&t, &[]);
        assert!(diagnostics.is_empty());
        assert_eq!(compiled.rules[0].values.len(), 1);
    }

    #[test]
    fn invalid_rule_is_dropped_but_others_are_kept() {
        let t = theme(&[], "Grid, A > > B[", &["Bad", "Good=1"]);
        let (compiled, diagnostics) = compile(&t, &[]);
        assert_eq!(compiled.rules.len(), 1);
        assert_eq!(compiled.rules[0].values.len(), 1);
        assert_eq!(diagnostics.len(), 2);
    }

    #[test]
    fn blur_is_approximated_with_a_warning() {
        let t = theme(
            &[],
            "Grid",
            &["Fill:=<WindhawkBlur BlurAmount=\"5\" TintColor=\"{ThemeResource X}\" TintOpacity=\"0.5\" />"],
        );
        let (compiled, diagnostics) = compile(&t, &[]);
        assert_eq!(
            compiled.rules[0].values[0].value,
            "<AcrylicBrush TintColor=\"{ThemeResource X}\" TintOpacity=\"0.5\" FallbackColor=\"{ThemeResource X}\" />"
        );
        assert_eq!(diagnostics[0].code, "blur-approximated");
    }
}
