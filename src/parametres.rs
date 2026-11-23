//! Fenêtre de paramètres native Win32 (aucun framework graphique), v0.6.0.
//!
//! Tous les réglages de l'application (l'ancien menu de l'icône n'a plus
//! que « Paramètres », « Recharger la configuration » et « Quitter ») sont
//! répartis dans cinq onglets (`SysTabControl32`, `TCS_MULTILINE` : les
//! titres tiennent sur deux rangées) :
//! - « Param Transparence » : interrupteur général (non persisté, comme
//!   l'ancienne entrée de menu), transparence au démarrage, Explorateur
//!   automatiquement transparent, opacité par défaut (10-100 %), effet DWM à
//!   appliquer à la fenêtre sous le curseur 3 s après « Valider » (action
//!   ponctuelle, aucune clé de configuration), trois raccourcis globaux saisis
//!   par capture clavier ;
//! - « Param Jelly » : activation et puissance (0-100 %, pas de 1) ;
//! - « Param Effets Ouverture/Fermeture » : animation de l'ouverture, de la
//!   fermeture, choix de l'effet (tous les effets, shaders compris) ;
//! - « Param Barre des Taches » (v0.6.0) : activation et thème de la barre
//!   des tâches (54 thèmes de `themes_barre`, triés par nom) ;
//! - « Param miniatures » (v0.6.0) : activation et taille (100-300 %) des
//!   miniatures personnalisées.
//!
//! Les deux derniers onglets enregistrent leurs réglages ; à la validation,
//! `tray::recharger_configuration` les transmet à l'intégration de la barre
//! des tâches (`barre::appliquer`, fil dédié). Une ligne de texte discrète
//! rappelle dans chacun les conditions d'application.
//!
//! Les contrôles des pages sont des enfants DIRECTS de la fenêtre,
//! affichés/masqués selon l'onglet (`TCN_SELCHANGE`) ; l'onglet est placé
//! en bas de l'ordre Z (sinon il recouvrirait les contrôles et capterait les
//! clics). Les interrupteurs sont des boutons `BS_OWNERDRAW` dessinés en GDI
//! (état dans `GWLP_USERDATA` du bouton : bit 0 = activé, bit 1 = touché
//! pendant cette ouverture de la fenêtre). Police : police des messages du
//! système ; dimensions mises à l'échelle du DPI système (processus « system
//! DPI aware »). Navigation clavier : la boucle de messages de `main.rs`
//! passe les messages par `IsDialogMessageW` pour CETTE fenêtre seulement
//! ([`traiter_message_dialogue`]) : Tab, Entrée (= Valider, ou Fermer si ce
//! bouton a le focus) et Echap (= Fermer).
//!
//! Capture d'un raccourci : les champs sont des EDIT en lecture seule
//! sous-classés (`SetWindowSubclass`). Au focus (ou au clic), le champ passe
//! en capture : les raccourcis globaux sont SUSPENDUS
//! (`tray::suspendre_raccourcis`, sinon `RegisterHotKey` intercepterait la
//! combinaison courante avant le champ), les touches sont interceptées
//! (`WM_KEYDOWN`/`WM_SYSKEYDOWN`, caractères avalés, `DLGC_WANTALLKEYS`), et
//! la combinaison est décidée par `touches::evaluer_appui` puis vérifiée par
//! `hotkey::analyser`. Echap seul annule, Tab (ou Shift+Tab) seul quitte le
//! champ, une perte de focus sans combinaison valide restaure l'ancien texte.
//! Les raccourcis globaux sont réenregistrés par « Valider » (rechargement
//! de la configuration) ou à la destruction de la fenêtre
//! (`tray::reprendre_raccourcis`, sans effet s'ils n'étaient pas suspendus).
//!
//! « Valider » écrit `config.toml` en une seule écriture atomique
//! (`config::enregistrer_parametres`), demande les réarmements de l'ancien
//! menu (`animations::rearmer`), recharge la configuration à chaud
//! (`tray::recharger_configuration`), applique l'interrupteur général et
//! programme l'effet DWM éventuel. « Fermer », la croix et Echap ferment
//! sans rien enregistrer.

use std::cell::{Cell, RefCell};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteObject, DrawFocusRect, DrawTextW,
    Ellipse, FillRect, GetMonitorInfoW, GetStockObject, GetSysColor, GetSysColorBrush,
    InvalidateRect, MonitorFromPoint, RoundRect, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_GUI_FONT,
    DT_CALCRECT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HBRUSH,
    HDC, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, DRAWITEMSTRUCT, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES,
    ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, NMHDR, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT,
    ODT_BUTTON, TBM_SETLINESIZE, TBM_SETPAGESIZE, TBM_SETPOS, TBM_SETRANGE, TBM_SETTICFREQ,
    TBS_AUTOTICKS, TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT, TCM_GETCURSEL, TCM_INSERTITEMW,
    TCN_SELCHANGE, TCS_MULTILINE, WC_TABCONTROLW,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos,
    GetDlgCtrlID, GetDlgItem, GetNextDlgTabItem, GetParent, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowTextW, HideCaret, IsChild, IsDialogMessageW, IsWindow,
    IsWindowVisible, LoadCursorW, LoadIconW, RegisterClassW, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, BN_CLICKED, BN_DOUBLECLICKED,
    BS_DEFPUSHBUTTON, BS_OWNERDRAW, BS_PUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, DC_HASDEFID, DLGC_HASSETSEL, DLGC_WANTALLKEYS,
    DLGC_WANTARROWS, DLGC_WANTCHARS, DLGC_WANTTAB, DM_GETDEFID, ES_AUTOHSCROLL, ES_CENTER,
    ES_READONLY, GWLP_USERDATA, HMENU, HWND_BOTTOM, IDC_ARROW, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOW, WA_INACTIVE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_CREATE,
    WM_CTLCOLORSTATIC, WM_DEADCHAR, WM_DESTROY, WM_DRAWITEM, WM_GETDLGCODE, WM_GETFONT,
    WM_HSCROLL, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_NCDESTROY, WM_NOTIFY,
    WM_SETFOCUS, WM_SETFONT, WM_SETTEXT, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_GROUP,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::animations::{self, EffetAnimation, Moteur};
use crate::config::{self, Parametres, Raccourcis};
use crate::dwm::EffetDwm;
use crate::hotkey;
use crate::logging;
use crate::themes_barre;
use crate::touches::{self, Appui, Modificateurs};
use crate::tray;
use crate::window;

/// `TBM_GETPOS` (WM_USER) : absent de la crate, défini localement.
const TBM_GETPOS: u32 = 1024;

// Styles des contrôles STATIC (valeurs Win32 documentées, absentes sous ces
// noms de la crate).
const SS_NOPREFIX: u32 = 0x0080;
const SS_CENTERIMAGE: u32 = 0x0200;
const SS_ENDELLIPSIS: u32 = 0x4000;

// Identifiants des contrôles. Valider = IDOK et Fermer = IDCANCEL :
// `IsDialogMessageW` envoie ces commandes pour Entrée et Echap.
const ID_VALIDER: i32 = 1;
const ID_FERMER: i32 = 2;
const ID_ONGLETS: i32 = 3001;
const ID_MESSAGE: i32 = 3002;
// Page 1 : transparence.
const ID_SW_ACTIF: i32 = 3101;
const ID_SW_DEMARRAGE: i32 = 3102;
const ID_SW_EXPLORATEUR: i32 = 3103;
const ID_LBL_OPACITE: i32 = 3104;
const ID_TB_OPACITE: i32 = 3105;
const ID_VAL_OPACITE: i32 = 3106;
const ID_LBL_DWM: i32 = 3107;
const ID_CB_DWM: i32 = 3108;
const ID_AIDE_DWM: i32 = 3109;
const ID_LBL_BASCULE: i32 = 3110;
const ID_ED_BASCULE: i32 = 3111;
const ID_LBL_DIMINUER: i32 = 3112;
const ID_ED_DIMINUER: i32 = 3113;
const ID_LBL_AUGMENTER: i32 = 3114;
const ID_ED_AUGMENTER: i32 = 3115;
const ID_AIDE_RACCOURCIS: i32 = 3116;
// Page 2 : Jelly.
const ID_SW_JELLY: i32 = 3201;
const ID_LBL_JELLY: i32 = 3202;
const ID_TB_JELLY: i32 = 3203;
const ID_VAL_JELLY: i32 = 3204;
const ID_ETAT_JELLY: i32 = 3205;
// Page 3 : effets d'ouverture / fermeture.
const ID_SW_OUVERTURE: i32 = 3301;
const ID_SW_FERMETURE: i32 = 3302;
const ID_LBL_EFFET: i32 = 3303;
const ID_CB_EFFET: i32 = 3304;
const ID_ETAT_EFFETS: i32 = 3305;
// Page 4 : barre des tâches (v0.6.0).
const ID_SW_BARRE: i32 = 3401;
const ID_LBL_THEME: i32 = 3402;
const ID_CB_THEME: i32 = 3403;
const ID_NOTE_BARRE: i32 = 3404;
// Page 5 : miniatures (v0.6.0).
const ID_SW_MINIATURES: i32 = 3501;
const ID_LBL_MINIATURES: i32 = 3502;
const ID_TB_MINIATURES: i32 = 3503;
const ID_VAL_MINIATURES: i32 = 3504;
const ID_NOTE_MINIATURES: i32 = 3505;

