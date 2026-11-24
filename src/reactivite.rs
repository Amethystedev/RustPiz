//! Application événementielle des règles automatiques (profils et
//! transparence de l'Explorateur) dès l'affichage d'une fenêtre, sans
//! attendre le scan périodique de secours (`profils::scanner`).
//!
//! Un crochet `SetWinEventHook` **hors contexte** (`WINEVENT_OUTOFCONTEXT`
//! | `WINEVENT_SKIPOWNPROCESS` : aucune injection, nos propres fenêtres
//! ignorées) écoute `EVENT_OBJECT_SHOW`. `EVENT_OBJECT_CREATE` n'est pas
//! écouté : à la création, la fenêtre est invisible et son titre souvent
//! vide, le SHOW est le premier moment utile.
//!
//! Le rappel ne fait QUE filtrer (`OBJID_WINDOW`, `CHILDID_SELF`, fenêtre
//! racine de premier niveau) puis poster [`WM_FENETRE_AFFICHEE`] à la
//! fenêtre de message (`tray.rs`) avec le HWND (wParam) et l'horodatage de
//! l'événement (lParam). Aucun journal, aucune autre logique. Le nombre de
//! messages en vol est borné ([`MAX_EN_VOL`]) : au-delà, l'événement est
//! ignoré (le scan de secours rattrapera la fenêtre).
//!
//! Le traitement ([`traiter`], fil principal) appelle
//! `window::appliquer_profil_evenementiel` : si aucune règle ne s'applique,
//! la fenêtre reste à évaluer par le scan. Quand « Animer l'ouverture » est
//! cochée, le chemin événementiel est inactif (collision avec le masquage
//! précoce d'ouverture : fenêtre déjà transparente jouée opaque) et le scan
//! reste le seul mécanisme ; le réglage est relu à chaque message, donc un
//! rechargement de configuration est pris en compte sans réinstallation.
//!
//! Crochet permanent (bon marché), retiré à l'arrêt ([`arreter`]). Échec
//! d'installation : rien n'est installé, repli sur le scan, une ligne de
//! journal.

use std::cell::Cell;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, IsWindow, PostMessageW, CHILDID_SELF, EVENT_OBJECT_SHOW, GA_ROOT, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
};

use crate::animations;
use crate::logging;
use crate::window;

/// Message privé posté à la fenêtre de message (`tray.rs`) :
/// wParam = HWND affiché, lParam = horodatage de l'événement (ms, base
/// `GetTickCount`). `WM_APP + 1` est le rappel de l'icône du tray.
pub const WM_FENETRE_AFFICHEE: u32 = WM_APP + 2;

/// Nombre maximal de messages [`WM_FENETRE_AFFICHEE`] postés et pas encore
/// traités (protection contre une inondation d'événements SHOW).
pub const MAX_EN_VOL: u32 = 32;

