//! Cadence de l'effet « Jelly » (module pur, sans `windows`) : décisions
//! et état atomique du fil de battement synchronisé sur le rafraîchissement
//! de l'écran ([`metronome`](super::metronome), partie Win32).
//!
//! Le fil de battement attend chaque image du compositeur
//! (`DCompositionWaitForCompositorClock`, ou `DwmFlush` en repli) puis
//! poste UN message au fil principal, qui exécute le pas de simulation et
//! le rendu. Coalescence : tant que le message précédent n'a pas été pris
//! en charge par le fil principal, aucun nouveau message n'est posté (la
//! file ne grossit jamais, même si un pas dure plus d'une image).
//!
//! Tout l'état partagé entre les deux fils est ici, uniquement atomique.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::time::Duration;

/// `WAIT_OBJECT_0`.
pub const WAIT_OBJECT_0: u32 = 0;
/// `WAIT_TIMEOUT`.
pub const WAIT_TIMEOUT: u32 = 258;
/// `WAIT_FAILED`.
pub const WAIT_FAILED: u32 = 0xFFFF_FFFF;

/// Attente maximale d'un tic de l'horloge du compositeur (ms) : au-delà, on
/// bat quand même (compositeur au repos : la simulation doit continuer, et
/// le drapeau d'arrêt est relu au moins à cette fréquence).
pub const ATTENTE_MAX_MS: u32 = 50;
/// Intervalle minimal entre deux battements : garde-fou contre une attente
/// qui rendrait la main immédiatement (`DwmFlush` sans composition en
/// cours), plafond ~333 Hz (au-delà des écrans 240 Hz).
pub const INTERVALLE_MIN: Duration = Duration::from_millis(3);
/// Attente de repli quand `DwmFlush` lui-même échoue (~60 Hz).
pub const ATTENTE_SOMMEIL: Duration = Duration::from_millis(16);
/// Période du minuteur de repli (`SetTimer`) si le fil ne peut pas démarrer.
pub const PERIODE_MINUTEUR_MS: u32 = 15;
/// Garde-fou du fil de battement : un message réservé depuis plus longtemps
/// sans prise en charge (drapeau resté bloqué : message perdu, prise en
/// charge manquée) est considéré comme perdu ; le drapeau est libéré et un
/// nouveau battement posté.
pub const DELAI_DEBLOCAGE: Duration = Duration::from_millis(250);

/// Faut-il forcer la libération du drapeau de coalescence, le message en
/// file ayant été réservé il y a `depuis_reservation` ?
pub fn deblocage_necessaire(depuis_reservation: Duration) -> bool {
    depuis_reservation >= DELAI_DEBLOCAGE
}

/// Mode de cadence effectivement utilisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeCadence {
    /// `DCompositionWaitForCompositorClock` (Windows 11) : une attente par
    /// image composée par DWM, à la fréquence de l'écran.
    HorlogeCompositeur,
    /// `DwmFlush` : attend la prochaine présentation de DWM.
    DwmFlush,
    /// `DwmFlush` en échec : simple attente de [`ATTENTE_SOMMEIL`].
    Sommeil,
    /// Fil indisponible : minuteur `SetTimer` de [`PERIODE_MINUTEUR_MS`].
    Minuteur,
}

impl ModeCadence {
    pub fn code(self) -> u8 {
        match self {
            ModeCadence::HorlogeCompositeur => 1,
            ModeCadence::DwmFlush => 2,
            ModeCadence::Sommeil => 3,
            ModeCadence::Minuteur => 4,
        }
    }

    pub fn depuis_code(code: u8) -> Option<ModeCadence> {
        match code {
            1 => Some(ModeCadence::HorlogeCompositeur),
            2 => Some(ModeCadence::DwmFlush),
            3 => Some(ModeCadence::Sommeil),
            4 => Some(ModeCadence::Minuteur),
            _ => None,
        }
    }