/// Note de l'onglet « barre des tâches » : conditions d'application.
const TEXTE_NOTE_BARRE: &str =
    "Appliqué à l'Explorateur à la validation (Windows 11). En cas de conflit (Windhawk, \
     TranslucentTB…), rien n'est appliqué : une notification le signale et RustPiz ne \
     réessaie qu'après un changement de réglage ou un redémarrage de l'Explorateur.";
/// Note de l'onglet « miniatures » : conditions d'application.
const TEXTE_NOTE_MINIATURES: &str =
    "Appliqué à la validation. Les miniatures demandent Windows 11 24H2 (build 26100) ou plus \
     récent.";

/// Libellés des onglets.
const ONGLETS: [&str; 5] = [
    "Param Transparence",
    "Param Jelly",
    "Param Effets Ouverture/Fermeture",
    "Param Barre des Taches",
    "Param miniatures",
];

/// Contrôles de chaque page (affichés seulement quand l'onglet est actif).
const PAGES: [&[i32]; 5] = [
    &[
        ID_SW_ACTIF,
        ID_SW_DEMARRAGE,
        ID_SW_EXPLORATEUR,
        ID_LBL_OPACITE,
        ID_TB_OPACITE,
        ID_VAL_OPACITE,
        ID_LBL_DWM,
        ID_CB_DWM,
        ID_AIDE_DWM,
        ID_LBL_BASCULE,
        ID_ED_BASCULE,
        ID_LBL_DIMINUER,
        ID_ED_DIMINUER,
        ID_LBL_AUGMENTER,
        ID_ED_AUGMENTER,
        ID_AIDE_RACCOURCIS,
    ],
    &[ID_SW_JELLY, ID_LBL_JELLY, ID_TB_JELLY, ID_VAL_JELLY, ID_ETAT_JELLY],
    &[ID_SW_OUVERTURE, ID_SW_FERMETURE, ID_LBL_EFFET, ID_CB_EFFET, ID_ETAT_EFFETS],
    &[ID_SW_BARRE, ID_LBL_THEME, ID_CB_THEME, ID_NOTE_BARRE],
    &[
        ID_SW_MINIATURES,
        ID_LBL_MINIATURES,
        ID_TB_MINIATURES,
        ID_VAL_MINIATURES,
        ID_NOTE_MINIATURES,
    ],
];

/// Interrupteurs (boutons owner-draw).
const INTERRUPTEURS: [i32; 8] = [
    ID_SW_ACTIF,
    ID_SW_DEMARRAGE,
    ID_SW_EXPLORATEUR,
    ID_SW_JELLY,
    ID_SW_OUVERTURE,
    ID_SW_FERMETURE,
    ID_SW_BARRE,
    ID_SW_MINIATURES,
];

/// Champs de raccourci, dans l'ordre de `RACCOURCIS` : bascule, opacité -,
/// opacité +.
const CHAMPS: [i32; 3] = [ID_ED_BASCULE, ID_ED_DIMINUER, ID_ED_AUGMENTER];

/// Effets DWM de la liste ; l'élément 0 de la liste est « (ne rien changer) ».
const CHOIX_DWM: [EffetDwm; 5] = [
    EffetDwm::Aucun,
    EffetDwm::Mica,
    EffetDwm::MicaAlt,
    EffetDwm::Acrylique,
    EffetDwm::Verre,
];

/// Texte d'un champ en attente de combinaison.
const TEXTE_CAPTURE: &str = "Appuyez sur les touches…";

/// Couleurs (COLORREF = 0x00BBGGRR).
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const BLEU_WINDOWS: COLORREF = rgb(0x00, 0x78, 0xD4);
const BLANC: COLORREF = rgb(0xFF, 0xFF, 0xFF);
const GRIS_FOND: COLORREF = rgb(0xCC, 0xCC, 0xCC);
const GRIS_BORD: COLORREF = rgb(0x8A, 0x8A, 0x8A);
const GRIS_INACTIF_FOND: COLORREF = rgb(0xE6, 0xE6, 0xE6);
const GRIS_INACTIF_BORD: COLORREF = rgb(0xC8, 0xC8, 0xC8);
const BLEU_INACTIF: COLORREF = rgb(0x9D, 0xC3, 0xE6);
const ROUGE_ERREUR: COLORREF = rgb(0xC4, 0x2B, 0x1C);
const ORANGE_ETAT: COLORREF = rgb(0x9D, 0x5D, 0x00);

thread_local! {
    /// Handle de la fenêtre de paramètres (0 = fermée).
    static FENETRE: Cell<isize> = const { Cell::new(0) };
    /// Police créée pour la fenêtre (0 = police de stock, à ne pas détruire).
    static POLICE: Cell<isize> = const { Cell::new(0) };
    /// DPI utilisé pour la mise à l'échelle (DPI système).
    static DPI: Cell<u32> = const { Cell::new(96) };
    /// Raccourcis affichés (bascule, opacité -, opacité +), mis à jour par
    /// la capture ; lus par « Valider ».
    static RACCOURCIS: RefCell<[String; 3]> =
        const { RefCell::new([String::new(), String::new(), String::new()]) };
    /// Capture en cours : (HWND du champ, texte à restaurer en cas d'abandon).
    static CAPTURE: RefCell<Option<(isize, String)>> = const { RefCell::new(None) };
    /// L'utilisateur a choisi un effet dans la liste (CBN_SELCHANGE).
    static EFFET_CHOISI: Cell<bool> = const { Cell::new(false) };
    /// Contrôle qui avait le focus quand la fenêtre a été désactivée.
    static FOCUS_MEMO: Cell<isize> = const { Cell::new(0) };
}

/// Convertit une chaîne Rust en chaîne large terminée par NUL (Win32).
fn large(texte: &str) -> Vec<u16> {
    texte.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Met une dimension logique (96 DPI) à l'échelle du DPI système.
fn px(valeur: i32) -> i32 {
    let dpi = DPI.with(|d| d.get()).max(96) as i32;
    (valeur * dpi + 48) / 96
}

fn hwnd_de(brut: isize) -> HWND {
    HWND(brut as *mut core::ffi::c_void)
}

/// Ouvre la fenêtre de paramètres (ou la met au premier plan si déjà ouverte).
pub fn ouvrir() {
    let existant = FENETRE.with(|c| c.get());
    if existant != 0 {
        unsafe {
            let _ = SetForegroundWindow(hwnd_de(existant));
        }
        return;
    }
    unsafe {
        match creer_fenetre() {
            Ok(hwnd) => {
                FENETRE.with(|c| c.set(hwnd.0 as isize));
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                if let Ok(onglets) = GetDlgItem(Some(hwnd), ID_ONGLETS) {
                    let _ = SetFocus(Some(onglets));
                }
            }
            Err(e) => logging::erreur(&format!(
                "Impossible de créer la fenêtre de paramètres : {e}"
            )),
        }
    }
}

/// Navigation clavier de la fenêtre de paramètres, appelée par la boucle de
/// messages de `main.rs` pour CHAQUE message : vrai si `IsDialogMessageW` a
/// traité le message (il est alors déjà distribué : ne pas le redistribuer).
/// Sans effet (faux) si la fenêtre est fermée ou si le message ne la
/// concerne pas.
pub fn traiter_message_dialogue(message: &MSG) -> bool {
    let brut = FENETRE.with(|c| c.get());
    if brut == 0 {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd_de(brut), message).as_bool() }
}

/// Crée la police des contrôles : police des messages du système (Segoe UI
/// sous Windows 11), repli sur `DEFAULT_GUI_FONT`.
unsafe fn creer_police() -> HFONT {
    let mut metriques: windows::Win32::UI::WindowsAndMessaging::NONCLIENTMETRICSW =
        std::mem::zeroed();
    metriques.cbSize =
        std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::NONCLIENTMETRICSW>() as u32;
    let lu = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
        windows::Win32::UI::WindowsAndMessaging::SPI_GETNONCLIENTMETRICS,
        metriques.cbSize,
        Some(&mut metriques as *mut _ as *mut core::ffi::c_void),
        windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    if lu.is_ok() {
        let police = CreateFontIndirectW(&metriques.lfMessageFont);
        if !police.0.is_null() {
            POLICE.with(|p| p.set(police.0 as isize));
            return police;
        }
    }
    logging::avertir("Paramètres : police système indisponible, police par défaut utilisée.");
    HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
}

