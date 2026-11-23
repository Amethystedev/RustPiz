//! Client du tube de contrôle de la DLL (`\\.\pipe\rustpiz.<pid>`).
//!
//! Porté de RustHawk (`windows_ops.rs`, partie échange). Chaque échange a
//! lieu dans un fil éphémère borné par un délai : l'appelant n'est jamais
//! bloqué indéfiniment par un explorer figé.
//!
//! Un seul échange à la fois (jeton atomique) : si le précédent est encore
//! bloqué (explorer figé), le suivant échoue immédiatement au lieu
//! d'accumuler des fils. Seule la restauration d'arrêt
//! ([`restaurer_best_effort`]) attend le jeton, dans son propre délai.
//! Limite : un échange dont le serveur ne répond jamais garde le jeton
//! jusqu'à la fin du processus (lecture bloquante sur le tube) ; le tube est
//! alors de toute façon inutilisable.
//!
//! Les échanges du fil dédié sont interrompus (attente abandonnée) dès que
//! l'arrêt est demandé (`orchestration::arret_demande`).

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use rustpiz_protocol::{pipe_name, Request, Response, StatusReport, ThumbnailHook, PROTOCOL_VERSION};

use super::orchestration::{arret_demande, dormir};

/// Vrai pendant un échange (fil éphémère vivant).
static ECHANGE_EN_COURS: AtomicBool = AtomicBool::new(false);

/// Tranche d'attente de la réponse (réactivité à l'arrêt).
const TRANCHE: Duration = Duration::from_millis(100);

/// Résultat d'un échange sur le tube.
pub enum Echange {
    Reponse(Response),
    AucuneSession,
}

/// Jeton d'échange : libéré à la fin du fil éphémère (ou si sa création
/// échoue, la fermeture étant alors détruite avec lui).
struct Jeton;

impl Drop for Jeton {
    fn drop(&mut self) {
        ECHANGE_EN_COURS.store(false, Ordering::Release);
    }
}

fn prendre_jeton(attente: Duration) -> Option<Jeton> {
    let fin = Instant::now() + attente;
    loop {
        if ECHANGE_EN_COURS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(Jeton);
        }
        let maintenant = Instant::now();
        if maintenant >= fin {
            return None;
        }
        std::thread::sleep((fin - maintenant).min(Duration::from_millis(25)));
    }
}

/// Échange complet (exécuté dans le fil éphémère).
fn echange_brut(nom: &str, requete: &Request) -> Result<Echange, String> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let mut essais = 0;
    let mut absents = 0;
    let flux = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(nom) {
            Ok(flux) => break flux,
            // Le serveur recrée le tube entre deux requêtes : une
            // absence brève ne prouve pas que la session est finie.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if absents >= 4 {
                    return Ok(Echange::AucuneSession);
                }
                absents += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && essais < 50 => {
                essais += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("tube de contrôle : {e}")),
        }
    };
    let mut message = serde_json::to_string(requete).map_err(|e| e.to_string())?;
    message.push('\n');
    (&flux).write_all(message.as_bytes()).map_err(|e| e.to_string())?;
    let mut ligne = String::new();
    BufReader::new(&flux)
        .read_line(&mut ligne)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(ligne.trim())
        .map(Echange::Reponse)
        .map_err(|e| format!("réponse invalide : {e}"))
}

/// Envoie une requête et attend la réponse, borné par `delai` (attente du
/// jeton comprise). `attente_jeton` : temps accordé si un échange précédent
/// est encore en cours (zéro = échec immédiat). `interruptible` : l'attente
/// est abandonnée dès que l'arrêt est demandé.
fn echanger_avec(
    pid: u32,
    requete: Request,
    delai: Duration,
    attente_jeton: Duration,
    interruptible: bool,
) -> Result<Echange, String> {
    let debut = Instant::now();
    let jeton = prendre_jeton(attente_jeton.min(delai))
        .ok_or_else(|| "échange précédent encore en cours (runtime bloqué ?)".to_owned())?;
    let nom = pipe_name(pid);
    let (envoi, reception) = mpsc::channel();
    std::thread::Builder::new()
        .name("rustpiz-tube".to_owned())
        .spawn(move || {
            let _jeton = jeton;
            let _ = envoi.send(echange_brut(&nom, &requete));
        })
        .map_err(|e| format!("fil d'échange non créé : {e}"))?;
    loop {
        let restant = delai.saturating_sub(debut.elapsed());
        if restant.is_zero() {
            return Err("le runtime ne répond pas".to_owned());
        }
        match reception.recv_timeout(restant.min(TRANCHE)) {
            Ok(issue) => return issue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("fil d'échange interrompu".to_owned())
            }
            Err(RecvTimeoutError::Timeout) => {
                if interruptible && arret_demande() {
                    return Err("arrêt demandé".to_owned());
                }
            }
        }
    }
}

