//! Icône de la zone de notification (system tray) et menu contextuel.
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
    IDI_APPLICATION, LR_DEFAULTCOLOR, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
    WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NULL, WM_QUERYENDSESSION, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW,
};

use crate::animations::{self, EffetAnimation};
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
/// replacer le curseur sur la fenêtre visée après un clic de menu.
const IDT_DIFFERE: usize = 2;
const DELAI_DIFFERE_MS: u32 = 3000;

// Identifiants des commandes du menu contextuel.
const CMD_BASCULE_ACTIF: usize = 1001;
const CMD_RESTAURER: usize = 1002;
const CMD_PARAMETRES: usize = 1003;
const CMD_RECHARGER: usize = 1004;
const CMD_QUITTER: usize = 1005;
const CMD_IDENTIFIER: usize = 1006;
const CMD_ANIM_OUVERTURE: usize = 1007;
const CMD_ANIM_FERMETURE: usize = 1008;
/// Case « Effet Jelly (déplacement des fenêtres) ».
const CMD_JELLY: usize = 1009;
/// Base des identifiants du sous-menu « Puissance du Jelly » (1400-1404).
const CMD_JELLY_PUISSANCE_BASE: usize = 1400;
/// Base des identifiants du sous-menu « Animation des fenêtres » (1300-1399).
const CMD_ANIM_BASE: usize = 1300;
/// Base des identifiants du sous-menu « Opacité globale ».
const CMD_OPACITE_BASE: usize = 1100;
/// Base des identifiants du sous-menu « Effet DWM ».
const CMD_EFFET_BASE: usize = 1200;

/// Choix d'opacité proposés dans le sous-menu (en %).
const CHOIX_OPACITE: [u8; 6] = [100, 90, 80, 70, 60, 50];
/// Puissances du Jelly proposées dans le sous-menu (en %).
const CHOIX_JELLY: [u8; 5] = [0, 25, 50, 75, 100];

/// Effets DWM proposés dans le sous-menu.
const CHOIX_EFFETS: [EffetDwm; 5] = [
    EffetDwm::Aucun,
    EffetDwm::Mica,
    EffetDwm::MicaAlt,
    EffetDwm::Acrylique,
    EffetDwm::Verre,
];

/// Icône embarquée dans l'exécutable (format ICO standard, 16+32 px).
const ICONE_OCTETS: &[u8] = include_bytes!("../assets/icon.ico");

/// Action à exécuter après le délai de 3 secondes.
enum ActionDifferee {
    /// Identifier la fenêtre sous le curseur (processus/classe/titre).
    Identifier,
    /// Appliquer un effet DWM à la fenêtre sous le curseur.
    Effet(EffetDwm),
}

thread_local! {
    /// Handle de la fenêtre de message (pour y revenir depuis les modules).
    static FENETRE_MSG: Cell<isize> = const { Cell::new(0) };
    /// Action différée en attente.
    static ACTION_DIFFEREE: RefCell<Option<ActionDifferee>> = const { RefCell::new(None) };
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
        hotkey::liberer(hwnd);
        let nouvelle = config::charger();
        hotkey::enregistrer(hwnd, &nouvelle.raccourcis);
        animations::appliquer_config(&nouvelle);
        window::remplacer_config(nouvelle);
    }
}