/// Police courante des contrôles.
fn police() -> HFONT {
    let brut = POLICE.with(|p| p.get());
    if brut != 0 {
        HFONT(brut as *mut core::ffi::c_void)
    } else {
        unsafe { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) }
    }
}

/// Dimensions de la zone cliente (pixels logiques à 96 DPI).
const LARGEUR_CLIENT: i32 = 560;
const HAUTEUR_CLIENT: i32 = 550;
/// Hauteur du contrôle d'onglets (deux rangées de titres comprises).
const HAUTEUR_ONGLETS: i32 = 470;

/// Crée la fenêtre (les contrôles sont créés dans `WM_CREATE`).
unsafe fn creer_fenetre() -> windows::core::Result<HWND> {
    let module = GetModuleHandleW(None)?;
    let instance = HINSTANCE::from(module);

    // Curseurs (trackbar), onglets, et contrôles standard v6.
    let controles = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_STANDARD_CLASSES,
    };
    if !InitCommonControlsEx(&controles).as_bool() {
        logging::avertir("Paramètres : InitCommonControlsEx a échoué.");
    }

    let dpi = GetDpiForSystem();
    DPI.with(|d| d.set(if dpi == 0 { 96 } else { dpi }));

    let classe = WNDCLASSW {
        lpfnWndProc: Some(procedure_parametres),
        hInstance: instance,
        lpszClassName: w!("TransparentWindowsParametres"),
        // Fond blanc système, identique au corps des onglets v6.
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut core::ffi::c_void),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        // Icône de l'application (ressource PE n° 1, voir tray.rs).
        hIcon: LoadIconW(Some(instance), PCWSTR(std::ptr::without_provenance(1)))
            .unwrap_or_default(),
        ..Default::default()
    };
    // Échec attendu si la classe est déjà enregistrée (2e ouverture).
    let _ = RegisterClassW(&classe);

    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut cadre = RECT {
        left: 0,
        top: 0,
        right: px(LARGEUR_CLIENT),
        bottom: px(HAUTEUR_CLIENT),
    };
    let _ = AdjustWindowRectEx(&mut cadre, style, false, WINDOW_EX_STYLE(0));
    let largeur = cadre.right - cadre.left;
    let hauteur = cadre.bottom - cadre.top;

    // Centrée sur la zone de travail de l'écran qui contient la souris.
    let mut zone = RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    };
    let mut souris = POINT::default();
    if GetCursorPos(&mut souris).is_ok() {
        let moniteur = MonitorFromPoint(souris, MONITOR_DEFAULTTONEAREST);
        let mut infos = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(moniteur, &mut infos).as_bool() {
            zone = infos.rcWork;
        }
    }
    let x = (zone.left + (zone.right - zone.left - largeur) / 2).max(zone.left);
    let y = (zone.top + (zone.bottom - zone.top - hauteur) / 2).max(zone.top);

    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("TransparentWindowsParametres"),
        w!("RustPiz — Paramètres"),
        style,
        x,
        y,
        largeur,
        hauteur,
        None,
        None,
        Some(instance),
        None,
    )
}

/// Crée un contrôle enfant (style `WS_CHILD | WS_VISIBLE | style`) et lui
/// applique la police de la fenêtre. HWND nul en cas d'échec (journalisé).
#[allow(clippy::too_many_arguments)]
unsafe fn creer(
    parent: HWND,
    classe: PCWSTR,
    texte: &str,
    style: u32,
    exstyle: u32,
    x: i32,
    y: i32,
    largeur: i32,
    hauteur: i32,
    id: i32,
) -> HWND {
    let texte = large(texte);
    match CreateWindowExW(
        WINDOW_EX_STYLE(exstyle),
        classe,
        PCWSTR(texte.as_ptr()),
        WINDOW_STYLE((WS_CHILD | WS_VISIBLE).0 | style),
        x,
        y,
        largeur,
        hauteur,
        Some(parent),
        Some(HMENU(id as isize as *mut core::ffi::c_void)),
        None,
        None,
    ) {
        Ok(controle) => {
            let _ = SendMessageW(
                controle,
                WM_SETFONT,
                Some(WPARAM(police().0 as usize)),
                Some(LPARAM(1)),
            );
            controle
        }
        Err(e) => {
            logging::erreur(&format!("Paramètres : création du contrôle {id} impossible : {e}"));
            HWND::default()
        }
    }
}

/// Libellé sur une ligne, centré verticalement.
unsafe fn libelle(parent: HWND, texte: &str, x: i32, y: i32, l: i32, h: i32, id: i32) {
    creer(
        parent,
        w!("STATIC"),
        texte,
        SS_NOPREFIX | SS_CENTERIMAGE | SS_ENDELLIPSIS,
        0,
        x,
        y,
        l,
        h,
        id,
    );
}

/// Texte sur plusieurs lignes (aide, état, message d'erreur).
unsafe fn texte_multiligne(parent: HWND, texte: &str, x: i32, y: i32, l: i32, h: i32, id: i32) {
    creer(parent, w!("STATIC"), texte, SS_NOPREFIX, 0, x, y, l, h, id);
}

/// Interrupteur owner-draw, état initial `actif`.
unsafe fn interrupteur(parent: HWND, texte: &str, actif: bool, x: i32, y: i32, l: i32, id: i32) {
    let h = creer(
        parent,
        w!("BUTTON"),
        texte,
        BS_OWNERDRAW as u32 | (WS_TABSTOP | WS_GROUP).0,
        0,
        x,
        y,
        l,
        px(28),
        id,
    );
    if !h.0.is_null() {
        SetWindowLongPtrW(h, GWLP_USERDATA, actif as isize);
    }
}

/// Curseur horizontal (trackbar) `min..=max` à la position `pos`.
#[allow(clippy::too_many_arguments)]
unsafe fn curseur(parent: HWND, min: u16, max: u16, pos: i32, x: i32, y: i32, l: i32, id: i32) {
    let h = creer(
        parent,
        w!("msctls_trackbar32"),
        "",
        TBS_AUTOTICKS | (WS_TABSTOP | WS_GROUP).0,
        0,
        x,
        y,
        l,
        px(30),
        id,
    );
    if h.0.is_null() {
        return;
    }
    let plage = ((max as isize) << 16) | min as isize;
    let _ = SendMessageW(h, TBM_SETRANGE, Some(WPARAM(1)), Some(LPARAM(plage)));
    let _ = SendMessageW(h, TBM_SETTICFREQ, Some(WPARAM(10)), Some(LPARAM(0)));
    let _ = SendMessageW(h, TBM_SETPAGESIZE, Some(WPARAM(0)), Some(LPARAM(10)));
    let _ = SendMessageW(h, TBM_SETLINESIZE, Some(WPARAM(0)), Some(LPARAM(1)));
    let pos = pos.clamp(min as i32, max as i32);
    let _ = SendMessageW(h, TBM_SETPOS, Some(WPARAM(1)), Some(LPARAM(pos as isize)));
}

/// Liste déroulante non modifiable contenant `elements`, sélection `index`.
unsafe fn liste(
    parent: HWND,
    elements: &[String],
    index: usize,
    x: i32,
    y: i32,
    l: i32,
    id: i32,
) {
    let h = creer(
        parent,
        w!("COMBOBOX"),
        "",
        CBS_DROPDOWNLIST as u32 | (WS_VSCROLL | WS_TABSTOP | WS_GROUP).0,
        0,
        x,
        y,
        l,
        px(320),
        id,
    );
    if h.0.is_null() {
        return;
    }
    for element in elements {
        let texte = large(element);
        let _ = SendMessageW(h, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(texte.as_ptr() as isize)));
    }
    let _ = SendMessageW(h, CB_SETCURSEL, Some(WPARAM(index)), Some(LPARAM(0)));
}

