//! Fenêtre de paramètres native Win32 (aucun framework graphique).
//!
//! Petite fenêtre utilitaire créée avec les contrôles classiques de
//! Windows (STATIC, EDIT, BUTTON, msctls_trackbar32) : opacité par défaut,
//! activation au démarrage et raccourcis clavier. L'enregistrement met à
//! jour `config.toml` puis recharge la configuration à chaud.

use std::cell::Cell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_BAR_CLASSES, TBS_AUTOTICKS, TBM_SETPOS,
    TBM_SETRANGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetDlgItem, GetWindowTextW,
    RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow, BM_GETCHECK, BM_SETCHECK,
    BS_AUTOCHECKBOX, ES_AUTOHSCROLL, SW_SHOW, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_HSCROLL, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};

use crate::config;
use crate::hotkey;
use crate::logging;
use crate::tray;

/// `TBM_GETPOS` (WM_USER) : absent de la crate, défini localement.
const TBM_GETPOS: u32 = 1024;

// Identifiants des contrôles.
const ID_CURSEUR: i32 = 2001;
const ID_VALEUR: i32 = 2002;
const ID_CASE_DEMARRAGE: i32 = 2003;
const ID_CASE_EXPLORATEUR: i32 = 2009;
const ID_EDIT_BASCULE: i32 = 2004;
const ID_EDIT_AUGMENTER: i32 = 2005;
const ID_EDIT_DIMINUER: i32 = 2006;
const ID_ENREGISTRER: i32 = 2007;
const ID_ANNULER: i32 = 2008;

thread_local! {
    /// Handle de la fenêtre de paramètres (0 = fermée).
    static FENETRE: Cell<isize> = const { Cell::new(0) };
}

