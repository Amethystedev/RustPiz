//! Orchestration de l'intégration de la barre des tâches (v0.6.0).
//!
//! Relie la configuration RustPiz à la DLL injectée. Toute action lourde
//! (injection, tube, téléchargement de PDB) a lieu dans un **fil dédié** :
//! la boucle de messages n'est jamais bloquée. Le fil n'existe que lorsque
//! au moins une fonction est active (coût nul sinon).
//!
//! Cycle de vie :
//! - [`appliquer`] : appelée au démarrage et à chaque rechargement de config ;
//!   ne fait rien si l'état voulu est inchangé.
//! - [`arreter`] : appelée à Quitter, en fin de session (`WM_ENDSESSION`
//!   confirmée) et dans le hook de panique (fil principal seulement). Pose
//!   d'abord [`ARRET_DEMANDE`] (le fil n'entame plus aucune étape, refuse
//!   d'injecter et abandonne ses attentes), attend (borné) que le fil soit
//!   inactif, puis demande à la DLL la restauration (best effort, bornée).
//!
//! Échecs : mémorisés par couple (pid d'explorer, souhait) dans
//! [`politique::Reessais`] : échec passager = réessais à 30 s, 2 min, 10 min
//! puis arrêt ; échec durable (conflit XAML, build trop ancien, SAC/WDAC,
//! thème invalide, disjoncteur) = aucun réessai tant que le souhait et le pid
//! ne changent pas. La surveillance (3 s) ne réapplique que si explorer a
//! changé de pid, si la session est morte après une application réussie, ou
//! si un réessai est échu.
//!
//! Après désactivation, la DLL reste chargée mais **inerte** dans explorer
//! jusqu'au redémarrage de celui-ci (limite documentée de `InitializeXaml
//! DiagnosticsEx`).
//!
//! Limite « session résiduelle après plantage » : si RustPiz a été tué (ou
//! a planté hors du fil principal) pendant une session, la barre reste
//! personnalisée. Au démarrage suivant avec les fonctions désactivées, rien
//! n'est fait (coût nul : ni fil, ni sonde du tube) ; la session résiduelle
//! est restaurée par « Quitter » (sonde bornée à 200 ms), par une activation
//! puis désactivation, ou par le redémarrage de l'Explorateur.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use rustpiz_theme::CompiledTheme;

use super::bilan::{self, Attendu};
use super::decision::{self, Action, Souhait};
use super::injection::{self, BUILD_MIN_BARRE, BUILD_MIN_MINIATURES};
use super::politique::{self, Motif, Nature, Reessais, Surveillance};
use super::tube;
use crate::config::Config;
use crate::logging;
use crate::themes_barre;

/// Intervalle de surveillance du redémarrage d'explorer (tant qu'actif).
const INTERVALLE_SURVEILLANCE: Duration = Duration::from_secs(3);
/// Délai laissé aux règles du thème avant de lire le disjoncteur : l'arbre
/// existant est rejoué de façon asynchrone après `AdviseVisualTreeChange`
/// (même attente que RustHawk, `start_session`).
const DELAI_AVANT_STATUT: Duration = Duration::from_secs(2);
/// Seconde lecture du rapport après une application (arbre rejoué en
/// entier, éléments tardifs) : faite par la surveillance au premier tour
/// qui suit ce délai, sans bloquer le fil dédié.
const DELAI_RELECTURE: Duration = Duration::from_secs(5);
/// Attente maximale de la barre XAML (module chargé) avant la première
/// injection dans un explorer (RustHawk : `wait_for_taskbar`, 45 s).
const DELAI_MAX_BARRE: Duration = Duration::from_secs(45);
/// Délai laissé à la construction de l'arbre XAML une fois le module chargé
/// (RustHawk : 2 s après la détection du module).
const DELAI_ARBRE_BARRE: Duration = Duration::from_secs(2);
/// Lectures supplémentaires du rapport (1 s d'intervalle) quand les
/// tentatives d'`AdviseVisualTreeChange` sont encore en cours après 2 s : le
/// verdict (réussite ou échec) est ainsi connu avant de compter la session.
const LECTURES_ADVISE_EN_COURS: u32 = 3;

/// Contexte d'un arrêt (budgets de temps différents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeArret {
    /// « Quitter » / fin de la boucle de messages : attente du fil 3 s,
    /// restauration 5 s ; sonde bornée (200 ms) même si rien n'a démarré.
    Quitter,
    /// `WM_ENDSESSION` confirmée : attente 2 s, restauration 3 s.
    FinSession,
    /// Panique du fil principal : aucun verrou bloquant, 500 ms au total ;
    /// l'expéditeur et le dernier souhait ne sont pas vidés.
    Panique,
}

enum Tache {
    Appliquer(Souhait),
    Arreter,
}

/// Arrêt demandé : posé EN PREMIER par [`arreter`] ; testé par le fil
/// dédié entre chaque étape et par les attentes du tube/téléchargement.
static ARRET_DEMANDE: AtomicBool = AtomicBool::new(false);
/// Vrai dès qu'un fil dédié a été créé dans ce processus : sinon [`arreter`]
/// ne sonde ni le tube ni `FindWindow` (coût nul), sauf « Quitter ».
static DEMARREE: AtomicBool = AtomicBool::new(false);
/// Le fil dédié traite une tâche (protégé pour l'attente bornée d'arrêt).
static OCCUPE: Mutex<bool> = Mutex::new(false);
static FIL_INACTIF: Condvar = Condvar::new();

