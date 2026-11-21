//! Vérification de compatibilité avec Windows 11.
//!
//! `GetVersionExW` ment sans manifeste : on lit donc le numéro de build
//! directement dans le registre (`HKLM\SOFTWARE\Microsoft\Windows NT\
//! CurrentVersion`). Windows 11 = build 22000 et supérieur. Le résultat
//! est consigné dans le journal au démarrage.

use windows::core::w;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

use crate::logging;

/// Lit une valeur chaîne (REG_SZ) de la clé « CurrentVersion » de Windows.
fn lire_valeur_registre(nom: &str) -> Option<String> {
    unsafe {
        let nom_large: Vec<u16> = nom.encode_utf16().chain(std::iter::once(0)).collect();
        let mut tampon = [0u16; 64];
        let mut taille = (tampon.len() * std::mem::size_of::<u16>()) as u32;
        let resultat = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            windows::core::PCWSTR(nom_large.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(tampon.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut taille),
        );
        if resultat != ERROR_SUCCESS {
            return None;
        }
        let longueur = (taille as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&tampon[..longueur]))
    }
}

/// Vérifie la version de Windows et consigne le résultat.
/// Retourne vrai si l'on tourne sur Windows 11 (build ≥ 22000).
pub fn verifier() -> bool {
    let build = lire_valeur_registre("CurrentBuildNumber")
        .and_then(|b| b.parse::<u32>().ok())
        .unwrap_or(0);
    let version = lire_valeur_registre("DisplayVersion").unwrap_or_else(|| "?".to_string());

    if build >= 22000 {
        logging::info(&format!(
            "Compatibilité OK : Windows 11 (build {build}, version {version})."
        ));
        true
    } else if build > 0 {
        logging::avertir(&format!(
            "Windows 10 ou antérieur détecté (build {build}, version {version}) : \
             RustPiz est conçu pour Windows 11. Les effets Mica/Acrylique \
             ne fonctionneront pas."
        ));
        false
    } else {
        logging::avertir(
            "Impossible de déterminer la version de Windows dans le registre.",
        );
        false
    }
}
