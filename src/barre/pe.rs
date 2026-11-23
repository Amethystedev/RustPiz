//! Lecture minimale d'un fichier PE64 : signature et identifiant de PDB.
//!
//! Porté de RustHawk (`pe.rs`). Pur : aucune dépendance Win32. Sert à
//! retrouver le PDB public correspondant au module de la barre des tâches.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoPe {
    pub horodatage: u32,
    pub taille_image: u32,
    pub nom_pdb: String,
    /// Dossier du serveur de symboles : GUID suivi de l'âge.
    pub id_pdb: String,
}

fn lire_u16(data: &[u8], decalage: usize) -> Option<u16> {
    let octets = data.get(decalage..decalage.checked_add(2)?)?;
    Some(u16::from_le_bytes(octets.try_into().ok()?))
}

fn lire_u32(data: &[u8], decalage: usize) -> Option<u32> {
    let octets = data.get(decalage..decalage.checked_add(4)?)?;
    Some(u32::from_le_bytes(octets.try_into().ok()?))
}

/// Convertit une RVA en position dans le fichier.
fn rva_vers_decalage(data: &[u8], sections: usize, nombre: usize, rva: u32) -> Option<usize> {
    for index in 0..nombre {
        let entete = sections.checked_add(index.checked_mul(40)?)?;
        let taille_virtuelle = lire_u32(data, entete + 8)?;
        let adresse_virtuelle = lire_u32(data, entete + 12)?;
        let taille_brute = lire_u32(data, entete + 16)?;
        let pointeur_brut = lire_u32(data, entete + 20)?;
        let etendue = taille_virtuelle.max(taille_brute);
        if rva >= adresse_virtuelle && rva - adresse_virtuelle < etendue {
            return (pointeur_brut as usize).checked_add((rva - adresse_virtuelle) as usize);
        }
    }
    None
}

/// GUID au format du serveur de symboles (32 chiffres hexadécimaux
/// majuscules, sans tiret) ; l'identifiant complet y ajoute l'âge.
pub fn guid_texte(d1: u32, d2: u16, d3: u16, d4: &[u8]) -> String {
    let fin: String = d4.iter().map(|b| format!("{b:02X}")).collect();
    format!("{d1:08X}{d2:04X}{d3:04X}{fin}")
}

