//! Journalisation très simple dans un fichier texte local.
//!
//! Le journal est écrit dans `%APPDATA%\RustPiz\journal.log`.
//! Chaque ligne est horodatée. La journalisation ne doit jamais faire
//! échouer l'application : toute erreur d'écriture est silencieusement ignorée.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use time::macros::format_description;
use time::OffsetDateTime;

/// Fichier journal partagé (protégé par un Mutex).
static JOURNAL: Mutex<Option<File>> = Mutex::new(None);

/// Ouvre le fichier journal en mode « ajout ». Crée le dossier parent si besoin.
pub fn initialiser(chemin: &Path) {
    if let Some(dossier) = chemin.parent() {
        let _ = fs::create_dir_all(dossier);
    }
    if let Ok(fichier) = OpenOptions::new().create(true).append(true).open(chemin) {
        if let Ok(mut garde) = JOURNAL.lock() {
            *garde = Some(fichier);
        }
    }
}

/// Horodatage local « AAAA-MM-JJ HH:MM:SS » (UTC en secours).
fn horodatage() -> String {
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&format)
        .unwrap_or_else(|_| "????-??-?? ??:??:??".to_string())
}

/// Écrit une ligne dans le journal (en ignorant toute erreur).
fn ecrire(niveau: &str, message: &str) {
    if let Ok(mut garde) = JOURNAL.lock() {
        if let Some(fichier) = garde.as_mut() {
            let _ = writeln!(fichier, "[{}] [{}] {}", horodatage(), niveau, message);
            let _ = fichier.flush();
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
