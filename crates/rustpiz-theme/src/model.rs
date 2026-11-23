//! Modèle de thème et conversions TOML, JSON et YAML Windhawk.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Version courante du format.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("lecture de {path} impossible : {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("TOML invalide : {0}")]
    Toml(#[from] toml::de::Error),
    #[error("écriture TOML impossible : {0}")]
    TomlWrite(#[from] toml::ser::Error),
    #[error("JSON invalide : {0}")]
    Json(#[from] serde_json::Error),
    #[error("YAML Windhawk invalide : {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("extension non reconnue pour {0} (attendu : .toml, .json, .yaml, .yml)")]
    UnknownFormat(String),
}

/// Thème complet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub style_constants: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub theme_resource_variables: Vec<String>,
    #[serde(default)]
    pub meta: ThemeMeta,
    #[serde(default)]
    pub compat: Compat,
    #[serde(default)]
    pub control_styles: Vec<ControlStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Plage de builds Windows déclarée pour le thème.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_build: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tested_build: Option<u32>,
}

/// Une cible et ses styles, à la syntaxe Windhawk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlStyle {
    pub target: String,
    #[serde(default)]
    pub styles: Vec<String>,
}

/// « Mode textuel » du Styler. Les réglages inconnus sont ignorés.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindhawkSettings {
    #[serde(default)]
    style_constants: Vec<String>,
    #[serde(default)]
    theme_resource_variables: Vec<String>,
    #[serde(default)]
    control_styles: Vec<ControlStyle>,
}

impl Theme {
    /// Lit un thème natif TOML.
    pub fn from_toml(text: &str) -> Result<Self, ThemeError> {
        Ok(toml::from_str(text)?)
    }

    /// Lit un thème natif JSON.
    pub fn from_json(text: &str) -> Result<Self, ThemeError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Convertit le « mode textuel » YAML du Styler.
    pub fn from_windhawk_yaml(text: &str, name: &str) -> Result<Self, ThemeError> {
        let settings: WindhawkSettings = serde_yaml_ng::from_str(text)?;
        Ok(Theme {
            schema: SCHEMA_VERSION,
            style_constants: settings.style_constants,
            theme_resource_variables: settings.theme_resource_variables,
            meta: ThemeMeta {
                name: name.to_owned(),
                ..ThemeMeta::default()
            },
            compat: Compat::default(),
            control_styles: settings.control_styles,
        })
    }

    /// Charge un fichier selon son extension.
    pub fn load(path: &Path) -> Result<Self, ThemeError> {
        let text = std::fs::read_to_string(path).map_err(|source| ThemeError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => Self::from_toml(&text),
            Some("json") => Self::from_json(&text),
            Some("yaml" | "yml") => Self::from_windhawk_yaml(&text, &stem),
            _ => Err(ThemeError::UnknownFormat(path.display().to_string())),
        }
    }

    pub fn to_toml(&self) -> Result<String, ThemeError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn to_json(&self) -> Result<String, ThemeError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windhawk_yaml_round_trips_through_toml_and_json() {
        let yaml = "theme: ignored\nstyleConstants:\n  - A=#FF000000\ncontrolStyles:\n  - target: Grid#X > Border\n    styles:\n      - Background:=<SolidColorBrush Color=\"$A\"/>\n      - Height={{a==`b`?skip():70}}\n";
        let theme = Theme::from_windhawk_yaml(yaml, "t").unwrap();
        assert_eq!(theme.control_styles[0].styles.len(), 2);
        assert_eq!(Theme::from_toml(&theme.to_toml().unwrap()).unwrap(), theme);
        assert_eq!(Theme::from_json(&theme.to_json().unwrap()).unwrap(), theme);
    }

    #[test]
    fn unknown_native_key_is_rejected() {
        assert!(Theme::from_toml("schema = 1\nnope = 2\n").is_err());
    }
}
