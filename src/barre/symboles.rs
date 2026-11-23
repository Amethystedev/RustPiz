//! Recherche des fonctions de miniatures dans un PDB public de Microsoft.
//!
//! Porté de RustHawk (`symbols.rs`). Pur (crate `pdb`, Rust intégral) :
//! les RVA trouvées sont passées à la DLL, qui les détourne dans explorer.

use std::fs::File;
use std::path::Path;

const PREFIXE_ECHELLE: &str =
    "?GetScaledThumbnailSize@ThumbnailHelpers@implementation@Taskbar@winrt@@";
const PREFIXE_TEMPLATE: &str =
    "?OnApplyTemplate@TaskItemThumbnailView@implementation@Taskbar@winrt@@";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RvasMiniatures {
    pub taille_echelle: u32,
    pub on_apply_template: u32,
}

#[derive(Default)]
struct Collecteur {
    echelle: Vec<u32>,
    template: Vec<u32>,
}

impl Collecteur {
    fn proposer(&mut self, nom: &str, rva: u32) {
        if nom.starts_with(PREFIXE_ECHELLE) {
            self.echelle.push(rva);
        } else if nom.starts_with(PREFIXE_TEMPLATE) && nom.ends_with("XXZ") {
            self.template.push(rva);
        }
    }

    fn terminer(mut self) -> Result<RvasMiniatures, String> {
        let unique = |valeurs: &mut Vec<u32>, etiquette: &str| {
            valeurs.sort_unstable();
            valeurs.dedup();
            match valeurs.as_slice() {
                [rva] => Ok(*rva),
                [] => Err(format!("symbole introuvable : {etiquette}")),
                _ => Err(format!("symbole ambigu : {etiquette}")),
            }
        };
        Ok(RvasMiniatures {
            taille_echelle: unique(&mut self.echelle, "GetScaledThumbnailSize")?,
            on_apply_template: unique(&mut self.template, "OnApplyTemplate")?,
        })
    }
}

/// Taille minimale plausible d'un PDB public de module de la barre (les vrais
/// font plusieurs Mio ; un PDB MSF vide fait déjà plusieurs Kio).
pub const TAILLE_MIN_PDB: u64 = 16 * 1024;

/// Vérifie un PDB téléchargé AVANT de l'adopter : taille minimale, format
/// MSF lisible par la crate `pdb`, et GUID égal à celui attendu (32 chiffres
/// hexadécimaux, voir `pe::guid_texte` ; l'âge n'est pas comparé, celui du
/// flux d'information pouvant dépasser celui de l'image).
pub fn verifier_pdb(chemin: &Path, guid_attendu: &str) -> Result<(), String> {
    let taille = std::fs::metadata(chemin)
        .map_err(|e| format!("{}: {e}", chemin.display()))?
        .len();
    if taille < TAILLE_MIN_PDB {
        return Err(format!("PDB trop petit ({taille} octets)"));
    }
    let fichier = File::open(chemin).map_err(|e| format!("{}: {e}", chemin.display()))?;
    let mut base = pdb::PDB::open(fichier).map_err(|e| format!("PDB illisible : {e}"))?;
    let info = base
        .pdb_information()
        .map_err(|e| format!("PDB sans flux d'information : {e}"))?;
    let (d1, d2, d3, d4) = info.guid.as_fields();
    let guid = super::pe::guid_texte(d1, d2, d3, d4);
    if !guid_attendu.eq_ignore_ascii_case(&guid) {
        return Err(format!("PDB d'un autre module (GUID {guid}, attendu {guid_attendu})"));
    }
    Ok(())
}

/// Lit les symboles publics d'un PDB.
pub fn localiser(chemin: &Path) -> Result<RvasMiniatures, String> {
    let decrire = |e: pdb::Error| format!("PDB {} : {e}", chemin.display());
    let fichier = File::open(chemin).map_err(|e| format!("{}: {e}", chemin.display()))?;
    let mut base = pdb::PDB::open(fichier).map_err(decrire)?;
    let symboles = base.global_symbols().map_err(decrire)?;
    let adresses = base.address_map().map_err(decrire)?;

    let mut collecteur = Collecteur::default();
    let mut iterateur = symboles.iter();
    while let Some(symbole) = pdb::FallibleIterator::next(&mut iterateur).map_err(decrire)? {
        if let Ok(pdb::SymbolData::Public(public)) = symbole.parse() {
            if let Some(rva) = public.offset.to_rva(&adresses) {
                collecteur.proposer(&public.name.to_string(), rva.0);
            }
        }
    }
    collecteur.terminer()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ECHELLE: &str = "?GetScaledThumbnailSize@ThumbnailHelpers@implementation@Taskbar@winrt@@YA?AUSize@Foundation@Windows@4@U5674@M@Z";
    const TEMPLATE: &str =
        "?OnApplyTemplate@TaskItemThumbnailView@implementation@Taskbar@winrt@@QEAAXXZ";

    #[test]
    fn fonctions_exactes_selectionnees() {
        let mut collecteur = Collecteur::default();
        collecteur.proposer(ECHELLE, 0x100);
        collecteur.proposer(TEMPLATE, 0x200);
        collecteur.proposer(
            "?OnApplyTemplate@Other@implementation@Taskbar@winrt@@QEAAXXZ",
            0x300,
        );
        collecteur.proposer(
            "?OnApplyTemplate@TaskItemThumbnailView@implementation@Taskbar@winrt@@QEAAJXZ",
            0x400,
        );
        assert_eq!(
            collecteur.terminer().unwrap(),
            RvasMiniatures {
                taille_echelle: 0x100,
                on_apply_template: 0x200
            }
        );
    }

    #[test]
    fn symboles_absents_ou_ambigus_sont_des_erreurs() {
        assert!(Collecteur::default().terminer().is_err());
        let mut collecteur = Collecteur::default();
        collecteur.proposer(ECHELLE, 1);
        collecteur.proposer(ECHELLE, 2);
        collecteur.proposer(TEMPLATE, 3);
        assert!(collecteur.terminer().unwrap_err().contains("ambigu"));
    }

    #[test]
    fn pdb_tronque_ou_invalide_refuse() {
        let dossier = std::env::temp_dir().join(format!("rustpiz-pdb-{}", std::process::id()));
        std::fs::create_dir_all(&dossier).unwrap();
        let petit = dossier.join("petit.pdb");
        std::fs::write(&petit, b"Microsoft C/C++ MSF 7.00\r\n").unwrap();
        assert!(verifier_pdb(&petit, "0").unwrap_err().contains("trop petit"));
        let faux = dossier.join("faux.pdb");
        std::fs::write(&faux, vec![0x5Au8; TAILLE_MIN_PDB as usize]).unwrap();
        assert!(verifier_pdb(&faux, "0").unwrap_err().contains("illisible"));
        assert!(verifier_pdb(&dossier.join("absent.pdb"), "0").is_err());
        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn doublons_replies_acceptes() {
        let mut collecteur = Collecteur::default();
        collecteur.proposer(ECHELLE, 1);
        collecteur.proposer(ECHELLE, 1);
        collecteur.proposer(TEMPLATE, 3);
        assert!(collecteur.terminer().is_ok());
    }
}
