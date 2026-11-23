//! Le thème Surface livré doit rester fidèle à sa source Windhawk.

use rustpiz_theme::rule::{parse_rule, Rule};
use rustpiz_theme::{validate, Severity, Theme};

const YAML: &str = include_str!("../../../themes/surface.windhawk.yaml");
const TOML: &str = include_str!("../../../themes/surface.toml");

#[test]
fn native_theme_has_the_same_rules_as_the_windhawk_source() {
    let source = Theme::from_windhawk_yaml(YAML, "Surface").unwrap();
    let native = Theme::from_toml(TOML).unwrap();
    assert_eq!(source.control_styles, native.control_styles);
    assert_eq!(source.style_constants, native.style_constants);
}

#[test]
fn surface_compiles_without_errors() {
    let theme = Theme::from_toml(TOML).unwrap();
    let (compiled, diagnostics) = validate(&theme, &[]);
    assert_eq!(compiled.rules.len(), 19);
    assert!(
        !diagnostics.iter().any(|d| d.severity == Severity::Error),
        "{diagnostics:?}"
    );
    let blur = diagnostics
        .iter()
        .filter(|d| d.code == "blur-approximated")
        .count();
    assert_eq!(blur, 2);
    assert_eq!(diagnostics.len(), blur, "{diagnostics:?}");
}

#[test]
fn constants_are_expanded_in_task_item_rules() {
    let theme = Theme::from_toml(TOML).unwrap();
    let (compiled, _) = validate(&theme, &[]);
    let rule = compiled
        .rules
        .iter()
        .find(|r| {
            r.selector
                .raw
                .starts_with("Grid#IconPanel@RunningIndicatorStates > Border")
        })
        .unwrap();
    let background = rule
        .values
        .iter()
        .find(|v| v.property == "Background")
        .unwrap();
    assert!(background.is_xaml);
    assert!(background.value.starts_with("<AcrylicBrush"));
    assert!(!background.value.contains('$'));
}

#[test]
fn profile_constant_overrides_the_theme() {
    let theme = Theme::from_toml(TOML).unwrap();
    let (compiled, _) = validate(&theme, &["TaskItemBackground=#E6202020".to_owned()]);
    let found = compiled
        .rules
        .iter()
        .flat_map(|r| &r.values)
        .any(|v| v.property == "Background" && v.value == "#E6202020");
    assert!(found);
}

#[test]
fn every_style_of_the_source_is_understood() {
    let source = Theme::from_windhawk_yaml(YAML, "Surface").unwrap();
    for style in source.control_styles.iter().flat_map(|c| &c.styles) {
        assert!(
            matches!(parse_rule(style), Ok(Rule::Value(_) | Rule::Capture(_))),
            "{style}"
        );
    }
}

/// Tous les thèmes livrés doivent se compiler sans erreur.
#[test]
fn every_shipped_theme_compiles() {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        checked += 1;
        let theme = Theme::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!theme.meta.name.is_empty(), "{}", path.display());
        let (compiled, diagnostics) = validate(&theme, &[]);
        assert!(!compiled.rules.is_empty(), "{}", path.display());
        // Le runtime ne connaît pas ce pinceau : il doit toujours avoir été remplacé.
        let leftover = compiled
            .rules
            .iter()
            .flat_map(|r| r.values.iter().map(|v| v.value.as_str()))
            .chain(compiled.resources.iter().map(|r| r.value.as_str()))
            .any(|value| value.contains("<WindhawkBlur"));
        assert!(!leftover, "{}", path.display());
        for diagnostic in diagnostics.iter().filter(|d| d.severity == Severity::Error) {
            failures.push(format!("{}: {diagnostic:?}", path.display()));
        }
    }
    assert!(checked >= 50, "seulement {checked} thèmes");
    assert!(failures.is_empty(), "{failures:#?}");
}
