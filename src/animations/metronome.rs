//! Fil de battement de l'effet « Jelly » : une impulsion par image
//! composée par DWM (rafraîchissement de l'écran), postée au fil principal
//! sous forme d'un message. Décisions et état partagé :
//! [`cadence`](super::cadence) (pur, atomique).
//!
//! - Attente : `DCompositionWaitForCompositorClock` (Windows 11), résolue
//!   DYNAMIQUEMENT dans `dcomp.dll` (pas d'import statique : le programme
//!   doit démarrer même si la fonction est absente) ; en cas d'absence ou
//!   d'échec, `DwmFlush` ; si `DwmFlush` échoue aussi, attente de 16 ms.
//! - Le fil n'accède qu'à des atomiques ([`EtatBattement`]), à son propre
//!   événement d'arrêt et à `PostMessageW` : aucun état de l'application,
//!   aucun journal (le fil principal journalise les compteurs).
//! - Il n'existe que pendant l'effet (armement, glisser, retour) : créé par
//!   [`Metronome::demarrer`], arrêté par `Drop` (drapeau atomique +
//!   événement qui réveille l'attente), attente bornée à
//!   [`ATTENTE_ARRET`] puis détachement sûr (le fil ne garde que des `Arc`).

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::cadence::{
    deblocage_necessaire, interpreter_attente, repos_avant_battement, suite_attente, Compteurs,
    EtatBattement, ModeCadence, Suite, ATTENTE_MAX_MS, ATTENTE_SOMMEIL,
};

/// Taille de pile du fil de battement (marge confortable pour les appels
/// système : DComp, DWM, PostMessageW).
const PILE_FIL: usize = 256 * 1024;

/// Attente maximale de la fin du fil à l'arrêt (fil principal), avant
/// détachement.
const ATTENTE_ARRET: Duration = Duration::from_millis(50);

/// `DCompositionWaitForCompositorClock` (dcomp.dll, Windows 11).
type FnAttenteHorloge =
    unsafe extern "system" fn(count: u32, handles: *const HANDLE, timeout_ms: u32) -> u32;

/// Résolution unique de `DCompositionWaitForCompositorClock`.
static HORLOGE: OnceLock<Result<FnAttenteHorloge, String>> = OnceLock::new();

fn horloge() -> Result<FnAttenteHorloge, String> {
    HORLOGE
        .get_or_init(|| unsafe {
            let module = LoadLibraryExW(w!("dcomp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(|e| {
                    format!(
                        "dcomp.dll non chargée — HRESULT {:#010x} ({})",
                        e.code().0 as u32,
                        e.message()
                    )
                })?;
            // Module jamais libéré (déjà chargé par DirectComposition).
            match GetProcAddress(module, s!("DCompositionWaitForCompositorClock")) {
                // SAFETY : signature documentée (dcomp.h) :
                // DWORD DCompositionWaitForCompositorClock(UINT, const HANDLE*, DWORD).
                Some(f) => Ok(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    FnAttenteHorloge,
                >(f)),
                None => Err(
                    "DCompositionWaitForCompositorClock absente de dcomp.dll (Windows 10 ?)"
                        .to_string(),
                ),
            }
        })
        .clone()
}

/// Événement d'arrêt (réveille l'attente de l'horloge). Fermé quand le
/// dernier `Arc` (fil principal ou fil de battement) disparaît.
struct EvenementArret(HANDLE);

// SAFETY : une poignée d'événement noyau est utilisable depuis n'importe
// quel fil ; elle n'est fermée qu'une fois (Drop du dernier Arc).
unsafe impl Send for EvenementArret {}
unsafe impl Sync for EvenementArret {}

