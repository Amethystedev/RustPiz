//! Noms de touches et raccourcis clavier au format texte (logique pure).
//!
//! Module sans aucun type Win32 (testable sur n'importe quel hôte) :
//! - [`code_touche`] : nom de touche -> code de touche virtuelle (VK) ;
//! - [`nom_touche`] : code VK -> nom CANONIQUE (relu par [`code_touche`]) ;
//! - [`decomposer`] : texte `Ctrl+Alt+T` -> modificateurs + code VK (base de
//!   `hotkey::analyser`) ;
//! - [`evaluer_appui`] : décision de la capture d'un raccourci dans la
//!   fenêtre de paramètres (touche appuyée + modificateurs tenus).

/// Modificateurs d'un raccourci.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modificateurs {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modificateurs {
    /// Aucun modificateur tenu ?
    pub fn aucun(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.win)
    }

    /// Préfixe texte canonique (`Ctrl+Alt+Shift+Win+`, dans cet ordre).
    pub fn prefixe(self) -> String {
        let mut texte = String::new();
        for (actif, nom) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if actif {
                texte.push_str(nom);
                texte.push('+');
            }
        }
        texte
    }
}

/// Codes VK utiles (valeurs Win32 documentées).
const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_F1: u32 = 0x70;
const VK_F24: u32 = 0x87;

/// Table des touches nommées : (code VK, nom canonique, alias anglais).
const TOUCHES_NOMMEES: [(u32, &str, &str); 13] = [
    (0x26, "Haut", "up"),
    (0x28, "Bas", "down"),
    (0x25, "Gauche", "left"),
    (0x27, "Droite", "right"),
    (0x20, "Espace", "space"),
    (VK_ESCAPE, "Echap", "esc"),
    (0x2E, "Suppr", "del"),
    (0x2D, "Insertion", "ins"),
    (0x24, "Debut", "home"),
    (0x23, "Fin", "end"),
    (0x21, "PageHaut", "pageup"),
    (0x22, "PageBas", "pagedown"),
    (VK_TAB, "Tab", "tab"),
];

/// Convertit un nom de touche en code de touche virtuelle Win32.
pub fn code_touche(nom: &str) -> Option<u32> {
    let nom = nom.trim().to_lowercase();
    // Touches de fonction F1-F24.
    if let Some(chiffres) = nom.strip_prefix('f') {
        if let Ok(n) = chiffres.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(VK_F1 + n - 1);
            }
        }
    }
    // Lettres A-Z et chiffres 0-9 : le code VK correspond au caractère ASCII
    // majuscule.
    if nom.len() == 1 {
        let c = nom.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
    }
    TOUCHES_NOMMEES
        .iter()
        .find(|(_, canonique, alias)| nom == canonique.to_lowercase() || nom == *alias)
        .map(|(vk, _, _)| *vk)
}

/// Nom canonique d'une touche (inverse de [`code_touche`]) ; `None` pour une
/// touche non prise en charge par les raccourcis.
pub fn nom_touche(vk: u32) -> Option<String> {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from_u32(vk).map(|c| c.to_string()),
        VK_F1..=VK_F24 => Some(format!("F{}", vk - VK_F1 + 1)),
        _ => TOUCHES_NOMMEES
            .iter()
            .find(|(code, _, _)| *code == vk)
            .map(|(_, nom, _)| nom.to_string()),
    }
}

