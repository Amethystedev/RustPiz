//! Journalisation très simple dans un fichier texte local.
//!
//! Le journal courant est `%APPDATA%\RustPiz\journal.log`. Chaque ligne est
//! horodatée. La journalisation ne doit jamais faire échouer l'application :
//! toute erreur d'écriture est silencieusement ignorée.
//!
//! Rotation quotidienne : à minuit (détecté à l'écriture d'une ligne) ou au
//! démarrage (`rotation_demarrage`, appelée par main seulement une fois
//! l'instance unique confirmée) si le fichier date d'un jour antérieur, `journal.log` est renommé
//! `journal-AAAAMMJJ.log` (AAAAMMJJ = jour des entrées, heure locale) ; si cette
//! archive existe déjà, le contenu y est ajouté. Rétention : les archives de plus
//! de `JOURS_CONSERVES` jours sont supprimées (démarrage et après rotation).
//! Une horloge reculée ne déclenche aucune rotation. La logique pure est dans
//! `logging_dates.rs`.

use crate::logging_dates::{
    archiver, faut_il_tourner, jour_de, jour_local_de_systeme, purger,
};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use time::macros::format_description;
use time::OffsetDateTime;

/// État du journal (protégé par un Mutex).
struct Etat {
    fichier: Option<File>,
    chemin: PathBuf,
    /// Jour AAAAMMJJ des entrées du fichier ouvert.
    jour: u32,
}

static JOURNAL: Mutex<Option<Etat>> = Mutex::new(None);

fn ouvrir(chemin: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(chemin).ok()
}

/// Tourne le journal courant (fichier déjà fermé). Les erreurs sont ignorées :
/// on continue alors d'écrire dans le journal courant.
fn tourner(chemin: &Path, jour_entrees: u32, jour_courant: u32) {
    let _ = archiver(chemin, jour_entrees);
    if let Some(dossier) = chemin.parent() {
        purger(dossier, jour_courant);
    }
}

/// Ouvre le fichier journal en mode « ajout ». Crée le dossier parent si besoin.
/// Ne fait ni rotation ni purge (voir `rotation_demarrage`) : un second
/// lancement, qui s'arrête aussitôt, ne doit pas faire pivoter le journal de la
/// première instance. Le jour mémorisé vaut `u32::MAX` : aucune rotation en
/// cours d'écriture tant que `rotation_demarrage` n'a pas été appelée.
pub fn initialiser(chemin: &Path) {
    if let Some(dossier) = chemin.parent() {
        let _ = fs::create_dir_all(dossier);
    }
    if let Some(fichier) = ouvrir(chemin) {
        if let Ok(mut garde) = JOURNAL.lock() {
            *garde = Some(Etat {
                fichier: Some(fichier),
                chemin: chemin.to_path_buf(),
                jour: u32::MAX,
            });
        }
    }
}

/// Rotation et purge de démarrage, à appeler une fois l'instance unique
/// confirmée. Aucun appel `logging::*` ici (verrou tenu).
pub fn rotation_demarrage() {
    let Ok(mut garde) = JOURNAL.lock() else {
        return;
    };
    let Some(etat) = garde.as_mut() else {
        return;
    };
    let maintenant = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let aujourdhui = jour_de(maintenant);
    // Fermer le handle (indispensable sous Windows) avant de renommer.
    etat.fichier = None;
    let mut jour = aujourdhui;
    if let Some(jour_fichier) = fs::metadata(&etat.chemin)
        .and_then(|m| m.modified())
        .ok()
        .map(jour_local_de_systeme)
    {
        if faut_il_tourner(jour_fichier, aujourdhui) {
            tourner(&etat.chemin, jour_fichier, aujourdhui);
            // Échec (le fichier existe encore) : le prochain write retentera une fois.
            if etat.chemin.exists() {
                jour = jour_fichier;
            }
        }
    }
    if let Some(dossier) = etat.chemin.parent() {
        purger(dossier, aujourdhui);
    }
    etat.fichier = ouvrir(&etat.chemin);
    etat.jour = jour;
}

/// Horodatage « AAAA-MM-JJ HH:MM:SS » d'un instant.
fn horodatage(instant: OffsetDateTime) -> String {
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    instant
        .format(&format)
        .unwrap_or_else(|_| "????-??-?? ??:??:??".to_string())
}

/// Écrit une ligne dans le journal (en ignorant toute erreur).
fn ecrire(niveau: &str, message: &str) {
    if let Ok(mut garde) = JOURNAL.lock() {
        if let Some(etat) = garde.as_mut() {
            let instant = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
            let jour = jour_de(instant);
            if faut_il_tourner(etat.jour, jour) {
                // Fermer le handle (indispensable sous Windows) avant de renommer.
                etat.fichier = None;
                let ancien = etat.jour;
                // Jour mémorisé même en cas d'échec : pas de nouvelle tentative par ligne.
                etat.jour = jour;
                tourner(&etat.chemin, ancien, jour);
                etat.fichier = ouvrir(&etat.chemin);
            }
            if let Some(fichier) = etat.fichier.as_mut() {
                let _ = writeln!(fichier, "[{}] [{}] {}", horodatage(instant), niveau, message);
                let _ = fichier.flush();
            }
        }
    }
}

/// Remplace les caractères de contrôle (`\r`, `\n`, tabulation, etc.) d'un
/// texte venu d'une autre application (titre, classe, processus) par un
/// espace, pour qu'il ne puisse ni couper ni forger une ligne du journal.
pub fn sans_controle(texte: &str) -> String {
    texte
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Journalise un message d'information.
pub fn info(message: &str) {
    ecrire("INFO", message);
}

/// Journalise un avertissement.
pub fn avertir(message: &str) {
    ecrire("AVERTISSEMENT", message);
}

/// Journalise une erreur.
pub fn erreur(message: &str) {
    ecrire("ERREUR", message);
}