/// Convertit une chaîne Rust en chaîne large terminée par NUL (Win32).
fn large(texte: &str) -> Vec<u16> {
    texte.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ouvre la fenêtre de paramètres (ou la met au premier plan si déjà ouverte).
pub fn ouvrir() {
    let existant = FENETRE.with(|c| c.get());
    if existant != 0 {
        unsafe {
            let _ = SetForegroundWindow(HWND(existant as *mut core::ffi::c_void));
        }
        return;
    }
    unsafe {
        match creer_fenetre() {
            Ok(hwnd) => {
                FENETRE.with(|c| c.set(hwnd.0 as isize));
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Err(e) => logging::erreur(&format!(
                "Impossible de créer la fenêtre de paramètres : {e}"
            )),
        }
    }
}

/// Crée la fenêtre et ses contrôles.
unsafe fn creer_fenetre() -> windows::core::Result<HWND> {
    let module = GetModuleHandleW(None)?;
    let instance = HINSTANCE::from(module);

    // Nécessaire pour le contrôle trackbar (curseur).
    let mut controles = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_BAR_CLASSES,
    };
    let _ = InitCommonControlsEx(&mut controles);

    let classe = WNDCLASSW {
        lpfnWndProc: Some(procedure_parametres),
        hInstance: instance,
        lpszClassName: w!("TransparentWindowsParametres"),
        ..Default::default()
    };
    let _ = RegisterClassW(&classe);

    CreateWindowExW(
        Default::default(),
        w!("TransparentWindowsParametres"),
        w!("RustPiz — Paramètres"),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        200,
        200,
        500,
        330,
        None,
        None,
        Some(instance),
        None,
    )
}

/// Crée un contrôle enfant et lui applique la police GUI par défaut.
unsafe fn creer_controle(
    parent: HWND,
    classe: PCWSTR,
    texte: &str,
    style_extra: u32,
    exstyle: u32,
    x: i32,
    y: i32,
    largeur: i32,
    hauteur: i32,
    id: i32,
) {
    let texte = large(texte);
    if let Ok(controle) = CreateWindowExW(
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(exstyle),
        classe,
        PCWSTR(texte.as_ptr()),
        WINDOW_STYLE((WS_CHILD | WS_VISIBLE).0 | style_extra),
        x,
        y,
        largeur,
        hauteur,
        Some(parent),
        Some(windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut core::ffi::c_void)),
        None,
        None,
    ) {
        let police = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SendMessageW(
            controle,
            WM_SETFONT,
            Some(WPARAM(police.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

/// Lit le texte d'un contrôle EDIT.
unsafe fn lire_texte(parent: HWND, id: i32) -> String {
    let Ok(controle) = GetDlgItem(Some(parent), id) else {
        return String::new();
    };
    let mut tampon = [0u16; 64];
    let longueur = GetWindowTextW(controle, &mut tampon);
    String::from_utf16_lossy(&tampon[..longueur.max(0) as usize])
}

/// Définit le texte d'un contrôle (STATIC ou EDIT).
unsafe fn definir_texte(parent: HWND, id: i32, texte: &str) {
    if let Ok(controle) = GetDlgItem(Some(parent), id) {
        let texte = large(texte);
        let _ = SendMessageW(
            controle,
            0x000C, // WM_SETTEXT
            Some(WPARAM(0)),
            Some(LPARAM(texte.as_ptr() as isize)),
        );
    }
}

/// Remplit les contrôles avec la configuration courante.
unsafe fn remplir(hwnd: HWND) {
    let cfg = config::charger();

    // Curseur d'opacité : plage 10-100, position initiale.
    if let Ok(curseur) = GetDlgItem(Some(hwnd), ID_CURSEUR) {
        let _ = SendMessageW(
            curseur,
            TBM_SETRANGE,
            Some(WPARAM(1)),
            Some(LPARAM((100 << 16) | 10)),
        );
        let _ = SendMessageW(
            curseur,
            TBM_SETPOS,
            Some(WPARAM(1)),
            Some(LPARAM(cfg.opacite_defaut as isize)),
        );
    }
    definir_texte(hwnd, ID_VALEUR, &format!("{} %", cfg.opacite_defaut));

    if let Ok(case) = GetDlgItem(Some(hwnd), ID_CASE_DEMARRAGE) {
        let coche = if cfg.activer_au_demarrage { 1 } else { 0 };
        let _ = SendMessageW(case, BM_SETCHECK, Some(WPARAM(coche)), Some(LPARAM(0)));
    }
    if let Ok(case) = GetDlgItem(Some(hwnd), ID_CASE_EXPLORATEUR) {
        let coche = if cfg.transparence_explorateur { 1 } else { 0 };
        let _ = SendMessageW(case, BM_SETCHECK, Some(WPARAM(coche)), Some(LPARAM(0)));
    }

    definir_texte(hwnd, ID_EDIT_BASCULE, &cfg.raccourcis.bascule);
    definir_texte(hwnd, ID_EDIT_AUGMENTER, &cfg.raccourcis.augmenter);
    definir_texte(hwnd, ID_EDIT_DIMINUER, &cfg.raccourcis.diminuer);
}

/// Lit les contrôles et enregistre la configuration.
unsafe fn enregistrer(hwnd: HWND) {
    let opacite = GetDlgItem(Some(hwnd), ID_CURSEUR)
        .map(|c| SendMessageW(c, TBM_GETPOS, None, None).0 as u8)
        .unwrap_or(80);
    let activer = GetDlgItem(Some(hwnd), ID_CASE_DEMARRAGE)
        .map(|c| SendMessageW(c, BM_GETCHECK, None, None).0 == 1)
        .unwrap_or(true);
    let explorateur = GetDlgItem(Some(hwnd), ID_CASE_EXPLORATEUR)
        .map(|c| SendMessageW(c, BM_GETCHECK, None, None).0 == 1)
        .unwrap_or(true);
    let bascule = lire_texte(hwnd, ID_EDIT_BASCULE);
    let augmenter = lire_texte(hwnd, ID_EDIT_AUGMENTER);
    let diminuer = lire_texte(hwnd, ID_EDIT_DIMINUER);

    // Validation des raccourcis avant écriture.
    for (nom, texte) in [
        ("bascule", &bascule),
        ("augmenter", &augmenter),
        ("diminuer", &diminuer),
    ] {
        if hotkey::analyser(texte).is_none() {
            logging::erreur(&format!(
                "Raccourci « {nom} » invalide (« {texte} ») : enregistrement annulé."
            ));
            definir_texte(hwnd, ID_VALEUR, "Raccourci invalide !");
            return;
        }
    }

    match config::mettre_a_jour_valeurs(
        opacite,
        activer,
        explorateur,
        &bascule,
        &augmenter,
        &diminuer,
    ) {
        Ok(()) => {
            tray::recharger_configuration();
            let _ = DestroyWindow(hwnd);
        }
        Err(e) => {
            logging::erreur(&format!("Enregistrement des paramètres impossible : {e}"));
            definir_texte(hwnd, ID_VALEUR, "Erreur d'enregistrement !");
        }
    }
}

/// Procédure de fenêtre des paramètres.
unsafe extern "system" fn procedure_parametres(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            // Libellés et champs.
            creer_controle(hwnd, w!("STATIC"), "Opacité par défaut :", 0, 0, 15, 15, 130, 20, 0);
            creer_controle(hwnd, w!("msctls_trackbar32"), "", TBS_AUTOTICKS as u32, 0, 145, 8, 230, 30, ID_CURSEUR);
            creer_controle(hwnd, w!("STATIC"), "80 %", 0, 0, 385, 15, 70, 20, ID_VALEUR);
            creer_controle(hwnd, w!("BUTTON"), "Activer la transparence au démarrage", BS_AUTOCHECKBOX as u32, 0, 15, 50, 340, 22, ID_CASE_DEMARRAGE);
            creer_controle(hwnd, w!("BUTTON"), "Explorateur automatiquement transparent (nouvelles fenêtres)", BS_AUTOCHECKBOX as u32, 0, 15, 74, 400, 22, ID_CASE_EXPLORATEUR);

            creer_controle(hwnd, w!("STATIC"), "Raccourci bascule :", 0, 0, 15, 110, 130, 20, 0);
            creer_controle(hwnd, w!("EDIT"), "", ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE.0, 150, 107, 160, 24, ID_EDIT_BASCULE);
            creer_controle(hwnd, w!("STATIC"), "Raccourci opacité + :", 0, 0, 15, 142, 130, 20, 0);
            creer_controle(hwnd, w!("EDIT"), "", ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE.0, 150, 139, 160, 24, ID_EDIT_AUGMENTER);
            creer_controle(hwnd, w!("STATIC"), "Raccourci opacité - :", 0, 0, 15, 174, 130, 20, 0);
            creer_controle(hwnd, w!("EDIT"), "", ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE.0, 150, 171, 160, 24, ID_EDIT_DIMINUER);

            creer_controle(hwnd, w!("STATIC"), "Format : Ctrl/Alt/Shift/Win + touche (ex. Ctrl+Alt+T, Ctrl+Alt+Haut).", 0, 0, 15, 210, 450, 20, 0);

            creer_controle(hwnd, w!("BUTTON"), "Enregistrer", 0, 0, 130, 240, 110, 28, ID_ENREGISTRER);
            creer_controle(hwnd, w!("BUTTON"), "Annuler", 0, 0, 255, 240, 110, 28, ID_ANNULER);

            remplir(hwnd);
            LRESULT(0)
        }
        WM_HSCROLL => {
            // Le curseur d'opacité a bougé : mise à jour de l'étiquette.
            if let Ok(curseur) = GetDlgItem(Some(hwnd), ID_CURSEUR) {
                let valeur = SendMessageW(curseur, TBM_GETPOS, None, None).0;
                definir_texte(hwnd, ID_VALEUR, &format!("{valeur} %"));
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            match id {
                ID_ENREGISTRER => enregistrer(hwnd),
                ID_ANNULER => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            FENETRE.with(|c| c.set(0));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

// Ré-export silencieux pour garder `WS_BORDER` documenté (style EDIT alternatif).
#[allow(dead_code)]
const _STYLE_BORDURE_EDIT: u32 = WS_BORDER.0;