/// Construit le menu contextuel (reconstruit à chaque affichage pour
/// refléter l'état courant : case cochée, opacité sélectionnée...).
unsafe fn construire_menu() -> Option<HMENU> {
    let menu = CreatePopupMenu().ok()?;

    // Interrupteur général.
    let actif = window::est_actif();
    let mut drapeaux = MF_STRING;
    if actif {
        drapeaux |= MF_CHECKED;
    }
    let _ = AppendMenuW(menu, drapeaux, CMD_BASCULE_ACTIF, w!("Transparence activée"));

    // Sous-menu « Opacité globale ».
    if let Ok(sous_menu) = CreatePopupMenu() {
        let opacite = window::opacite_courante();
        for (index, choix) in CHOIX_OPACITE.iter().enumerate() {
            let etiquette = large(&format!("{choix} %"));
            let mut drapeaux = MF_STRING;
            if *choix == opacite {
                drapeaux |= MF_CHECKED;
            }
            let _ = AppendMenuW(
                sous_menu,
                drapeaux,
                CMD_OPACITE_BASE + index,
                PCWSTR(etiquette.as_ptr()),
            );
        }
        let _ = AppendMenuW(menu, MF_POPUP, sous_menu.0 as usize, w!("Opacité globale"));
    }

    // Sous-menu « Effet DWM » (appliqué à la fenêtre sous le curseur
    // après un délai de 3 secondes, le temps de viser la fenêtre).
    if let Ok(sous_menu) = CreatePopupMenu() {
        for (index, effet) in CHOIX_EFFETS.iter().enumerate() {
            let etiquette = large(effet.nom());
            let _ = AppendMenuW(
                sous_menu,
                MF_STRING,
                CMD_EFFET_BASE + index,
                PCWSTR(etiquette.as_ptr()),
            );
        }
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            sous_menu.0 as usize,
            w!("Effet DWM (fenêtre sous le curseur)"),
        );
    }

    // Animation d'ouverture / fermeture des fenêtres.
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    if let Ok(sous_menu) = CreatePopupMenu() {
        let courant = animations::effet();
        // Effets à shader (v0.4.7) : sous-menu imbriqué « Effets GPU
        // (shaders) », après un séparateur. Les identifiants restent
        // CMD_ANIM_BASE + index dans EffetAnimation::TOUS.
        let shaders = CreatePopupMenu().ok();
        for (index, effet) in EffetAnimation::TOUS.iter().enumerate() {
            let shader = effet.moteur() == animations::Moteur::Shader;
            let mut drapeaux = MF_STRING;
            if *effet == courant {
                drapeaux |= MF_CHECKED;
            }
            // Effet non rendu par ce GPU (flou sans IDCompositionDevice3,
            // shader non compilé).
            let disponible = animations::effet_disponible(*effet);
            if !disponible {
                drapeaux |= MF_GRAYED;
            }
            // « (indisponible : voir journal) » pour un effet à shader non
            // disponible (shader non compilé ou en échec).
            let texte = if shader && !disponible {
                format!("{} (indisponible : voir journal)", effet.libelle_menu())
            } else {
                effet.libelle_menu()
            };
            let etiquette = large(&texte);
            let parent = match (shader, shaders) {
                (true, Some(m)) => m,
                _ => sous_menu,
            };
            let _ = AppendMenuW(
                parent,
                drapeaux,
                CMD_ANIM_BASE + index,
                PCWSTR(etiquette.as_ptr()),
            );
        }
        if let Some(m) = shaders {
            let _ = AppendMenuW(sous_menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(sous_menu, MF_POPUP, m.0 as usize, w!("Effets GPU (shaders)"));
        }
        let _ = AppendMenuW(menu, MF_POPUP, sous_menu.0 as usize, w!("Animation des fenêtres"));
    }
    let indisponible = animations::indisponibilite().is_some();
    for (id, libelle, coche) in [
        (
            CMD_ANIM_OUVERTURE,
            "Animer l'ouverture des fenêtres",
            animations::ouverture_active(),
        ),
        (
            CMD_ANIM_FERMETURE,
            "Animer la fermeture des fenêtres",
            animations::fermeture_active(),
        ),
    ] {
        let mut drapeaux = MF_STRING;
        if coche {
            drapeaux |= MF_CHECKED;
        }
        let texte = if indisponible {
            drapeaux |= MF_GRAYED;
            format!("{libelle} (indisponible)")
        } else {
            libelle.to_string()
        };
        let etiquette = large(&texte);
        let _ = AppendMenuW(menu, drapeaux, id, PCWSTR(etiquette.as_ptr()));
    }

    // Effet Jelly (déformation élastique pendant le déplacement).
    {
        let coche = animations::jelly_actif();
        let mut drapeaux = MF_STRING;
        if coche {
            drapeaux |= MF_CHECKED;
        }
        let libelle = "Effet Jelly (déplacement des fenêtres)";
        let texte = if indisponible {
            drapeaux |= MF_GRAYED;
            format!("{libelle} (indisponible)")
        } else if coche && animations::jelly_desactive() {
            // Non grisé : décocher puis recocher réarme le Jelly.
            format!("{libelle} (désactivé : voir journal)")
        } else if coche && animations::jelly_puissance() == 0 {
            format!("{libelle} (puissance 0 : aucun mouvement)")
        } else {
            libelle.to_string()
        };
        let etiquette = large(&texte);
        let _ = AppendMenuW(menu, drapeaux, CMD_JELLY, PCWSTR(etiquette.as_ptr()));
    }
    if let Ok(sous_menu) = CreatePopupMenu() {
        let palier = animations::jelly_palier();
        for (index, choix) in CHOIX_JELLY.iter().enumerate() {
            let texte = match choix {
                0 => "0 % (aucun mouvement)".to_string(),
                100 => "100 % (très forte déformation)".to_string(),
                v => format!("{v} %"),
            };
            let etiquette = large(&texte);
            let mut drapeaux = MF_STRING;
            if *choix == palier {
                drapeaux |= MF_CHECKED;
            }
            let _ = AppendMenuW(
                sous_menu,
                drapeaux,
                CMD_JELLY_PUISSANCE_BASE + index,
                PCWSTR(etiquette.as_ptr()),
            );
        }
        let _ = AppendMenuW(menu, MF_POPUP, sous_menu.0 as usize, w!("Puissance du Jelly"));
    }

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        CMD_IDENTIFIER,
        w!("Identifier la fenêtre sous le curseur…"),
    );
    let _ = AppendMenuW(menu, MF_STRING, CMD_RESTAURER, w!("Restaurer toutes les fenêtres"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, CMD_PARAMETRES, w!("Paramètres…"));
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

/// Identifie la fenêtre sous le curseur et affiche ses caractéristiques
/// dans une boîte de dialogue (pour écrire des profils facilement).
unsafe fn identifier_fenetre_sous_curseur(hwnd: HWND) {
    let Some(cible) = window::fenetre_sous_curseur() else {
        logging::avertir("Identification : aucune fenêtre sous le curseur.");
        return;
    };
    let (processus, classe, titre) = window::infos_fenetre(cible);
    logging::info(&format!(
        "Identification : processus={}, classe={}, titre={}",
        logging::sans_controle(&processus),
        logging::sans_controle(&classe),
        logging::sans_controle(&titre)
    ));
    let texte = format!(
        "Fenêtre sous le curseur :\r\n\r\n\
         Processus : {processus}\r\n\
         Classe : {classe}\r\n\
         Titre : {titre}\r\n\r\n\
         Exemple de profil pour config.toml :\r\n\
         [[profils]]\r\n\
         nom = \"Mon profil\"\r\n\
         processus = \"{processus}\"\r\n\
         classe = \"{classe}\"\r\n\
         opacite = 85\r\n\
         effet_dwm = \"aucun\""
    );
    let texte = large(&texte);
    MessageBoxW(
        Some(hwnd),
        PCWSTR(texte.as_ptr()),
        w!("RustPiz — Identification de fenêtre"),
        MB_OK | MB_ICONINFORMATION,
    );
}

/// Exécute l'action associée à une commande du menu.
unsafe fn traiter_commande(hwnd: HWND, id: usize) {
    match id {
        CMD_BASCULE_ACTIF => window::basculer_activation(),
        CMD_RESTAURER => window::restaurer_toutes(),
        CMD_PARAMETRES => parametres::ouvrir(),
        CMD_IDENTIFIER => programmer_action_differee(
            hwnd,
            ActionDifferee::Identifier,
            "Placez le curseur sur la fenêtre à identifier (3 secondes)…",
        ),
        CMD_RECHARGER => recharger_configuration(),
        CMD_ANIM_OUVERTURE => animations::basculer_ouverture(),
        CMD_ANIM_FERMETURE => animations::basculer_fermeture(),
        CMD_JELLY => animations::basculer_jelly(),
        _ if (CMD_JELLY_PUISSANCE_BASE..CMD_JELLY_PUISSANCE_BASE + CHOIX_JELLY.len()).contains(&id) => {
            animations::definir_puissance_jelly(CHOIX_JELLY[id - CMD_JELLY_PUISSANCE_BASE]);
        }
        _ if (CMD_ANIM_BASE..CMD_ANIM_BASE + EffetAnimation::TOUS.len()).contains(&id) => {
            animations::definir_effet(EffetAnimation::TOUS[id - CMD_ANIM_BASE]);
        }
        CMD_QUITTER => {
            let _ = DestroyWindow(hwnd);
        }
        _ if (CMD_OPACITE_BASE..CMD_OPACITE_BASE + CHOIX_OPACITE.len()).contains(&id) => {
            let index = id - CMD_OPACITE_BASE;
            window::definir_opacite(CHOIX_OPACITE[index]);
        }
        _ if (CMD_EFFET_BASE..CMD_EFFET_BASE + CHOIX_EFFETS.len()).contains(&id) => {
            let index = id - CMD_EFFET_BASE;
            let effet = CHOIX_EFFETS[index];
            programmer_action_differee(
                hwnd,
                ActionDifferee::Effet(effet),
                &format!(
                    "Placez le curseur sur la fenêtre cible : effet « {} » dans 3 secondes…",
                    effet.nom()
                ),
            );
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
                    match action {
                        Some(ActionDifferee::Identifier) => {
                            identifier_fenetre_sous_curseur(hwnd)
                        }
                        Some(ActionDifferee::Effet(effet)) => {
                            window::appliquer_effet_sous_curseur(effet)
                        }
                        None => {}
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
