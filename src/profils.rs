//! Profils automatiques : application de règles d'opacité et d'effets DWM
//! selon le processus, la classe de fenêtre et/ou le titre.
//!
//! Un minuteur Win32 déclenche `scanner()` toutes les deux secondes :
//! on énumère les fenêtres de premier niveau visibles (`EnumWindows`) et,
//! pour chaque fenêtre pas encore traitée, on cherche le premier profil
//! correspondant et on l'applique. Une fenêtre n'est traitée qu'une seule
//! fois (elle reste dans un ensemble de HWND déjà vus).

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible};

use crate::config::Profil;
use crate::logging;
use crate::window;

/// Vrai si le profil correspond à la fenêtre décrite.
/// Tous les critères non vides doivent correspondre (ET logique) ; un
/// profil sans aucun critère ne correspond jamais.
pub fn profil_correspond(profil: &Profil, processus: &str, classe: &str, titre: &str) -> bool {
    let mut au_moins_un_critere = false;

    if !profil.processus.is_empty() {
        au_moins_un_critere = true;
        if !processus.eq_ignore_ascii_case(&profil.processus) {
            return false;
        }
    }
    if !profil.classe.is_empty() {
        au_moins_un_critere = true;
        if !classe.eq_ignore_ascii_case(&profil.classe) {
            return false;
        }
    }
    if !profil.titre_contient.is_empty() {
        au_moins_un_critere = true;
        if !titre
            .to_lowercase()
            .contains(&profil.titre_contient.to_lowercase())
        {
            return false;
        }
    }
    au_moins_un_critere
}

/// Rappel `EnumWindows` : collecte les fenêtres visibles de premier niveau.
unsafe extern "system" fn rappel_enum(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let liste = &mut *(lparam.0 as *mut Vec<HWND>);
    if IsWindowVisible(hwnd).as_bool() {
        liste.push(hwnd);
    }
    true.into() // continuer l'énumération
}

/// Scan périodique : applique les profils aux nouvelles fenêtres.
pub fn scanner() {
    let mut fenetres: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(rappel_enum),
            LPARAM(&mut fenetres as *mut Vec<HWND> as isize),
        );
    }
    window::nettoyer_fenetres_traitees();
    let mut appliques = 0;
    for hwnd in fenetres {
        if window::appliquer_profil_automatique(hwnd) {
            appliques += 1;
        }
    }
    if appliques > 0 {
        logging::info(&format!("Scan des profils : {appliques} fenêtre(s) traitée(s)."));
    }
}