/// Vrai pour une touche de modificateur seule (Shift, Ctrl, Alt, Win, en
/// version générique ou gauche/droite).
pub fn est_modificateur(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Vrai pour une touche de fonction F1-F24.
pub fn est_touche_fonction(vk: u32) -> bool {
    (VK_F1..=VK_F24).contains(&vk)
}

/// Analyse un raccourci texte (ex. `Ctrl+Alt+T`) en modificateurs et code
/// de touche virtuelle. `None` si le texte est invalide (aucune touche,
/// plusieurs touches non-modificateurs ou touche inconnue). Les noms de
/// modificateurs sont insensibles à la casse ; les parties vides sont
/// ignorées.
pub fn decomposer(raccourci: &str) -> Option<(Modificateurs, u32)> {
    let mut modificateurs = Modificateurs::default();
    let mut touche: Option<u32> = None;

    for partie in raccourci.split('+') {
        match partie.trim().to_lowercase().as_str() {
            "ctrl" | "controle" | "control" => modificateurs.ctrl = true,
            "alt" => modificateurs.alt = true,
            "shift" | "maj" => modificateurs.shift = true,
            "win" | "super" => modificateurs.win = true,
            "" => {}
            autre => {
                if touche.is_some() {
                    return None; // plusieurs touches non-modificateurs : invalide
                }
                touche = Some(code_touche(autre)?);
            }
        }
    }

    touche.map(|vk| (modificateurs, vk))
}

/// Texte canonique d'un raccourci (ex. `Ctrl+Alt+Haut`), relu à l'identique
/// par [`decomposer`] ; `None` si la touche n'a pas de nom.
pub fn formater(modificateurs: Modificateurs, vk: u32) -> Option<String> {
    nom_touche(vk).map(|nom| format!("{}{nom}", modificateurs.prefixe()))
}

/// Décision de la capture d'un raccourci à l'appui d'une touche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Appui {
    /// Touche de modificateur seule : on continue d'attendre.
    ModificateurSeul,
    /// Echap sans modificateur : capture annulée (ancien texte restauré).
    Annuler,
    /// Tab sans modificateur (`arriere` = Shift+Tab, Shift seul tenu) : on
    /// quitte le champ sans rien changer.
    Quitter { arriere: bool },
    /// Touche ordinaire sans Ctrl/Alt/Win (Shift seul refusé ; seules F1-F24
    /// sont admises seules ou avec Shift).
    SansModificateur,
    /// Touche non prise en charge par les raccourcis.
    Inconnue,
    /// Combinaison acceptée (texte canonique).
    Valide(String),
}

