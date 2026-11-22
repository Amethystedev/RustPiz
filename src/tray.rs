//! Icône de la zone de notification (system tray) et menu contextuel.
//!
//! Menu (v0.5.0) : « Paramètres », « Recharger la configuration »,
//! « Quitter » ; double-clic sur l'icône = interrupteur général de la
//! transparence. Tous les réglages sont dans la fenêtre de paramètres
//! (`parametres.rs`), qui s'appuie ici sur `recharger_configuration`,
//! `programmer_effet_dwm` et la suspension des raccourcis globaux.
//!
//! L'application n'a pas de fenêtre visible : on crée une fenêtre de
//! message cachée qui reçoit :
//! - les clics sur l'icône du tray (`Shell_NotifyIconW` + message de rappel) ;
//! - les raccourcis globaux (`WM_HOTKEY`) ;
//! - les minuteurs (`WM_TIMER` : scan des profils, actions différées) ;
//! - la fermeture de session (`WM_QUERYENDSESSION` / `WM_ENDSESSION`).

use std::cell::{Cell, RefCell};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, GetCursorPos, KillTimer, LoadIconW,
    LookupIconIdFromDirectoryEx, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetTimer, TrackPopupMenu, CW_USEDEFAULT, HICON, HMENU,
    IDI_APPLICATION, LR_DEFAULTCOLOR, MB_ICONERROR, MB_OK, MF_SEPARATOR, MF_STRING,
    SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
    WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NULL, WM_QUERYENDSESSION, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW,
};

use crate::animations;
use crate::config;
use crate::dwm::EffetDwm;
use crate::hotkey;
use crate::logging;
use crate::parametres;
use crate::profils;
use crate::window;

/// Message privé envoyé par l'icône du tray à notre fenêtre cachée.
const RAPPEL_TRAY: u32 = WM_APP + 1;
/// Identifiant de notre icône de notification.
const ID_ICONE: u32 = 1;

// Minuteurs.
/// Minuteur du scan périodique des profils (toutes les 2 secondes).
const IDT_SCAN: usize = 1;
/// Minuteur unique des actions différées (3 secondes), le temps de
/// placer le curseur sur la fenêtre visée après « Valider » (effet DWM).
const IDT_DIFFERE: usize = 2;
const DELAI_DIFFERE_MS: u32 = 3000;

// Identifiants des commandes du menu contextuel (v0.5.0 : trois entrées,
// tous les réglages sont dans la fenêtre de paramètres).
const CMD_PARAMETRES: usize = 1003;
const CMD_RECHARGER: usize = 1004;
const CMD_QUITTER: usize = 1005;

/// Icône embarquée dans l'exécutable (format ICO standard, 16+32 px).
const ICONE_OCTETS: &[u8] = include_bytes!("../assets/icon.ico");

/// Action à exécuter après le délai de 3 secondes.
enum ActionDifferee {
    /// Appliquer un effet DWM à la fenêtre sous le curseur (choisi dans la
    /// fenêtre de paramètres).
    Effet(EffetDwm),
}

thread_local! {
    /// Handle de la fenêtre de message (pour y revenir depuis les modules).
    static FENETRE_MSG: Cell<isize> = const { Cell::new(0) };
    /// Action différée en attente.
    static ACTION_DIFFEREE: RefCell<Option<ActionDifferee>> = const { RefCell::new(None) };
    /// Vrai si les raccourcis globaux ont été libérés pendant la capture
    /// d'un raccourci dans la fenêtre de paramètres (à réenregistrer).
    static RACCOURCIS_SUSPENDUS: Cell<bool> = const { Cell::new(false) };
}

