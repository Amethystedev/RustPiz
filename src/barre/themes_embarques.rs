//! Thèmes de la barre des tâches embarqués dans l'exécutable (v0.6.0).
//!
//! Module pur : les 54 fichiers `themes/<id>.toml` (portés de RustHawk,
//! GPL-3.0, voir `crates/LICENSE-GPL3-NOTICE.md`) sont inclus par
//! `include_str!`, un par identifiant de [`crate::themes_barre::THEMES`]
//! (même ordre). L'exécutable reste autonome : aucun fichier de thème n'est
//! lu sur le disque à l'exécution.

/// (identifiant, contenu TOML) — même ordre que `themes_barre::THEMES`.
const CONTENUS: [(&str, &str); 54] = [
    ("aeris", include_str!("../../themes/aeris.toml")),
    ("blob", include_str!("../../themes/blob.toml")),
    ("borderless", include_str!("../../themes/borderless.toml")),
    (
        "bottomdensy-noind",
        include_str!("../../themes/bottomdensy-noind.toml"),
    ),
    ("bottomdensy", include_str!("../../themes/bottomdensy.toml")),
    ("bubbles", include_str!("../../themes/bubbles.toml")),
    ("cleanslate", include_str!("../../themes/cleanslate.toml")),
    (
        "command-center",
        include_str!("../../themes/command-center.toml"),
    ),
    ("docklike", include_str!("../../themes/docklike.toml")),
    ("fluentglass", include_str!("../../themes/fluentglass.toml")),
    ("fluid", include_str!("../../themes/fluid.toml")),
    (
        "frostedacrylic",
        include_str!("../../themes/frostedacrylic.toml"),
    ),
    ("frostyglass", include_str!("../../themes/frostyglass.toml")),
    ("layermicaui", include_str!("../../themes/layermicaui.toml")),
    (
        "liquidglass-legacy-alternate",
        include_str!("../../themes/liquidglass-legacy-alternate.toml"),
    ),
    (
        "liquidglass-legacy",
        include_str!("../../themes/liquidglass-legacy.toml"),
    ),
    ("liquidglass", include_str!("../../themes/liquidglass.toml")),
    (
        "lucent-accented-bar",
        include_str!("../../themes/lucent-accented-bar.toml"),
    ),
    (
        "lucent-light-bar",
        include_str!("../../themes/lucent-light-bar.toml"),
    ),
    (
        "luminosity-classic",
        include_str!("../../themes/luminosity-classic.toml"),
    ),
    (
        "luminosity-compact",
        include_str!("../../themes/luminosity-compact.toml"),
    ),
    (
        "luminosity-dock",
        include_str!("../../themes/luminosity-dock.toml"),
    ),
    ("matter", include_str!("../../themes/matter.toml")),
    (
        "minecraft-hotbar",
        include_str!("../../themes/minecraft-hotbar.toml"),
    ),
    (
        "one-ui-8-5-dock",
        include_str!("../../themes/one-ui-8-5-dock.toml"),
    ),
    (
        "one-ui-8-5-taskbar",
        include_str!("../../themes/one-ui-8-5-taskbar.toml"),
    ),
    (
        "os26-liquid-glass-clear-macdock-compact",
        include_str!("../../themes/os26-liquid-glass-clear-macdock-compact.toml"),
    ),
    (
        "os26-liquid-glass-clear-macdock",
        include_str!("../../themes/os26-liquid-glass-clear-macdock.toml"),
    ),
    (
        "os26-liquid-glass-clear-taskbar",
        include_str!("../../themes/os26-liquid-glass-clear-taskbar.toml"),
    ),
    (
        "os26-liquid-glass-dark-macdock-compact",
        include_str!("../../themes/os26-liquid-glass-dark-macdock-compact.toml"),
    ),
    (
        "os26-liquid-glass-dark-macdock",
        include_str!("../../themes/os26-liquid-glass-dark-macdock.toml"),
    ),
    (
        "os26-liquid-glass-dark-taskbar",
        include_str!("../../themes/os26-liquid-glass-dark-taskbar.toml"),
    ),
    (
        "oversimplified-accentuated",
        include_str!("../../themes/oversimplified-accentuated.toml"),
    ),
    ("pills", include_str!("../../themes/pills.toml")),
    ("plasma", include_str!("../../themes/plasma.toml")),
    ("rosepine", include_str!("../../themes/rosepine.toml")),
    (
        "simplytransparent",
        include_str!("../../themes/simplytransparent.toml"),
    ),
    ("squircle", include_str!("../../themes/squircle.toml")),
    (
        "sunvalley-legacy",
        include_str!("../../themes/sunvalley-legacy.toml"),
    ),
    ("sunvalley", include_str!("../../themes/sunvalley.toml")),
    ("surface", include_str!("../../themes/surface.toml")),
    (
        "taskbartostatusbar",
        include_str!("../../themes/taskbartostatusbar.toml"),
    ),
    ("taskbarxii", include_str!("../../themes/taskbarxii.toml")),
    ("tintedglass", include_str!("../../themes/tintedglass.toml")),
    (
        "translucenttaskbar",
        include_str!("../../themes/translucenttaskbar.toml"),
    ),
    (
        "ultrawidefriendly",
        include_str!("../../themes/ultrawidefriendly.toml"),
    ),
    (
        "windowglass-full-length",
        include_str!("../../themes/windowglass-full-length.toml"),
    ),
    (
        "windowglass-split",
        include_str!("../../themes/windowglass-split.toml"),
    ),
    ("windowglass", include_str!("../../themes/windowglass.toml")),
    ("windows7", include_str!("../../themes/windows7.toml")),
    ("winvista", include_str!("../../themes/winvista.toml")),
    ("winxp", include_str!("../../themes/winxp.toml")),
    ("winxp-zune", include_str!("../../themes/winxp-zune.toml")),
    ("xdark", include_str!("../../themes/xdark.toml")),
];

