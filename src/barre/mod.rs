//! Intégration de la barre des tâches (v0.6.0, portage de RustHawk).
//!
//! La DLL `rustpiz_runtime.dll` (crate `crates/rustpiz-runtime`, construite
//! par `build.rs`) et les 54 thèmes sont embarqués dans l'exécutable.
//!
//! La logique utile de RustHawk est portée ici et branchée aux réglages
//! (onglets « barre des tâches » et « miniatures »). Sous-modules :
//! - [`bilan`] (pur) : lecture du rapport d'état (thème réellement appliqué ?) ;
//! - [`decision`] (pur) : décisions, génération d'`active.json`, empreinte,
//!   compilation de thème ;
//! - [`politique`] (pur) : réessais (30 s, 2 min, 10 min puis arrêt), échecs
//!   durables, décisions de la surveillance ;
//! - [`pe`] (pur) : lecture de l'en-tête PE du module de la barre ;
//! - [`symboles`] (pur) : RVA des fonctions de miniatures dans le PDB ;
//! - `injection` (Windows) : déploiement de la DLL, `InitializeXamlDiagnosticsEx` ;
//! - `tube` (Windows) : client du tube de contrôle ;
//! - `orchestration` (Windows) : fil dédié, cycle de vie, [`appliquer`]/[`arreter`].
//!
//! La DLL n'est jamais écrite dans `explorer.exe` ni `Taskbar.View.dll` : elle
//! est extraite dans `%LOCALAPPDATA%\RustPiz\runtime\` et chargée par les
//! diagnostics XAML. Après désactivation elle reste chargée mais inerte dans
//! explorer jusqu'à son redémarrage.
//!
//! Apartement COM : `injection::injecter` initialise le fil dédié en STA
//! (`CoInitializeEx(COINIT_APARTMENTTHREADED)`) sans pompe de messages ni
//! `CoUninitialize` ; ce fil n'héberge aucun objet COM appelé de l'extérieur.
//! Laissé tel quel (point 12 de l'audit, documenté).

pub mod themes_embarques;

mod bilan;
mod decision;
mod pe;
mod politique;
mod symboles;

#[cfg(windows)]
mod injection;
#[cfg(windows)]
mod orchestration;
#[cfg(windows)]
mod tube;

#[cfg(windows)]
pub use orchestration::{appliquer, arreter, ModeArret};

/// Chaîne d'outils qui a produit la DLL embarquée (posée par `build.rs` :
/// MSVC via cargo-xwin comme RustHawk, ou repli GNU/MinGW).
pub const CHAINE_DLL: &str = env!("RUSTPIZ_RUNTIME_CHAINE");

/// Image PE de `rustpiz_runtime.dll`, produite par `build.rs`.
static DLL_RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rustpiz_runtime.dll"));

/// Vérifie l'en-tête d'une image PE x64 de type DLL : signature `MZ`,
/// signature `PE\0\0` à `e_lfanew`, machine AMD64 (0x8664) et indicateur
/// `IMAGE_FILE_DLL` (0x2000). Fonction pure.
pub fn valider_dll_pe(octets: &[u8]) -> Result<(), &'static str> {
    let lire_u16 = |pos: usize| -> Option<u16> {
        octets
            .get(pos..pos.checked_add(2)?)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    if !octets.starts_with(b"MZ") {
        return Err("signature MZ absente");
    }
    let e_lfanew = octets
        .get(0x3C..0x40)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or("en-tête DOS tronqué")?;
    let pe = octets
        .get(e_lfanew..e_lfanew.checked_add(4).ok_or("e_lfanew invalide")?)
        .ok_or("en-tête PE hors du fichier")?;
    if pe != b"PE\0\0" {
        return Err("signature PE absente");
    }
    let machine = lire_u16(e_lfanew + 4).ok_or("en-tête COFF tronqué")?;
    if machine != 0x8664 {
        return Err("machine différente de x64");
    }
    let caracteristiques = lire_u16(e_lfanew + 22).ok_or("en-tête COFF tronqué")?;
    if caracteristiques & 0x2000 == 0 {
        return Err("image qui n'est pas une DLL");
    }
    Ok(())
}

/// Vérifie la DLL embarquée ; renvoie sa taille en octets.
///
/// `black_box` empêche l'optimiseur de replier la vérification à la
/// compilation (contrôle au démarrage, journalisé ; l'image est écrite sur
/// disque par `injection::deployer_dll` avant l'injection).
pub fn verifier_dll_embarquee() -> Result<usize, &'static str> {
    let octets: &[u8] = std::hint::black_box(DLL_RUNTIME);
    valider_dll_pe(octets).map(|()| octets.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pe_minimal(machine: u16, caracteristiques: u16) -> Vec<u8> {
        let mut o = vec![0u8; 0x100];
        o[0] = b'M';
        o[1] = b'Z';
        o[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        o[0x80..0x84].copy_from_slice(b"PE\0\0");
        o[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        o[0x96..0x98].copy_from_slice(&caracteristiques.to_le_bytes());
        o
    }

    #[test]
    fn pe_valide_et_invalides() {
        assert_eq!(valider_dll_pe(&pe_minimal(0x8664, 0x2022)), Ok(()));
        assert!(valider_dll_pe(&pe_minimal(0x014C, 0x2022)).is_err());
        assert!(valider_dll_pe(&pe_minimal(0x8664, 0x0022)).is_err());
        assert!(valider_dll_pe(b"MZ").is_err());
        assert!(valider_dll_pe(b"").is_err());
        let mut hors = pe_minimal(0x8664, 0x2022);
        hors[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(valider_dll_pe(&hors).is_err());
    }

    #[test]
    fn dll_embarquee_valide() {
        assert!(verifier_dll_embarquee().unwrap() > 100_000);
    }
}
