//! Validation statique des thèmes et compatibilité avec le build de Windows.

use crate::compile::{compile, CompiledTheme};
use crate::expr::Template;
use crate::model::{Theme, SCHEMA_VERSION};
use crate::resource::ResourceKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub location: String,
    pub message: String,
}

impl Diagnostic {
    fn new(
        severity: Severity,
        code: &str,
        location: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.to_owned(),
            location: location.into(),
            message: message.into(),
        }
    }

    pub fn error(code: &str, location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, location, message)
    }

    pub fn warning(code: &str, location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, location, message)
    }

    pub fn info(code: &str, location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, location, message)
    }
}

/// Vérifie qu'une valeur `:=` est du XML bien formé.
fn check_xaml(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    let wrapped = format!(
        "<r xmlns=\"urn:p\" xmlns:x=\"urn:x\" xmlns:muxc=\"urn:m\" xmlns:d=\"urn:d\" xmlns:mc=\"urn:c\">{value}</r>"
    );
    roxmltree::Document::parse(&wrapped)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Texte de type `$Nom` resté sans substitution.
fn unresolved_constant(value: &str) -> Option<&str> {
    let start = value.find('$')? + 1;
    let rest = &value[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Valide un thème sans rien appliquer.
pub fn validate(theme: &Theme, extra_constants: &[String]) -> (CompiledTheme, Vec<Diagnostic>) {
    let (compiled, mut diagnostics) = compile(theme, extra_constants);

    if theme.schema != SCHEMA_VERSION {
        diagnostics.push(Diagnostic::error(
            "schema-version",
            "schema",
            format!(
                "version {} non prise en charge (attendu : {SCHEMA_VERSION})",
                theme.schema
            ),
        ));
    }
    if theme.control_styles.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "empty-theme",
            "control_styles",
            "le thème ne contient aucune règle",
        ));
    }
    for variable in &compiled.resources {
        if variable.kind == ResourceKind::Xaml {
            if let Err(message) = check_xaml(&variable.value) {
                diagnostics.push(Diagnostic::error(
                    "xaml-syntax",
                    "theme_resource_variables",
                    format!("{} : {message}", variable.key),
                ));
            }
        }
    }

    let captured: BTreeSet<&str> = compiled
        .rules
        .iter()
        .flat_map(|rule| rule.captures.iter().map(|c| c.variable.as_str()))
        .collect();

    let mut reported_variables = BTreeSet::new();
    let mut reported_values = BTreeSet::new();
    for rule in &compiled.rules {
        let location = format!("control_styles[{}]", rule.source_index);
        for value in &rule.values {
            // Une règle à cibles multiples n'est signalée qu'une fois.
            if !reported_values.insert((
                rule.source_index,
                value.property.clone(),
                value.state.clone(),
                value.value.clone(),
            )) {
                continue;
            }
            if let Some(name) = unresolved_constant(&value.value) {
                diagnostics.push(Diagnostic::warning(
                    "unresolved-constant",
                    location.clone(),
                    format!("{} : constante ${name} non définie", value.property),
                ));
            }
            if value.is_dynamic() {
                if let Ok(template) = Template::parse(&value.value) {
                    for variable in template.variables() {
                        if !captured.contains(variable.as_str())
                            && reported_variables.insert(variable.clone())
                        {
                            diagnostics.push(Diagnostic::warning(
                                "undefined-variable",
                                location.clone(),
                                format!("la variable {variable} n'est capturée par aucune règle"),
                            ));
                        }
                    }
                }
            } else if value.is_xaml {
                if let Err(message) = check_xaml(&value.value) {
                    diagnostics.push(Diagnostic::error(
                        "xaml-syntax",
                        location.clone(),
                        format!("{} : {message}", value.property),
                    ));
                }
            }
        }
    }
    (compiled, diagnostics)
}

/// Compare le build de Windows à la plage déclarée par le thème.
pub fn check_build(theme: &Theme, build: u32) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if build < 22000 {
        diagnostics.push(Diagnostic::error(
            "not-windows-11",
            "compat",
            format!("build {build} : Windows 11 est requis"),
        ));
    }
    if let Some(minimum) = theme.compat.min_build {
        if build < minimum {
            diagnostics.push(Diagnostic::warning(
                "build-too-old",
                "compat.min_build",
                format!("build {build} antérieur au minimum déclaré ({minimum})"),
            ));
        }
    }
    match theme.compat.max_tested_build {
        Some(maximum) if build > maximum => diagnostics.push(Diagnostic::warning(
            "build-untested",
            "compat.max_tested_build",
            format!("build {build} plus récent que le dernier testé ({maximum}) : des sélecteurs peuvent ne plus correspondre"),
        )),
        None => diagnostics.push(Diagnostic::info(
            "build-unknown",
            "compat",
            "le thème ne déclare aucun build testé",
        )),
        _ => {}
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ControlStyle;

    fn theme(styles: &[&str]) -> Theme {
        Theme {
            schema: 1,
            style_constants: vec![],
            theme_resource_variables: vec![],
            meta: Default::default(),
            compat: Default::default(),
            control_styles: vec![ControlStyle {
                target: "Grid".to_owned(),
                styles: styles.iter().map(ToString::to_string).collect(),
            }],
        }
    }

    fn codes(styles: &[&str]) -> Vec<String> {
        validate(&theme(styles), &[])
            .1
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn malformed_xaml_is_an_error() {
        assert_eq!(codes(&["Fill:=<SolidColorBrush"]), ["xaml-syntax"]);
        assert!(codes(&["Fill:=<muxc:Foo A=\"1\"/>"]).is_empty());
    }

    #[test]
    fn missing_constant_and_variable_are_warnings() {
        assert_eq!(codes(&["Fill=$Nope"]), ["unresolved-constant"]);
        assert_eq!(codes(&["Width={{a + 1}}"]), ["undefined-variable"]);
        assert!(codes(&["Tag=>a", "Width={{a + 1}}"]).is_empty());
    }

    #[test]
    fn expression_error_is_reported() {
        assert_eq!(codes(&["Width={{1 +}}"]), ["expression-syntax"]);
    }

    #[test]
    fn wrong_schema_is_an_error() {
        let mut t = theme(&["A=1"]);
        t.schema = 2;
        assert!(validate(&t, &[])
            .1
            .iter()
            .any(|d| d.severity == Severity::Error));
    }

    #[test]
    fn build_ranges() {
        let mut t = theme(&[]);
        t.compat.min_build = Some(22621);
        t.compat.max_tested_build = Some(26100);
        let codes = |b| {
            check_build(&t, b)
                .into_iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        };
        assert!(codes(26100).is_empty());
        assert_eq!(codes(22000), ["build-too-old"]);
        assert_eq!(codes(26200), ["build-untested"]);
        assert_eq!(codes(19045), ["not-windows-11", "build-too-old"]);
    }
}
