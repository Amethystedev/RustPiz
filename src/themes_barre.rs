//! Catalogue des thèmes de la barre des tâches (v0.6.0).
//!
//! Module pur (aucune dépendance Win32) : identifiants de configuration
//! (`barre_theme` dans `config.toml`) et noms affichés dans la fenêtre de
//! paramètres. Le thème choisi est appliqué par l'intégration de la barre
//! des tâches (`barre`, qui compile le fichier embarqué correspondant de
//! `themes/`).

/// Thème utilisé par défaut et en repli quand un identifiant est inconnu.
pub const THEME_DEFAUT: &str = "surface";

/// Thèmes connus : (identifiant de configuration, nom affiché).
/// Ordre des identifiants ; l'affichage est trié par nom ([`tries_par_nom`]).
pub const THEMES: [(&str, &str); 54] = [
    ("aeris", "Aeris"),
    ("blob", "Blob"),
    ("borderless", "Borderless"),
    ("bottomdensy-noind", "BottomDensy (NoInd)"),
    ("bottomdensy", "BottomDensy"),
    ("bubbles", "Bubbles"),
    ("cleanslate", "CleanSlate"),
    ("command-center", "Command Center"),
    ("docklike", "DockLike"),
    ("fluentglass", "FluentGlass"),
    ("fluid", "Fluid"),
    ("frostedacrylic", "FrostedAcrylic"),
    ("frostyglass", "FrostyGlass"),
    ("layermicaui", "LayerMicaUI"),
    ("liquidglass-legacy-alternate", "LiquidGlass (Legacy - Alternate)"),
    ("liquidglass-legacy", "LiquidGlass (Legacy)"),
    ("liquidglass", "LiquidGlass"),
    ("lucent-accented-bar", "Lucent (Accented Bar)"),
    ("lucent-light-bar", "Lucent (Light Bar)"),
    ("luminosity-classic", "Luminosity (Classic)"),
    ("luminosity-compact", "Luminosity (Compact)"),
    ("luminosity-dock", "Luminosity (Dock)"),
    ("matter", "Matter"),
    ("minecraft-hotbar", "Minecraft Hotbar"),
    ("one-ui-8-5-dock", "One UI 8.5 (Dock)"),
    ("one-ui-8-5-taskbar", "One UI 8.5 (Taskbar)"),
    ("os26-liquid-glass-clear-macdock-compact", "OS26 Liquid Glass (Clear MacDock Compact)"),
    ("os26-liquid-glass-clear-macdock", "OS26 Liquid Glass (Clear MacDock)"),
    ("os26-liquid-glass-clear-taskbar", "OS26 Liquid Glass (Clear Taskbar)"),
    ("os26-liquid-glass-dark-macdock-compact", "OS26 Liquid Glass (Dark MacDock Compact)"),
    ("os26-liquid-glass-dark-macdock", "OS26 Liquid Glass (Dark MacDock)"),
    ("os26-liquid-glass-dark-taskbar", "OS26 Liquid Glass (Dark Taskbar)"),
    ("oversimplified-accentuated", "Oversimplified&Accentuated"),
    ("pills", "Pills"),
    ("plasma", "Plasma"),
    ("rosepine", "RosePine"),
    ("simplytransparent", "SimplyTransparent"),
    ("squircle", "Squircle"),
    ("sunvalley-legacy", "SunValley (Legacy)"),
    ("sunvalley", "SunValley"),
    ("surface", "Surface"),
    ("taskbartostatusbar", "TaskbarToStatusbar"),
    ("taskbarxii", "TaskbarXII"),
    ("tintedglass", "TintedGlass"),
    ("translucenttaskbar", "TranslucentTaskbar"),
    ("ultrawidefriendly", "UltraWideFriendly"),
    ("windowglass-full-length", "WindowGlass (Full-Length)"),
    ("windowglass-split", "WindowGlass (Split)"),
    ("windowglass", "WindowGlass"),
    ("windows7", "Windows7"),
    ("winvista", "WinVista"),
    ("winxp", "WinXP"),
    ("winxp-zune", "WinXP (Zune)"),
    ("xdark", "xdark"),
];