thread_local! {
    /// Crochet installé (valeur brute de `HWINEVENTHOOK`, 0 = aucun).
    static CROCHET: Cell<isize> = const { Cell::new(0) };
    /// Fenêtre de message destinataire (0 = aucune).
    static DESTINATAIRE: Cell<isize> = const { Cell::new(0) };
    /// Messages postés non encore traités.
    static EN_VOL: Cell<u32> = const { Cell::new(0) };
    /// Dernier état journalisé du chemin événementiel (actif = ouverture
    /// animée décochée) ; `None` = jamais journalisé.
    static DERNIER_MODE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Décision pure : peut-on poster un nouveau message ?
pub fn peut_poster(en_vol: u32) -> bool {
    en_vol < MAX_EN_VOL
}

/// Décision pure : le chemin événementiel doit-il agir ? Non si
/// « Animer l'ouverture » est cochée (le scan de secours s'en charge).
pub fn chemin_evenementiel_actif(ouverture_animee: bool) -> bool {
    !ouverture_animee
}

/// Installe le crochet (à appeler une fois, sur le fil de la boucle de
/// messages, après la création de la fenêtre de message). En cas
/// d'échec : rien n'est installé, une ligne de journal, repli sur le scan.
pub fn installer(destinataire: HWND) {
    if CROCHET.with(|c| c.get()) != 0 {
        return;
    }
    DESTINATAIRE.with(|d| d.set(destinataire.0 as isize));
    EN_VOL.with(|e| e.set(0));
    let crochet = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(rappel),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if crochet.0.is_null() {
        let erreur = windows::core::Error::from_thread();
        DESTINATAIRE.with(|d| d.set(0));
        logging::avertir(&format!(
            "Réactivité : crochet EVENT_OBJECT_SHOW impossible ({erreur}) — \
             règles automatiques appliquées par le scan périodique seulement."
        ));
        return;
    }
    CROCHET.with(|c| c.set(crochet.0 as isize));
    logging::info(
        "Réactivité : crochet EVENT_OBJECT_SHOW installé (règles automatiques appliquées \
         dès l'affichage, scan périodique en secours).",
    );
}

/// Retire le crochet (arrêt). Idempotent.
pub fn arreter() {
    let brut = CROCHET.with(|c| c.replace(0));
    DESTINATAIRE.with(|d| d.set(0));
    if brut != 0 {
        unsafe {
            let _ = UnhookWinEvent(HWINEVENTHOOK(brut as *mut core::ffi::c_void));
        }
    }
}

/// Rappel WinEvent (appelé par notre boucle de messages, hors contexte).
/// Filtre + `PostMessageW` seulement ; ne panique jamais, ne journalise pas.
unsafe extern "system" fn rappel(
    _crochet: HWINEVENTHOOK,
    _evenement: u32,
    hwnd: HWND,
    id_objet: i32,
    id_enfant: i32,
    _fil: u32,
    horodatage: u32,
) {
    if hwnd.0.is_null() || id_objet != OBJID_WINDOW.0 || id_enfant != CHILDID_SELF as i32 {
        return;
    }
    // Fenêtres racines de premier niveau seulement (pas les contrôles).
    if GetAncestor(hwnd, GA_ROOT) != hwnd {
        return;
    }
    let destinataire = DESTINATAIRE.with(|d| d.get());
    if destinataire == 0 || !peut_poster(EN_VOL.with(|e| e.get())) {
        return;
    }
    if PostMessageW(
        Some(HWND(destinataire as *mut core::ffi::c_void)),
        WM_FENETRE_AFFICHEE,
        WPARAM(hwnd.0 as usize),
        LPARAM(horodatage as isize),
    )
    .is_ok()
    {
        EN_VOL.with(|e| e.set(e.get().saturating_add(1)));
    }
}

/// Traitement de [`WM_FENETRE_AFFICHEE`] (fil principal, fenêtre de
/// message).
pub fn traiter(wparam: WPARAM, lparam: LPARAM) {
    EN_VOL.with(|e| e.set(e.get().saturating_sub(1)));
    let actif = chemin_evenementiel_actif(animations::ouverture_active());
    if DERNIER_MODE.with(|m| m.replace(Some(actif))) != Some(actif) {
        logging::info(if actif {
            "Réactivité : règles automatiques appliquées dès l'affichage des fenêtres."
        } else {
            "Réactivité : « Animer l'ouverture » cochée — application événementielle \
             suspendue, scan périodique seul."
        });
    }
    if !actif {
        return;
    }
    let hwnd = HWND(wparam.0 as *mut core::ffi::c_void);
    if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return;
    }
    if window::appliquer_profil_evenementiel(hwnd) {
        let delai = unsafe { GetTickCount() }.wrapping_sub(lparam.0 as u32);
        logging::info(&format!(
            "Réactivité : règle appliquée à {hwnd:?} dès l'affichage ({delai} ms après \
             l'événement SHOW)."
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borne_des_messages_en_vol() {
        assert!(peut_poster(0));
        assert!(peut_poster(MAX_EN_VOL - 1));
        assert!(!peut_poster(MAX_EN_VOL));
        assert!(!peut_poster(u32::MAX));
    }

    #[test]
    fn suspendu_si_ouverture_animee() {
        assert!(chemin_evenementiel_actif(false));
        assert!(!chemin_evenementiel_actif(true));
    }
}
