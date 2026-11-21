//! Effets visuels modernes via le Desktop Window Manager (DWM).
//!
//! - **Mica / Mica Alt / Acrylique** : appliqués avec `DwmSetWindowAttribute`
//!   et l'attribut `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2 et +).
//! - **Verre** : obtention d'une feuille de verre complète avec
//!   `DwmExtendFrameIntoClientArea` et des marges à -1.
//!
//! Ces effets sont indépendants de la transparence `WS_EX_LAYERED` : ils
//! modifient la matière d'arrière-plan de la fenêtre. Les deux peuvent se
//! combiner, avec des résultats variables selon les applications.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
    DWMWA_SYSTEMBACKDROP_TYPE, DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMSBT_NONE,
    DWMSBT_TABBEDWINDOW, DWMSBT_TRANSIENTWINDOW,
};
use windows::Win32::UI::Controls::MARGINS;

/// Effet DWM applicable à une fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffetDwm {
    /// Aucun effet (restauration).
    Aucun,
    /// Mica : matière opaque teintée du fond d'écran (Windows 11).
    Mica,
    /// Mica Alt (variante « fenêtre à onglets », teinte plus prononcée).
    MicaAlt,
    /// Acrylique : flou translucide.
    Acrylique,
    /// Verre complet via `DwmExtendFrameIntoClientArea(-1)`.
    Verre,
}

impl EffetDwm {
    /// Convertit un nom de la configuration (ou du menu) en effet.
    pub fn depuis_nom(nom: &str) -> Option<EffetDwm> {
        match nom.trim().to_lowercase().as_str() {
            "aucun" | "none" | "" => Some(EffetDwm::Aucun),
            "mica" => Some(EffetDwm::Mica),
            "micaalt" | "mica_alt" | "mica-alt" => Some(EffetDwm::MicaAlt),
            "acrylique" | "acrylic" => Some(EffetDwm::Acrylique),
            "verre" | "glass" => Some(EffetDwm::Verre),
            _ => None,
        }
    }

    /// Nom affichable de l'effet.
    pub fn nom(self) -> &'static str {
        match self {
            EffetDwm::Aucun => "Aucun",
            EffetDwm::Mica => "Mica",
            EffetDwm::MicaAlt => "Mica Alt",
            EffetDwm::Acrylique => "Acrylique",
            EffetDwm::Verre => "Verre",
        }
    }
}

/// Lit la valeur actuelle de `DWMWA_SYSTEMBACKDROP_TYPE` (0-4),
/// pour pouvoir restaurer l'état d'origine plus tard.
pub fn lire_backdrop(hwnd: HWND) -> Option<i32> {
    unsafe {
        let mut valeur: i32 = 0;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &mut valeur as *mut i32 as *mut core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
        .ok()?;
        Some(valeur)
    }
}

/// Écrit une valeur `DWMWA_SYSTEMBACKDROP_TYPE` brute (0-4).
pub fn definir_backdrop_brut(hwnd: HWND, valeur: i32) -> windows::core::Result<()> {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &valeur as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
    }
}

/// Applique le « verre » complet (marges -1) ou le retire (marges 0).
pub fn definir_verre(hwnd: HWND, actif: bool) -> windows::core::Result<()> {
    let marge = if actif { -1 } else { 0 };
    let marges = MARGINS {
        cxLeftWidth: marge,
        cxRightWidth: marge,
        cyTopHeight: marge,
        cyBottomHeight: marge,
    };
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &marges) }
}

/// Applique un effet DWM à une fenêtre.
/// Retourne la valeur d'origine du backdrop (pour restauration) si l'on
/// vient de le modifier pour la première fois.
pub fn appliquer(hwnd: HWND, effet: EffetDwm) -> windows::core::Result<Option<i32>> {
    let origine = lire_backdrop(hwnd);
    match effet {
        EffetDwm::Aucun => {
            definir_backdrop_brut(hwnd, DWMSBT_NONE.0)?;
            definir_verre(hwnd, false)?;
        }
        EffetDwm::Mica => definir_backdrop_brut(hwnd, DWMSBT_MAINWINDOW.0)?,
        EffetDwm::MicaAlt => definir_backdrop_brut(hwnd, DWMSBT_TABBEDWINDOW.0)?,
        EffetDwm::Acrylique => definir_backdrop_brut(hwnd, DWMSBT_TRANSIENTWINDOW.0)?,
        EffetDwm::Verre => definir_verre(hwnd, true)?,
    }
    // Conversion vers le type opaque de la crate pour la cohérence d'API.
    let _ = DWM_SYSTEMBACKDROP_TYPE(0);
    Ok(origine)
}

/// Restaure l'effet d'origine mémorisé (backdrop + verre).
pub fn restaurer(hwnd: HWND, backdrop_origine: Option<i32>, verre_applique: bool) {
    if let Some(valeur) = backdrop_origine {
        let _ = definir_backdrop_brut(hwnd, valeur);
    }
    if verre_applique {
        let _ = definir_verre(hwnd, false);
    }
}