/// Convertit une chaîne Rust en chaîne large terminée par NUL (Win32).
fn large(texte: &str) -> Vec<u16> {
    texte.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Charge l'icône de l'application.
///
/// Priorité 1 : la ressource PE embarquée (ID 1) via `LoadIconW`, le
/// chemin standard de Windows — robuste et utilisé aussi pour l'icône
/// du fichier .exe.
/// Priorité 2 : les octets du `.ico` embarqués via
/// `CreateIconFromResourceEx` (secours si la ressource est absente).
unsafe fn charger_icone() -> Option<HICON> {
    // MAKEINTRESOURCEW(1) : l'identifiant déclaré dans assets/ressources.rc.
    if let Ok(module) = GetModuleHandleW(None) {
        let ressource = PCWSTR(1usize as *const u16);
        if let Ok(icone) = LoadIconW(Some(HINSTANCE::from(module)), ressource) {
            if !icone.0.is_null() {
                logging::info("Icône chargée depuis la ressource PE (LoadIconW).");
                return Some(icone);
            }
        }
    }
    logging::avertir("Ressource d'icône introuvable, tentative depuis les octets embarqués.");
    charger_icone_depuis_octets()
}

/// Secours : charge l'icône depuis les octets du `.ico` embarqués.
///
/// `LookupIconIdFromDirectoryEx` choisit la meilleure image du fichier
/// `.ico`, puis `CreateIconFromResourceEx` la convertit en HICON.
unsafe fn charger_icone_depuis_octets() -> Option<HICON> {
    let cx = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CXSMICON);
    let cy = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CYSMICON);
    if ICONE_OCTETS.len() < 6 {
        return None;
    }
    let index = LookupIconIdFromDirectoryEx(ICONE_OCTETS.as_ptr(), true, cx, cy, LR_DEFAULTCOLOR);
    let index = if index < 0 { 0 } else { index as usize };
    // ICONDIRENTRY choisie : base = 6 (ICONDIR) + index * 16.
    let base = 6 + index * 16;
    if base + 16 > ICONE_OCTETS.len() {
        return None;
    }
    let taille = u32::from_le_bytes(ICONE_OCTETS[base + 8..base + 12].try_into().ok()?) as usize;
    let decalage = u32::from_le_bytes(ICONE_OCTETS[base + 12..base + 16].try_into().ok()?) as usize;
    if decalage + taille > ICONE_OCTETS.len() {
        return None;
    }
    let donnees = &ICONE_OCTETS[decalage..decalage + taille];
    CreateIconFromResourceEx(donnees, true, 0x0003_0000, cx, cy, LR_DEFAULTCOLOR)
        .ok()
        .filter(|h| !h.0.is_null())
}

/// Crée la fenêtre de message cachée qui pilote toute l'application.
pub unsafe fn creer_fenetre_message() -> windows::core::Result<HWND> {
    let module = GetModuleHandleW(None)?;
    let instance = HINSTANCE::from(module);

    let classe = WNDCLASSW {
        lpfnWndProc: Some(procedure_fenetre),
        hInstance: instance,
        lpszClassName: w!("TransparentWindowsFenetreMessage"),
        ..Default::default()
    };
    if RegisterClassW(&classe) == 0 {
        logging::avertir("RegisterClassW : classe peut-être déjà enregistrée.");
    }

    let hwnd = CreateWindowExW(
        Default::default(),
        w!("TransparentWindowsFenetreMessage"),
        w!("RustPiz"),
        Default::default(), // aucun style : fenêtre jamais affichée
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        None,
        None,
        Some(instance),
        None,
    )?;
    FENETRE_MSG.with(|c| c.set(hwnd.0 as isize));
    Ok(hwnd)
}

/// Remplit une structure NOTIFYICONDATAW avec les champs communs.
unsafe fn donnees_icone(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut donnees: NOTIFYICONDATAW = std::mem::zeroed();
    donnees.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    donnees.hWnd = hwnd;
    donnees.uID = ID_ICONE;
    donnees
}

/// Ajoute l'icône dans la zone de notification.
pub unsafe fn ajouter_icone(hwnd: HWND) {
    let mut donnees = donnees_icone(hwnd);
    donnees.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    donnees.uCallbackMessage = RAPPEL_TRAY;

    // Icône personnalisée, avec repli sur l'icône système si elle est
    // illisible : l'icône du tray ne doit JAMAIS être invisible.
    donnees.hIcon = charger_icone().unwrap_or_else(|| {
        logging::avertir("Icône embarquée illisible, icône système utilisée.");
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    });

    let info = large("RustPiz");
    let fin = info.len().min(donnees.szTip.len() - 1);
    donnees.szTip[..fin].copy_from_slice(&info[..fin]);

    if !Shell_NotifyIconW(NIM_ADD, &donnees).as_bool() {
        logging::erreur("Shell_NotifyIconW(NIM_ADD) a échoué : icône absente du tray !");
        MessageBoxW(
            Some(hwnd),
            w!("Impossible d'ajouter l'icône dans la zone de notification.\nConsultez le journal : %APPDATA%\\RustPiz\\journal.log"),
            w!("RustPiz — Erreur"),
            MB_OK | MB_ICONERROR,
        );
        return;
    }

    // Comportement moderne (Windows Vista+) : rappels avec coordonnées,
    // info-bulles enrichies.
    donnees.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    if !Shell_NotifyIconW(NIM_SETVERSION, &donnees).as_bool() {
        logging::avertir("Shell_NotifyIconW(NIM_SETVERSION) a échoué (non bloquant).");
    }
    logging::info("Icône ajoutée à la zone de notification.");
}