    pub fn libelle(self) -> &'static str {
        match self {
            ModeCadence::HorlogeCompositeur => {
                "horloge du compositeur (DCompositionWaitForCompositorClock)"
            }
            ModeCadence::DwmFlush => "DwmFlush (repli)",
            ModeCadence::Sommeil => "attente 16 ms (DwmFlush en échec)",
            ModeCadence::Minuteur => "minuteur SetTimer 15 ms (repli : fil de battement indisponible)",
        }
    }

    /// Mode qui suit un échec du mode courant (`None` : on y reste).
    pub fn repli(self) -> Option<ModeCadence> {
        match self {
            ModeCadence::HorlogeCompositeur => Some(ModeCadence::DwmFlush),
            ModeCadence::DwmFlush => Some(ModeCadence::Sommeil),
            ModeCadence::Sommeil | ModeCadence::Minuteur => None,
        }
    }
}

/// Lecture d'un code de retour de `DCompositionWaitForCompositorClock`
/// appelée avec `nb_poignees` poignées (toutes des demandes d'arrêt) :
/// `WAIT_OBJECT_0 + i` (i < n) = poignée i signalée ; `WAIT_OBJECT_0 + n` =
/// tic de l'horloge ; `WAIT_TIMEOUT` ; `WAIT_FAILED` ou code inconnu = échec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultatAttente {
    Tic,
    Arret,
    Delai,
    Echec,
}

pub fn interpreter_attente(code: u32, nb_poignees: u32) -> ResultatAttente {
    if code == WAIT_FAILED {
        ResultatAttente::Echec
    } else if code == WAIT_TIMEOUT {
        ResultatAttente::Delai
    } else if (WAIT_OBJECT_0..WAIT_OBJECT_0.saturating_add(nb_poignees)).contains(&code) {
        ResultatAttente::Arret
    } else if code == WAIT_OBJECT_0.saturating_add(nb_poignees) {
        ResultatAttente::Tic
    } else {
        ResultatAttente::Echec
    }
}

/// Que faire après une attente ?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    /// Battre (tic, ou délai dépassé : la simulation continue).
    Battre,
    /// Sortir de la boucle.
    Sortir,
    /// Passer au mode de repli puis battre.
    Replier,
}

pub fn suite_attente(r: ResultatAttente) -> Suite {
    match r {
        ResultatAttente::Tic | ResultatAttente::Delai => Suite::Battre,
        ResultatAttente::Arret => Suite::Sortir,
        ResultatAttente::Echec => Suite::Replier,
    }
}

/// Attente complémentaire pour respecter [`INTERVALLE_MIN`] depuis le
/// battement précédent (`None` : battre tout de suite).
pub fn repos_avant_battement(ecoule: Duration) -> Option<Duration> {
    (ecoule < INTERVALLE_MIN).then(|| INTERVALLE_MIN - ecoule)
}

/// Fréquence (Hz) d'après l'intervalle moyen (ms) ; 0 si inconnu.
pub fn frequence_hz(intervalle_moyen_ms: f64) -> f64 {
    if intervalle_moyen_ms.is_finite() && intervalle_moyen_ms > 0.0 {
        1000.0 / intervalle_moyen_ms
    } else {
        0.0
    }
}

/// État partagé entre le fil de battement et le fil principal (atomique).
#[derive(Debug, Default)]
pub struct EtatBattement {
    /// Demande d'arrêt (posée par le fil principal).
    arret: AtomicBool,
    /// Un message de battement est en file, non encore pris en charge.
    en_attente: AtomicBool,
    /// Code du mode courant ([`ModeCadence::code`]).
    mode: AtomicU8,
    /// Battements postés.
    postes: AtomicU32,
    /// Battements non postés car le précédent n'était pas pris en charge.
    coalesces: AtomicU32,
    /// Attentes terminées par le délai maximal (aucun tic).
    delais: AtomicU32,
    /// `PostMessageW` en échec.
    echecs_post: AtomicU32,
    /// Dernier code / HRESULT d'échec de l'attente (0 = aucun).
    dernier_echec: AtomicU32,
    /// Drapeaux de coalescence libérés par le garde-fou du fil.
    debloquages: AtomicU32,
}

