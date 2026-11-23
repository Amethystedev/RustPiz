//! Journal texte : `%LOCALAPPDATA%\RustPiz\logs\runtime.log`.

use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

/// Taille au-delà de laquelle `runtime.log` est renommé en `runtime.log.old`
/// (l'ancien `.old` est remplacé) : au plus ~4 Mo sur disque.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Dossier de données de RustPiz.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("RustPiz"))
}

/// Ajoute une ligne au journal. Une erreur d'écriture est ignorée.
pub fn write(message: impl AsRef<str>) {
    let Some(directory) = data_dir().map(|d| d.join("logs")) else {
        return;
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = create_dir_all(&directory);
    let path = directory.join("runtime.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_LOG_BYTES) {
        // `rename` remplace la cible sous Windows (MOVEFILE_REPLACE_EXISTING).
        // Échec (fichier ouvert ailleurs) : on continue d'ajouter au fichier.
        let _ = std::fs::rename(&path, directory.join("runtime.log.old"));
    }
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(file, "[{seconds}] {}", message.as_ref());
}
