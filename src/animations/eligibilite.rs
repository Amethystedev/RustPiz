//! Éligibilité d'une fenêtre aux animations d'ouverture / fermeture.
//!
//! La partie [`raison_style`] est pure (testable
//! hors Windows) ; `verifier` interroge Win32 / DWM et renvoie la raison
//! d'un rejet (journalisée par l'appelant).

/// Constantes de style Win32 (valeurs documentées, recopiées ici pour que
/// la partie pure ne dépende pas de la crate `windows`).
const WS_CHILD: u32 = 0x4000_0000;
/// `WS_CAPTION` = `WS_BORDER | WS_DLGFRAME` : barre de titre complète.
const WS_CAPTION: u32 = 0x00C0_0000;
/// Cadre redimensionnable.
const WS_THICKFRAME: u32 = 0x0004_0000;
/// Boutons Réduire / Agrandir (sens valable pour les fenêtres racines).
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// Dimensions minimales / maximales (pixels physiques).
const LARGEUR_MIN: i32 = 100;
const HAUTEUR_MIN: i32 = 60;
/// Limite des textures Direct3D 11 (et des surfaces DirectComposition).
const COTE_MAX: i32 = 16384;

/// Caractéristiques d'une fenêtre utiles au filtrage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub style: u32,
    pub exstyle: u32,
    /// La fenêtre est sa propre racine (`GetAncestor(GA_ROOT) == hwnd`).
    pub est_racine: bool,
    pub pid: u32,
    pub pid_courant: u32,
    pub largeur: i32,
    pub hauteur: i32,
}

/// Vrai si le style ressemble à une fenêtre d'application : barre de titre
/// complète (`WS_CAPTION`, cas du Bloc-notes, de l'Explorateur, d'Edge /
/// Chrome, des hôtes UWP), OU cadre redimensionnable avec bouton Réduire /
/// Agrandir (applications à barre de titre personnalisée sans
/// `WS_CAPTION`, ex. certaines applications Electron).
fn cadre_application(style: u32) -> bool {
    style & WS_CAPTION == WS_CAPTION
        || (style & WS_THICKFRAME != 0 && style & (WS_MINIMIZEBOX | WS_MAXIMIZEBOX) != 0)
}

/// Raison du rejet par le style (`None` = acceptable). Fenêtre
/// d'application « normale » : racine, non enfant, avec cadre
/// d'application, ni fenêtre d'outil ni non activable, appartenant à un
/// autre processus, de taille raisonnable.
pub fn raison_style(s: &Style) -> Option<&'static str> {
    if !s.est_racine || s.style & WS_CHILD != 0 {
        return Some("fenêtre enfant / non racine");
    }
    if !cadre_application(s.style) {
        return Some("pas de cadre d'application (ni WS_CAPTION ni cadre redimensionnable)");
    }
    if s.exstyle & WS_EX_TOOLWINDOW != 0 {
        return Some("fenêtre d'outil (WS_EX_TOOLWINDOW)");
    }
    if s.exstyle & WS_EX_NOACTIVATE != 0 {
        return Some("fenêtre non activable (WS_EX_NOACTIVATE)");
    }
    if s.pid == 0 {
        return Some("processus inconnu");
    }
    if s.pid == s.pid_courant {
        return Some("fenêtre de RustPiz");
    }
    if !(LARGEUR_MIN..=COTE_MAX).contains(&s.largeur) || !(HAUTEUR_MIN..=COTE_MAX).contains(&s.hauteur) {
        return Some("taille hors limites");
    }
    None
}

/// Filtre minimal appliqué dans le rappel `EVENT_OBJECT_CREATE` (v0.4.7),
/// avant toute mise en file : fenêtre non enfant, avec cadre
/// d'application, ni fenêtre d'outil ni non activable. Pur, sans appel
/// Win32 (la taille et le processus ne sont pas encore fiables à la
/// création ; l'éligibilité complète est vérifiée au SHOW).
pub fn filtre_creation(style: u32, exstyle: u32) -> bool {
    style & WS_CHILD == 0
        && cadre_application(style)
        && exstyle & WS_EX_TOOLWINDOW == 0
        && exstyle & WS_EX_NOACTIVATE == 0
}

/// Vrai si [`raison_style`] ne trouve aucun motif de rejet (tests).
#[cfg(test)]
pub fn style_acceptable(s: &Style) -> bool {
    raison_style(s).is_none()
}

#[cfg(windows)]
pub use win32::{bornes_fenetre, verifier};