static EXPEDITEUR: Mutex<Option<Sender<Tache>>> = Mutex::new(None);
static DERNIER_DEMANDE: Mutex<Option<Souhait>> = Mutex::new(None);
static DERNIERE_NOTIF: Mutex<Option<String>> = Mutex::new(None);

/// L'arrêt de l'intégration est-il demandé ?
pub fn arret_demande() -> bool {
    ARRET_DEMANDE.load(Ordering::SeqCst)
}

/// Dort `duree` par tranches de 250 ms ; renvoie faux (au plus tôt) si
/// l'arrêt est demandé.
pub fn dormir(duree: Duration) -> bool {
    let fin = Instant::now() + duree;
    loop {
        if arret_demande() {
            return false;
        }
        let maintenant = Instant::now();
        if maintenant >= fin {
            return true;
        }
        std::thread::sleep((fin - maintenant).min(Duration::from_millis(250)));
    }
}

/// État voulu dérivé de la configuration.
pub fn souhait_depuis(config: &Config) -> Souhait {
    Souhait {
        theme: config
            .barre_active
            .then(|| themes_barre::normaliser(&config.barre_theme).to_owned()),
        miniatures: config.miniatures_actives.then_some(config.miniatures_taille),
    }
}

/// Oublie le dernier message notifié (souhait changé ou succès) : un même
/// échec pourra de nouveau être notifié.
fn oublier_notification() {
    *DERNIERE_NOTIF.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Journalise toujours, notifie une seule fois par message distinct.
fn signaler_une_fois(message: &str) {
    logging::avertir(&format!("Barre : {message}"));
    let mut garde = DERNIERE_NOTIF.lock().unwrap_or_else(|e| e.into_inner());
    if garde.as_deref() != Some(message) {
        *garde = Some(message.to_owned());
        crate::tray::notifier_arriere_plan(message);
    }
}

/// Journalise le bilan d'un rapport d'état (journal.log) et le renvoie.
/// `quand` : moment de la lecture (« après 2 s », « relecture après 7 s »…).
fn journaliser_bilan(
    rapport: &rustpiz_protocol::StatusReport,
    attendu: &Attendu,
    quand: &str,
) -> bilan::Bilan {
    let bilan = bilan::evaluer(rapport, attendu);
    logging::info(&format!("Barre : rapport d'état ({quand}) : {}.", bilan.resume));
    for detail in &bilan.details {
        logging::info(&format!("Barre : {detail}."));
    }
    for avertissement in &bilan.avertissements {
        logging::avertir(&format!("Barre : {avertissement}."));
    }
    if bilan.theme_sans_effet || bilan.regles_en_echec {
        logging::avertir(
            "Barre : détails côté explorer dans %LOCALAPPDATA%\\RustPiz\\logs\\runtime.log \
             (lignes « session N », « échec … », « aucun élément pour »).",
        );
    }
    bilan
}

/// Applique l'état voulu de la configuration. Non bloquant.
pub fn appliquer(config: &Config) {
    if arret_demande() {
        return;
    }
    let souhait = souhait_depuis(config);
    let mut demande = DERNIER_DEMANDE.lock().unwrap_or_else(|e| e.into_inner());
    let inchange = demande.as_ref() == Some(&souhait);
    let worker_present = EXPEDITEUR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    if !inchange {
        oublier_notification();
    }

    // Désactivé et aucun fil : on ne démarre rien (coût nul).
    if souhait.est_inactif() && !worker_present {
        *demande = Some(souhait);
        return;
    }
    if inchange && worker_present {
        return;
    }
    *demande = Some(souhait.clone());
    drop(demande);
    envoyer(Tache::Appliquer(souhait));
}

/// Attend (borné) que le fil dédié soit inactif. Faux si le délai expire.
fn attendre_inactivite(delai: Duration) -> bool {
    let garde = OCCUPE.lock().unwrap_or_else(|e| e.into_inner());
    let (garde, _) = FIL_INACTIF
        .wait_timeout_while(garde, delai, |occupe| *occupe)
        .unwrap_or_else(|e| e.into_inner());
    !*garde
}

/// Variante sans verrou bloquant (hook de panique) : `try_lock` répété.
fn attendre_inactivite_sans_bloquer(delai: Duration) -> bool {
    let fin = Instant::now() + delai;
    loop {
        if let Ok(occupe) = OCCUPE.try_lock() {
            if !*occupe {
                return true;
            }
        }
        if Instant::now() >= fin {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Arrête l'intégration : drapeau d'arrêt, attente bornée du fil, puis
/// restauration best effort par la DLL.
pub fn arreter(mode: ModeArret) {
    // En premier : plus aucune étape (injection comprise) ne démarre.
    // Un second appel (fin de session puis fin de la boucle de messages)
    // ne refait rien.
    if ARRET_DEMANDE.swap(true, Ordering::SeqCst) {
        return;
    }
    let demarree = DEMARREE.load(Ordering::SeqCst);

    if mode == ModeArret::Panique {
        // Aucun verrou bloquant, rien n'est vidé ; 500 ms au total.
        if demarree {
            let inactif = attendre_inactivite_sans_bloquer(Duration::from_millis(250));
            if !inactif {
                logging::avertir("Barre (panique) : fil dédié encore occupé, restauration tentée.");
            }
            tube::restaurer_best_effort(Duration::from_millis(250));
        }
        return;
    }

    if let Some(expediteur) = EXPEDITEUR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = expediteur.send(Tache::Arreter);
    }
    *DERNIER_DEMANDE.lock().unwrap_or_else(|e| e.into_inner()) = None;

    let (attente, restauration) = match mode {
        ModeArret::FinSession => (Duration::from_secs(2), Duration::from_secs(3)),
        _ => (Duration::from_secs(3), Duration::from_secs(5)),
    };
    if demarree {
        if !attendre_inactivite(attente) {
            logging::avertir(&format!(
                "Barre : fil dédié encore occupé après {} s, restauration tentée quand même.",
                attente.as_secs()
            ));
        }
        tube::restaurer_best_effort(restauration);
    } else if mode == ModeArret::Quitter {
        // Rien n'a démarré dans ce processus : sonde courte d'une éventuelle
        // session résiduelle (plantage précédent).
        tube::restaurer_best_effort(Duration::from_millis(200));
    }
}

fn envoyer(tache: Tache) {
    let mut garde = EXPEDITEUR.lock().unwrap_or_else(|e| e.into_inner());
    if garde.is_none() {
        let (expediteur, reception) = channel();
        match std::thread::Builder::new()
            .name("rustpiz-barre".to_owned())
            .spawn(move || boucle_worker(reception))
        {
            Ok(_) => {
                DEMARREE.store(true, Ordering::SeqCst);
                *garde = Some(expediteur);
            }
            Err(e) => {
                // Pas de fil : on n'enregistre aucun expéditeur (un prochain
                // `appliquer` réessaiera de le créer).
                logging::erreur(&format!(
                    "Barre : création du fil dédié impossible ({e}) ; intégration non appliquée."
                ));
                drop(garde);
                *DERNIER_DEMANDE.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return;
            }
        }
    }
    if let Some(expediteur) = garde.as_ref() {
        let _ = expediteur.send(tache);
    }
}

/// Marque le fil occupé/inactif (et réveille [`arreter`]).
fn marquer_occupe(valeur: bool) {
    *OCCUPE.lock().unwrap_or_else(|e| e.into_inner()) = valeur;
    FIL_INACTIF.notify_all();
}

/// Remet « inactif » à la sortie du fil, même par panique.
struct FinDeFil;

impl Drop for FinDeFil {
    fn drop(&mut self) {
        marquer_occupe(false);
    }
}

fn boucle_worker(reception: Receiver<Tache>) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let _fin = FinDeFil;
    let mut orch = Orchestrateur::new();
    loop {
        let surveiller = orch.surveillance_requise();
        let recu = if surveiller {
            reception.recv_timeout(INTERVALLE_SURVEILLANCE)
        } else {
            // Rien à surveiller : on bloque, aucun coût CPU.
            reception.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        // Occupé AVANT de relire le drapeau : `arreter` pose le drapeau puis
        // lit `OCCUPE` ; l'un des deux voit forcément l'autre.
        marquer_occupe(true);
        if arret_demande() {
            return;
        }
        match recu {
            Ok(Tache::Appliquer(mut souhait)) => {
                // Clics rapprochés : seul le dernier état compte.
                while let Ok(tache) = reception.try_recv() {
                    match tache {
                        Tache::Appliquer(suivant) => souhait = suivant,
                        Tache::Arreter => return,
                    }
                }
                let _ = catch_unwind(AssertUnwindSafe(|| orch.synchroniser(&souhait)));
            }
            // La restauration est faite par `arreter` (fil appelant).
            Ok(Tache::Arreter) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let _ = catch_unwind(AssertUnwindSafe(|| orch.verifier_explorer()));
            }
        }
        marquer_occupe(false);
    }
}

/// Échec d'une tentative d'application.
enum Echec {
    /// Arrêt demandé : rien n'est mémorisé ni notifié.
    Interrompu,
    Panne { nature: Nature, message: String },
}

impl Echec {
    fn passager(message: impl Into<String>) -> Self {
        Echec::Panne {
            nature: Nature::Passager,
            message: message.into(),
        }
    }

    fn durable(message: impl Into<String>) -> Self {
        Echec::Panne {
            nature: Nature::Durable,
            message: message.into(),
        }
    }
}

/// `Err(Interrompu)` si l'arrêt est demandé.
fn verifier_arret() -> Result<(), Echec> {
    if arret_demande() {
        Err(Echec::Interrompu)
    } else {
        Ok(())
    }
}

/// État de la session gérée par RustPiz dans l'explorer courant.
struct Orchestrateur {
    /// Ce que l'on croit avoir appliqué (avec succès) dans l'explorer `pid`.
    applique: Option<Souhait>,
    /// Pid de l'explorer de la dernière tentative.
    pid: Option<u32>,
    /// Dernier état demandé (pour réappliquer si explorer redémarre).
    dernier_souhait: Option<Souhait>,
    /// Échecs mémorisés par (pid, souhait).
    reessais: Reessais,
    /// Origine de l'horloge monotone de la politique.
    origine: Instant,
    /// Tours de surveillance avec session vérifiée (disjoncteur périodique).
    tours: u32,
    /// Seconde lecture du rapport prévue (échéance, ce qui a été demandé).
    relecture: Option<(Instant, Attendu)>,
    /// Explorer pour lequel la barre a déjà été attendue (une fois par pid).
    barre_attendue: Option<u32>,
    /// Suspect fort relevé au dernier examen d'explorer (DLL runtime de
    /// RustHawk chargée) : cité si l'abonnement à l'arbre visuel échoue.
    suspect_fort: Option<String>,
}

impl Orchestrateur {
    fn new() -> Self {
        Orchestrateur {
            applique: None,
            pid: None,
            dernier_souhait: None,
            reessais: Reessais::default(),
            origine: Instant::now(),
            tours: 0,
            relecture: None,
            barre_attendue: None,
            suspect_fort: None,
        }
    }

    /// Message utilisateur d'un échec d'`AdviseVisualTreeChange`, complété du
    /// suspect fort éventuel.
    fn message_advise(&self, message: &str) -> String {
        match &self.suspect_fort {
            Some(suspect) => format!("thème non appliqué : {message} ; {suspect}"),
            None => format!("thème non appliqué : {message}"),
        }
    }

    fn maintenant_ms(&self) -> u64 {
        u64::try_from(self.origine.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn surveillance_requise(&self) -> bool {
        self.applique.is_some()
            || self
                .dernier_souhait
                .as_ref()
                .is_some_and(|s| !s.est_inactif())
    }

    /// Tente d'atteindre `voulu`, puis mémorise l'issue (réessais, notif).
    fn synchroniser(&mut self, voulu: &Souhait) {
        self.dernier_souhait = Some(voulu.clone());
        match self.tenter(voulu) {
            Ok(()) => {}
            Err(Echec::Interrompu) => {
                logging::info("Barre : arrêt demandé, application interrompue.");
            }
            Err(Echec::Panne { nature, message }) => {
                signaler_une_fois(&message);
                if let Some(pid) = self.pid {
                    let suite =
                        self.reessais
                            .noter_echec(pid, voulu, nature, self.maintenant_ms());
                    logging::info(&format!("Barre : {}", suite.decrire()));
                }
            }
        }
    }

    /// Application réussie dans `pid`.
    fn noter_succes(&mut self, pid: u32, voulu: &Souhait) {
        self.applique = Some(voulu.clone());
        self.reessais.noter_succes(pid, voulu);
        oublier_notification();
    }

    fn tenter(&mut self, voulu: &Souhait) -> Result<(), Echec> {
        verifier_arret()?;
        let Some(pid) = injection::explorer_pid() else {
            if !voulu.est_inactif() {
                logging::avertir("Barre : explorer.exe introuvable, application différée.");
            }
            self.pid = None;
            self.applique = None;
            return Ok(());
        };

        // Explorer a changé : l'état appliqué précédent n'existe plus.
        if self.pid != Some(pid) {
            self.pid = Some(pid);
            self.applique = None;
            self.tours = 0;
        }

        let examen = (!voulu.est_inactif()).then(|| injection::examiner_explorer(pid));
        if let Some(examen) = &examen {
            logging::info(&format!("Barre : {}.", examen.journal));
            self.suspect_fort = examen.suspect_fort.clone();
        }

        let (build, _) = injection::build_windows().unwrap_or((0, 0));
        verifier_arret()?;
        let vivante = tube::session_vivante(pid);
        verifier_arret()?;
        let action = decision::planifier(self.applique.as_ref(), vivante, voulu);
        // Un conflit ne bloque qu'une NOUVELLE injection : une session RustPiz
        // déjà vivante (abonnement acquis) reste pilotable (miniatures).
        if let Some(conflit) = examen.and_then(|e| e.conflit) {
            if decision::conflit_bloque(action) {
                return Err(Echec::durable(format!(
                    "fonction indisponible : {conflit} utilise déjà les diagnostics XAML (un seul \
                     consommateur par processus). Quittez-le puis redémarrez l'Explorateur."
                )));
            }
            logging::avertir(&format!(
                "Barre : {conflit} détecté dans explorer, mais la session RustPiz est déjà vivante : \
                 action {action:?} poursuivie (aucune nouvelle injection)."
            ));
        }
        match action {
            Action::Rien => Ok(()),
            Action::Arreter => {
                self.relecture = None;
                match tube::arreter_session(pid) {
                    Ok(Some(rapport)) if rapport.unresponsive_threads > 0 => logging::avertir(&format!(
                        "Barre : {} fil(s) XAML n'ont pas répondu à la restauration ; si la barre \
                         reste modifiée, redémarrez l'Explorateur.",
                        rapport.unresponsive_threads
                    )),
                    Ok(_) => {}
                    Err(message) => logging::avertir(&format!(
                        "Barre : arrêt de la session sans réponse ({message})."
                    )),
                }
                self.applique = None;
                self.reessais.noter_succes(pid, voulu);
                oublier_notification();
                logging::info("Barre : session arrêtée, état d'origine restauré.");
                Ok(())
            }
            Action::MajMiniatures => self.maj_miniatures(pid, build, voulu),
            Action::Redemarrer => self.redemarrer(pid, build, voulu),
        }
    }

    /// Compile le thème voulu, en respectant le build minimal.
    /// `Ok(None)` : aucun thème demandé.
    fn theme_voulu(&self, build: u32, voulu: &Souhait) -> Result<Option<CompiledTheme>, Echec> {
        let Some(id) = &voulu.theme else {
            return Ok(None);
        };
        if build != 0 && build < BUILD_MIN_BARRE {
            return Err(Echec::durable(format!(
                "thème ignoré : Windows 11 (build {BUILD_MIN_BARRE}+) requis (build {build})."
            )));
        }
        match decision::compiler_theme_pour_build(id, build) {
            Ok((theme, avertissements)) => {
                for avertissement in avertissements {
                    logging::avertir(&format!("Barre : thème « {id} » : {avertissement}"));
                }
                Ok(Some(theme))
            }
            Err(message) => Err(Echec::durable(format!("thème « {id} » invalide : {message}"))),
        }
    }

    /// Résout le hook de miniatures voulu, en respectant le build minimal.
    /// `Ok(None)` : aucune miniature demandée.
    fn hook_voulu(
        &self,
        pid: u32,
        build: u32,
        voulu: &Souhait,
    ) -> Result<Option<rustpiz_protocol::ThumbnailHook>, Echec> {
        let Some(taille) = voulu.miniatures else {
            return Ok(None);
        };
        if build != 0 && build < BUILD_MIN_MINIATURES {
            return Err(Echec::durable(format!(
                "miniatures ignorées : build {BUILD_MIN_MINIATURES}+ requis (build {build})."
            )));
        }
        match injection::resoudre_hook_miniatures(pid, decision::config_miniatures(taille)) {
            Ok(hook) => Ok(Some(hook)),
            Err(_) if arret_demande() => Err(Echec::Interrompu),
            Err(message) => Err(Echec::passager(format!("miniatures indisponibles : {message}"))),
        }
    }

    fn maj_miniatures(&mut self, pid: u32, build: u32, voulu: &Souhait) -> Result<(), Echec> {
        // Échec de résolution : les hooks sont retirés (comportement
        // d'origine), l'échec est signalé mais le thème reste appliqué.
        let hook = match self.hook_voulu(pid, build, voulu) {
            Ok(hook) => hook,
            Err(Echec::Interrompu) => return Err(Echec::Interrompu),
            Err(Echec::Panne { message, .. }) => {
                signaler_une_fois(&message);
                None
            }
        };
        verifier_arret()?;
        let demandees = hook.is_some();
        match tube::maj_miniatures(pid, hook) {
            Ok(rapport) => {
                self.noter_succes(pid, voulu);
                let etat = &rapport.thumbnails;
                match (demandees, etat.active) {
                    (true, true) => logging::info(&format!(
                        "Barre : miniatures mises à jour ({}) : hooks actifs, thème inchangé (« {} »).",
                        voulu
                            .miniatures
                            .map_or_else(|| "?".to_owned(), |t| format!("{t} %")),
                        rapport.theme
                    )),
                    (true, false) => logging::avertir(&format!(
                        "Barre : miniatures mises à jour mais hooks INACTIFS : {}.",
                        etat.message.as_deref().unwrap_or("raison inconnue")
                    )),
                    (false, _) => logging::info("Barre : miniatures retirées, thème inchangé."),
                }
                Ok(())
            }
            Err(_) if arret_demande() => Err(Echec::Interrompu),
            Err(message) => Err(Echec::passager(format!("miniatures : {message}"))),
        }
    }

    fn redemarrer(&mut self, pid: u32, build: u32, voulu: &Souhait) -> Result<(), Echec> {
        self.relecture = None;
        let theme = self.theme_voulu(build, voulu);
        verifier_arret()?;
        let hook = self.hook_voulu(pid, build, voulu);
        verifier_arret()?;

        // Échecs partiels : signalés ; s'il ne reste rien d'applicable, la
        // session est laissée restaurée et l'échec global est durable
        // seulement si toutes ses causes le sont.
        let mut causes: Vec<(Nature, String)> = Vec::new();
        let theme = match theme {
            Ok(theme) => theme.unwrap_or_default(),
            Err(Echec::Panne { nature, message }) => {
                causes.push((nature, message));
                CompiledTheme::default()
            }
            Err(Echec::Interrompu) => return Err(Echec::Interrompu),
        };
        let hook = match hook {
            Ok(hook) => hook,
            Err(Echec::Panne { nature, message }) => {
                causes.push((nature, message));
                None
            }
            Err(Echec::Interrompu) => return Err(Echec::Interrompu),
        };
        if theme.rules.is_empty() && hook.is_none() {
            let _ = tube::arreter_session(pid);
            self.applique = None;
            let nature = if !causes.is_empty() && causes.iter().all(|(n, _)| *n == Nature::Durable) {
                Nature::Durable
            } else {
                Nature::Passager
            };
            let message = if causes.is_empty() {
                "rien d'applicable, session laissée restaurée.".to_owned()
            } else {
                causes
                    .into_iter()
                    .map(|(_, m)| m)
                    .collect::<Vec<_>>()
                    .join(" ; ")
            };
            return Err(Echec::Panne { nature, message });
        }
        for (_, message) in &causes {
            signaler_une_fois(message);
        }

        // Ce qui part réellement dans `active.json` (et non le souhait : un
        // thème en échec de compilation n'est pas envoyé).
        let attendu = Attendu {
            nom_theme: theme.name.clone(),
            regles: theme.rules.len(),
            miniatures: hook.is_some(),
        };
        let theme_effectif = match (&voulu.theme, attendu.regles) {
            (None, _) => "(aucun)".to_owned(),
            (Some(id), 0) => format!("(aucun : « {id} » non compilé, voir ci-dessus)"),
            (Some(id), n) => format!("{id} (« {} », {n} règle(s) compilée(s))", attendu.nom_theme),
        };
        let miniatures_effectives = match (voulu.miniatures, attendu.miniatures) {
            (Some(t), true) => format!("{t} %"),
            (Some(_), false) => "(aucune : résolution en échec, voir ci-dessus)".to_owned(),
            (None, _) => "(aucune)".to_owned(),
        };
        logging::info(&format!(
            "Barre : application dans explorer (pid {pid}, build {build}) : thème {theme_effectif} ; \
             miniatures {miniatures_effectives}."
        ));

        let dll = injection::deployer_dll()
            .map_err(|m| Echec::passager(format!("déploiement DLL impossible : {m}")))?;
        let config = decision::config_active(theme, hook);
        let json = decision::serialiser_active(&config)
            .map_err(|m| Echec::durable(format!("active.json illisible : {m}")))?;
        injection::ecrire_active_json(&json)
            .map_err(|m| Echec::passager(format!("active.json non écrit : {m}")))?;

        let sac = injection::smart_app_control_actif();
        if sac {
            logging::avertir(
                "Barre : Smart App Control est actif, l'injection de la DLL non signée peut être \
                 bloquée (RustPiz ne contourne pas cette protection).",
            );
        }

        // Restaure une éventuelle session précédente avant de réinjecter.
        // Comme RustHawk (`stop_session(pid)?`), un échec est bloquant : sinon
        // la DLL refuserait la nouvelle session (« déjà active ») et
        // l'ancienne, qui répond encore, serait prise pour la nouvelle.
        verifier_arret()?;
        match tube::arreter_session(pid) {
            Ok(Some(rapport)) => logging::info(&format!(
                "Barre : session précédente restaurée (thème « {} », session n° {}).",
                rapport.theme, rapport.session
            )),
            Ok(None) => {}
            Err(_) if arret_demande() => return Err(Echec::Interrompu),
            Err(message) => {
                self.applique = None;
                return Err(Echec::passager(format!(
                    "la session précédente n'a pas pu être arrêtée ({message}) : nouvelle \
                     injection reportée"
                )));
            }
        }

        // Première injection dans cet explorer : attendre que la barre XAML
        // existe (ouverture de session, explorer redémarré), comme RustHawk.
        if self.barre_attendue != Some(pid) {
            self.attendre_barre(pid)?;
            self.barre_attendue = Some(pid);
        }

        // Jamais d'injection une fois l'arrêt demandé.
        verifier_arret()?;
        if let Err(erreur) = injection::injecter(pid, &dll) {
            self.applique = None;
            verifier_arret()?;
            let bloque = sac || erreur.code.is_some_and(politique::code_bloque_par_politique);
            let message = format!(
                "injection dans explorer refusée : {} ({})",
                erreur.message,
                if bloque {
                    "bloquée par Smart App Control / WDAC"
                } else {
                    "droits ou état d'explorer ?"
                }
            );
            return Err(if bloque {
                Echec::durable(message)
            } else {
                Echec::passager(message)
            });
        }
        if let Err(message) = tube::attendre_runtime(pid) {
            self.applique = None;
            verifier_arret()?;
            let _ = tube::arreter_session(pid);
            return Err(Echec::passager(format!("runtime non démarré : {message}")));
        }
        // L'arbre existant est rejoué de façon asynchrone après Advise :
        // attente avant de lire le rapport (tranches de 250 ms, interrompues
        // par l'arrêt).
        if !dormir(DELAI_AVANT_STATUT) {
            return Err(Echec::Interrompu);
        }
        // Tentatives d'Advise encore en cours : quelques lectures de plus
        // (1 s d'intervalle) pour connaître leur issue avant de conclure.
        let mut statut = tube::statut(pid);
        let mut quand = "après 2 s".to_owned();
        for supplement in 1..=LECTURES_ADVISE_EN_COURS {
            let en_cours = matches!(
                &statut,
                Ok(Some(r)) if attendu.regles > 0 && r.advise.as_ref().is_some_and(|a| !a.termine)
            );
            if !en_cours {
                break;
            }
            if !dormir(Duration::from_secs(1)) {
                return Err(Echec::Interrompu);
            }
            statut = tube::statut(pid);
            quand = format!("après {} s (abonnement encore en cours)", 2 + supplement);
        }
        match statut {
            Ok(Some(rapport)) if rapport.tripped => {
                journaliser_bilan(&rapport, &attendu, &quand);
                let _ = tube::arreter_session(pid);
                self.applique = None;
                Err(Echec::durable(
                    "trop d'échecs d'écriture (disjoncteur) : état d'origine restauré.",
                ))
            }
            Ok(Some(rapport)) => {
                let bilan = journaliser_bilan(&rapport, &attendu, &quand);
                if bilan.session_perimee {
                    // L'ancienne session répond encore : la nouvelle n'a pas
                    // démarré. On arrête tout et on réessaie plus tard.
                    let _ = tube::arreter_session(pid);
                    self.applique = None;
                    return Err(Echec::passager(
                        "une ancienne session répondait à la place de la nouvelle : arrêtée, \
                         nouvel essai prévu",
                    ));
                }
                if let Some(message) = &bilan.advise_echoue {
                    let texte = self.message_advise(message);
                    if !attendu.miniatures {
                        // Thème seul : la session n'apporte rien. Elle est
                        // arrêtée ; échec passager (réessais 30 s, 2 min,
                        // 10 min puis abandon : jamais de boucle).
                        let _ = tube::arreter_session(pid);
                        self.applique = None;
                        return Err(Echec::passager(format!("{texte} — session arrêtée")));
                    }
                    // Miniatures actives : la session est conservée, le thème
                    // est en échec (notification unique).
                    self.noter_succes(pid, voulu);
                    signaler_une_fois(&format!("{texte} (miniatures conservées)"));
                    logging::avertir(&format!(
                        "Barre appliquée PARTIELLEMENT : miniatures {miniatures_effectives} ; thème \
                         {theme_effectif} EN ÉCHEC (abonnement à l'arbre visuel refusé)."
                    ));
                    return Ok(());
                }
                let advise_en_cours = attendu.regles > 0
                    && rapport.advise.as_ref().is_some_and(|a| !a.termine);
                if advise_en_cours {
                    // Issue encore inconnue : session suivie, mais les échecs
                    // passés ne sont pas oubliés (succès confirmé seulement à
                    // la relecture, sinon un échec tardif bouclerait).
                    self.applique = Some(voulu.clone());
                } else {
                    self.noter_succes(pid, voulu);
                }
                logging::info(&format!(
                    "Barre appliquée : thème {theme_effectif} ; miniatures {miniatures_effectives}{}.",
                    if advise_en_cours {
                        " — abonnement à l'arbre visuel encore en cours (verdict à la relecture)"
                    } else if bilan.theme_sans_effet {
                        " — ATTENTION : thème encore sans effet après 2 s (relecture prévue)"
                    } else {
                        ""
                    }
                ));
                self.relecture = Some((Instant::now() + DELAI_RELECTURE, attendu));
                Ok(())
            }
            // Le tube a disparu pendant l'attente : la session s'est arrêtée
            // (RustHawk : « rapport indisponible : session terminée »).
            Ok(None) => {
                self.applique = None;
                Err(Echec::passager(
                    "rapport indisponible : la session s'est terminée juste après l'injection \
                     (voir runtime.log)",
                ))
            }
            Err(_) if arret_demande() => Err(Echec::Interrompu),
            Err(message) => {
                self.applique = None;
                Err(Echec::passager(format!("rapport d'état indisponible : {message}")))
            }
        }
    }

    /// Attend (borné, interruptible) que le module XAML de la barre soit
    /// chargé, puis laisse l'arbre se construire (RustHawk :
    /// `wait_for_taskbar`). Sans module au bout du délai, on injecte quand
    /// même (le thème s'appliquera aux éléments créés ensuite).
    fn attendre_barre(&self, pid: u32) -> Result<(), Echec> {
        let debut = Instant::now();
        let deja = injection::barre_chargee(pid);
        if !deja {
            logging::info("Barre : barre des tâches XAML pas encore chargée, attente (45 s max).");
            loop {
                if !dormir(Duration::from_millis(500)) {
                    return Err(Echec::Interrompu);
                }
                if injection::barre_chargee(pid) {
                    break;
                }
                if debut.elapsed() >= DELAI_MAX_BARRE {
                    logging::avertir(
                        "Barre : module de la barre toujours absent après 45 s, injection quand même.",
                    );
                    return Ok(());
                }
            }
        }
        if !dormir(DELAI_ARBRE_BARRE) {
            return Err(Echec::Interrompu);
        }
        if !deja {
            logging::info(&format!(
                "Barre : barre des tâches prête après {} ms.",
                debut.elapsed().as_millis()
            ));
        }
        Ok(())
    }

    /// Tour de surveillance : voir [`politique::decider_surveillance`].
    fn verifier_explorer(&mut self) {
        let Some(voulu) = self.dernier_souhait.clone() else {
            return;
        };
        if voulu.est_inactif() || arret_demande() {
            return;
        }
        let pid_actuel = injection::explorer_pid();
        let maintenant = self.maintenant_ms();
        let reessai_du =
            pid_actuel.is_some_and(|pid| self.reessais.reessai_du(pid, &voulu, maintenant));
        match politique::decider_surveillance(
            pid_actuel,
            self.pid,
            self.applique.is_some(),
            reessai_du,
        ) {
            Surveillance::Rien => {}
            Surveillance::Oublier => {
                self.pid = None;
                self.applique = None;
            }
            Surveillance::Reappliquer(motif) => {
                logging::info(match motif {
                    Motif::ExplorerChange => "Barre : explorer redémarré, réapplication.",
                    Motif::Reessai => "Barre : nouvel essai après échec.",
                });
                self.synchroniser(&voulu);
            }
            Surveillance::VerifierSession => {
                let Some(pid) = pid_actuel else { return };
                self.verifier_session(pid, &voulu);
            }
        }
    }

    /// Seconde lecture du rapport (quelques secondes après l'application) :
    /// journalise le bilan et prévient l'utilisateur (une fois) si le thème
    /// n'a toujours aucun effet. Vrai si le disjoncteur est déclenché.
    fn relire_rapport(&mut self, pid: u32, voulu: &Souhait) -> bool {
        let Some((echeance, attendu)) = self.relecture.take() else {
            return false;
        };
        if Instant::now() < echeance {
            self.relecture = Some((echeance, attendu));
            return false;
        }
        let rapport = match tube::statut(pid) {
            Ok(Some(rapport)) => rapport,
            Ok(None) => {
                logging::avertir("Barre : relecture du rapport : plus aucune session.");
                return false;
            }
            Err(message) => {
                logging::avertir(&format!("Barre : relecture du rapport impossible : {message}."));
                return false;
            }
        };
        let bilan = journaliser_bilan(&rapport, &attendu, "relecture");
        if let Some(message) = &bilan.advise_echoue {
            let texte = self.message_advise(message);
            if !attendu.miniatures && !rapport.tripped && !arret_demande() {
                // Thème seul : session inutile, arrêtée ; échec passager
                // (compteurs non remis à zéro par l'application : pas de boucle).
                let _ = tube::arreter_session(pid);
                self.applique = None;
                let suite = self.reessais.noter_echec(
                    pid,
                    voulu,
                    Nature::Passager,
                    self.maintenant_ms(),
                );
                signaler_une_fois(&format!("{texte} — session arrêtée"));
                logging::info(&format!("Barre : {}", suite.decrire()));
                return false;
            }
            signaler_une_fois(&format!("{texte} (miniatures conservées)"));
            return rapport.tripped;
        }
        // Abonnement acquis (ou sans objet) : l'application est confirmée.
        self.reessais.noter_succes(pid, voulu);
        if bilan.theme_sans_effet && !rapport.tripped {
            signaler_une_fois(&format!(
                "le thème « {} » ne s'applique pas (aucun élément stylé ou aucune valeur appliquée) : \
                 voir journal.log et runtime.log",
                attendu.nom_theme
            ));
        } else if !bilan.theme_sans_effet && attendu.regles > 0 && !rapport.tripped {
            logging::info("Barre : thème confirmé appliqué à la relecture.");
        }
        rapport.tripped
    }

    /// Session supposée active dans `pid` : vivante ? disjoncteur ?
    fn verifier_session(&mut self, pid: u32, voulu: &Souhait) {
        let vivante = match tube::sonder_session(pid) {
            Some(vivante) => vivante,
            // État inconnu (runtime figé, échange bloqué) : rien ce tour-ci.
            None => return,
        };
        if !vivante {
            if arret_demande() {
                return;
            }
            self.applique = None;
            if self.reessais.accorder_perte_session(pid, voulu) {
                logging::info("Barre : session perdue, réapplication.");
                self.synchroniser(voulu);
            } else {
                let suite = self
                    .reessais
                    .noter_echec(pid, voulu, Nature::Durable, self.maintenant_ms());
                signaler_une_fois(&format!(
                    "session perdue {} fois : intégration suspendue ({})",
                    politique::MAX_PERTES_SESSION,
                    suite.decrire()
                ));
            }
            return;
        }
        self.tours = self.tours.wrapping_add(1);
        let declenche_relecture = self.relire_rapport(pid, voulu);
        if self.applique.is_none() {
            // Session arrêtée par la relecture (thème seul en échec).
            return;
        }
        if !declenche_relecture && !politique::controle_disjoncteur_du(self.tours) {
            return;
        }
        let tripped = declenche_relecture
            || matches!(tube::statut(pid), Ok(Some(rapport)) if rapport.tripped);
        {
            if tripped && !arret_demande() {
                let _ = tube::arreter_session(pid);
                self.applique = None;
                let suite = self
                    .reessais
                    .noter_echec(pid, voulu, Nature::Durable, self.maintenant_ms());
                signaler_une_fois(
                    "trop d'échecs d'écriture (disjoncteur) : état d'origine restauré, \
                     fonction désactivée jusqu'à un changement de réglage ou un redémarrage de \
                     l'Explorateur.",
                );
                logging::info(&format!("Barre : {}", suite.decrire()));
            }
        }
    }
}