/// Évalue l'appui de la touche `vk` avec les `modificateurs` tenus.
pub fn evaluer_appui(modificateurs: Modificateurs, vk: u32) -> Appui {
    if est_modificateur(vk) {
        return Appui::ModificateurSeul;
    }
    if vk == VK_ESCAPE && modificateurs.aucun() {
        return Appui::Annuler;
    }
    if vk == VK_TAB {
        if modificateurs.aucun() {
            return Appui::Quitter { arriere: false };
        }
        let shift_seul = Modificateurs { shift: true, ..Modificateurs::default() };
        if modificateurs == shift_seul {
            return Appui::Quitter { arriere: true };
        }
    }
    let Some(texte) = formater(modificateurs, vk) else {
        return Appui::Inconnue;
    };
    // Ctrl, Alt ou Win exigé (Shift seul refusé), sauf pour F1-F24.
    if !(modificateurs.ctrl || modificateurs.alt || modificateurs.win) && !est_touche_fonction(vk) {
        return Appui::SansModificateur;
    }
    // Garde-fou : le texte produit doit être relu à l'identique.
    match decomposer(&texte) {
        Some((m, code)) if m == modificateurs && code == vk => Appui::Valide(texte),
        _ => Appui::Inconnue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: Modificateurs = Modificateurs { ctrl: true, alt: true, shift: false, win: false };

    #[test]
    fn noms_canoniques_relus() {
        let mut codes: Vec<u32> = (0x41..=0x5A).chain(0x30..=0x39).chain(VK_F1..=VK_F24).collect();
        codes.extend(TOUCHES_NOMMEES.iter().map(|t| t.0));
        for vk in codes {
            let nom = nom_touche(vk).expect("nom");
            assert_eq!(code_touche(&nom), Some(vk), "{nom}");
        }
        assert_eq!(nom_touche(0x60), None); // pavé numérique 0 : non pris en charge
        assert_eq!(nom_touche(0x10), None);
    }

    #[test]
    fn alias_et_casse() {
        assert_eq!(code_touche("UP"), Some(0x26));
        assert_eq!(code_touche("pagehaut"), Some(0x21));
        assert_eq!(code_touche("PageDown"), Some(0x22));
        assert_eq!(code_touche("f12"), Some(0x7B));
        assert_eq!(code_touche("f25"), None);
        assert_eq!(code_touche("t"), Some(0x54));
        assert_eq!(code_touche("é"), None);
    }

    #[test]
    fn decomposer_comme_avant() {
        assert_eq!(decomposer("Ctrl+Alt+T"), Some((CTRL_ALT, 0x54)));
        assert_eq!(decomposer("ctrl + alt + haut"), Some((CTRL_ALT, 0x26)));
        assert_eq!(decomposer("Maj+Super+F5").map(|(m, _)| (m.shift, m.win)), Some((true, true)));
        assert_eq!(decomposer("T"), Some((Modificateurs::default(), 0x54)));
        assert_eq!(decomposer("+Ctrl++T"), Some((Modificateurs { ctrl: true, ..Default::default() }, 0x54)));
        assert_eq!(decomposer("Ctrl+Alt"), None);
        assert_eq!(decomposer("Ctrl+A+B"), None);
        assert_eq!(decomposer("Ctrl+Inconnue"), None);
        assert_eq!(decomposer(""), None);
    }

    #[test]
    fn formater_aller_retour() {
        let m = Modificateurs { ctrl: true, alt: true, shift: true, win: true };
        assert_eq!(formater(m, 0x26).as_deref(), Some("Ctrl+Alt+Shift+Win+Haut"));
        assert_eq!(formater(CTRL_ALT, 0x54).as_deref(), Some("Ctrl+Alt+T"));
        assert_eq!(formater(CTRL_ALT, 0x28).as_deref(), Some("Ctrl+Alt+Bas"));
        for vk in [0x54, 0x26, 0x70, 0x87, 0x30, 0x22, 0x09, 0x1B] {
            let texte = formater(CTRL_ALT, vk).expect("texte");
            assert_eq!(decomposer(&texte), Some((CTRL_ALT, vk)), "{texte}");
        }
    }

    #[test]
    fn decisions_de_capture() {
        let aucun = Modificateurs::default();
        let shift = Modificateurs { shift: true, ..Default::default() };
        assert_eq!(evaluer_appui(CTRL_ALT, 0x11), Appui::ModificateurSeul);
        assert_eq!(evaluer_appui(aucun, 0xA4), Appui::ModificateurSeul);
        assert_eq!(evaluer_appui(aucun, 0x5B), Appui::ModificateurSeul);
        assert_eq!(evaluer_appui(aucun, 0x1B), Appui::Annuler);
        assert_eq!(evaluer_appui(CTRL_ALT, 0x1B), Appui::Valide("Ctrl+Alt+Echap".into()));
        assert_eq!(evaluer_appui(aucun, 0x09), Appui::Quitter { arriere: false });
        assert_eq!(evaluer_appui(shift, 0x09), Appui::Quitter { arriere: true });
        assert_eq!(evaluer_appui(CTRL_ALT, 0x09), Appui::Valide("Ctrl+Alt+Tab".into()));
        assert_eq!(evaluer_appui(aucun, 0x54), Appui::SansModificateur);
        assert_eq!(evaluer_appui(aucun, 0x0D), Appui::Inconnue); // Entrée : sans nom
        assert_eq!(evaluer_appui(aucun, 0x7B), Appui::Valide("F12".into()));
        assert_eq!(evaluer_appui(shift, 0x41), Appui::SansModificateur);
        assert_eq!(evaluer_appui(shift, 0x26), Appui::SansModificateur);
        assert_eq!(evaluer_appui(shift, 0x74), Appui::Valide("Shift+F5".into()));
        let ctrl = Modificateurs { ctrl: true, ..Default::default() };
        let win = Modificateurs { win: true, ..Default::default() };
        let ctrl_shift = Modificateurs { ctrl: true, shift: true, ..Default::default() };
        assert_eq!(evaluer_appui(ctrl, 0x41), Appui::Valide("Ctrl+A".into()));
        assert_eq!(evaluer_appui(win, 0x41), Appui::Valide("Win+A".into()));
        assert_eq!(evaluer_appui(ctrl_shift, 0x41), Appui::Valide("Ctrl+Shift+A".into()));
        assert_eq!(evaluer_appui(CTRL_ALT, 0x60), Appui::Inconnue);
        assert_eq!(evaluer_appui(CTRL_ALT, 0x26), Appui::Valide("Ctrl+Alt+Haut".into()));
    }
}
