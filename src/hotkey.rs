//! Raccourcis clavier globaux via l'API Win32 `RegisterHotKey`.
//!
//! Les raccourcis sont définis sous forme de texte dans `config.toml`
//! (ex. `Ctrl+Alt+T`), convertis ici en modificateurs Win32 + code de
//! touche virtuelle, puis enregistrés sur la fenêtre de message cachée.
//! Lors de l'appui, Windows envoie un message `WM_HOTKEY` à cette fenêtre.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

use crate::config::Raccourcis;
use crate::logging;

/// Identifiant du raccourci « bascule de la transparence ».
pub const ID_BASCULE: i32 = 1;
/// Identifiant du raccourci « augmenter l'opacité ».
pub const ID_AUGMENTER: i32 = 2;
/// Identifiant du raccourci « diminuer l'opacité ».
pub const ID_DIMINUER: i32 = 3;

/// Convertit un nom de touche en code de touche virtuelle Win32.
fn code_touche(nom: &str) -> Option<u32> {
    let nom = nom.trim().to_lowercase();
    // Touches de fonction F1-F24.
    if let Some(chiffres) = nom.strip_prefix('f') {
        if let Ok(n) = chiffres.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1); // VK_F1 = 0x70
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
    match nom.as_str() {
        "haut" | "up" => Some(0x26),           // VK_UP
        "bas" | "down" => Some(0x28),          // VK_DOWN
        "gauche" | "left" => Some(0x25),       // VK_LEFT
        "droite" | "right" => Some(0x27),      // VK_RIGHT
        "espace" | "space" => Some(0x20),      // VK_SPACE
        "echap" | "esc" => Some(0x1B),         // VK_ESCAPE
        "suppr" | "del" => Some(0x2E),         // VK_DELETE
        "insertion" | "ins" => Some(0x2D),     // VK_INSERT
        "debut" | "home" => Some(0x24),        // VK_HOME
        "fin" | "end" => Some(0x23),           // VK_END
        "pagehaut" | "pageup" => Some(0x21),   // VK_PRIOR
        "pagebas" | "pagedown" => Some(0x22),  // VK_NEXT
        "tab" => Some(0x09),                   // VK_TAB
        _ => None,
    }
}

/// Analyse un raccourci texte (ex. `Ctrl+Alt+T`) en modificateurs Win32
/// et code de touche virtuelle. Retourne `None` si le texte est invalide.
pub fn analyser(raccourci: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut modificateurs = MOD_NOREPEAT; // évite la répétition quand on reste appuyé
    let mut touche: Option<u32> = None;

    for partie in raccourci.split('+') {
        match partie.trim().to_lowercase().as_str() {
            "ctrl" | "controle" | "control" => modificateurs |= MOD_CONTROL,
            "alt" => modificateurs |= MOD_ALT,
            "shift" | "maj" => modificateurs |= MOD_SHIFT,
            "win" | "super" => modificateurs |= MOD_WIN,
            "" => {}
            autre => {
                if touche.is_some() {
                    return None; // plusieurs touches non-modificateurs : invalide
                }
                touche = code_touche(autre);
                if touche.is_none() {
                    return None;
                }
            }
        }
    }

    touche.map(|vk| (modificateurs, vk))
}

/// Enregistre les trois raccourcis globaux sur la fenêtre de message.
/// Les erreurs sont journalisées sans interrompre l'application.
pub unsafe fn enregistrer(hwnd: HWND, raccourcis: &Raccourcis) {
    let definitions = [
        (ID_BASCULE, &raccourcis.bascule),
        (ID_AUGMENTER, &raccourcis.augmenter),
        (ID_DIMINUER, &raccourcis.diminuer),
    ];

    for (id, texte) in definitions {
        match analyser(texte) {
            Some((modificateurs, vk)) => {
                match RegisterHotKey(Some(hwnd), id, modificateurs, vk) {
                    Ok(()) => logging::info(&format!("Raccourci enregistré : {texte}")),
                    Err(e) => logging::erreur(&format!(
                        "Impossible d'enregistrer le raccourci « {texte} » : {e}. \
                         Il est peut-être déjà utilisé par une autre application."
                    )),
                }
            }
            None => logging::erreur(&format!(
                "Raccourci invalide dans la configuration : « {texte} »."
            )),
        }
    }
}

/// Supprime tous les raccourcis enregistrés.
pub unsafe fn liberer(hwnd: HWND) {
    for id in [ID_BASCULE, ID_AUGMENTER, ID_DIMINUER] {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}
