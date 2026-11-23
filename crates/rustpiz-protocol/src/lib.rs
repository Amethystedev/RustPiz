//! Profil utilisateur, configuration active et messages hôte ↔ runtime.

use rustpiz_theme::CompiledTheme;
use rustpiz_thumbnails::ThumbnailConfig;
use rustpiz_xaml::{Mode, RuleStat};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod diagnostic;

/// Incrémenté à chaque changement incompatible entre hôte et DLL.
pub const PROTOCOL_VERSION: u32 = 3;

/// CLSID de l'objet COM exposé par la DLL : {EE3334EB-7E0F-4909-87F3-49A086BC926D}.
/// Propre à RustPiz (RustHawk utilise {7A1F5C3E-9B42-4D6E-8F10-2C5B7E91A4D3}) :
/// les deux DLL ne peuvent pas être confondues par `DllGetClassObject`.
pub const TAP_CLSID: u128 = 0xEE3334EB_7E0F_4909_87F3_49A086BC926D;

/// Nom du tube de contrôle pour un processus explorer donné.
pub fn pipe_name(explorer_pid: u32) -> String {
    format!(r"\\.\pipe\rustpiz.{explorer_pid}")
}

/// Fichier de profil choisi par l'utilisateur.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Chemin du thème, relatif au profil.
    #[serde(default)]
    pub theme: Option<PathBuf>,
    /// Constantes qui remplacent celles du thème.
    #[serde(default)]
    pub style_constants: Vec<String>,
    #[serde(default)]
    pub thumbnails: ThumbnailConfig,
}

/// Fonctions à détourner dans le module de la barre des tâches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailHook {
    pub config: ThumbnailConfig,
    pub module_name: String,
    /// Signature PE attendue. Tout écart annule les hooks.
    pub timestamp: u32,
    pub size_of_image: u32,
    pub scaled_size_rva: u32,
    pub on_apply_template_rva: u32,
}

/// Configuration lue par la DLL au démarrage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub protocol: u32,
    pub mode: Mode,
    pub theme: CompiledTheme,
    pub thumbnails: Option<ThumbnailHook>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Hello,
    Status,
    Disable,
    /// Change les miniatures sans toucher au thème. `None` retire les hooks.
    SetThumbnails(Option<ThumbnailHook>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailStatus {
    pub requested: bool,
    pub active: bool,
    pub message: Option<String>,
    /// Compteurs d'appels des détours, pour le diagnostic.
    #[serde(default)]
    pub activity: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    pub dry_run: bool,
    pub ui_threads: u32,
    /// Threads XAML qui n'ont pas répondu : leur état n'est pas restauré.
    pub unresponsive_threads: u32,
    pub tracked_elements: u32,
    pub write_failures: u32,
    /// Vrai si le disjoncteur a restauré l'état d'origine.
    pub tripped: bool,
    pub rules: Vec<RuleStat>,
    pub thumbnails: ThumbnailStatus,
    /// Nom du thème de la session qui répond (`CompiledTheme::name`). Ajout
    /// RustPiz compatible (vide si la DLL est plus ancienne) : permet à
    /// l'hôte de détecter qu'une ancienne session répond encore.
    #[serde(default)]
    pub theme: String,
    /// Numéro de session dans ce processus explorer (0 si inconnu).
    #[serde(default)]
    pub session: u64,
    /// Issue de `AdviseVisualTreeChange` pour cette session (`None` : DLL
    /// antérieure ou session pas encore démarrée). Ajout RustPiz compatible.
    #[serde(default)]
    pub advise: Option<diagnostic::AdviseStatus>,
}

impl StatusReport {
    /// Ajoute les compteurs d'un autre thread XAML.
    pub fn merge(&mut self, other: StatusReport) {
        self.ui_threads += other.ui_threads;
        self.unresponsive_threads += other.unresponsive_threads;
        self.tracked_elements += other.tracked_elements;
        self.write_failures += other.write_failures;
        self.tripped |= other.tripped;
        if self.rules.is_empty() {
            self.rules = other.rules;
            return;
        }
        for (mine, theirs) in self.rules.iter_mut().zip(other.rules) {
            mine.matched += theirs.matched;
            mine.applied += theirs.applied;
            mine.failed += theirs.failed;
            if theirs.last_error.is_some() {
                mine.last_error = theirs.last_error;
            }
        }
    }

    /// Règles sans aucun élément correspondant.
    pub fn unmatched(&self) -> impl Iterator<Item = &RuleStat> {
        self.rules.iter().filter(|rule| rule.matched == 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Hello { protocol: u32, pid: u32 },
    Status(StatusReport),
    Disabled(StatusReport),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(matched: u32) -> RuleStat {
        RuleStat {
            matched,
            ..RuleStat::default()
        }
    }

    #[test]
    fn merge_sums_threads() {
        let mut first = StatusReport {
            ui_threads: 1,
            rules: vec![rule(0), rule(2)],
            ..StatusReport::default()
        };
        first.merge(StatusReport {
            ui_threads: 1,
            tripped: true,
            rules: vec![rule(0), rule(1)],
            ..StatusReport::default()
        });
        assert_eq!(first.ui_threads, 2);
        assert!(first.tripped);
        assert_eq!(first.rules[1].matched, 3);
        assert_eq!(first.unmatched().count(), 1);
    }

    #[test]
    fn report_without_theme_fields_still_parses() {
        // Rapport d'une DLL antérieure : les champs ajoutés prennent leur défaut.
        let ancien = r#"{"dry_run":false,"ui_threads":1,"unresponsive_threads":0,
            "tracked_elements":3,"write_failures":0,"tripped":false,"rules":[],
            "thumbnails":{"requested":false,"active":false,"message":null}}"#;
        let rapport: StatusReport = serde_json::from_str(ancien).unwrap();
        assert_eq!(rapport.theme, "");
        assert_eq!(rapport.session, 0);
        assert_eq!(rapport.advise, None);
        assert_eq!(rapport.tracked_elements, 3);
    }

    #[test]
    fn profile_defaults_keep_thumbnails_off() {
        assert!(!Profile::default().thumbnails.enabled);
    }

    #[test]
    fn messages_round_trip() {
        let text = serde_json::to_string(&Response::Hello {
            protocol: 1,
            pid: 7,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&text).unwrap(),
            Response::Hello {
                protocol: 1,
                pid: 7
            }
        );
    }
}
