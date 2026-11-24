//! Logique de rotation/rétention du journal, sans dépendance Win32
//! (testable sur l'hôte). Les jours sont représentés par un entier AAAAMMJJ.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use time::{Date, Month, OffsetDateTime, UtcOffset};

/// Nombre de jours pendant lesquels les journaux datés sont conservés.
pub const JOURS_CONSERVES: u32 = 30;

const PREFIXE: &str = "journal-";
const SUFFIXE: &str = ".log";

/// Jour AAAAMMJJ à partir d'une date calendaire.
pub fn jour_depuis_calendrier(annee: i32, mois: u8, jour: u8) -> u32 {
    let a = u32::try_from(annee).unwrap_or(0);
    a * 10_000 + u32::from(mois) * 100 + u32::from(jour)
}

/// Jour AAAAMMJJ d'un instant (dans son propre décalage horaire).
pub fn jour_de(instant: OffsetDateTime) -> u32 {
    jour_depuis_calendrier(instant.year(), u8::from(instant.month()), instant.day())
}

/// Jour local d'une date de modification (UTC en secours si le décalage local
/// est indisponible).
pub fn jour_local_de_systeme(t: SystemTime) -> u32 {
    let utc = OffsetDateTime::from(t);
    let decalage = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    jour_de(utc.to_offset(decalage))
}

/// Date calendaire valide à partir d'un entier AAAAMMJJ.
pub fn date_depuis_jour(jour: u32) -> Option<Date> {
    let annee = i32::try_from(jour / 10_000).ok()?;
    let mois = u8::try_from((jour / 100) % 100).ok()?;
    let j = u8::try_from(jour % 100).ok()?;
    Date::from_calendar_date(annee, Month::try_from(mois).ok()?, j).ok()
}

/// Nom du journal archivé d'un jour : `journal-AAAAMMJJ.log`.
pub fn nom_journal_date(jour: u32) -> String {
    format!("{PREFIXE}{jour:08}{SUFFIXE}")
}

/// Analyse stricte d'un nom : `journal-` + 8 chiffres ASCII d'une date valide + `.log`.
pub fn analyser_nom(nom: &str) -> Option<Date> {
    let milieu = nom.strip_prefix(PREFIXE)?.strip_suffix(SUFFIXE)?;
    if milieu.len() != 8 || !milieu.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    date_depuis_jour(milieu.parse::<u32>().ok()?)
}

/// Faut-il tourner ? Seulement si le fichier date d'un jour strictement antérieur
/// (une horloge reculée ne déclenche rien).
pub fn faut_il_tourner(jour_fichier: u32, jour_courant: u32) -> bool {
    jour_fichier < jour_courant
}

/// Faut-il supprimer ce fichier ? Seulement un journal daté plus ancien que la rétention.
pub fn faut_il_purger(nom: &str, jour_courant: u32) -> bool {
    let (Some(date), Some(courante)) = (analyser_nom(nom), date_depuis_jour(jour_courant)) else {
        return false;
    };
    (courante - date).whole_days() > i64::from(JOURS_CONSERVES)
}

/// Archive le journal courant sous `journal-<jour_entrees>.log` (même dossier).
/// Si l'archive existe, le contenu est ajouté à sa suite ; le courant n'est
/// supprimé que si la copie a réussi ; si sa suppression échoue, il est tronqué
/// (erreur de troncature ignorée) pour ne pas être dupliqué à la rotation suivante.
pub fn archiver(courant: &Path, jour_entrees: u32) -> io::Result<PathBuf> {
    let dossier = courant
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "pas de dossier parent"))?;
    let archive = dossier.join(nom_journal_date(jour_entrees));
    if archive.exists() {
        {
            let mut source = fs::File::open(courant)?;
            let mut dest = OpenOptions::new().create(true).append(true).open(&archive)?;
            io::copy(&mut source, &mut dest)?;
            dest.sync_all()?;
        }
        if fs::remove_file(courant).is_err() {
            let _ = OpenOptions::new().write(true).truncate(true).open(courant);
        }
    } else {
        fs::rename(courant, &archive)?;
    }
    Ok(archive)
}