/// Affiche une notification (bulle/toast) attachée à l'icône du tray.
pub unsafe fn afficher_notification(hwnd: HWND, titre: &str, texte: &str) {
    let mut donnees = donnees_icone(hwnd);
    donnees.uFlags = NIF_INFO;
    donnees.dwInfoFlags = NIIF_INFO;
    let titre = large(titre);
    let fin = titre.len().min(donnees.szInfoTitle.len() - 1);
    donnees.szInfoTitle[..fin].copy_from_slice(&titre[..fin]);
    let texte = large(texte);
    let fin = texte.len().min(donnees.szInfo.len() - 1);
    donnees.szInfo[..fin].copy_from_slice(&texte[..fin]);
    if !Shell_NotifyIconW(NIM_MODIFY, &donnees).as_bool() {
        logging::avertir("Affichage de la notification impossible (NIM_MODIFY).");
    }
}

/// Affiche une notification depuis un autre module (utilise la fenêtre
/// de message mémorisée ; sans effet si elle n'existe pas encore).
pub fn notifier(texte: &str) {
    let brut = FENETRE_MSG.with(|c| c.get());
    if brut != 0 {
        unsafe {
            afficher_notification(
                HWND(brut as *mut core::ffi::c_void),
                "RustPiz",
                texte,
            )
        };
    }
}

/// Retire l'icône de la zone de notification.
pub unsafe fn retirer_icone(hwnd: HWND) {
    let donnees = donnees_icone(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &donnees);
    if !donnees.hIcon.0.is_null() {
        let _ = DestroyIcon(donnees.hIcon);
    }
}

/// Démarre le minuteur du scan périodique des profils.
pub unsafe fn demarrer_minuteurs(hwnd: HWND) {
    if SetTimer(Some(hwnd), IDT_SCAN, 2000, None) == 0 {
        logging::avertir("SetTimer (scan des profils) a échoué.");
    }
}

/// Recharge la configuration et réenregistre les raccourcis
/// (utilisé par le menu et par la fenêtre de paramètres).
pub fn recharger_configuration() {
    let brut = FENETRE_MSG.with(|c| c.get());
    if brut == 0 {
        return;
    }
    let hwnd = HWND(brut as *mut core::ffi::c_void);
    unsafe {
        let nouvelle = config::charger();
        // Pendant une capture (raccourcis suspendus), on ne réenregistre
        // rien : la reprise (avec la nouvelle configuration) se fera à la
        // fin de la capture.
        if !RACCOURCIS_SUSPENDUS.with(|c| c.get()) {
            hotkey::liberer(hwnd);
            hotkey::enregistrer(hwnd, &nouvelle.raccourcis);
        }
        animations::appliquer_config(&nouvelle);
        window::remplacer_config(nouvelle);
    }
}

/// Libère temporairement les raccourcis globaux (début de la capture d'un
/// raccourci dans la fenêtre de paramètres) : sans cela, `RegisterHotKey`
/// intercepterait la combinaison courante avant le champ de saisie, qui ne
/// pourrait jamais la recevoir. Idempotent.
pub fn suspendre_raccourcis() {
    let brut = FENETRE_MSG.with(|c| c.get());
    if brut == 0 || RACCOURCIS_SUSPENDUS.with(|c| c.get()) {
        return;
    }
    unsafe { hotkey::liberer(HWND(brut as *mut core::ffi::c_void)) };
    RACCOURCIS_SUSPENDUS.with(|c| c.set(true));
    logging::info("Raccourcis globaux suspendus (capture dans les paramètres).");
}

/// Réenregistre les raccourcis globaux s'ils avaient été suspendus, avec
/// ceux de la configuration EN MÉMOIRE (ceux qui étaient actifs avant la
/// suspension ; fermeture des paramètres sans enregistrer). Sans effet
/// sinon (après « Valider », `recharger_configuration` l'a déjà fait).
pub fn reprendre_raccourcis() {
    let brut = FENETRE_MSG.with(|c| c.get());
    if brut == 0 || !RACCOURCIS_SUSPENDUS.with(|c| c.get()) {
        return;
    }
    RACCOURCIS_SUSPENDUS.with(|c| c.set(false));
    let hwnd = HWND(brut as *mut core::ffi::c_void);
    unsafe {
        hotkey::liberer(hwnd);
        hotkey::enregistrer(hwnd, &window::raccourcis_actifs());
    }
    logging::info("Raccourcis globaux rétablis (fermeture des paramètres).");
}

