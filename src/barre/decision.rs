//! Logique pure de l'intégration de la barre des tâches (v0.6.0).
//!
//! Aucune dépendance Win32 : décisions (quoi faire selon l'état appliqué et
//! l'état voulu), génération de la configuration active (`active.json`),
//! empreinte de la DLL, compilation d'un thème embarqué et réglage des
//! miniatures. Entièrement testable hors de Windows.

use rustpiz_protocol::{RuntimeConfig, ThumbnailHook, PROTOCOL_VERSION};
use rustpiz_theme::{CompiledTheme, Severity};
use rustpiz_thumbnails::{Mode as ModeMiniatures, ThumbnailConfig};
use rustpiz_xaml::Mode;

/// Taille minimale des miniatures acceptée (en %).
pub const MINIATURES_MIN: u32 = 100;
/// Taille maximale des miniatures acceptée (en %).
pub const MINIATURES_MAX: u32 = 300;

/// État voulu de l'intégration, dérivé de la configuration RustPiz.
///
/// `theme = None` : la barre n'est pas personnalisée.
/// `miniatures = None` : la taille des miniatures n'est pas modifiée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Souhait {
    /// Identifiant du thème embarqué à appliquer (validé en amont).
    pub theme: Option<String>,
    /// Taille des miniatures en %, bornée à [`MINIATURES_MIN`]–[`MINIATURES_MAX`].
    pub miniatures: Option<u32>,
}

impl Souhait {
    /// Vrai si rien n'est demandé (ni thème, ni miniatures) : coût nul.
    pub fn est_inactif(&self) -> bool {
        self.theme.is_none() && self.miniatures.is_none()
    }
}

/// Action à mener pour atteindre l'état voulu depuis l'état appliqué.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Rien à faire : l'état appliqué correspond déjà au voulu.
    Rien,
    /// Arrêter la session (rollback complet dans explorer).
    Arreter,
    /// Thème inchangé : seules les miniatures changent (sans relancer la session).
    MajMiniatures,
    /// Restaurer puis réappliquer entièrement (changement de thème ou session absente).
    Redemarrer,
}

/// Décide l'action à mener.
///
/// `applique` : ce que RustPiz croit avoir appliqué dans l'explorer courant
/// (`None` si rien). `vivante` : le runtime répond-il sur le tube ?
pub fn planifier(applique: Option<&Souhait>, vivante: bool, voulu: &Souhait) -> Action {
    if voulu.est_inactif() {
        return if applique.is_some() || vivante {
            Action::Arreter
        } else {
            Action::Rien
        };
    }
    match applique {
        Some(a) if vivante && a.theme == voulu.theme => {
            if a.miniatures == voulu.miniatures {
                Action::Rien
            } else {
                Action::MajMiniatures
            }
        }
        _ => Action::Redemarrer,
    }
}

/// Un conflit de diagnostics XAML détecté dans explorer bloque-t-il cette
/// action ? Seule une nouvelle injection (`Redemarrer`) est bloquée : une
/// session RustPiz déjà vivante (mise à jour des miniatures, rien à faire)
/// ou un arrêt ne dépendent pas d'un nouvel abonnement à l'arbre visuel.
pub fn conflit_bloque(action: Action) -> bool {
    action == Action::Redemarrer
}

/// Le chemin d'image `chemin` est-il celui d'`explorer.exe` ? (nom de
/// fichier seul, sans tenir compte de la casse ; séparateurs `\` et `/`).
pub fn est_image_explorer(chemin: &str) -> bool {
    chemin
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|nom| nom.eq_ignore_ascii_case("explorer.exe"))
}

