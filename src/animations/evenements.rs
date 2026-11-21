//! Détection de l'affichage / masquage / destruction des fenêtres des
//! autres applications par `SetWinEventHook`, **hors contexte**
//! (`WINEVENT_OUTOFCONTEXT`) : aucune DLL n'est injectée dans les autres
//! processus, Windows nous transmet les événements via notre propre file
//! de messages.
//!
//! Le rappel ne fait QUE mettre l'événement en file et poster un unique
//! `WM_EVENEMENTS` à la fenêtre de contrôle ; tout le traitement a lieu
//! plus tard, hors du rappel (pas de réentrance possible).
//!
//! v0.4.7 : crochet facultatif sur `EVENT_OBJECT_CREATE` (désactivation de
//! la transition native d'apparition de DWM au plus tôt, voir
//! `transitions`), installé seulement si l'ouverture animée est active.
//! Dans le rappel, seul un filtre de style pur
//! ([`eligibilite::filtre_creation`]) précède la mise en file ; aucun
//! journal.
//!
//! Correctif audit v0.4.7 : les CREATE ont leur PROPRE file bornée
//! ([`CAPACITE_CREATIONS`]) ; un CREATE ne peut donc jamais évincer un
//! SHOW/HIDE/DESTROY de la file principale. L'ordre d'arrivée est conservé
//! à la vidange (numéro de séquence). Compteurs (reçus / filtrés / enfilés
//! / abandonnés) lus par [`compteurs_creations`] pour le journal d'état.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, PostMessageW, CHILDID_SELF, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, GWL_EXSTYLE, GWL_STYLE, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::eligibilite;
use crate::logging;
use super::erreur::ErreurAnimation;
use super::WM_EVENEMENTS;

/// Capacité maximale de la file principale (SHOW/HIDE/DESTROY, premier
/// plan, fin de déplacement ; au-delà, les plus anciens sont jetés).
const CAPACITE: usize = 256;
/// Capacité maximale de la file des CREATE (au-delà, le CREATE le plus
/// ancien est abandonné ; la file principale n'est jamais touchée).
pub const CAPACITE_CREATIONS: usize = 64;

/// Compteurs des événements `EVENT_OBJECT_CREATE` (session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompteursCreations {
    /// CREATE reçus pour une fenêtre (`OBJID_WINDOW`, `CHILDID_SELF`).
    pub recus: u64,
    /// Écartés par le filtre de style.
    pub filtres: u64,
    /// Mis en file.
    pub enfiles: u64,
    /// Abandonnés (file des CREATE pleine, ou file inaccessible).
    pub abandonnes: u64,
}

impl CompteursCreations {
    /// Texte pour le journal.
    pub fn texte(&self) -> String {
        format!(
            "CREATE reçus {}, filtrés {}, enfilés {}, abandonnés {} (file dédiée {})",
            self.recus, self.filtres, self.enfiles, self.abandonnes, CAPACITE_CREATIONS
        )
    }
}

/// Événement de fenêtre (valeur brute du HWND).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evenement {
    /// Fenêtre affichée ; 2e champ : horodatage de l'événement
    /// (`dwmsEventTime`, ms, même base que `GetTickCount`).
    Affichee(isize, u32),
    /// Fenêtre d'application créée (filtre de style passé) ; 2e champ :
    /// horodatage de l'événement (base `GetTickCount`).
    Creee(isize, u32),
    Masquee(isize),
    Detruite(isize),
    PremierPlan(isize),
    DeplacementFini(isize),
}

thread_local! {
    /// File des événements reçus (hors CREATE), vidée par le traitement de
    /// `WM_EVENEMENTS` ; chaque événement porte son numéro de séquence.
    static FILE: RefCell<VecDeque<(u64, Evenement)>> = RefCell::new(VecDeque::with_capacity(CAPACITE));
    /// File dédiée des CREATE (bornée à [`CAPACITE_CREATIONS`]).
    static FILE_CREATIONS: RefCell<VecDeque<(u64, Evenement)>> =
        RefCell::new(VecDeque::with_capacity(CAPACITE_CREATIONS));
    /// Prochain numéro de séquence (ordre d'arrivée entre les deux files).
    static SEQUENCE: Cell<u64> = const { Cell::new(0) };
    /// Compteurs des CREATE.
    static COMPTEURS_CREATIONS: Cell<CompteursCreations> = const {
        Cell::new(CompteursCreations { recus: 0, filtres: 0, enfiles: 0, abandonnes: 0 })
    };
    /// Vrai si un `WM_EVENEMENTS` est déjà posté et pas encore traité.
    static NOTIFICATION_EN_ATTENTE: Cell<bool> = const { Cell::new(false) };
    /// Fenêtre de contrôle destinataire de `WM_EVENEMENTS`.
    static CONTROLE: Cell<isize> = const { Cell::new(0) };
}