/// Champ de raccourci (EDIT en lecture seule, sous-classé pour la capture).
unsafe fn champ_raccourci(parent: HWND, texte: &str, x: i32, y: i32, l: i32, id: i32) {
    let h = creer(
        parent,
        w!("EDIT"),
        texte,
        (ES_READONLY | ES_CENTER | ES_AUTOHSCROLL) as u32 | (WS_TABSTOP | WS_GROUP).0,
        WS_EX_CLIENTEDGE.0,
        x,
        y,
        l,
        px(24),
        id,
    );
    if h.0.is_null() {
        return;
    }
    // Identifiant de sous-classe = identifiant du contrôle ; aucune donnée
    // de référence (l'état de capture est dans un thread_local indexé par
    // HWND). Retrait au WM_NCDESTROY du champ.
    if !SetWindowSubclass(h, Some(procedure_champ), id as usize, 0).as_bool() {
        logging::erreur(&format!(
            "Paramètres : sous-classement du champ {id} impossible (capture indisponible)."
        ));
    }
}

/// Lit la position d'un curseur.
unsafe fn position_curseur(parent: HWND, id: i32) -> Option<i32> {
    let h = GetDlgItem(Some(parent), id).ok()?;
    Some(SendMessageW(h, TBM_GETPOS, None, None).0 as i32)
}

/// Sélection d'une liste déroulante (`None` si aucune).
unsafe fn selection_liste(parent: HWND, id: i32) -> Option<usize> {
    let h = GetDlgItem(Some(parent), id).ok()?;
    let index = SendMessageW(h, CB_GETCURSEL, None, None).0;
    usize::try_from(index).ok()
}

/// Remplace le texte d'une fenêtre (WM_SETTEXT).
unsafe fn definir_texte_hwnd(h: HWND, texte: &str) {
    if h.0.is_null() {
        return;
    }
    let texte = large(texte);
    let _ = SendMessageW(h, WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(texte.as_ptr() as isize)));
}

/// Remplace le texte d'un contrôle par son identifiant.
unsafe fn definir_texte(parent: HWND, id: i32, texte: &str) {
    if let Ok(h) = GetDlgItem(Some(parent), id) {
        definir_texte_hwnd(h, texte);
    }
}

/// Message en bas de la fenêtre (erreurs, refus de capture ; vide = effacé).
unsafe fn message(parent: HWND, texte: &str) {
    definir_texte(parent, ID_MESSAGE, texte);
}

// ---------------------------------------------------------------------------
// Interrupteurs (état dans GWLP_USERDATA : bit 0 = activé, bit 1 = touché)
// ---------------------------------------------------------------------------

unsafe fn interrupteur_bits(parent: HWND, id: i32) -> isize {
    GetDlgItem(Some(parent), id)
        .map(|h| GetWindowLongPtrW(h, GWLP_USERDATA))
        .unwrap_or(0)
}

/// État d'un interrupteur.
unsafe fn interrupteur_actif(parent: HWND, id: i32) -> bool {
    interrupteur_bits(parent, id) & 1 != 0
}

/// Vrai si l'utilisateur a basculé l'interrupteur depuis l'ouverture.
unsafe fn interrupteur_touche(parent: HWND, id: i32) -> bool {
    interrupteur_bits(parent, id) & 2 != 0
}

/// Bascule un interrupteur (clic, espace, double-clic) et le redessine.
unsafe fn basculer_interrupteur(h: HWND) {
    let bits = GetWindowLongPtrW(h, GWLP_USERDATA);
    SetWindowLongPtrW(h, GWLP_USERDATA, (bits ^ 1) | 2);
    let _ = InvalidateRect(Some(h), None, false);
}