/// FNV-1a 64 bits : nomme la copie de la DLL d'après son contenu.
pub fn empreinte(octets: &[u8]) -> u64 {
    octets.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &octet| {
        (hash ^ u64::from(octet)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Nom de fichier de la DLL déployée, dérivé de son empreinte.
pub fn nom_dll(empreinte: u64) -> String {
    format!("rustpiz_runtime-{empreinte:016x}.dll")
}

/// Vrai si `nom` est une copie déployée de la DLL (ou un fichier partiel)
/// autre que la version courante `actuel` : `rustpiz_runtime-` suivi de 16
/// chiffres hexadécimaux puis `.dll` ou `.dll.part`. Sert au nettoyage
/// (best effort) des anciennes versions ; rien d'autre n'est jamais supprimé.
pub fn est_ancienne_dll(nom: &str, actuel: &str) -> bool {
    let Some(reste) = nom.strip_prefix("rustpiz_runtime-") else {
        return false;
    };
    let empreinte = reste
        .strip_suffix(".dll.part")
        .or_else(|| reste.strip_suffix(".dll"));
    let forme_valide = empreinte
        .is_some_and(|e| e.len() == 16 && e.bytes().all(|b| b.is_ascii_hexdigit()));
    forme_valide && nom != actuel
}

/// Compile un thème embarqué par son identifiant.
///
/// Erreur si l'identifiant est inconnu, le TOML illisible ou le thème
/// contient au moins une erreur de validation.
pub fn compiler_theme(id: &str) -> Result<CompiledTheme, String> {
    let texte =
        super::themes_embarques::contenu(id).ok_or_else(|| format!("thème inconnu : {id}"))?;
    let theme = rustpiz_theme::Theme::from_toml(texte).map_err(|e| e.to_string())?;
    let (compile, diagnostics) = rustpiz_theme::validate(&theme, &[]);
    let erreurs = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if erreurs > 0 {
        return Err(format!("{id} : {erreurs} erreur(s) de validation du thème"));
    }
    if compile.rules.is_empty() {
        return Err(format!("{id} : aucune règle compilée"));
    }
    Ok(compile)
}

/// Compile un thème embarqué et renvoie aussi les avertissements de
/// compatibilité avec le build de Windows (`rustpiz_theme::check_build`,
/// sévérité avertissement ou erreur ; `build == 0` = inconnu, non vérifié).
pub fn compiler_theme_pour_build(id: &str, build: u32) -> Result<(CompiledTheme, Vec<String>), String> {
    let compile = compiler_theme(id)?;
    let mut avertissements = Vec::new();
    if build != 0 {
        let texte =
            super::themes_embarques::contenu(id).ok_or_else(|| format!("thème inconnu : {id}"))?;
        let theme = rustpiz_theme::Theme::from_toml(texte).map_err(|e| e.to_string())?;
        avertissements = rustpiz_theme::check_build(&theme, build)
            .into_iter()
            .filter(|d| d.severity >= Severity::Warning)
            .map(|d| format!("{} ({}) : {}", d.code, d.location, d.message))
            .collect();
    }
    Ok((compile, avertissements))
}

/// Construit le réglage des miniatures (mode pourcentage, taille bornée).
pub fn config_miniatures(taille: u32) -> ThumbnailConfig {
    ThumbnailConfig {
        enabled: true,
        mode: ModeMiniatures::Percent,
        percent: taille.clamp(MINIATURES_MIN, MINIATURES_MAX),
        ..ThumbnailConfig::default()
    }
}

/// Assemble la configuration lue par la DLL (`active.json`).
pub fn config_active(theme: CompiledTheme, thumbnails: Option<ThumbnailHook>) -> RuntimeConfig {
    RuntimeConfig {
        protocol: PROTOCOL_VERSION,
        mode: Mode::Apply,
        theme,
        thumbnails,
    }
}

/// Sérialise la configuration active en JSON compact.
pub fn serialiser_active(config: &RuntimeConfig) -> Result<String, String> {
    serde_json::to_string(config).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn souhait(theme: Option<&str>, miniatures: Option<u32>) -> Souhait {
        Souhait {
            theme: theme.map(str::to_owned),
            miniatures,
        }
    }

    #[test]
    fn tout_inactif_ne_fait_rien_sans_session() {
        assert_eq!(
            planifier(None, false, &souhait(None, None)),
            Action::Rien
        );
    }

    #[test]
    fn tout_inactif_arrete_si_session_ou_applique() {
        assert_eq!(planifier(None, true, &souhait(None, None)), Action::Arreter);
        assert_eq!(
            planifier(Some(&souhait(Some("surface"), None)), false, &souhait(None, None)),
            Action::Arreter
        );
    }

    #[test]
    fn miniatures_seules_ne_redemarrent_pas() {
        let applique = souhait(Some("surface"), Some(150));
        assert_eq!(
            planifier(Some(&applique), true, &souhait(Some("surface"), Some(200))),
            Action::MajMiniatures
        );
    }

    #[test]
    fn etat_identique_ne_fait_rien() {
        let applique = souhait(Some("surface"), Some(150));
        assert_eq!(
            planifier(Some(&applique), true, &souhait(Some("surface"), Some(150))),
            Action::Rien
        );
    }

    #[test]
    fn changement_de_theme_ou_session_absente_redemarre() {
        let applique = souhait(Some("surface"), Some(150));
        assert_eq!(
            planifier(Some(&applique), true, &souhait(Some("bubbles"), Some(150))),
            Action::Redemarrer
        );
        // Session morte : il faut tout réinjecter.
        assert_eq!(
            planifier(Some(&applique), false, &souhait(Some("surface"), Some(150))),
            Action::Redemarrer
        );
        // Rien appliqué encore.
        assert_eq!(
            planifier(None, true, &souhait(Some("surface"), None)),
            Action::Redemarrer
        );
    }

    #[test]
    fn empreinte_et_nom_dll_stables() {
        assert_eq!(empreinte(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(empreinte(b"a"), empreinte(b"b"));
        let nom = nom_dll(0x1234_5678_9abc_def0);
        assert_eq!(nom, "rustpiz_runtime-123456789abcdef0.dll");
        assert!(nom.ends_with(".dll"));
    }

    #[test]
    fn anciennes_dll_reconnues() {
        let actuel = nom_dll(0x1234_5678_9abc_def0);
        assert!(est_ancienne_dll("rustpiz_runtime-0000000000000001.dll", &actuel));
        assert!(est_ancienne_dll("rustpiz_runtime-00000000000000ff.dll.part", &actuel));
        assert!(!est_ancienne_dll(&actuel, &actuel), "version courante conservée");
        assert!(!est_ancienne_dll("rustpiz_runtime-123.dll", &actuel));
        assert!(!est_ancienne_dll("rustpiz_runtime-zzzzzzzzzzzzzzzz.dll", &actuel));
        assert!(!est_ancienne_dll("active.json", &actuel));
        assert!(!est_ancienne_dll("autre-0000000000000001.dll", &actuel));
        assert!(!est_ancienne_dll("rustpiz_runtime-0000000000000001.exe", &actuel));
    }

    #[test]
    fn avertissements_de_build() {
        let (_, sans) = compiler_theme_pour_build("surface", 0).unwrap();
        assert!(sans.is_empty(), "build inconnu : aucune vérification");
        let (_, avec) = compiler_theme_pour_build("surface", 21000).unwrap();
        assert!(!avec.is_empty(), "build antérieur à Windows 11 signalé");
        assert!(compiler_theme_pour_build("n-existe-pas", 26100).is_err());
    }

    #[test]
    fn miniatures_bornees() {
        assert_eq!(config_miniatures(50).percent, MINIATURES_MIN);
        assert_eq!(config_miniatures(999).percent, MINIATURES_MAX);
        assert_eq!(config_miniatures(175).percent, 175);
        assert!(config_miniatures(150).enabled);
    }

    #[test]
    fn theme_surface_se_compile_theme_inconnu_echoue() {
        assert!(compiler_theme("surface").is_ok());
        assert!(compiler_theme("n-existe-pas").is_err());
    }

    #[test]
    fn active_json_est_relisible() {
        let theme = compiler_theme("surface").unwrap();
        let config = config_active(theme, None);
        let texte = serialiser_active(&config).unwrap();
        let relu: RuntimeConfig = serde_json::from_str(&texte).unwrap();
        assert_eq!(relu.protocol, PROTOCOL_VERSION);
        assert_eq!(relu.mode, Mode::Apply);
        assert!(relu.thumbnails.is_none());
    }

    #[test]
    fn conflit_et_image_explorer() {
        assert!(conflit_bloque(Action::Redemarrer));
        assert!(!conflit_bloque(Action::MajMiniatures));
        assert!(!conflit_bloque(Action::Rien));
        assert!(!conflit_bloque(Action::Arreter));
        assert!(est_image_explorer(r"C:\Windows\explorer.exe"));
        assert!(est_image_explorer(r"C:\WINDOWS\EXPLORER.EXE"));
        assert!(est_image_explorer("explorer.exe"));
        assert!(!est_image_explorer(r"C:\Tools\fakeexplorer.exe"));
        assert!(!est_image_explorer(r"C:\explorer.exe\autre.exe"));
        assert!(!est_image_explorer(""));
    }
}