/// Crochets installés ; retirés automatiquement à la destruction.
pub struct Crochets {
    objets: HWINEVENTHOOK,
    systeme: HWINEVENTHOOK,
    /// Crochet `EVENT_OBJECT_CREATE` (facultatif, v0.4.7).
    creations: Option<HWINEVENTHOOK>,
}

/// Installe le crochet `EVENT_OBJECT_CREATE` seul.
unsafe fn installer_creations() -> Result<HWINEVENTHOOK, ErreurAnimation> {
    let crochet = SetWinEventHook(
        EVENT_OBJECT_CREATE,
        EVENT_OBJECT_CREATE,
        None,
        Some(rappel),
        0,
        0,
        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
    );
    if crochet.0.is_null() {
        return Err(windows::core::Error::from_thread().into());
    }
    Ok(crochet)
}

impl Crochets {
    /// Installe les deux crochets (objets : destruction/affichage/masquage ;
    /// système : premier plan ... fin de déplacement), plus celui des
    /// créations si `creations` (son échec n'empêche pas l'installation :
    /// [`Crochets::creations_actives`] reste faux, l'appelant le constate).
    pub fn installer(controle: HWND, creations: bool) -> Result<Crochets, ErreurAnimation> {
        CONTROLE.with(|c| c.set(controle.0 as isize));
        NOTIFICATION_EN_ATTENTE.with(|n| n.set(false));
        let drapeaux = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        unsafe {
            let objets = SetWinEventHook(
                EVENT_OBJECT_DESTROY,
                EVENT_OBJECT_HIDE,
                None,
                Some(rappel),
                0,
                0,
                drapeaux,
            );
            if objets.0.is_null() {
                return Err(windows::core::Error::from_thread().into());
            }
            let systeme = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_MOVESIZEEND,
                None,
                Some(rappel),
                0,
                0,
                drapeaux,
            );
            if systeme.0.is_null() {
                let erreur = windows::core::Error::from_thread();
                let _ = UnhookWinEvent(objets);
                return Err(erreur.into());
            }
            let creations = if creations {
                match installer_creations() {
                    Ok(c) => Some(c),
                    Err(e) => {
                        logging::avertir(&format!(
                            "Animations : crochet EVENT_OBJECT_CREATE impossible ({e}) — \
                             transition DWM désactivée au SHOW seulement."
                        ));
                        None
                    }
                }
            } else {
                None
            };
            Ok(Crochets {
                objets,
                systeme,
                creations,
            })
        }
    }

    /// Installe ou retire le crochet des créations. `Ok(true)` si l'état a
    /// changé, `Ok(false)` s'il était déjà celui demandé.
    pub fn definir_creations(&mut self, actif: bool) -> Result<bool, ErreurAnimation> {
        match (actif, self.creations) {
            (true, None) => {
                self.creations = Some(unsafe { installer_creations()? });
                Ok(true)
            }
            (false, Some(c)) => {
                unsafe {
                    let _ = UnhookWinEvent(c);
                }
                self.creations = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Vrai si le crochet des créations est installé.
    pub fn creations_actives(&self) -> bool {
        self.creations.is_some()
    }
}

impl Drop for Crochets {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.objets);
            let _ = UnhookWinEvent(self.systeme);
            if let Some(c) = self.creations.take() {
                let _ = UnhookWinEvent(c);
            }
        }
        vider_file();
    }
}

/// Retire tous les événements en attente (les deux files, fusionnées dans
/// l'ordre d'arrivée) et réarme la notification.
pub fn vider_file() -> Vec<Evenement> {
    NOTIFICATION_EN_ATTENTE.with(|n| n.set(false));
    let principaux: Vec<(u64, Evenement)> = FILE.with(|f| match f.try_borrow_mut() {
        Ok(mut file) => file.drain(..).collect(),
        Err(_) => Vec::new(),
    });
    let creations: Vec<(u64, Evenement)> = FILE_CREATIONS.with(|f| match f.try_borrow_mut() {
        Ok(mut file) => file.drain(..).collect(),
        Err(_) => Vec::new(),
    });
    fusionner(principaux, creations)
}