/// Dessine un interrupteur : pilule (bleue si activé, grise bordée sinon),
/// pastille blanche, libellé à droite, rectangle de focus.
unsafe fn dessiner_interrupteur(d: &DRAWITEMSTRUCT) {
    let hdc = d.hDC;
    let r = d.rcItem;
    let actif = GetWindowLongPtrW(d.hwndItem, GWLP_USERDATA) & 1 != 0;
    let inactif = d.itemState.0 & ODS_DISABLED.0 != 0;

    FillRect(hdc, &r, GetSysColorBrush(COLOR_WINDOW));

    let hauteur = px(20);
    let largeur = px(40);
    let haut = r.top + (r.bottom - r.top - hauteur) / 2;
    let gauche = r.left + px(2);
    let (fond, bord, pastille, bord_pastille) = match (actif, inactif) {
        (true, false) => (BLEU_WINDOWS, BLEU_WINDOWS, BLANC, BLEU_WINDOWS),
        (false, false) => (GRIS_FOND, GRIS_BORD, BLANC, GRIS_BORD),
        (true, true) => (BLEU_INACTIF, BLEU_INACTIF, BLANC, BLEU_INACTIF),
        (false, true) => (GRIS_INACTIF_FOND, GRIS_INACTIF_BORD, BLANC, GRIS_INACTIF_BORD),
    };

    let crayon = CreatePen(PS_SOLID, 1, bord);
    let pinceau = CreateSolidBrush(fond);
    let ancien_crayon = SelectObject(hdc, HGDIOBJ(crayon.0));
    let ancien_pinceau = SelectObject(hdc, HGDIOBJ(pinceau.0));
    let _ = RoundRect(hdc, gauche, haut, gauche + largeur, haut + hauteur, hauteur, hauteur);

    let crayon_pastille = CreatePen(PS_SOLID, 1, bord_pastille);
    let pinceau_pastille = CreateSolidBrush(pastille);
    let _ = SelectObject(hdc, HGDIOBJ(crayon_pastille.0));
    let _ = SelectObject(hdc, HGDIOBJ(pinceau_pastille.0));
    let marge = px(3);
    let diametre = hauteur - 2 * marge;
    let x = if actif {
        gauche + largeur - marge - diametre
    } else {
        gauche + marge
    };
    let _ = Ellipse(hdc, x, haut + marge, x + diametre, haut + marge + diametre);

    let _ = SelectObject(hdc, ancien_crayon);
    let _ = SelectObject(hdc, ancien_pinceau);
    let _ = DeleteObject(HGDIOBJ(crayon.0));
    let _ = DeleteObject(HGDIOBJ(pinceau.0));
    let _ = DeleteObject(HGDIOBJ(crayon_pastille.0));
    let _ = DeleteObject(HGDIOBJ(pinceau_pastille.0));

    // Libellé.
    let mut tampon = [0u16; 256];
    let longueur = GetWindowTextW(d.hwndItem, &mut tampon).max(0) as usize;
    let texte = &mut tampon[..longueur.min(255)];
    let police_controle = SendMessageW(d.hwndItem, WM_GETFONT, None, None).0;
    let ancienne_police = if police_controle != 0 {
        Some(SelectObject(hdc, HGDIOBJ(police_controle as *mut core::ffi::c_void)))
    } else {
        None
    };
    let _ = SetBkMode(hdc, TRANSPARENT);
    let couleur = if inactif { COLOR_GRAYTEXT } else { COLOR_WINDOWTEXT };
    let _ = SetTextColor(hdc, COLORREF(GetSysColor(couleur)));
    let mut zone = RECT {
        left: gauche + largeur + px(10),
        top: r.top,
        right: r.right - px(2),
        bottom: r.bottom,
    };
    if !texte.is_empty() {
        let _ = DrawTextW(
            hdc,
            texte,
            &mut zone,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
    // Rectangle de focus autour du libellé (masqué tant que le clavier
    // n'a pas été utilisé, selon les réglages d'interface de Windows).
    if d.itemState.0 & ODS_FOCUS.0 != 0 && d.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
        let mut mesure = zone;
        if !texte.is_empty() {
            let _ = DrawTextW(
                hdc,
                texte,
                &mut mesure,
                DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_CALCRECT,
            );
        }
        let hauteur_texte = mesure.bottom - mesure.top;
        let haut_texte = r.top + (r.bottom - r.top - hauteur_texte) / 2;
        let focus = RECT {
            left: zone.left - px(2),
            top: haut_texte - px(1),
            right: mesure.right.min(zone.right) + px(2),
            bottom: haut_texte + hauteur_texte + px(1),
        };
        let _ = DrawFocusRect(hdc, &focus);
    }
    if let Some(ancienne) = ancienne_police {
        let _ = SelectObject(hdc, ancienne);
    }
}

// ---------------------------------------------------------------------------
// Construction et remplissage
// ---------------------------------------------------------------------------

/// Libellé d'un effet dans la liste (« (approx.) » conservé ; effet connu
/// indisponible signalé, l'élément restant sélectionnable).
fn libelle_effet(effet: EffetAnimation) -> String {
    if animations::effet_disponible(effet) {
        effet.libelle_menu()
    } else if effet.moteur() == Moteur::Shader {
        format!("{} (indisponible : voir journal)", effet.libelle_menu())
    } else {
        format!("{} (indisponible sur ce GPU)", effet.libelle_menu())
    }
}

/// Crée tous les contrôles et les pré-remplit avec `config.toml`.
unsafe fn construire(hwnd: HWND) {
    let cfg = config::charger();
    let largeur_client = px(LARGEUR_CLIENT);
    let marge = px(12);

    // Onglets (créés d'abord pour connaître la zone d'affichage, puis
    // renvoyés en bas de l'ordre Z à la fin).
    let onglet_x = marge;
    let onglet_y = marge;
    let onglet_l = largeur_client - 2 * marge;
    let onglet_h = px(HAUTEUR_ONGLETS);
    // Cinq titres : plusieurs rangées plutôt qu'une fenêtre très large (la
    // zone d'affichage est relue par TCM_ADJUSTRECT après l'insertion).
    let onglets = creer(
        hwnd,
        WC_TABCONTROLW,
        "",
        TCS_MULTILINE | (WS_CLIPSIBLINGS | WS_TABSTOP | WS_GROUP).0,
        0,
        onglet_x,
        onglet_y,
        onglet_l,
        onglet_h,
        ID_ONGLETS,
    );
    for (index, titre) in ONGLETS.iter().enumerate() {
        let mut texte = large(titre);
        let element = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(texte.as_mut_ptr()),
            ..Default::default()
        };
        let _ = SendMessageW(
            onglets,
            TCM_INSERTITEMW,
            Some(WPARAM(index)),
            Some(LPARAM(&element as *const TCITEMW as isize)),
        );
    }
    let mut zone = RECT {
        left: onglet_x,
        top: onglet_y,
        right: onglet_x + onglet_l,
        bottom: onglet_y + onglet_h,
    };
    if !onglets.0.is_null() {
        let _ = SendMessageW(
            onglets,
            TCM_ADJUSTRECT,
            Some(WPARAM(0)),
            Some(LPARAM(&mut zone as *mut RECT as isize)),
        );
    } else {
        zone.top += px(30);
    }
    let x0 = zone.left + px(16);
    let y0 = zone.top + px(14);
    let lu = (zone.right - px(16) - x0).max(px(300));

    // --- Page 1 : transparence ---------------------------------------------
    let mut y = y0;
    interrupteur(hwnd, "Activer Transparence", window::est_actif(), x0, y, lu, ID_SW_ACTIF);
    y += px(32);
    interrupteur(
        hwnd,
        "Activer la transparence au démarrage",
        cfg.activer_au_demarrage,
        x0,
        y,
        lu,
        ID_SW_DEMARRAGE,
    );
    y += px(32);
    interrupteur(
        hwnd,
        "Explorateur automatiquement transparent (nouvelles fenêtres)",
        cfg.transparence_explorateur,
        x0,
        y,
        lu,
        ID_SW_EXPLORATEUR,
    );
    y += px(40);
    let largeur_libelle = px(170);
    let largeur_valeur = px(55);
    libelle(hwnd, "Opacité par défaut", x0, y, largeur_libelle, px(30), ID_LBL_OPACITE);
    // Minimum 10 % (comme `Config::valider` / OPACITE_MIN) : 0 % rendrait
    // les fenêtres invisibles.
    curseur(
        hwnd,
        10,
        100,
        cfg.opacite_defaut as i32,
        x0 + largeur_libelle,
        y,
        lu - largeur_libelle - largeur_valeur,
        ID_TB_OPACITE,
    );
    libelle(
        hwnd,
        &format!("{} %", cfg.opacite_defaut),
        x0 + lu - largeur_valeur + px(6),
        y,
        largeur_valeur - px(6),
        px(30),
        ID_VAL_OPACITE,
    );
    y += px(42);
    libelle(hwnd, "Effet DWM (fenêtre sous le curseur)", x0, y, px(240), px(24), ID_LBL_DWM);
    let mut choix_dwm = vec!["(ne rien changer)".to_string()];
    choix_dwm.extend(CHOIX_DWM.iter().map(|e| e.nom().to_string()));
    liste(hwnd, &choix_dwm, 0, x0 + px(245), y, px(200), ID_CB_DWM);
    y += px(30);
    texte_multiligne(
        hwnd,
        "Appliqué après Valider, à la fenêtre sous le curseur au bout de 3 secondes.",
        x0,
        y,
        lu,
        px(20),
        ID_AIDE_DWM,
    );
    y += px(34);
    let raccourcis = [
        cfg.raccourcis.bascule.clone(),
        cfg.raccourcis.diminuer.clone(),
        cfg.raccourcis.augmenter.clone(),
    ];
    for ((id_libelle, id_champ, texte), valeur) in [
        (ID_LBL_BASCULE, ID_ED_BASCULE, "Raccourci bascule"),
        (ID_LBL_DIMINUER, ID_ED_DIMINUER, "Raccourci Opacité -"),
        (ID_LBL_AUGMENTER, ID_ED_AUGMENTER, "Raccourci Opacité +"),
    ]
    .into_iter()
    .zip(raccourcis.iter())
    {
        libelle(hwnd, texte, x0, y, largeur_libelle, px(24), id_libelle);
        champ_raccourci(hwnd, valeur, x0 + largeur_libelle, y, px(220), id_champ);
        y += px(32);
    }
    RACCOURCIS.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut() {
            *r = raccourcis;
        }
    });
    texte_multiligne(
        hwnd,
        "Cliquez dans un champ puis appuyez sur la combinaison voulue \
         (Echap : annuler, Tab : champ suivant).",
        x0,
        y,
        lu,
        px(36),
        ID_AIDE_RACCOURCIS,
    );

    // --- Page 2 : Jelly ----------------------------------------------------
    let mut y = y0;
    interrupteur(hwnd, "Activer Jelly", cfg.jelly_actif, x0, y, lu, ID_SW_JELLY);
    y += px(40);
    libelle(hwnd, "Puissance de Jelly", x0, y, largeur_libelle, px(30), ID_LBL_JELLY);
    curseur(
        hwnd,
        0,
        100,
        cfg.jelly_puissance as i32,
        x0 + largeur_libelle,
        y,
        lu - largeur_libelle - largeur_valeur,
        ID_TB_JELLY,
    );
    libelle(
        hwnd,
        &format!("{} %", cfg.jelly_puissance.min(100)),
        x0 + lu - largeur_valeur + px(6),
        y,
        largeur_valeur - px(6),
        px(30),
        ID_VAL_JELLY,
    );
    y += px(44);
    texte_multiligne(hwnd, "", x0, y, lu, px(60), ID_ETAT_JELLY);

    // --- Page 3 : effets d'ouverture / fermeture ---------------------------
    let mut y = y0;
    interrupteur(
        hwnd,
        "Animer l'ouverture des fenêtres",
        cfg.effet_ouverture,
        x0,
        y,
        lu,
        ID_SW_OUVERTURE,
    );
    y += px(32);
    interrupteur(
        hwnd,
        "Animer la fermeture des fenêtres",
        cfg.effet_fermeture,
        x0,
        y,
        lu,
        ID_SW_FERMETURE,
    );
    y += px(40);
    libelle(hwnd, "Effet d'animation des fenêtres", x0, y, lu, px(24), ID_LBL_EFFET);
    y += px(26);
    let effet_initial =
        EffetAnimation::depuis_nom(&cfg.effet_fenetre).unwrap_or(EffetAnimation::Zoom);
    let index_effet = EffetAnimation::TOUS
        .iter()
        .position(|e| *e == effet_initial)
        .unwrap_or(0);
    let libelles: Vec<String> = EffetAnimation::TOUS.iter().map(|e| libelle_effet(*e)).collect();
    liste(hwnd, &libelles, index_effet, x0, y, px(380).min(lu), ID_CB_EFFET);
    y += px(40);
    texte_multiligne(hwnd, &texte_etat_effets(), x0, y, lu, px(90), ID_ETAT_EFFETS);

    // --- Page 4 : barre des tâches ------------------------------------------
    let mut y = y0;
    interrupteur(
        hwnd,
        "Activer la barre des tâches",
        cfg.barre_active,
        x0,
        y,
        lu,
        ID_SW_BARRE,
    );
    y += px(40);
    libelle(hwnd, "Thème de la barre des tâches", x0, y, lu, px(24), ID_LBL_THEME);
    y += px(26);
    let themes = themes_barre::tries_par_nom();
    let index_theme = themes
        .iter()
        .position(|(id, _)| *id == themes_barre::normaliser(&cfg.barre_theme))
        .unwrap_or(0);
    let noms_themes: Vec<String> = themes.iter().map(|(_, n)| n.to_string()).collect();
    liste(hwnd, &noms_themes, index_theme, x0, y, px(380).min(lu), ID_CB_THEME);
    y += px(40);
    texte_multiligne(hwnd, TEXTE_NOTE_BARRE, x0, y, lu, px(64), ID_NOTE_BARRE);

    // --- Page 5 : miniatures ------------------------------------------------
    let mut y = y0;
    interrupteur(
        hwnd,
        "Activer les miniatures personnalisées",
        cfg.miniatures_actives,
        x0,
        y,
        lu,
        ID_SW_MINIATURES,
    );
    y += px(40);
    let taille = cfg
        .miniatures_taille
        .clamp(config::MINIATURES_TAILLE_MIN, config::MINIATURES_TAILLE_MAX);
    libelle(hwnd, "Taille des miniatures", x0, y, largeur_libelle, px(30), ID_LBL_MINIATURES);
    curseur(
        hwnd,
        config::MINIATURES_TAILLE_MIN as u16,
        config::MINIATURES_TAILLE_MAX as u16,
        taille as i32,
        x0 + largeur_libelle,
        y,
        lu - largeur_libelle - largeur_valeur,
        ID_TB_MINIATURES,
    );
    libelle(
        hwnd,
        &format!("{taille} %"),
        x0 + lu - largeur_valeur + px(6),
        y,
        largeur_valeur - px(6),
        px(30),
        ID_VAL_MINIATURES,
    );
    y += px(44);
    texte_multiligne(hwnd, TEXTE_NOTE_MINIATURES, x0, y, lu, px(40), ID_NOTE_MINIATURES);

    // --- Bas de la fenêtre : message et boutons ----------------------------
    let bouton_l = px(100);
    let bouton_h = px(30);
    let bouton_y = onglet_y + onglet_h + px(14);
    let fermer_x = largeur_client - marge - bouton_l;
    let valider_x = fermer_x - px(10) - bouton_l;
    texte_multiligne(
        hwnd,
        "",
        marge,
        bouton_y - px(8),
        valider_x - 2 * marge,
        px(48),
        ID_MESSAGE,
    );
    creer(
        hwnd,
        w!("BUTTON"),
        "Valider",
        BS_DEFPUSHBUTTON as u32 | (WS_TABSTOP | WS_GROUP).0,
        0,
        valider_x,
        bouton_y,
        bouton_l,
        bouton_h,
        ID_VALIDER,
    );
    creer(
        hwnd,
        w!("BUTTON"),
        "Fermer",
        BS_PUSHBUTTON as u32 | (WS_TABSTOP | WS_GROUP).0,
        0,
        fermer_x,
        bouton_y,
        bouton_l,
        bouton_h,
        ID_FERMER,
    );

    // L'onglet passe sous les contrôles des pages (sinon il les recouvre et
    // capte leurs clics) ; il devient le dernier de l'ordre de tabulation.
    if !onglets.0.is_null() {
        let _ = SetWindowPos(
            onglets,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }

    maj_etat_jelly(hwnd);
    afficher_page(hwnd, 0);
}

