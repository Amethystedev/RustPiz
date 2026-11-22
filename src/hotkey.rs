//! Raccourcis clavier globaux via l'API Win32 `RegisterHotKey`.
//!
//! Les raccourcis sont définis sous forme de texte dans `config.toml`
//! (ex. `Ctrl+Alt+T`), convertis ici en modificateurs Win32 + code de
//! touche virtuelle (analyse pure dans [`crate::touches`]), puis
//! enregistrés sur la fenêtre de message cachée. Lors de l'appui, Windows
//! envoie un message `WM_HOTKEY` à cette fenêtre.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

use crate::config::Raccourcis;
use crate::logging;
use crate::touches;

/// Identifiant du raccourci « bascule de la transparence ».
pub const ID_BASCULE: i32 = 1;
/// Identifiant du raccourci « augmenter l'opacité ».
pub const ID_AUGMENTER: i32 = 2;
/// Identifiant du raccourci « diminuer l'opacité ».
pub const ID_DIMINUER: i32 = 3;

/// Analyse un raccourci texte (ex. `Ctrl+Alt+T`) en modificateurs Win32
/// et code de touche virtuelle. Retourne `None` si le texte est invalide.
pub fn analyser(raccourci: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let (m, vk) = touches::decomposer(raccourci)?;
    let mut modificateurs = MOD_NOREPEAT; // évite la répétition quand on reste appuyé
    for (actif, drapeau) in [
        (m.ctrl, MOD_CONTROL),
        (m.alt, MOD_ALT),
        (m.shift, MOD_SHIFT),
        (m.win, MOD_WIN),
    ] {
        if actif {
            modificateurs |= drapeau;
        }
    }
    Some((modificateurs, vk))
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
                    Err(e) => {
                        logging::erreur(&format!(
                            "Impossible d'enregistrer le raccourci « {texte} » : {e}. \
                             Il est peut-être déjà utilisé par une autre application."
                        ));
                        crate::tray::notifier(&format!(
                            "Raccourci déjà utilisé par une autre application : {texte}"
                        ));
                    }
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