/// Échange du fil dédié : échec immédiat si un échange est déjà en cours,
/// interrompu par l'arrêt.
pub fn echanger(pid: u32, requete: Request, delai: Duration) -> Result<Echange, String> {
    echanger_avec(pid, requete, delai, Duration::ZERO, true)
}

/// Le runtime répond-il (après `Hello`) ?
pub fn session_vivante(pid: u32) -> bool {
    matches!(
        echanger(pid, Request::Hello, Duration::from_secs(3)),
        Ok(Echange::Reponse(Response::Hello { .. }))
    )
}

/// Sonde de surveillance : `Some(true)` si le runtime répond, `Some(false)`
/// si le tube n'existe plus (session finie), `None` si l'état est inconnu
/// (pas de réponse, échange précédent encore bloqué, erreur) : on ne
/// réinjecte pas dans un explorer figé.
pub fn sonder_session(pid: u32) -> Option<bool> {
    match echanger(pid, Request::Hello, Duration::from_secs(3)) {
        Ok(Echange::Reponse(Response::Hello { .. })) => Some(true),
        Ok(Echange::AucuneSession) => Some(false),
        _ => None,
    }
}

/// Attend que le runtime réponde après son chargement (interrompu par
/// l'arrêt).
pub fn attendre_runtime(pid: u32) -> Result<(), String> {
    for _ in 0..40 {
        if arret_demande() {
            return Err("arrêt demandé".to_owned());
        }
        if let Ok(Echange::Reponse(Response::Hello { protocol, .. })) =
            echanger(pid, Request::Hello, Duration::from_secs(2))
        {
            return if protocol == PROTOCOL_VERSION {
                Ok(())
            } else {
                Err("version du runtime différente : redémarrez explorer".to_owned())
            };
        }
        if !dormir(Duration::from_millis(250)) {
            return Err("arrêt demandé".to_owned());
        }
    }
    Err("le runtime n'a pas démarré (voir runtime.log)".to_owned())
}

/// Arrête la session en cours. `Ok(None)` s'il n'y en a pas.
pub fn arreter_session(pid: u32) -> Result<Option<StatusReport>, String> {
    match echanger(pid, Request::Disable, Duration::from_secs(30))? {
        Echange::AucuneSession => Ok(None),
        Echange::Reponse(Response::Disabled(rapport)) => Ok(Some(rapport)),
        Echange::Reponse(autre) => Err(format!("réponse inattendue : {autre:?}")),
    }
}

/// Demande un rapport d'état.
pub fn statut(pid: u32) -> Result<Option<StatusReport>, String> {
    match echanger(pid, Request::Status, Duration::from_secs(30))? {
        Echange::AucuneSession => Ok(None),
        Echange::Reponse(Response::Status(rapport)) => Ok(Some(rapport)),
        Echange::Reponse(autre) => Err(format!("réponse inattendue : {autre:?}")),
    }
}

/// Met à jour les miniatures sans relancer la session (`None` retire les hooks).
pub fn maj_miniatures(pid: u32, hook: Option<ThumbnailHook>) -> Result<StatusReport, String> {
    match echanger(pid, Request::SetThumbnails(hook), Duration::from_secs(30))? {
        Echange::Reponse(Response::Status(rapport)) => Ok(rapport),
        Echange::Reponse(Response::Error(message)) => Err(message),
        _ => Err("réponse inattendue du runtime".to_owned()),
    }
}

/// Restauration best effort : arrête la session si le runtime répond, borné
/// par `delai` (attente d'un échange en cours comprise). Utilisée aux points
/// d'arrêt (Quitter, fin de session, panique du fil principal) depuis le fil
/// appelant ; non interrompue par le drapeau d'arrêt (elle en fait partie).
pub fn restaurer_best_effort(delai: Duration) {
    let Some(pid) = super::injection::explorer_pid() else {
        return;
    };
    let _ = echanger_avec(pid, Request::Disable, delai, delai, false);
}