/// Vrai si `id` est l'identifiant d'un thème connu (comparaison exacte).
pub fn existe(id: &str) -> bool {
    THEMES.iter().any(|(i, _)| *i == id)
}

/// Nom affiché du thème `id` ; identifiant inconnu : nom du thème par défaut.
pub fn nom(id: &str) -> &'static str {
    THEMES
        .iter()
        .find(|(i, _)| *i == id)
        .or_else(|| THEMES.iter().find(|(i, _)| *i == THEME_DEFAUT))
        .map(|(_, n)| *n)
        .unwrap_or("Surface")
}

/// Identifiant valide pour `id` : lui-même s'il est connu, sinon
/// [`THEME_DEFAUT`].
pub fn normaliser(id: &str) -> &'static str {
    THEMES
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(i, _)| *i)
        .unwrap_or(THEME_DEFAUT)
}

/// Thèmes triés par nom affiché (insensible à la casse, puis identifiant
/// pour un ordre total), pour la liste de la fenêtre de paramètres.
pub fn tries_par_nom() -> Vec<(&'static str, &'static str)> {
    let mut liste: Vec<(&'static str, &'static str)> = THEMES.to_vec();
    liste.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.0.cmp(b.0))
    });
    liste
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cinquante_quatre_themes_uniques() {
        assert_eq!(THEMES.len(), 54);
        let mut ids: Vec<&str> = THEMES.iter().map(|(i, _)| *i).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 54, "identifiants en double");
        let mut noms: Vec<&str> = THEMES.iter().map(|(_, n)| *n).collect();
        noms.sort_unstable();
        noms.dedup();
        assert_eq!(noms.len(), 54, "noms en double");
    }

    #[test]
    fn identifiants_bien_formes() {
        for (id, nom) in THEMES {
            assert!(!id.is_empty() && !nom.is_empty());
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "identifiant mal formé : {id}"
            );
        }
    }

    #[test]
    fn defaut_existe() {
        assert!(existe(THEME_DEFAUT));
        assert_eq!(nom(THEME_DEFAUT), "Surface");
    }

    #[test]
    fn existe_et_nom() {
        assert!(existe("liquidglass-legacy-alternate"));
        assert_eq!(nom("liquidglass-legacy-alternate"), "LiquidGlass (Legacy - Alternate)");
        assert_eq!(nom("oversimplified-accentuated"), "Oversimplified&Accentuated");
        assert_eq!(nom("xdark"), "xdark");
        assert!(!existe("Surface"), "comparaison sensible à la casse");
        assert!(!existe(""));
        assert!(!existe("inconnu"));
    }

    #[test]
    fn inconnu_replie_sur_surface() {
        assert_eq!(nom("inconnu"), "Surface");
        assert_eq!(normaliser("inconnu"), "surface");
        assert_eq!(normaliser(""), "surface");
        assert_eq!(normaliser("winxp-zune"), "winxp-zune");
    }

    #[test]
    fn tri_par_nom() {
        let liste = tries_par_nom();
        assert_eq!(liste.len(), 54);
        assert_eq!(liste[0], ("aeris", "Aeris"));
        assert_eq!(liste[53], ("xdark", "xdark"));
        for paire in liste.windows(2) {
            assert!(paire[0].1.to_lowercase() <= paire[1].1.to_lowercase());
        }
        // « BottomDensy » avant « BottomDensy (NoInd) » (préfixe).
        let a = liste.iter().position(|t| t.0 == "bottomdensy").expect("présent");
        let b = liste.iter().position(|t| t.0 == "bottomdensy-noind").expect("présent");
        assert!(a < b);
    }
}