/// Texte d'état de l'onglet des effets (vide si tout va bien).
fn texte_etat_effets() -> String {
    let mut lignes = Vec::new();
    if let Some(raison) = animations::indisponibilite() {
        lignes.push(format!(
            "Animations indisponibles : {raison}. Les réglages sont enregistrés mais restent sans effet."
        ));
    }
    if EffetAnimation::TOUS
        .iter()
        .any(|e| e.moteur() == Moteur::Shader && !animations::effet_disponible(*e))
    {
        lignes.push(
            "Un effet « indisponible » n'a pas pu être compilé par le GPU : le choisir puis \
             Valider retente sa compilation (détails dans journal.log)."
                .to_string(),
        );
    }
    lignes.join("\r\n")
}

/// Met à jour le texte d'état de l'onglet Jelly (interrupteur et curseur
/// courants + état du module).
unsafe fn maj_etat_jelly(hwnd: HWND) {
    let actif = interrupteur_actif(hwnd, ID_SW_JELLY);
    let puissance = position_curseur(hwnd, ID_TB_JELLY).unwrap_or(50);
    let texte = if let Some(raison) = animations::indisponibilite() {
        format!("Indisponible : {raison}.")
    } else if actif && animations::jelly_desactive() {
        if interrupteur_touche(hwnd, ID_SW_JELLY) {
            "Désactivé après une erreur (voir journal.log) : il sera réarmé à la validation."
                .to_string()
        } else {
            "Désactivé après une erreur (voir journal.log). Pour le réarmer : désactivez puis \
             réactivez « Activer Jelly », puis Valider."
                .to_string()
        }
    } else if actif && puissance == 0 {
        "Puissance 0 : aucun mouvement.".to_string()
    } else {
        String::new()
    };
    definir_texte(hwnd, ID_ETAT_JELLY, &texte);
}

