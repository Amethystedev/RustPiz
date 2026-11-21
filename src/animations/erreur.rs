//! Erreurs du module d'animations.

use std::fmt;

/// Erreur rencontrée lors de la préparation ou du lancement d'une animation.
#[derive(Debug, Clone)]
pub enum ErreurAnimation {
    /// Échec d'un appel Windows (COM, Direct3D, DirectComposition, WinRT).
    Win32(windows::core::Error),
    /// Aucun périphérique Direct3D 11 matériel (pas de repli logiciel WARP).
    GpuMaterielAbsent,
    /// Windows.Graphics.Capture n'est pas pris en charge.
    CaptureNonSupportee,
    /// Impossible de capturer sans la bordure jaune de capture.
    BordureImposee,
    /// Taille de fenêtre nulle ou démesurée.
    TailleInvalide,
    /// Fil de capture WGC occupé, bloqué ou délai de création dépassé
    /// (erreur limitée à une fenêtre : le fil principal n'est jamais bloqué).
    FilCapture(String),
    /// Rendu Direct3D du maillage Jelly impossible (compilateur de shaders
    /// absent, shader refusé, objet non créé...) : aucun rendu de repli.
    Rendu(String),
    /// Effet à shader impossible pour cette animation (v0.4.7) : erreur
    /// non globale (repli sur « Fondu » ou animation abandonnée).
    Shader(String),
}

impl ErreurAnimation {
    /// Vrai si l'erreur rend les animations indisponibles pour toute la
    /// session (et non pour une seule fenêtre).
    pub fn est_globale(&self) -> bool {
        matches!(
            self,
            ErreurAnimation::GpuMaterielAbsent
                | ErreurAnimation::CaptureNonSupportee
                | ErreurAnimation::BordureImposee
        )
    }
}

impl From<windows::core::Error> for ErreurAnimation {
    fn from(e: windows::core::Error) -> Self {
        ErreurAnimation::Win32(e)
    }
}

impl fmt::Display for ErreurAnimation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErreurAnimation::Win32(e) => write!(
                f,
                "erreur Windows {:#010x} : {}",
                e.code().0 as u32,
                e.message()
            ),
            ErreurAnimation::GpuMaterielAbsent => {
                write!(f, "aucun processeur graphique Direct3D 11 matériel disponible")
            }
            ErreurAnimation::CaptureNonSupportee => write!(
                f,
                "la capture de fenêtres (Windows.Graphics.Capture) n'est pas prise en charge"
            ),
            ErreurAnimation::BordureImposee => write!(
                f,
                "Windows impose la bordure de capture (capture sans bordure indisponible)"
            ),
            ErreurAnimation::TailleInvalide => write!(f, "taille de fenêtre invalide"),
            ErreurAnimation::FilCapture(t) => write!(f, "fil de capture : {t}"),
            ErreurAnimation::Rendu(t) => write!(f, "rendu du maillage : {t}"),
            ErreurAnimation::Shader(t) => write!(f, "effet GPU : {t}"),
        }
    }
}

impl std::error::Error for ErreurAnimation {}