#[cfg(windows)]
mod win32 {
    use super::{raison_style, Style};

    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowDisplayAffinity, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsWindow, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, WDA_NONE,
    };

    use crate::window;

    /// Rectangle visible de la fenêtre (sans l'ombre DWM), en pixels
    /// physiques. Repli sur `GetWindowRect` si DWM ne répond pas.
    pub unsafe fn bornes_fenetre(hwnd: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok();
        if !ok && GetWindowRect(hwnd, &mut r).is_err() {
            return None;
        }
        Some(r)
    }

    /// Vérifie que la fenêtre peut être animée : non exclue (mêmes règles
    /// que la transparence, sans journal), style acceptable, non masquée
    /// par DWM et non protégée contre la capture. `Err(raison)` sinon (la
    /// raison est destinée au journal).
    pub unsafe fn verifier(hwnd: HWND) -> Result<(), String> {
        if hwnd.0.is_null() || !IsWindow(Some(hwnd)).as_bool() {
            return Err("handle invalide ou fenêtre détruite".into());
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Some(r) = bornes_fenetre(hwnd) else {
            return Err("bornes illisibles".into());
        };
        let style = Style {
            style: GetWindowLongW(hwnd, GWL_STYLE) as u32,
            exstyle: GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            est_racine: GetAncestor(hwnd, GA_ROOT) == hwnd,
            pid,
            pid_courant: GetCurrentProcessId(),
            largeur: r.right - r.left,
            hauteur: r.bottom - r.top,
        };
        if let Some(raison) = raison_style(&style) {
            return Err(format!(
                "{raison} (style {:#010x}, exstyle {:#010x}, {}x{})",
                style.style, style.exstyle, style.largeur, style.hauteur
            ));
        }
        // Fenêtre déjà masquée par DWM (bureau virtuel, application UWP
        // suspendue...) : rien à animer.
        let mut masquee = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut masquee as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && masquee != 0
        {
            return Err(format!("masquée par DWM (DWMWA_CLOAKED = {masquee:#x})"));
        }
        // Fenêtre protégée (lecteurs DRM, gestionnaires de mots de passe...) :
        // on ne la capture jamais.
        let mut affinite = 0u32;
        if GetWindowDisplayAffinity(hwnd, &mut affinite).is_err() {
            return Err("affinité d'affichage illisible".into());
        }
        if affinite != WDA_NONE.0 {
            return Err(format!("protégée contre la capture (affinité {affinite:#x})"));
        }
        // Exclusions de l'utilisateur (shell, plein écran, processus...) en
        // dernier : c'est le test le plus coûteux (ouverture de processus).
        if let Some(raison) = window::raison_exclusion_silencieuse(hwnd) {
            return Err(raison.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;

    fn normale() -> Style {
        Style {
            style: WS_OVERLAPPEDWINDOW | 0x1000_0000, // + WS_VISIBLE
            exstyle: 0x0000_0100,                      // WS_EX_WINDOWEDGE
            est_racine: true,
            pid: 42,
            pid_courant: 7,
            largeur: 1024,
            hauteur: 768,
        }
    }

    #[test]
    fn accepte_fenetre_normale() {
        assert!(style_acceptable(&normale()));
    }

    #[test]
    fn refuse_enfant() {
        let mut s = normale();
        s.style |= WS_CHILD;
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.est_racine = false;
        assert!(!style_acceptable(&s));
    }

    #[test]
    fn refuse_toolwindow_et_noactivate() {
        let mut s = normale();
        s.exstyle |= WS_EX_TOOLWINDOW;
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.exstyle |= WS_EX_NOACTIVATE;
        assert!(!style_acceptable(&s));
    }

    #[test]
    fn refuse_sans_barre_de_titre() {
        let mut s = normale();
        s.style = 0x8000_0000; // WS_POPUP seul
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.style = 0x0080_0000; // WS_BORDER seul (caption incomplète)
        assert!(!style_acceptable(&s));
        // Cadre redimensionnable sans bouton : refusé.
        let mut s = normale();
        s.style = 0x8000_0000 | WS_THICKFRAME;
        assert!(!style_acceptable(&s));
    }

    #[test]
    fn accepte_barre_de_titre_personnalisee() {
        // WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX (Electron sans cadre).
        let mut s = normale();
        s.style = 0x8000_0000 | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        assert!(style_acceptable(&s));
    }

    #[test]
    fn raisons_de_rejet_explicites() {
        assert_eq!(raison_style(&normale()), None);
        let mut s = normale();
        s.exstyle |= WS_EX_TOOLWINDOW;
        assert!(raison_style(&s).unwrap().contains("outil"));
        let mut s = normale();
        s.pid = s.pid_courant;
        assert!(raison_style(&s).unwrap().contains("RustPiz"));
        let mut s = normale();
        s.hauteur = 10;
        assert!(raison_style(&s).unwrap().contains("taille"));
    }

    #[test]
    fn filtre_creation_accepte_fenetre_d_application() {
        assert!(filtre_creation(WS_OVERLAPPEDWINDOW, 0x0000_0100));
        // Barre de titre personnalisée (Electron sans cadre).
        assert!(filtre_creation(
            0x8000_0000 | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
            0
        ));
    }

    #[test]
    fn filtre_creation_refuse_enfant_outil_non_activable() {
        assert!(!filtre_creation(WS_OVERLAPPEDWINDOW | WS_CHILD, 0));
        assert!(!filtre_creation(WS_OVERLAPPEDWINDOW, WS_EX_TOOLWINDOW));
        assert!(!filtre_creation(WS_OVERLAPPEDWINDOW, WS_EX_NOACTIVATE));
    }

    #[test]
    fn filtre_creation_refuse_sans_cadre() {
        assert!(!filtre_creation(0x8000_0000, 0)); // WS_POPUP seul
        assert!(!filtre_creation(0x8000_0000 | WS_THICKFRAME, 0));
        assert!(!filtre_creation(0, 0));
    }

    #[test]
    fn refuse_propre_processus() {
        let mut s = normale();
        s.pid = s.pid_courant;
        assert!(!style_acceptable(&s));
    }

    #[test]
    fn refuse_trop_petite_ou_trop_grande() {
        let mut s = normale();
        s.largeur = 99;
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.hauteur = 59;
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.largeur = 16385;
        assert!(!style_acceptable(&s));
        let mut s = normale();
        s.largeur = 100;
        s.hauteur = 60;
        assert!(style_acceptable(&s));
    }
}
