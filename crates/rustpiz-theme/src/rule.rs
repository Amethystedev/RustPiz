//! Règles de style `Prop[@Etat][:]=valeur` et captures `Prop=>variable`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRule {
    pub property: String,
    /// Vide : valeur par défaut, tous états confondus.
    pub state: String,
    pub value: String,
    pub is_xaml: bool,
}

impl ValueRule {
    /// Vrai si la valeur contient au moins une expression `{{…}}`.
    pub fn is_dynamic(&self) -> bool {
        self.value.contains("{{")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRule {
    pub property: String,
    pub variable: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Value(ValueRule),
    Capture(CaptureRule),
}

/// Identifiant valide pour une variable de style.
pub fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Analyse une entrée de `styles`.
pub fn parse_rule(text: &str) -> Result<Rule, String> {
    let (name, value) = text
        .split_once('=')
        .ok_or_else(|| "'=' manquant dans le style".to_owned())?;

    if let Some(variable) = value.strip_prefix('>') {
        if name.ends_with(':') {
            return Err("':=>' invalide : ':=' et '=>' sont incompatibles".to_owned());
        }
        if name.contains('@') {
            return Err("'@Etat' interdit sur une capture".to_owned());
        }
        let property = name.trim();
        let variable = variable.trim();
        if property.is_empty() {
            return Err("nom de propriété vide".to_owned());
        }
        if !is_identifier(variable) {
            return Err(format!("nom de variable invalide : {variable:?}"));
        }
        return Ok(Rule::Capture(CaptureRule {
            property: property.to_owned(),
            variable: variable.to_owned(),
        }));
    }

    let (name, is_xaml) = match name.strip_suffix(':') {
        Some(stripped) => (stripped, true),
        None => (name, false),
    };
    let (property, state) = match name.split_once('@') {
        Some((property, state)) => (property.trim(), state.trim()),
        None => (name.trim(), ""),
    };
    if property.is_empty() {
        return Err("nom de propriété vide".to_owned());
    }
    Ok(Rule::Value(ValueRule {
        property: property.to_owned(),
        state: state.to_owned(),
        value: value.trim().to_owned(),
        is_xaml,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> ValueRule {
        match parse_rule(text).unwrap() {
            Rule::Value(rule) => rule,
            Rule::Capture(_) => panic!("capture inattendue"),
        }
    }

    #[test]
    fn simple_xaml_and_state_forms() {
        assert!(!value("Margin=0,0,0,10").is_xaml);
        let xaml = value("Fill:=<SolidColorBrush Color=\"Red\"/>");
        assert!(xaml.is_xaml);
        assert_eq!(xaml.value, "<SolidColorBrush Color=\"Red\"/>");
        assert_eq!(value("Tag@DockedLeft=vertical").state, "DockedLeft");
        assert!(value("Height={{a?1:2}}").is_dynamic());
    }

    #[test]
    fn value_keeps_later_equals_signs() {
        assert_eq!(value("A=b=c").value, "b=c");
    }

    #[test]
    fn capture_form() {
        assert_eq!(
            parse_rule("Tag=>taskbarDock").unwrap(),
            Rule::Capture(CaptureRule {
                property: "Tag".into(),
                variable: "taskbarDock".into()
            })
        );
    }

    #[test]
    fn invalid_forms_are_rejected() {
        for bad in ["NoEquals", "=x", "A:=>b", "A@S=>b", "A=>1bad", "=>x"] {
            assert!(parse_rule(bad).is_err(), "devrait échouer : {bad:?}");
        }
    }
}