/// Fusionne deux files déjà triées par numéro de séquence.
fn fusionner(a: Vec<(u64, Evenement)>, b: Vec<(u64, Evenement)>) -> Vec<Evenement> {
    let mut sortie = Vec::with_capacity(a.len() + b.len());
    let mut ia = a.into_iter().peekable();
    let mut ib = b.into_iter().peekable();
    loop {
        let prendre_a = match (ia.peek(), ib.peek()) {
            (Some(x), Some(y)) => x.0 <= y.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let suivant = if prendre_a { ia.next() } else { ib.next() };
        if let Some((_, ev)) = suivant {
            sortie.push(ev);
        }
    }
    sortie
}

/// Compteurs des CREATE de la session.
pub fn compteurs_creations() -> CompteursCreations {
    COMPTEURS_CREATIONS.with(|c| c.get())
}

fn compter(f: impl FnOnce(&mut CompteursCreations)) {
    COMPTEURS_CREATIONS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// Rappel WinEvent (appelé par notre boucle de messages). Aucun traitement
/// ici : mise en file + un seul `PostMessageW`. Ne panique jamais.
unsafe extern "system" fn rappel(
    _crochet: HWINEVENTHOOK,
    evenement: u32,
    hwnd: HWND,
    id_objet: i32,
    id_enfant: i32,
    _fil: u32,
    horodatage: u32,
) {
    if hwnd.0.is_null() || id_objet != OBJID_WINDOW.0 || id_enfant != CHILDID_SELF as i32 {
        return;
    }
    let brut = hwnd.0 as isize;
    let ev = match evenement {
        EVENT_OBJECT_CREATE => {
            compter(|c| c.recus = c.recus.saturating_add(1));
            // Filtre de style pur avant toute mise en file (lectures seules,
            // aucun message envoyé à l'application).
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if !eligibilite::filtre_creation(style, exstyle) {
                compter(|c| c.filtres = c.filtres.saturating_add(1));
                return;
            }
            Evenement::Creee(brut, horodatage)
        }
        EVENT_OBJECT_SHOW => Evenement::Affichee(brut, horodatage),
        EVENT_OBJECT_HIDE => Evenement::Masquee(brut),
        EVENT_OBJECT_DESTROY => Evenement::Detruite(brut),
        EVENT_SYSTEM_FOREGROUND => Evenement::PremierPlan(brut),
        EVENT_SYSTEM_MOVESIZEEND => Evenement::DeplacementFini(brut),
        _ => return,
    };
    let sequence = SEQUENCE.with(|s| {
        let v = s.get();
        s.set(v.wrapping_add(1));
        v
    });
    let ajoute = if matches!(ev, Evenement::Creee(..)) {
        // File dédiée : un CREATE n'évince jamais un autre type d'événement.
        let issue = FILE_CREATIONS.with(|f| match f.try_borrow_mut() {
            Ok(mut file) => {
                let evince = file.len() >= CAPACITE_CREATIONS;
                if evince {
                    file.pop_front();
                }
                file.push_back((sequence, ev));
                Some(evince)
            }
            Err(_) => None,
        });
        compter(|c| match issue {
            Some(evince) => {
                c.enfiles = c.enfiles.saturating_add(1);
                if evince {
                    c.abandonnes = c.abandonnes.saturating_add(1);
                }
            }
            None => c.abandonnes = c.abandonnes.saturating_add(1),
        });
        issue.is_some()
    } else {
        FILE.with(|f| match f.try_borrow_mut() {
            Ok(mut file) => {
                if file.len() >= CAPACITE {
                    file.pop_front();
                }
                file.push_back((sequence, ev));
                true
            }
            Err(_) => false,
        })
    };
    if !ajoute {
        return;
    }
    let deja = NOTIFICATION_EN_ATTENTE.with(|n| n.replace(true));
    if !deja {
        let controle = CONTROLE.with(|c| c.get());
        if controle == 0
            || PostMessageW(
                Some(HWND(controle as *mut core::ffi::c_void)),
                WM_EVENEMENTS,
                WPARAM(0),
                LPARAM(0),
            )
            .is_err()
        {
            // Échec : on réarme pour retenter au prochain événement.
            NOTIFICATION_EN_ATTENTE.with(|n| n.set(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fusion_conserve_l_ordre_d_arrivee() {
        let a = vec![(0, Evenement::Affichee(1, 0)), (3, Evenement::Masquee(1))];
        let b = vec![(1, Evenement::Creee(2, 0)), (2, Evenement::Creee(3, 0))];
        assert_eq!(
            fusionner(a, b),
            vec![
                Evenement::Affichee(1, 0),
                Evenement::Creee(2, 0),
                Evenement::Creee(3, 0),
                Evenement::Masquee(1)
            ]
        );
        assert!(fusionner(Vec::new(), Vec::new()).is_empty());
    }
}