/// Supprime les journaux datés expirés du dossier (erreurs ignorées).
pub fn purger(dossier: &Path, jour_courant: u32) {
    let Ok(entrees) = fs::read_dir(dossier) else {
        return;
    };
    for entree in entrees.flatten() {
        let est_fichier = entree.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !est_fichier {
            continue;
        }
        if let Some(nom) = entree.file_name().to_str() {
            if faut_il_purger(nom, jour_courant) {
                let _ = fs::remove_file(entree.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jours_et_noms() {
        assert_eq!(jour_depuis_calendrier(2026, 11, 24), 20261124);
        assert_eq!(nom_journal_date(20261124), "journal-20261124.log");
        assert!(date_depuis_jour(20260230).is_none());
        assert!(date_depuis_jour(20261301).is_none());
    }

    #[test]
    fn analyse_stricte() {
        assert!(analyser_nom("journal-20261124.log").is_some());
        for mauvais in [
            "journal.log",
            "journal-2026112.log",
            "journal-202611245.log",
            "journal-2026112a.log",
            "journal-20260230.log",
            "journal-20261124.log.bak",
            "xjournal-20261124.log",
            "journal-+0261124.log",
            "journal-２０２６１１２４.log",
            "JOURNAL-20261124.log",
            "",
        ] {
            assert!(analyser_nom(mauvais).is_none(), "{mauvais}");
        }
    }

    #[test]
    fn decision_rotation() {
        assert!(faut_il_tourner(20261123, 20261124));
        assert!(!faut_il_tourner(20261124, 20261124));
        assert!(!faut_il_tourner(20261125, 20261124));
        assert!(faut_il_tourner(20251231, 20260101));
    }

    #[test]
    fn decision_purge() {
        let c = 20261124;
        // 30 jours exactement : conservé ; 31 : supprimé.
        assert!(!faut_il_purger("journal-20261025.log", c));
        assert!(faut_il_purger("journal-20261024.log", c));
        assert!(faut_il_purger("journal-20250101.log", c));
        assert!(!faut_il_purger("journal-20261124.log", c));
        assert!(!faut_il_purger("journal-20271124.log", c));
        assert!(!faut_il_purger("journal.log", c));
        assert!(!faut_il_purger("config.toml", c));
        assert!(!faut_il_purger("journal-20260230.log", c));
        assert!(!faut_il_purger("journal-20250101.log", 0));
    }

    #[test]
    fn jour_de_instant() {
        let d = Date::from_calendar_date(2026, Month::March, 5).unwrap();
        let t = d.with_hms(23, 59, 59).unwrap().assume_utc();
        assert_eq!(jour_de(t), 20260305);
    }

    fn dossier_test(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustpiz-log-{nom}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn archivage_simple_et_fusion() {
        let d = dossier_test("arch");
        let courant = d.join("journal.log");
        fs::write(&courant, "a\n").unwrap();
        let a = archiver(&courant, 20261123).unwrap();
        assert!(!courant.exists());
        assert_eq!(fs::read_to_string(&a).unwrap(), "a\n");
        // Seconde rotation du même jour : fusion à la suite, rien d'écrasé.
        fs::write(&courant, "b\n").unwrap();
        archiver(&courant, 20261123).unwrap();
        assert!(!courant.exists());
        assert_eq!(fs::read_to_string(&a).unwrap(), "a\nb\n");
        // Courant absent : erreur, archive intacte.
        assert!(archiver(&courant, 20261123).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "a\nb\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn purge_ciblee() {
        let d = dossier_test("purge");
        for n in [
            "journal.log",
            "journal-20200101.log",
            "journal-20261120.log",
            "journal-20200230.log",
            "autre.log",
            "journal-20200101.log.bak",
        ] {
            fs::write(d.join(n), "x").unwrap();
        }
        fs::create_dir(d.join("journal-20190101.log")).unwrap();
        purger(&d, 20261124);
        assert!(d.join("journal.log").exists());
        assert!(!d.join("journal-20200101.log").exists());
        assert!(d.join("journal-20261120.log").exists());
        assert!(d.join("journal-20200230.log").exists());
        assert!(d.join("autre.log").exists());
        assert!(d.join("journal-20200101.log.bak").exists());
        assert!(d.join("journal-20190101.log").is_dir());
        let _ = fs::remove_dir_all(&d);
    }
}