/// Contenu TOML du thème `id`, ou `None` si l'identifiant est inconnu.
pub fn contenu(id: &str) -> Option<&'static str> {
    CONTENUS
        .iter()
        .find(|(cle, _)| *cle == id)
        .map(|(_, texte)| *texte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes_barre::THEMES;
    use std::collections::BTreeSet;

    /// Dossier des thèmes à la racine du dépôt.
    const DOSSIER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/themes");

    #[test]
    fn chaque_theme_du_catalogue_a_un_contenu() {
        for (id, _) in THEMES {
            let texte = contenu(id).unwrap_or_else(|| panic!("thème sans contenu : {id}"));
            assert!(!texte.trim().is_empty(), "thème vide : {id}");
        }
        assert_eq!(CONTENUS.len(), THEMES.len());
        for ((a, _), (b, _)) in CONTENUS.iter().zip(THEMES.iter()) {
            assert_eq!(a, b, "ordre différent de themes_barre::THEMES");
        }
        assert!(contenu("inconnu").is_none());
    }

    #[test]
    fn identifiants_et_fichiers_concordent() {
        let fichiers: BTreeSet<String> = std::fs::read_dir(DOSSIER)
            .expect("dossier themes/ illisible")
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|nom| nom.strip_suffix(".toml").map(str::to_owned))
            .collect();
        let ids: BTreeSet<String> = THEMES.iter().map(|(id, _)| (*id).to_owned()).collect();
        assert_eq!(fichiers, ids);
        // Le contenu embarqué est bien celui du fichier du même nom.
        for (id, texte) in CONTENUS {
            let disque = std::fs::read_to_string(format!("{DOSSIER}/{id}.toml")).unwrap();
            assert_eq!(disque, texte, "{id}");
        }
    }

    #[test]
    fn chaque_theme_embarque_se_compile() {
        let mut echecs = Vec::new();
        for (id, texte) in CONTENUS {
            let theme = match rustpiz_theme::Theme::from_toml(texte) {
                Ok(t) => t,
                Err(e) => {
                    echecs.push(format!("{id} : lecture : {e}"));
                    continue;
                }
            };
            let (compile, diagnostics) = rustpiz_theme::validate(&theme, &[]);
            if compile.rules.is_empty() {
                echecs.push(format!("{id} : aucune règle compilée"));
            }
            for d in diagnostics
                .iter()
                .filter(|d| d.severity == rustpiz_theme::Severity::Error)
            {
                echecs.push(format!("{id} : {d:?}"));
            }
        }
        assert!(echecs.is_empty(), "{echecs:#?}");
    }
}