impl Drop for EvenementArret {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Fil de battement en cours (arrêté au `Drop`).
pub struct Metronome {
    etat: Arc<EtatBattement>,
    evenement: Arc<EvenementArret>,
    fil: Option<JoinHandle<()>>,
    generation: u32,
    debut: Instant,
    /// Raison pour laquelle l'horloge du compositeur n'est pas utilisée.
    pub motif_repli: Option<String>,
}

impl Metronome {
    /// Démarre le fil : chaque battement poste `message` (wparam =
    /// `generation`) à la fenêtre `controle`.
    pub fn demarrer(controle: isize, message: u32, generation: u32) -> Result<Metronome, String> {
        if controle == 0 {
            return Err("fenêtre de contrôle absente".into());
        }
        let (attente, motif_repli) = match horloge() {
            Ok(f) => (Some(f), None),
            Err(m) => (None, Some(m)),
        };
        let mode = if attente.is_some() {
            ModeCadence::HorlogeCompositeur
        } else {
            ModeCadence::DwmFlush
        };
        // Réinitialisation manuelle, non signalé.
        let evenement = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(|h| Arc::new(EvenementArret(h)))
            .map_err(|e| {
                format!(
                    "CreateEventW — HRESULT {:#010x} ({})",
                    e.code().0 as u32,
                    e.message()
                )
            })?;
        let etat = Arc::new(EtatBattement::nouveau(mode));
        let (etat_fil, evenement_fil) = (etat.clone(), evenement.clone());
        let fil = std::thread::Builder::new()
            .name("jelly-battement".into())
            .stack_size(PILE_FIL)
            .spawn(move || boucle(etat_fil, evenement_fil, attente, controle, message, generation))
            .map_err(|e| format!("création du fil impossible : {e}"))?;
        Ok(Metronome {
            etat,
            evenement,
            fil: Some(fil),
            generation,
            debut: Instant::now(),
            motif_repli,
        })
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Le message de battement en file est pris en charge (coalescence).
    pub fn prendre_en_charge(&self) {
        self.etat.prendre_en_charge();
    }

    /// Poignée légère (état partagé + génération) permettant de prendre en
    /// charge un battement sans accéder au `Metronome` lui-même.
    pub fn poignee(&self) -> PoigneeBattement {
        PoigneeBattement {
            etat: self.etat.clone(),
            generation: self.generation,
        }
    }

    pub fn compteurs(&self) -> Compteurs {
        self.etat.compteurs()
    }

    pub fn duree(&self) -> Duration {
        self.debut.elapsed()
    }
}

/// Poignée du fil de battement (voir [`Metronome::poignee`]).
#[derive(Clone)]
pub struct PoigneeBattement {
    etat: Arc<EtatBattement>,
    generation: u32,
}

impl PoigneeBattement {
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Libère le drapeau de coalescence (message pris en charge).
    pub fn prendre_en_charge(&self) {
        self.etat.prendre_en_charge();
    }
}

impl Drop for Metronome {
    fn drop(&mut self) {
        self.etat.demander_arret();
        unsafe {
            let _ = SetEvent(self.evenement.0);
        }
        let Some(fil) = self.fil.take() else {
            return;
        };
        let limite = Instant::now() + ATTENTE_ARRET;
        while !fil.is_finished() && Instant::now() < limite {
            std::thread::sleep(Duration::from_millis(1));
        }
        if fil.is_finished() {
            let _ = fil.join();
        }
        // Sinon : détachement. Le fil sort dès la fin de son attente en
        // cours (drapeau d'arrêt) et ne détient que des Arc.
    }
}

/// Corps du fil de battement (aucune panique possible hors bogue : pas
/// d'indexation, pas d'unwrap).
fn boucle(
    etat: Arc<EtatBattement>,
    evenement: Arc<EvenementArret>,
    attente: Option<FnAttenteHorloge>,
    controle: isize,
    message: u32,
    generation: u32,
) {
    let mut mode = etat.mode().unwrap_or(ModeCadence::DwmFlush);
    let poignees = [evenement.0];
    let mut dernier = Instant::now();
    // Instant de la dernière réservation réussie (garde-fou du drapeau).
    let mut reserve_a = Instant::now();
    while !etat.arret_demande() {
        let suite = match (mode, attente) {
            (ModeCadence::HorlogeCompositeur, Some(f)) => {
                let code = unsafe { f(poignees.len() as u32, poignees.as_ptr(), ATTENTE_MAX_MS) };
                let r = interpreter_attente(code, poignees.len() as u32);
                if r == super::cadence::ResultatAttente::Delai {
                    etat.compter_delai();
                }
                let s = suite_attente(r);
                if s == Suite::Replier {
                    if let Some(m) = mode.repli() {
                        etat.changer_mode(m, code);
                        mode = m;
                    }
                }
                s
            }
            (ModeCadence::HorlogeCompositeur, None) | (ModeCadence::DwmFlush, _) => {
                match unsafe { DwmFlush() } {
                    Ok(()) => Suite::Battre,
                    Err(e) => {
                        let m = ModeCadence::Sommeil;
                        etat.changer_mode(m, e.code().0 as u32);
                        mode = m;
                        Suite::Replier
                    }
                }
            }
            (ModeCadence::Sommeil | ModeCadence::Minuteur, _) => {
                std::thread::sleep(ATTENTE_SOMMEIL);
                Suite::Battre
            }
        };
        if suite == Suite::Sortir || etat.arret_demande() {
            break;
        }
        if let Some(reste) = repos_avant_battement(dernier.elapsed()) {
            std::thread::sleep(reste);
        }
        dernier = Instant::now();
        // Garde-fou : drapeau resté posé trop longtemps (prise en charge
        // manquée) -> libéré, puis nouveau battement.
        let reserve = etat.reserver()
            || (deblocage_necessaire(reserve_a.elapsed()) && etat.debloquer() && etat.reserver());
        if reserve {
            reserve_a = Instant::now();
            let poste = unsafe {
                PostMessageW(
                    Some(HWND(controle as *mut c_void)),
                    message,
                    WPARAM(generation as usize),
                    LPARAM(0),
                )
            };
            if poste.is_err() {
                etat.annuler_reservation();
            }
        }
    }
}