/// Programme l'application de l'effet DWM `effet` à la fenêtre qui sera
/// sous le curseur dans 3 secondes (choix validé dans les paramètres).
pub fn programmer_effet_dwm(effet: EffetDwm) {
    let brut = FENETRE_MSG.with(|c| c.get());
    if brut == 0 {
        return;
    }
    unsafe {
        programmer_action_differee(
            HWND(brut as *mut core::ffi::c_void),
            ActionDifferee::Effet(effet),
            &format!(
                "Placez le curseur sur la fenêtre cible : effet « {} » dans 3 secondes…",
                effet.nom()
            ),
        );
    }
}

/// Construit le menu contextuel : trois entrées (tous les réglages sont
/// dans la fenêtre de paramètres ; double-clic sur l'icône = bascule de la
/// transparence).
unsafe fn construire_menu() -> Option<HMENU> {
    let menu = CreatePopupMenu().ok()?;
    let _ = AppendMenuW(menu, MF_STRING, CMD_PARAMETRES, w!("Paramètres"));
    let _ = AppendMenuW(menu, MF_STRING, CMD_RECHARGER, w!("Recharger la configuration"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, CMD_QUITTER, w!("Quitter"));
    Some(menu)
}

/// Affiche le menu contextuel à la position du curseur et traite le choix.
unsafe fn afficher_menu(hwnd: HWND) {
    let Some(menu) = construire_menu() else {
        return;
    };
    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);

    // Nécessaire pour que le menu se referme correctement quand on clique
    // ailleurs (comportement documenté de TrackPopupMenu).
    let _ = SetForegroundWindow(hwnd);

    let commande = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        point.x,
        point.y,
        Some(0),
        hwnd,
        None,
    );

    // Force le traitement du menu avant destruction.
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);

    let id = commande.0 as usize;
    if id != 0 {
        traiter_commande(hwnd, id);
    }
}

/// Programme une action différée de 3 secondes et prévient l'utilisateur.
unsafe fn programmer_action_differee(hwnd: HWND, action: ActionDifferee, message: &str) {
    ACTION_DIFFEREE.with(|a| *a.borrow_mut() = Some(action));
    let _ = KillTimer(Some(hwnd), IDT_DIFFERE);
    if SetTimer(Some(hwnd), IDT_DIFFERE, DELAI_DIFFERE_MS, None) != 0 {
        afficher_notification(hwnd, "RustPiz", message);
    }
}

/// Exécute l'action associée à une commande du menu.
unsafe fn traiter_commande(hwnd: HWND, id: usize) {
    match id {
        CMD_PARAMETRES => parametres::ouvrir(),
        CMD_RECHARGER => recharger_configuration(),
        CMD_QUITTER => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

/// Procédure de fenêtre Win32 : cœur de l'application.
unsafe extern "system" fn procedure_fenetre(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Rappel de l'icône de notification. Avec NOTIFYICON_VERSION_4,
        // le mot de poids faible de lParam contient l'événement souris.
        m if m == RAPPEL_TRAY => {
            let evenement = (lparam.0 & 0xFFFF) as u32;
            if evenement == WM_RBUTTONUP || evenement == WM_CONTEXTMENU {
                afficher_menu(hwnd);
            } else if evenement == WM_LBUTTONDBLCLK {
                window::basculer_activation();
            }
            LRESULT(0)
        }
        // Raccourci global enregistré avec RegisterHotKey.
        WM_HOTKEY => {
            match wparam.0 as i32 {
                hotkey::ID_BASCULE => window::basculer_fenetre_sous_curseur(),
                hotkey::ID_AUGMENTER => window::ajuster_opacite_sous_curseur(10),
                hotkey::ID_DIMINUER => window::ajuster_opacite_sous_curseur(-10),
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                IDT_SCAN => profils::scanner(),
                IDT_DIFFERE => {
                    let _ = KillTimer(Some(hwnd), IDT_DIFFERE);
                    let action = ACTION_DIFFEREE.with(|a| a.borrow_mut().take());
                    if let Some(ActionDifferee::Effet(effet)) = action {
                        window::appliquer_effet_sous_curseur(effet);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            traiter_commande(hwnd, (wparam.0 & 0xFFFF) as usize);
            LRESULT(0)
        }
        // Fermeture de session Windows : on restaure tout avant de partir.
        WM_QUERYENDSESSION => {
            animations::arreter();
            window::restaurer_toutes();
            LRESULT(1) // TRUE : la fermeture de session peut continuer
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                animations::arreter();
                window::restaurer_toutes();
                retirer_icone(hwnd);
            } else {
                // Fermeture de session annulée : on réactive les animations.
                animations::reprendre();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