/// Le nom de PDB lu dans le PE est-il sûr pour composer un chemin local et
/// une URL ? Caractères `[A-Za-z0-9._-]` seulement, non vide, sans `..`,
/// 255 caractères au plus (le nom vient d'un fichier système, mais il est
/// joint à un dossier : aucune traversée de chemin n'est tolérée).
pub fn nom_pdb_valide(nom: &str) -> bool {
    !nom.is_empty()
        && nom.len() <= 255
        && !nom.contains("..")
        && nom
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Analyse un PE64. `None` si le fichier est invalide ou sans info CodeView.
pub fn analyser(data: &[u8]) -> Option<InfoPe> {
    if data.get(..2)? != b"MZ" {
        return None;
    }
    let nt = lire_u32(data, 0x3C)? as usize;
    if data.get(nt..nt.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let nombre_sections = lire_u16(data, nt + 6)? as usize;
    let horodatage = lire_u32(data, nt + 8)?;
    let taille_optionnel = lire_u16(data, nt + 20)? as usize;
    let optionnel = nt + 24;
    if lire_u16(data, optionnel)? != 0x20B {
        return None;
    }
    let taille_image = lire_u32(data, optionnel + 56)?;
    const REPERTOIRE_DEBOGAGE: usize = 6;
    let repertoire = optionnel + 112 + REPERTOIRE_DEBOGAGE * 8;
    let rva_debogage = lire_u32(data, repertoire)?;
    let taille_debogage = lire_u32(data, repertoire + 4)? as usize;
    let sections = optionnel.checked_add(taille_optionnel)?;
    let decalage_debogage = rva_vers_decalage(data, sections, nombre_sections, rva_debogage)?;

    for index in 0..taille_debogage / 28 {
        let entree = decalage_debogage.checked_add(index * 28)?;
        const CODEVIEW: u32 = 2;
        if lire_u32(data, entree + 12)? != CODEVIEW {
            continue;
        }
        let taille = lire_u32(data, entree + 16)? as usize;
        let pointeur = lire_u32(data, entree + 24)? as usize;
        let enregistrement = data.get(pointeur..pointeur.checked_add(taille)?)?;
        if enregistrement.get(..4)? != b"RSDS" {
            continue;
        }
        let guid = enregistrement.get(4..20)?;
        let age = lire_u32(enregistrement, 20)?;
        let chemin = enregistrement.get(24..)?;
        let fin = chemin.iter().position(|&b| b == 0).unwrap_or(chemin.len());
        let complet = String::from_utf8_lossy(&chemin[..fin]).into_owned();
        let nom_pdb = complet.rsplit(['\\', '/']).next()?.to_owned();
        let id_pdb = format!(
            "{}{:X}",
            guid_texte(
                u32::from_le_bytes(guid[0..4].try_into().ok()?),
                u16::from_le_bytes(guid[4..6].try_into().ok()?),
                u16::from_le_bytes(guid[6..8].try_into().ok()?),
                guid.get(8..16)?,
            ),
            age
        );
        return Some(InfoPe {
            horodatage,
            taille_image,
            nom_pdb,
            id_pdb,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PE64 minimal : une section contenant le répertoire de débogage.
    fn exemple() -> Vec<u8> {
        let mut data = vec![0u8; 0x600];
        data[..2].copy_from_slice(b"MZ");
        let nt = 0x80usize;
        data[0x3C..0x40].copy_from_slice(&(nt as u32).to_le_bytes());
        data[nt..nt + 4].copy_from_slice(b"PE\0\0");
        data[nt + 6..nt + 8].copy_from_slice(&1u16.to_le_bytes());
        data[nt + 8..nt + 12].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let taille_optionnel = 240usize;
        data[nt + 20..nt + 22].copy_from_slice(&(taille_optionnel as u16).to_le_bytes());
        let optionnel = nt + 24;
        data[optionnel..optionnel + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        data[optionnel + 56..optionnel + 60].copy_from_slice(&0x9000u32.to_le_bytes());
        let repertoire = optionnel + 112 + 48;
        data[repertoire..repertoire + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        data[repertoire + 4..repertoire + 8].copy_from_slice(&28u32.to_le_bytes());

        let section = optionnel + taille_optionnel;
        data[section + 8..section + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        data[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[section + 16..section + 20].copy_from_slice(&0x400u32.to_le_bytes());
        data[section + 20..section + 24].copy_from_slice(&0x200u32.to_le_bytes());

        let entree = 0x200usize;
        let enregistrement = 0x300usize;
        let nom = b"C:\\build\\Taskbar.View.pdb\0";
        data[entree + 12..entree + 16].copy_from_slice(&2u32.to_le_bytes());
        data[entree + 16..entree + 20].copy_from_slice(&((24 + nom.len()) as u32).to_le_bytes());
        data[entree + 24..entree + 28].copy_from_slice(&(enregistrement as u32).to_le_bytes());
        data[enregistrement..enregistrement + 4].copy_from_slice(b"RSDS");
        data[enregistrement + 4..enregistrement + 20].copy_from_slice(&[
            0x78, 0x56, 0x34, 0x12, 0xBC, 0x9A, 0xF0, 0xDE, 1, 2, 3, 4, 5, 6, 7, 8,
        ]);
        data[enregistrement + 20..enregistrement + 24].copy_from_slice(&10u32.to_le_bytes());
        data[enregistrement + 24..enregistrement + 24 + nom.len()].copy_from_slice(nom);
        data
    }

    #[test]
    fn signature_et_id_pdb_suivent_le_serveur_de_symboles() {
        let info = analyser(&exemple()).unwrap();
        assert_eq!(info.horodatage, 0x1234_5678);
        assert_eq!(info.taille_image, 0x9000);
        assert_eq!(info.nom_pdb, "Taskbar.View.pdb");
        assert_eq!(info.id_pdb, "123456789ABCDEF00102030405060708A");
    }

    #[test]
    fn noms_de_pdb_valides_et_dangereux() {
        assert!(nom_pdb_valide("Taskbar.View.pdb"));
        assert!(nom_pdb_valide("explorer_ext-1.pdb"));
        for nom in [
            "",
            "..",
            "a..pdb",
            "../x.pdb",
            "..\\x.pdb",
            "c:x.pdb",
            "dossier/x.pdb",
            "x y.pdb",
            "é.pdb",
            "x.pdb\0",
        ] {
            assert!(!nom_pdb_valide(nom), "{nom:?} accepté");
        }
        assert!(!nom_pdb_valide(&"a".repeat(256)));
    }

    #[test]
    fn guid_au_format_du_serveur() {
        assert_eq!(
            guid_texte(0x1234_5678, 0x9ABC, 0xDEF0, &[1, 2, 3, 4, 5, 6, 7, 8]),
            "123456789ABCDEF00102030405060708"
        );
    }

    #[test]
    fn fichiers_invalides_ou_tronques_rejetes() {
        assert_eq!(analyser(b"nope"), None);
        let data = exemple();
        for longueur in [0, 2, 0x40, 0x84, 0x100, 0x210, 0x310] {
            assert_eq!(analyser(&data[..longueur]), None, "taille {longueur}");
        }
    }
}
