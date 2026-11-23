//! Variables de ressources : `Cle=valeur`, `Cle@Dark=valeur`, `Cle:=<xaml/>`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceTheme {
    /// Remplace une ressource existante.
    None,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    String,
    Xaml,
    /// La valeur est le nom d'une autre ressource.
    ThemeReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceVariable {
    pub key: String,
    pub theme: ResourceTheme,
    pub kind: ResourceKind,
    pub value: String,
}

/// Analyse une entrée. `expand` substitue les constantes dans la valeur.
pub fn parse_resource_variable(
    entry: &str,
    expand: &dyn Fn(&str) -> String,
) -> Result<ResourceVariable, String> {
    let (key_part, raw) = entry
        .split_once('=')
        .ok_or_else(|| "'=' manquant dans la variable de ressource".to_owned())?;
    let mut value = expand(raw.trim());
    let mut key_part = key_part.trim();

    let mut kind = ResourceKind::String;
    if let Some(stripped) = key_part.strip_suffix(':') {
        kind = ResourceKind::Xaml;
        key_part = stripped.trim();
    } else if let Some(name) = value
        .strip_prefix("{ThemeResource ")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        kind = ResourceKind::ThemeReference;
        value = name.trim().to_owned();
    }

    let (key, theme) = match key_part.split_once('@') {
        None => (key_part, ResourceTheme::None),
        Some((key, "Dark")) => (key.trim(), ResourceTheme::Dark),
        Some((key, "Light")) => (key.trim(), ResourceTheme::Light),
        Some((_, other)) => return Err(format!("thème inconnu {other:?} (Dark ou Light attendu)")),
    };
    if key.is_empty() {
        return Err("nom de ressource vide".to_owned());
    }
    Ok(ResourceVariable {
        key: key.to_owned(),
        theme,
        kind,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(entry: &str) -> Result<ResourceVariable, String> {
        parse_resource_variable(entry, &|v| v.replace("$A", "#FF"))
    }

    #[test]
    fn override_of_an_existing_resource() {
        let parsed = parse("MediumTaskbarButtonExtent=48").unwrap();
        assert_eq!(parsed.theme, ResourceTheme::None);
        assert_eq!(parsed.kind, ResourceKind::String);
        assert_eq!(parsed.value, "48");
    }

    #[test]
    fn theme_aware_reference() {
        let parsed = parse("Auto@Dark={ThemeResource SystemAccentColorDark1}").unwrap();
        assert_eq!(parsed.key, "Auto");
        assert_eq!(parsed.theme, ResourceTheme::Dark);
        assert_eq!(parsed.kind, ResourceKind::ThemeReference);
        assert_eq!(parsed.value, "SystemAccentColorDark1");
    }

    #[test]
    fn xaml_value_keeps_equals_and_expands_constants() {
        let parsed = parse("Brush@Light:=<SolidColorBrush Color=\"$A\"/>").unwrap();
        assert_eq!(parsed.kind, ResourceKind::Xaml);
        assert_eq!(parsed.theme, ResourceTheme::Light);
        assert_eq!(parsed.value, "<SolidColorBrush Color=\"#FF\"/>");
    }

    #[test]
    fn invalid_forms_are_rejected() {
        for bad in ["NoEquals", "=1", "K@Blue=1", "@Dark=1"] {
            assert!(parse(bad).is_err(), "devrait échouer : {bad}");
        }
    }
}