/// Affiche les contrôles de la page `index` et masque les autres.
unsafe fn afficher_page(hwnd: HWND, index: usize) {
    let focus = GetFocus();
    for (page, ids) in PAGES.iter().enumerate() {
        for id in ids.iter() {
            if let Ok(h) = GetDlgItem(Some(hwnd), *id) {
                if page != index && h == focus {
                    // Le focus ne doit pas rester sur un contrôle masqué.
                    if let Ok(onglets) = GetDlgItem(Some(hwnd), ID_ONGLETS) {
                        let _ = SetFocus(Some(onglets));
                    }
                }
                let _ = ShowWindow(h, if page == index { SW_SHOW } else { SW_HIDE });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Lit tous les contrôles, enregistre `config.toml`, applique à chaud puis
/// ferme la fenêtre. Erreur : message dans la fenêtre, qui reste ouverte.
unsafe fn valider(hwnd: HWND) {
    let [bascule, diminuer, augmenter] = RACCOURCIS.with(|r| match r.try_borrow() {
        Ok(r) => r.clone(),
        Err(_) => [String::new(), String::new(), String::new()],
    });

    // Validation des raccourcis avant toute écriture.
    let mut analyses = Vec::with_capacity(3);
    for (nom, texte) in [
        ("bascule", &bascule),
        ("opacité -", &diminuer),
        ("opacité +", &augmenter),
    ] {
        match hotkey::analyser(texte) {
            Some(a) => analyses.push((nom, a)),
            None => {
                logging::erreur(&format!(
                    "Raccourci « {nom} » invalide (« {texte} ») : enregistrement annulé."
                ));
                message(hwnd, &format!("Raccourci « {nom} » invalide : saisissez-le de nouveau."));
                return;
            }
        }
    }
    for i in 0..analyses.len() {
        for j in i + 1..analyses.len() {
            if analyses[i].1 == analyses[j].1 {
                message(
                    hwnd,
                    &format!(
                        "Les raccourcis « {} » et « {} » sont identiques.",
                        analyses[i].0, analyses[j].0
                    ),
                );
                return;
            }
        }
    }

    let effet = selection_liste(hwnd, ID_CB_EFFET)
        .and_then(|i| EffetAnimation::TOUS.get(i).copied())
        .unwrap_or_else(|| {
            EffetAnimation::depuis_nom(&config::charger().effet_fenetre)
                .unwrap_or(EffetAnimation::Zoom)
        });
    // Thème : index de la liste triée par nom (même ordre qu'à la
    // construction) ; sans sélection, le thème enregistré est conservé.
    let barre_theme = selection_liste(hwnd, ID_CB_THEME)
        .and_then(|i| themes_barre::tries_par_nom().get(i).map(|(id, _)| id.to_string()))
        .unwrap_or_else(|| config::charger().barre_theme);
    let effet_dwm = selection_liste(hwnd, ID_CB_DWM)
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| CHOIX_DWM.get(i).copied());

    let parametres = Parametres {
        opacite_defaut: position_curseur(hwnd, ID_TB_OPACITE).unwrap_or(80).clamp(10, 100) as u8,
        activer_au_demarrage: interrupteur_actif(hwnd, ID_SW_DEMARRAGE),
        transparence_explorateur: interrupteur_actif(hwnd, ID_SW_EXPLORATEUR),
        effet_fenetre: effet.nom_config().to_string(),
        effet_ouverture: interrupteur_actif(hwnd, ID_SW_OUVERTURE),
        effet_fermeture: interrupteur_actif(hwnd, ID_SW_FERMETURE),
        jelly_actif: interrupteur_actif(hwnd, ID_SW_JELLY),
        jelly_puissance: position_curseur(hwnd, ID_TB_JELLY).unwrap_or(50).clamp(0, 100) as u8,
        barre_active: interrupteur_actif(hwnd, ID_SW_BARRE),
        barre_theme,
        miniatures_actives: interrupteur_actif(hwnd, ID_SW_MINIATURES),
        miniatures_taille: position_curseur(hwnd, ID_TB_MINIATURES)
            .unwrap_or(config::MINIATURES_TAILLE_DEFAUT as i32)
            .clamp(
                config::MINIATURES_TAILLE_MIN as i32,
                config::MINIATURES_TAILLE_MAX as i32,
            ) as u32,
        raccourcis: Raccourcis {
            bascule,
            augmenter,
            diminuer,
        },
    };
    let actif_voulu = interrupteur_actif(hwnd, ID_SW_ACTIF);

    // Réarmements de l'ancien menu, décidés sur l'état AVANT application :
    // case (ré)activée, ou basculée pendant cette ouverture (« décocher puis
    // recocher »).
    let rearmer_ouverture = parametres.effet_ouverture
        && (!animations::ouverture_active() || interrupteur_touche(hwnd, ID_SW_OUVERTURE));
    let rearmer_jelly = parametres.jelly_actif
        && (!animations::jelly_actif() || interrupteur_touche(hwnd, ID_SW_JELLY));
    let effet_choisi = EFFET_CHOISI.with(|c| c.get());

    match config::enregistrer_parametres(&parametres) {
        Ok(()) => {
            logging::info(&format!(
                "Barre des tâches : {} (thème « {} ») ; miniatures : {} ({} %) — \
                 application déléguée au fil d'intégration de la barre (v0.6.0).",
                if parametres.barre_active { "activée" } else { "désactivée" },
                themes_barre::nom(&parametres.barre_theme),
                if parametres.miniatures_actives { "activées" } else { "désactivées" },
                parametres.miniatures_taille
            ));
            animations::rearmer(effet_choisi, rearmer_ouverture, rearmer_jelly);
            // Applique tout à chaud (animations, Jelly, opacité, profils,
            // raccourcis réenregistrés).
            tray::recharger_configuration();
            if interrupteur_touche(hwnd, ID_SW_ACTIF) && actif_voulu != window::est_actif() {
                window::basculer_activation();
            }
            if let Some(e) = effet_dwm {
                tray::programmer_effet_dwm(e);
            }
            let _ = DestroyWindow(hwnd);
        }
        Err(e) => {
            logging::erreur(&format!("Enregistrement des paramètres impossible : {e}"));
            message(hwnd, "Erreur d'enregistrement de config.toml (détails dans journal.log).");
        }
    }
}

// ---------------------------------------------------------------------------
// Capture des raccourcis
// ---------------------------------------------------------------------------

/// Index du champ dans `RACCOURCIS`.
fn index_champ(id: i32) -> Option<usize> {
    CHAMPS.iter().position(|c| *c == id)
}

/// Vrai si `champ` est en capture.
fn en_capture(champ: HWND) -> bool {
    CAPTURE.with(|c| match c.try_borrow() {
        Ok(c) => c.as_ref().is_some_and(|(h, _)| *h == champ.0 as isize),
        Err(_) => false,
    })
}

/// Vrai si une capture est en cours dans un des champs.
fn capture_active() -> bool {
    CAPTURE.with(|c| match c.try_borrow() {
        Ok(c) => c.is_some(),
        Err(_) => true,
    })
}

/// Retire la capture de `champ` et renvoie le texte à restaurer.
fn prendre_capture(champ: HWND) -> Option<String> {
    CAPTURE.with(|c| match c.try_borrow_mut() {
        Ok(mut c) if c.as_ref().is_some_and(|(h, _)| *h == champ.0 as isize) => {
            c.take().map(|(_, texte)| texte)
        }
        _ => None,
    })
}

/// Modificateurs actuellement tenus (état synchronisé avec la file de
/// messages : celui du message clavier en cours de traitement).
fn modificateurs_tenus() -> Modificateurs {
    let tenue = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Modificateurs {
        ctrl: tenue(VK_CONTROL.0),
        alt: tenue(VK_MENU.0),
        shift: tenue(VK_SHIFT.0),
        win: tenue(VK_LWIN.0) || tenue(VK_RWIN.0),
    }
}

/// Passe `champ` en capture (focus ou clic).
unsafe fn demarrer_capture(champ: HWND) {
    if en_capture(champ) {
        return;
    }
    let Some(index) = index_champ(GetDlgCtrlID(champ)) else {
        return;
    };
    let ancien = RACCOURCIS.with(|r| match r.try_borrow() {
        Ok(r) => r[index].clone(),
        Err(_) => String::new(),
    });
    let pose = CAPTURE.with(|c| match c.try_borrow_mut() {
        Ok(mut c) => {
            *c = Some((champ.0 as isize, ancien));
            true
        }
        Err(_) => false,
    });
    if !pose {
        return;
    }
    // Sans cela, la combinaison déjà enregistrée par l'application serait
    // interceptée par RegisterHotKey et n'arriverait jamais au champ.
    tray::suspendre_raccourcis();
    definir_texte_hwnd(champ, TEXTE_CAPTURE);
}

/// Abandonne la capture de `champ` (Echap, Tab, perte de focus) en
/// restaurant l'ancien texte.
unsafe fn annuler_capture(champ: HWND) {
    if let Some(ancien) = prendre_capture(champ) {
        definir_texte_hwnd(champ, &ancien);
    }
}

/// Aperçu des modificateurs tenus pendant la capture.
unsafe fn apercu_capture(champ: HWND) {
    let m = modificateurs_tenus();
    let texte = if m.aucun() {
        TEXTE_CAPTURE.to_string()
    } else {
        format!("{}…", m.prefixe())
    };
    definir_texte_hwnd(champ, &texte);
}

/// Traite l'appui d'une touche pendant la capture.
unsafe fn traiter_touche(champ: HWND, vk: u32) {
    let parent = GetParent(champ).unwrap_or_default();
    match touches::evaluer_appui(modificateurs_tenus(), vk) {
        Appui::ModificateurSeul => apercu_capture(champ),
        Appui::Annuler => {
            annuler_capture(champ);
            tray::reprendre_raccourcis();
        }
        Appui::Quitter { arriere } => {
            annuler_capture(champ);
            tray::reprendre_raccourcis();
            if !parent.0.is_null() {
                if let Ok(suivant) = GetNextDlgTabItem(parent, Some(champ), arriere) {
                    let _ = SetFocus(Some(suivant));
                }
            }
        }
        Appui::SansModificateur => {
            definir_texte_hwnd(champ, TEXTE_CAPTURE);
            message(
                parent,
                "Une touche seule n'est admise que pour F1 à F24 : ajoutez Ctrl, Alt ou Win.",
            );
        }
        Appui::Inconnue => {
            definir_texte_hwnd(champ, TEXTE_CAPTURE);
            message(
                parent,
                "Touche non prise en charge : lettres, chiffres, F1-F24, flèches, Espace, Tab, \
                 Echap, Suppr, Insertion, Début, Fin, Page préc./suiv.",
            );
        }
        Appui::Valide(texte) => {
            // Double vérification : le texte doit être accepté par l'analyse
            // utilisée à l'enregistrement des raccourcis globaux.
            if hotkey::analyser(&texte).is_none() {
                definir_texte_hwnd(champ, TEXTE_CAPTURE);
                message(parent, "Combinaison refusée.");
                return;
            }
            let Some(index) = index_champ(GetDlgCtrlID(champ)) else {
                return;
            };
            RACCOURCIS.with(|r| {
                if let Ok(mut r) = r.try_borrow_mut() {
                    r[index] = texte.clone();
                }
            });
            let _ = prendre_capture(champ);
            tray::reprendre_raccourcis();
            definir_texte_hwnd(champ, &texte);
            message(parent, "");
        }
    }
}

/// Procédure de sous-classe des champs de raccourci.
unsafe extern "system" fn procedure_champ(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id_sous_classe: usize,
    _donnees: usize,
) -> LRESULT {
    match msg {
        WM_GETDLGCODE => {
            if en_capture(hwnd) {
                // Toutes les touches (Tab, Entrée, Echap, flèches) au champ.
                return LRESULT(
                    (DLGC_WANTALLKEYS | DLGC_WANTCHARS | DLGC_WANTARROWS | DLGC_WANTTAB) as isize,
                );
            }
            // Hors capture : comportement de l'EDIT, sans la sélection
            // automatique du texte à l'arrivée par Tab.
            let code = DefSubclassProc(hwnd, msg, wparam, lparam).0;
            return LRESULT(code & !(DLGC_HASSETSEL as isize));
        }
        WM_SETFOCUS => {
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            let _ = HideCaret(Some(hwnd));
            demarrer_capture(hwnd);
            return r;
        }
        WM_LBUTTONDOWN => {
            // Le 1er clic donne le focus (donc la capture) ; un clic sur le
            // champ qui a déjà le focus relance la capture.
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            if GetFocus() == hwnd && !en_capture(hwnd) {
                demarrer_capture(hwnd);
            }
            return r;
        }
        WM_KILLFOCUS => {
            annuler_capture(hwnd);
            // Raccourcis rétablis, sauf si le focus passe à un autre champ de
            // raccourci (sa capture re-suspend aussitôt).
            let suivant = HWND(wparam.0 as *mut core::ffi::c_void);
            let autre_champ = !suivant.0.is_null()
                && suivant != hwnd
                && index_champ(GetDlgCtrlID(suivant)).is_some();
            if !autre_champ && !capture_active() {
                tray::reprendre_raccourcis();
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if en_capture(hwnd) {
                traiter_touche(hwnd, wparam.0 as u32);
                return LRESULT(0);
            }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if en_capture(hwnd) {
                apercu_capture(hwnd);
                return LRESULT(0);
            }
        }
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => {
            if en_capture(hwnd) {
                return LRESULT(0);
            }
        }
        // Pas de menu Copier/Coller sur un champ de raccourci.
        WM_CONTEXTMENU => return LRESULT(0),
        WM_NCDESTROY => {
            let _ = prendre_capture(hwnd);
            let _ = RemoveWindowSubclass(hwnd, Some(procedure_champ), id_sous_classe);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// ---------------------------------------------------------------------------
// Procédure de la fenêtre
// ---------------------------------------------------------------------------

/// Procédure de fenêtre des paramètres.
unsafe extern "system" fn procedure_parametres(
    hwnd: HWND,
    message_win: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message_win {
        WM_CREATE => {
            let _ = creer_police();
            EFFET_CHOISI.with(|c| c.set(false));
            FOCUS_MEMO.with(|c| c.set(0));
            construire(hwnd);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let entete = lparam.0 as *const NMHDR;
            if !entete.is_null()
                && (*entete).idFrom == ID_ONGLETS as usize
                && (*entete).code == TCN_SELCHANGE
            {
                let index = SendMessageW((*entete).hwndFrom, TCM_GETCURSEL, None, None).0;
                if let Ok(index) = usize::try_from(index) {
                    afficher_page(hwnd, index.min(PAGES.len() - 1));
                }
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            // lParam = HWND du curseur qui a bougé.
            let source = HWND(lparam.0 as *mut core::ffi::c_void);
            if !source.0.is_null() {
                match GetDlgCtrlID(source) {
                    ID_TB_OPACITE => {
                        let v = position_curseur(hwnd, ID_TB_OPACITE).unwrap_or(80);
                        definir_texte(hwnd, ID_VAL_OPACITE, &format!("{v} %"));
                    }
                    ID_TB_JELLY => {
                        let v = position_curseur(hwnd, ID_TB_JELLY).unwrap_or(50);
                        definir_texte(hwnd, ID_VAL_JELLY, &format!("{v} %"));
                        maj_etat_jelly(hwnd);
                    }
                    ID_TB_MINIATURES => {
                        let v = position_curseur(hwnd, ID_TB_MINIATURES)
                            .unwrap_or(config::MINIATURES_TAILLE_DEFAUT as i32);
                        definir_texte(hwnd, ID_VAL_MINIATURES, &format!("{v} %"));
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            match id {
                ID_VALIDER if code == BN_CLICKED => {
                    valider(hwnd);
                    // Valider refusé : la fenêtre reste ouverte, les raccourcis
                    // ne doivent pas rester suspendus (sauf capture en cours).
                    if IsWindow(Some(hwnd)).as_bool() && !capture_active() {
                        tray::reprendre_raccourcis();
                    }
                }
                ID_FERMER if code == BN_CLICKED => {
                    let _ = DestroyWindow(hwnd);
                }
                ID_CB_EFFET if code == CBN_SELCHANGE => EFFET_CHOISI.with(|c| c.set(true)),
                _ if INTERRUPTEURS.contains(&id)
                    && (code == BN_CLICKED || code == BN_DOUBLECLICKED) =>
                {
                    let source = HWND(lparam.0 as *mut core::ffi::c_void);
                    if !source.0.is_null() {
                        basculer_interrupteur(source);
                    }
                    if id == ID_SW_JELLY {
                        maj_etat_jelly(hwnd);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            let dessin = lparam.0 as *const DRAWITEMSTRUCT;
            if !dessin.is_null() && (*dessin).CtlType == ODT_BUTTON {
                dessiner_interrupteur(&*dessin);
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, message_win, wparam, lparam)
        }
        WM_CTLCOLORSTATIC => {
            // Libellés, curseurs et champs en lecture seule : fond blanc
            // système, comme le corps des onglets.
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let controle = HWND(lparam.0 as *mut core::ffi::c_void);
            let couleur_texte = match GetDlgCtrlID(controle) {
                ID_MESSAGE => ROUGE_ERREUR,
                ID_ETAT_JELLY | ID_ETAT_EFFETS => ORANGE_ETAT,
                ID_AIDE_DWM | ID_AIDE_RACCOURCIS | ID_NOTE_BARRE | ID_NOTE_MINIATURES => {
                    COLORREF(GetSysColor(COLOR_GRAYTEXT))
                }
                _ => COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
            };
            let _ = SetTextColor(hdc, couleur_texte);
            let _ = SetBkColor(hdc, COLORREF(GetSysColor(COLOR_WINDOW)));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        DM_GETDEFID => {
            // Bouton par défaut pour Entrée (IsDialogMessageW) : Fermer s'il
            // a le focus, Valider sinon.
            let fermer = GetDlgItem(Some(hwnd), ID_FERMER).unwrap_or_default();
            let focus = GetFocus();
            let id_focus = if focus.0.is_null() { 0 } else { GetDlgCtrlID(focus) };
            let id_parent_focus = match GetParent(focus) {
                Ok(p) if p != hwnd => GetDlgCtrlID(p),
                _ => 0,
            };
            let sans_defaut = |id: i32| {
                INTERRUPTEURS.contains(&id)
                    || matches!(
                        id,
                        ID_TB_OPACITE
                            | ID_TB_JELLY
                            | ID_TB_MINIATURES
                            | ID_CB_EFFET
                            | ID_CB_DWM
                            | ID_CB_THEME
                    )
            };
            if sans_defaut(id_focus) || sans_defaut(id_parent_focus) {
                // Entrée sur un interrupteur, un curseur ou une liste ne valide pas.
                return LRESULT(0);
            }
            let defaut = if !fermer.0.is_null() && GetFocus() == fermer {
                ID_FERMER
            } else {
                ID_VALIDER
            };
            LRESULT(((DC_HASDEFID << 16) | defaut as u32) as isize)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                let focus = GetFocus();
                if !focus.0.is_null() && IsChild(hwnd, focus).as_bool() {
                    FOCUS_MEMO.with(|c| c.set(focus.0 as isize));
                }
            }
            DefWindowProcW(hwnd, message_win, wparam, lparam)
        }
        WM_SETFOCUS => {
            // Réactivation : focus rendu au dernier contrôle (sauf champ de
            // raccourci, qui relancerait une capture), sinon aux onglets.
            let memo = hwnd_de(FOCUS_MEMO.with(|c| c.get()));
            let valide = !memo.0.is_null()
                && IsWindow(Some(memo)).as_bool()
                && IsChild(hwnd, memo).as_bool()
                && IsWindowVisible(memo).as_bool()
                && index_champ(GetDlgCtrlID(memo)).is_none();
            if valide {
                let _ = SetFocus(Some(memo));
            } else if let Ok(onglets) = GetDlgItem(Some(hwnd), ID_ONGLETS) {
                let _ = SetFocus(Some(onglets));
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            FENETRE.with(|c| c.set(0));
            // Fermeture sans enregistrer (ou après une capture abandonnée) :
            // les raccourcis globaux suspendus sont rétablis. Sans effet
            // après « Valider » (déjà réenregistrés par le rechargement).
            tray::reprendre_raccourcis();
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // Les contrôles enfants sont détruits : la police peut l'être.
            let brut = POLICE.with(|p| p.replace(0));
            if brut != 0 {
                let _ = DeleteObject(HGDIOBJ(brut as *mut core::ffi::c_void));
            }
            CAPTURE.with(|c| {
                if let Ok(mut c) = c.try_borrow_mut() {
                    *c = None;
                }
            });
            RACCOURCIS.with(|r| {
                if let Ok(mut r) = r.try_borrow_mut() {
                    *r = [String::new(), String::new(), String::new()];
                }
            });
            EFFET_CHOISI.with(|c| c.set(false));
            FOCUS_MEMO.with(|c| c.set(0));
            DefWindowProcW(hwnd, message_win, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message_win, wparam, lparam),
    }
}