/// Instantané des compteurs (journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compteurs {
    pub mode: Option<ModeCadence>,
    pub postes: u32,
    pub coalesces: u32,
    pub delais: u32,
    pub echecs_post: u32,
    pub dernier_echec: u32,
    pub debloquages: u32,
}

impl EtatBattement {
    pub fn nouveau(mode: ModeCadence) -> Self {
        let e = EtatBattement::default();
        e.mode.store(mode.code(), Ordering::Relaxed);
        e
    }

    pub fn demander_arret(&self) {
        self.arret.store(true, Ordering::Release);
    }

    pub fn arret_demande(&self) -> bool {
        self.arret.load(Ordering::Acquire)
    }

    pub fn mode(&self) -> Option<ModeCadence> {
        ModeCadence::depuis_code(self.mode.load(Ordering::Relaxed))
    }

    pub fn changer_mode(&self, mode: ModeCadence, code_echec: u32) {
        self.dernier_echec.store(code_echec, Ordering::Relaxed);
        self.mode.store(mode.code(), Ordering::Relaxed);
    }

    pub fn compter_delai(&self) {
        self.delais.fetch_add(1, Ordering::Relaxed);
    }

    /// Fil de battement : réserve le droit de poster. `false` si un message
    /// est déjà en file (battement coalescé, compté).
    pub fn reserver(&self) -> bool {
        let ok = self
            .en_attente
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if ok {
            self.postes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.coalesces.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    /// Fil de battement : `PostMessageW` a échoué après [`reserver`](Self::reserver).
    pub fn annuler_reservation(&self) {
        self.postes.fetch_sub(1, Ordering::Relaxed);
        self.echecs_post.fetch_add(1, Ordering::Relaxed);
        self.en_attente.store(false, Ordering::Release);
    }

    /// Fil principal : le message en file est pris en charge (le suivant
    /// pourra être posté pendant le traitement du pas).
    pub fn prendre_en_charge(&self) {
        self.en_attente.store(false, Ordering::Release);
    }

    /// Fil de battement (garde-fou) : libère le drapeau de coalescence resté
    /// posé trop longtemps. `true` s'il était posé (libération comptée).
    pub fn debloquer(&self) -> bool {
        let ok = self
            .en_attente
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if ok {
            self.debloquages.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    pub fn compteurs(&self) -> Compteurs {
        Compteurs {
            mode: self.mode(),
            postes: self.postes.load(Ordering::Relaxed),
            coalesces: self.coalesces.load(Ordering::Relaxed),
            delais: self.delais.load(Ordering::Relaxed),
            echecs_post: self.echecs_post.load(Ordering::Relaxed),
            dernier_echec: self.dernier_echec.load(Ordering::Relaxed),
            debloquages: self.debloquages.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lecture_des_codes_d_attente() {
        // Une poignée (arrêt) : 0 = arrêt, 1 = tic.
        assert_eq!(interpreter_attente(0, 1), ResultatAttente::Arret);
        assert_eq!(interpreter_attente(1, 1), ResultatAttente::Tic);
        assert_eq!(interpreter_attente(WAIT_TIMEOUT, 1), ResultatAttente::Delai);
        assert_eq!(interpreter_attente(WAIT_FAILED, 1), ResultatAttente::Echec);
        assert_eq!(interpreter_attente(2, 1), ResultatAttente::Echec);
        // Sans poignée : 0 = tic.
        assert_eq!(interpreter_attente(0, 0), ResultatAttente::Tic);
        assert_eq!(interpreter_attente(0x80, 1), ResultatAttente::Echec);
    }

    #[test]
    fn suites() {
        assert_eq!(suite_attente(ResultatAttente::Tic), Suite::Battre);
        assert_eq!(suite_attente(ResultatAttente::Delai), Suite::Battre);
        assert_eq!(suite_attente(ResultatAttente::Arret), Suite::Sortir);
        assert_eq!(suite_attente(ResultatAttente::Echec), Suite::Replier);
    }

    #[test]
    fn chaine_de_repli() {
        assert_eq!(
            ModeCadence::HorlogeCompositeur.repli(),
            Some(ModeCadence::DwmFlush)
        );
        assert_eq!(ModeCadence::DwmFlush.repli(), Some(ModeCadence::Sommeil));
        assert_eq!(ModeCadence::Sommeil.repli(), None);
        assert_eq!(ModeCadence::Minuteur.repli(), None);
        for m in [
            ModeCadence::HorlogeCompositeur,
            ModeCadence::DwmFlush,
            ModeCadence::Sommeil,
            ModeCadence::Minuteur,
        ] {
            assert_eq!(ModeCadence::depuis_code(m.code()), Some(m));
            assert!(!m.libelle().is_empty());
        }
        assert_eq!(ModeCadence::depuis_code(0), None);
    }

    #[test]
    fn coalescence_un_seul_message_en_file() {
        let e = EtatBattement::nouveau(ModeCadence::HorlogeCompositeur);
        assert!(e.reserver());
        // Le précédent n'est pas pris en charge : pas de nouveau message.
        assert!(!e.reserver());
        assert!(!e.reserver());
        e.prendre_en_charge();
        assert!(e.reserver());
        // Échec de PostMessageW : la réservation est rendue.
        e.annuler_reservation();
        assert!(e.reserver());
        let c = e.compteurs();
        assert_eq!(c.postes, 2);
        assert_eq!(c.coalesces, 2);
        assert_eq!(c.echecs_post, 1);
        assert_eq!(c.mode, Some(ModeCadence::HorlogeCompositeur));
    }

    #[test]
    fn garde_fou_du_drapeau_bloque() {
        assert!(!deblocage_necessaire(Duration::from_millis(249)));
        assert!(deblocage_necessaire(DELAI_DEBLOCAGE));
        assert!(deblocage_necessaire(Duration::from_secs(5)));
        let e = EtatBattement::nouveau(ModeCadence::DwmFlush);
        // Drapeau libre : rien à débloquer.
        assert!(!e.debloquer());
        assert!(e.reserver());
        // Prise en charge jamais reçue : bloqué...
        assert!(!e.reserver());
        // ... jusqu'au garde-fou, qui libère et permet de reposter.
        assert!(e.debloquer());
        assert!(e.reserver());
        let c = e.compteurs();
        assert_eq!(c.debloquages, 1);
        assert_eq!(c.postes, 2);
        assert_eq!(c.coalesces, 1);
    }

    #[test]
    fn arret_et_changement_de_mode() {
        let e = EtatBattement::nouveau(ModeCadence::HorlogeCompositeur);
        assert!(!e.arret_demande());
        e.changer_mode(ModeCadence::DwmFlush, WAIT_FAILED);
        e.compter_delai();
        e.demander_arret();
        assert!(e.arret_demande());
        let c = e.compteurs();
        assert_eq!(c.mode, Some(ModeCadence::DwmFlush));
        assert_eq!(c.dernier_echec, WAIT_FAILED);
        assert_eq!(c.delais, 1);
    }

    #[test]
    fn intervalle_minimal_et_frequence() {
        assert_eq!(
            repos_avant_battement(Duration::from_millis(1)),
            Some(Duration::from_millis(2))
        );
        assert_eq!(repos_avant_battement(INTERVALLE_MIN), None);
        assert_eq!(repos_avant_battement(Duration::from_millis(16)), None);
        assert!((frequence_hz(16.666_667) - 60.0).abs() < 0.01);
        assert!((frequence_hz(6.944_444) - 144.0).abs() < 0.01);
        assert_eq!(frequence_hz(0.0), 0.0);
        assert_eq!(frequence_hz(f64::NAN), 0.0);
    }
}
