//! Masquage temporaire d'une fenêtre pendant son animation d'ouverture,
//! par **transparence temporaire** (« layered » à alpha 0 jusqu'en v0.4.4,
//! à l'alpha [`ALPHA_OUVERTURE`] = 1/255 depuis la v0.4.5), et pendant
//! l'effet Jelly (alpha 1).
//!
//! v0.4.5 : l'ouverture masque à l'alpha 1 et non plus 0. Hypothèse (non
//! prouvée) : à alpha 0, la fenêtre est traitée comme non rendue (DWM /
//! XAML) et l'application ne termine pas de se peindre avant la
//! restauration ; à alpha 1, elle reste « visible » pour le système. Toutes
//! les restaurations utilisent l'alpha posé mémorisé dans l'inscription
//! (`decision_restauration_avec(.., alpha_pose)`), jamais un alpha 0
//! supposé.
//!
//! v0.4.6 : prouvé par le journal v0.4.5, l'application continue bien de se
//! peindre sous le masque à l'alpha 1 (images WGC en continu). La fenêtre
//! reste masquée pendant l'attente de l'image d'ouverture (voir
//! `attente`) ; les aides du « maintien » v0.4.4/v0.4.5 (repaint forcé,
//! coups de pouce de relayout, redimensionnement 1 px, restauration sans
//! attente de composition) ont été supprimées avec lui.
//!
//! Historique : la v0.3.0 utilisait `DWMWA_CLOAK`, refusé par Windows sur
//! les fenêtres des autres processus (E_HANDLE 0x80070006, constaté sur un
//! vrai Windows 11). La v0.3.1 applique la stratégie « capture d'abord,
//! masquage ensuite » :
//! 1. la fenêtre s'affiche normalement, une session WGC est démarrée ;
//! 2. à la première image, l'état d'origine (style étendu + attributs
//!    layered) est mémorisé dans [`MASQUEES`] AVANT toute modification, puis
//!    `WS_EX_LAYERED` est ajouté si besoin et l'alpha est mis à 0 ;
//! 3. à la fin de l'animation (et sur tous les chemins d'abandon), l'état
//!    d'origine est restauré exactement, sauf si un tiers (module de
//!    transparence, profils, l'application elle-même) a modifié l'alpha
//!    entre-temps — voir [`decision_restauration`].
//!
//! Un clignotement de 1 à 3 images est ASSUMÉ : la fenêtre est visible
//! pendant la latence de l'événement `EVENT_OBJECT_SHOW` (asynchrone) et
//! jusqu'à la première image capturée, avant d'être remplacée par la
//! surimpression animée.
//!
//! La partie décisionnelle est pure (testable hors Windows) ; la partie
//! Win32 est dans le sous-module `win32`.

use std::fmt;

/// `WS_EX_LAYERED` (valeur documentée, recopiée pour la partie pure).
pub const WS_EX_LAYERED: isize = 0x0008_0000;
/// `LWA_COLORKEY` (utilisé par les tests).
#[cfg(test)]
pub const LWA_COLORKEY: u32 = 0x1;
/// `LWA_ALPHA`.
pub const LWA_ALPHA: u32 = 0x2;

/// Attributs layered (`GetLayeredWindowAttributes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributs {
    /// Couleur transparente (COLORREF brut).
    pub cle: u32,
    pub alpha: u8,
    /// Drapeaux `LWA_*`.
    pub drapeaux: u32,
}

impl Attributs {
    /// Vrai si ces attributs rendent la fenêtre totalement invisible
    /// (alpha 0 effectif) — c'est l'état posé par notre masquage.
    pub fn invisible(&self) -> bool {
        self.drapeaux & LWA_ALPHA != 0 && self.alpha == 0
    }
}

/// État « layered » d'une fenêtre à un instant donné.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EtatLayered {
    /// Style étendu (`GWL_EXSTYLE`).
    pub exstyle: isize,
    /// Attributs layered, `None` si la fenêtre n'est pas layered ou si la
    /// lecture a échoué (fenêtre `UpdateLayeredWindow`).
    pub attributs: Option<Attributs>,
}

impl EtatLayered {
    pub fn est_layered(&self) -> bool {
        self.exstyle & WS_EX_LAYERED != 0
    }
}

impl fmt::Display for EtatLayered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.est_layered() {
            return write!(f, "non layered (exstyle {:#x})", self.exstyle);
        }
        match self.attributs {
            Some(a) => write!(
                f,
                "layered (exstyle {:#x}, clé {:#08x}, alpha {}, drapeaux {:#x})",
                self.exstyle, a.cle, a.alpha, a.drapeaux
            ),
            None => write!(f, "layered sans attributs lisibles (exstyle {:#x})", self.exstyle),
        }
    }
}

/// Alpha posé par le masquage d'ouverture (v0.4.5 : 1/255 au lieu de 0).
/// Une fenêtre déjà layered à un alpha <= cette valeur n'est pas masquée
/// ([`Refus::DejaInvisible`]).
pub const ALPHA_OUVERTURE: u8 = 1;

/// Raison pour laquelle une fenêtre ne doit pas être masquée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refus {
    /// Layered mais attributs illisibles : fenêtre `UpdateLayeredWindow`
    /// (lui appliquer `SetLayeredWindowAttributes` détruirait son contenu).
    LayeredSansAttributs,
    /// Déjà invisible (alpha <= alpha de masquage) : notre heuristique de
    /// restauration (« alpha posé = c'est nous ») serait ambiguë, et il n'y
    /// a rien à animer.
    DejaInvisible,
    /// Déjà inscrite dans la liste des fenêtres masquées par nous (autre
    /// animation en cours : ouverture ou Jelly). Un second masquage
    /// écraserait l'état d'origine mémorisé par le premier, et la fenêtre
    /// ne serait plus jamais restaurée correctement.
    DejaMasqueeParNous,
}

impl fmt::Display for Refus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refus::LayeredSansAttributs => {
                write!(f, "fenêtre layered sans attributs lisibles (UpdateLayeredWindow)")
            }
            Refus::DejaInvisible => {
                write!(f, "fenêtre déjà invisible (alpha nul ou quasi nul)")
            }
            Refus::DejaMasqueeParNous => {
                write!(f, "fenêtre déjà masquée par une autre animation en cours")
            }
        }
    }
}

/// Décide si `cible` peut être inscrite dans la liste des fenêtres
/// masquées, d'après les HWND déjà inscrits : refus
/// [`Refus::DejaMasqueeParNous`] si elle y figure déjà (jamais de
/// remplacement d'une inscription existante).
pub fn inscription_possible(
    inscrits: impl IntoIterator<Item = isize>,
    cible: isize,
) -> Result<(), Refus> {
    if inscrits.into_iter().any(|h| h == cible) {
        Err(Refus::DejaMasqueeParNous)
    } else {
        Ok(())
    }
}

/// Vérifie qu'une fenêtre peut être masquée sans risque, d'après son état
/// d'origine.
pub fn masquage_possible(origine: &EtatLayered) -> Result<(), Refus> {
    masquage_possible_avec(origine, 0)
}

/// Variante de [`masquage_possible`] pour un masquage à l'alpha
/// `alpha_pose` (effet Jelly : alpha 1, pour que la fenêtre reste
/// cliquable — une fenêtre layered à alpha 0 laisse passer les clics).
/// Refus [`Refus::DejaInvisible`] si la fenêtre est déjà layered avec
/// `LWA_ALPHA` et un alpha inférieur ou égal à `alpha_pose` (l'heuristique
/// de restauration « alpha posé = c'est nous » serait ambiguë).
/// `masquage_possible(o)` équivaut à `masquage_possible_avec(o, 0)`.
pub fn masquage_possible_avec(origine: &EtatLayered, alpha_pose: u8) -> Result<(), Refus> {
    if !origine.est_layered() {
        return Ok(());
    }
    match origine.attributs {
        None => Err(Refus::LayeredSansAttributs),
        Some(a) if a.drapeaux & LWA_ALPHA != 0 && a.alpha <= alpha_pose => {
            Err(Refus::DejaInvisible)
        }
        Some(_) => Ok(()),
    }
}

/// Pourquoi aucune restauration n'est faite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisonAucune {
    /// La fenêtre n'existe plus.
    FenetreDisparue,
    /// `WS_EX_LAYERED` a été retiré par un tiers : la fenêtre est visible.
    LayeredRetireParTiers,
    /// Attributs illisibles (un tiers est passé à `UpdateLayeredWindow`).
    AttributsIllisibles,
    /// Un tiers a changé l'alpha (module de transparence, profils, appli).
    ModifieParTiers { alpha: u8, drapeaux: u32 },
}

/// Action de restauration à effectuer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// La fenêtre était layered : on remet ses attributs d'origine.
    RestaurerAttributs(Attributs),
    /// La fenêtre n'était pas layered : on retire `WS_EX_LAYERED` en
    /// écrivant ce style étendu (style COURANT sans `WS_EX_LAYERED`, afin
    /// de ne pas annuler d'autres changements de style faits entre-temps
    /// par l'application elle-même, ex. `WS_EX_TOPMOST`).
    RetirerLayered { exstyle: isize },
    /// Rien à faire.
    Aucune(RaisonAucune),
}

/// Décide de la restauration d'après l'état d'origine (mémorisé avant le
/// masquage) et l'état courant (`None` = fenêtre disparue).
///
/// Règle : on ne restaure QUE si l'alpha courant est toujours 0 avec
/// `LWA_ALPHA` (donc l'état posé par nous) ; sinon un tiers a pris la main
/// et on ne touche à rien.
pub fn decision_restauration(origine: &EtatLayered, courant: Option<&EtatLayered>) -> Action {
    decision_restauration_avec(origine, courant, 0)
}

/// Variante de [`decision_restauration`] pour un masquage à l'alpha
/// `alpha_pose` : « c'est nous » si l'état courant porte `LWA_ALPHA` avec
/// exactement cet alpha. `decision_restauration(o, c)` équivaut à
/// `decision_restauration_avec(o, c, 0)`.
pub fn decision_restauration_avec(
    origine: &EtatLayered,
    courant: Option<&EtatLayered>,
    alpha_pose: u8,
) -> Action {
    let Some(courant) = courant else {
        return Action::Aucune(RaisonAucune::FenetreDisparue);
    };
    if !courant.est_layered() {
        return Action::Aucune(RaisonAucune::LayeredRetireParTiers);
    }
    let Some(attributs) = courant.attributs else {
        return Action::Aucune(RaisonAucune::AttributsIllisibles);
    };
    // alpha 0 : critère historique (`invisible`) inchangé. Branche morte en
    // production depuis la v0.4.5 (ouverture et Jelly posent alpha 1) ;
    // conservée volontairement (tests purs, compatibilité de l'API).
    let pose_par_nous = if alpha_pose == 0 {
        attributs.invisible()
    } else {
        attributs.drapeaux & LWA_ALPHA != 0 && attributs.alpha == alpha_pose
    };
    if !pose_par_nous {
        return Action::Aucune(RaisonAucune::ModifieParTiers {
            alpha: attributs.alpha,
            drapeaux: attributs.drapeaux,
        });
    }
    if origine.est_layered() {
        match origine.attributs {
            Some(a) => Action::RestaurerAttributs(a),
            // Cas exclu par `masquage_possible` ; par prudence on rend la
            // fenêtre opaque plutôt que de la laisser invisible.
            None => Action::RestaurerAttributs(Attributs {
                cle: 0,
                alpha: 255,
                drapeaux: LWA_ALPHA,
            }),
        }
    } else {
        Action::RetirerLayered {
            exstyle: courant.exstyle & !WS_EX_LAYERED,
        }
    }
}

/// Nombre maximal de tentatives de restauration d'une fenêtre masquée à un
/// alpha NON nul (effet Jelly ; ouverture depuis la v0.4.5). Chaque tentative comprend un réessai
/// immédiat ; après un échec persistant, la fenêtre est réinscrite dans la
/// liste des fenêtres masquées (avec son état d'origine) pour qu'une
/// tentative ultérieure (minuteur de réessai, arrêt, fin de session) la
/// rattrape. Au-delà de ce nombre : abandon journalisé (pas de boucle
/// infinie). Le chemin alpha 0 (ouverture jusqu'en v0.4.4) n'est pas
/// concerné.
pub const MAX_ESSAIS_RESTAURATION: u8 = 3;

/// Après l'échec persistant de la tentative numéro `essais` (1 = la
/// première), faut-il réinscrire la fenêtre pour un nouvel essai ? Jamais
/// pour un masquage à alpha 0 (chemin historique inchangé).
pub fn reinscrire_apres_echec(alpha_pose: u8, essais: u8) -> bool {
    alpha_pose != 0 && essais < MAX_ESSAIS_RESTAURATION
}

/// Vrai si l'état `courant` est encore celui posé par un masquage à
/// `alpha_pose` (layered, `LWA_ALPHA`, alpha identique) : la restauration
/// n'a pas pris effet.
pub fn encore_masquee(courant: Option<&EtatLayered>, alpha_pose: u8) -> bool {
    match courant {
        Some(c) if c.est_layered() => matches!(
            c.attributs,
            Some(a) if a.drapeaux & LWA_ALPHA != 0 && a.alpha == alpha_pose
        ),
        _ => false,
    }
}

/// Identité d'une fenêtre, mémorisée à l'inscription dans la liste des
/// fenêtres masquées pour détecter un HWND recyclé (fenêtre détruite puis
/// handle réattribué à une autre fenêtre) avant toute restauration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identite {
    /// Identifiant du processus propriétaire.
    pub pid: u32,
    /// Identifiant du fil créateur de la fenêtre.
    pub tid: u32,
    /// Nom de classe Win32.
    pub classe: String,
}

impl fmt::Display for Identite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pid {}, tid {}, classe « {} »", self.pid, self.tid, self.classe)
    }
}

/// Vrai si l'identité courante est celle mémorisée à l'inscription : même
/// processus, même fil, même classe. Une différence signifie que le HWND a
/// été recyclé : il ne faut alors RIEN toucher.
pub fn meme_fenetre(inscrite: &Identite, courante: &Identite) -> bool {
    inscrite == courante
}

/// Durée (ms) pendant laquelle le suivi de fermeture continue d'ignorer
/// les images d'une fenêtre qui vient d'être démasquée : les images WGC
/// déjà en file ou livrées juste après la restauration peuvent encore
/// montrer l'état masqué.
pub const GRACE_DEMASQUAGE_MS: u64 = 300;

/// Vrai si une fenêtre démasquée à l'instant `marque_ms` (ms monotones,
/// 0 = emplacement vide) est encore dans la période de grâce à
/// `maintenant_ms`. Une horloge qui reculerait donne 0 d'écart (grâce).
pub fn en_grace(marque_ms: u64, maintenant_ms: u64) -> bool {
    marque_ms != 0 && maintenant_ms.saturating_sub(marque_ms) < GRACE_DEMASQUAGE_MS
}

#[cfg(windows)]
pub use win32::{
    est_masquee, etat_courant, masquee_sans_verrou, masquer_avec, reessayer_restaurations,
    restaurations_en_attente, restaurer, restaurer_tout, ResultatMasquage,
};

#[cfg(windows)]
mod win32 {
    use super::{
        decision_restauration, decision_restauration_avec, encore_masquee, inscription_possible,
        masquage_possible, masquage_possible_avec, meme_fenetre, reinscrire_apres_echec, Action,
        Attributs, EtatLayered, Identite, Refus, MAX_ESSAIS_RESTAURATION,
    };

    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};
    use std::time::Instant;

    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::Graphics::Dwm::DwmFlush;
    use windows::Win32::Graphics::Gdi::{
        RedrawWindow, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowThreadProcessId,
        IsWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE,
        LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
    };

    use crate::logging;

    /// Entrée de la liste des fenêtres masquées.
    struct Inscription {
        hwnd: isize,
        /// État layered d'origine (avant masquage).
        origine: EtatLayered,
        /// Identité à l'inscription (détection d'un HWND recyclé).
        identite: Identite,
        /// Alpha posé par le masquage (ouverture : `ALPHA_OUVERTURE` = 1
        /// depuis la v0.4.5, 0 avant ; effet Jelly : 1).
        alpha_pose: u8,
        /// Tentatives de restauration déjà échouées (0 = aucune ; > 0 :
        /// réinscrite après un échec, en attente d'un nouvel essai).
        essais: u8,
    }

    /// Issue d'une restauration.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Issue {
        /// Fenêtre modifiée (attendre la composition).
        Modifiee,
        /// Rien à faire (disparue, modifiée par un tiers...).
        Rien,
        /// Échec persistant (après un réessai immédiat) : la fenêtre reste
        /// masquée. Jamais renvoyé pour un masquage à alpha 0.
        Echec,
    }

    /// Fenêtres actuellement masquées par nous, avec leur état d'origine.
    /// Statique protégée par verrou (et non `thread_local!`) pour rester
    /// accessible depuis le crochet de panique (en `try_lock`).
    static MASQUEES: Mutex<Vec<Inscription>> = Mutex::new(Vec::new());

    /// Nombre d'emplacements du miroir sans verrou (au plus 4 ouvertures
    /// simultanées + 1 effet Jelly en pratique).
    const TAILLE_MIROIR: usize = 8;
    /// Miroir SANS VERROU des HWND de [`MASQUEES`], écrit uniquement sous
    /// le verrou de la liste (après chaque modification), lu par le
    /// gestionnaire d'images WGC (autre fil) via [`masquee_sans_verrou`].
    /// But : ne jamais faire prendre le verrou de `MASQUEES` à un autre fil
    /// (sinon `est_masquee` du fil principal, en `try_lock`, pourrait voir
    /// le verrou occupé et répondre « masquée » à tort).
    static MIROIR: [AtomicIsize; TAILLE_MIROIR] = [const { AtomicIsize::new(0) }; TAILLE_MIROIR];
    /// Plus d'inscriptions que d'emplacements : réponse prudente « masquée ».
    static MIROIR_DEBORDE: AtomicBool = AtomicBool::new(false);

    /// Recopie la liste dans le miroir (appelé verrou tenu). Écriture par
    /// index croissant, lecture ([`masquee_sans_verrou`]) par index
    /// DÉCROISSANT : un HWND présent avant et après la modification
    /// (déplacé par `swap_remove` vers un index inférieur) est écrit à son
    /// nouvel index avant que l'ancien soit effacé ; le lecteur, qui
    /// parcourt de la fin vers le début, le trouve donc toujours à l'un des
    /// deux endroits. (Un HWND retiré peut rester visible un court instant :
    /// sans conséquence, voir la période de grâce.)
    fn synchroniser_miroir(liste: &[Inscription]) {
        for (k, emplacement) in MIROIR.iter().enumerate() {
            emplacement.store(liste.get(k).map(|i| i.hwnd).unwrap_or(0), Ordering::SeqCst);
        }
        MIROIR_DEBORDE.store(liste.len() > TAILLE_MIROIR, Ordering::SeqCst);
    }

    /// Variante de [`est_masquee`] sans aucun verrou, utilisable depuis
    /// n'importe quel fil (gestionnaire d'images WGC). Peut être en retard
    /// d'une modification (sans conséquence : une image de plus ou de
    /// moins ignorée par le suivi de fermeture).
    pub fn masquee_sans_verrou(cible: isize) -> bool {
        if cible == 0 {
            return false;
        }
        // Ordre DÉCROISSANT : voir `synchroniser_miroir`.
        MIROIR_DEBORDE.load(Ordering::SeqCst)
            || MIROIR.iter().rev().any(|e| e.load(Ordering::SeqCst) == cible)
            || en_grace_recente(cible)
    }

    const TAILLE_RECENTES: usize = 4;
    /// Fenêtres récemment démasquées : HWND et instant (ms monotones + 1 ;
    /// 0 = vide), lus sans verrou par le fil WGC.
    static RECENTES_HWND: [AtomicIsize; TAILLE_RECENTES] =
        [const { AtomicIsize::new(0) }; TAILLE_RECENTES];
    static RECENTES_MS: [AtomicU64; TAILLE_RECENTES] =
        [const { AtomicU64::new(0) }; TAILLE_RECENTES];
    static RECENTES_SUIVANT: AtomicUsize = AtomicUsize::new(0);

    /// Millisecondes monotones depuis la première utilisation, plus 1
    /// (jamais 0).
    fn maintenant_ms() -> u64 {
        static BASE: OnceLock<Instant> = OnceLock::new();
        BASE.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
    }

    /// Note `cible` comme démasquée maintenant. À appeler AVANT son retrait
    /// du miroir (aucun intervalle sans protection). Instant écrit avant
    /// le HWND : au pire une grâce accordée à tort à un autre HWND.
    fn marquer_demasquee(cible: isize) {
        let k = (0..TAILLE_RECENTES)
            .find(|&k| RECENTES_HWND[k].load(Ordering::SeqCst) == cible)
            .unwrap_or_else(|| RECENTES_SUIVANT.fetch_add(1, Ordering::SeqCst) % TAILLE_RECENTES);
        RECENTES_MS[k].store(maintenant_ms(), Ordering::SeqCst);
        RECENTES_HWND[k].store(cible, Ordering::SeqCst);
    }

    fn en_grace_recente(cible: isize) -> bool {
        (0..TAILLE_RECENTES).any(|k| {
            RECENTES_HWND[k].load(Ordering::SeqCst) == cible
                && super::en_grace(RECENTES_MS[k].load(Ordering::SeqCst), maintenant_ms())
        })
    }

    /// Vrai si `cible` est actuellement masquée par une de nos animations
    /// (ouverture ou Jelly, alpha 1 temporaire ; inscription en attente
    /// d'un nouvel essai de restauration comprise). Utilisé par le module de
    /// transparence pour ne pas modifier une fenêtre pendant ce laps de
    /// temps (sinon il mémoriserait notre état temporaire comme état
    /// d'origine, et la fenêtre resterait layered/translucide).
    ///
    /// Ne bloque jamais (`try_lock`). Verrou empoisonné : la liste est lue
    /// quand même. Verrou indisponible (cas théorique : il n'est tenu que
    /// quelques instructions, sur le même fil) : renvoie `true` PAR
    /// PRUDENCE — le scanner de profils réessaie 2 s plus tard, et une
    /// action manuelle ignorée peut être refaite, alors qu'une modification
    /// pendant le masquage laisserait la fenêtre durablement altérée.
    pub fn est_masquee(cible: isize) -> bool {
        match MASQUEES.try_lock() {
            Ok(liste) => liste.iter().any(|i| i.hwnd == cible),
            Err(TryLockError::Poisoned(p)) => p.into_inner().iter().any(|i| i.hwnd == cible),
            Err(TryLockError::WouldBlock) => true,
        }
    }

    /// Lit l'identité courante (`None` si la fenêtre n'existe plus).
    unsafe fn lire_identite(cible: isize) -> Option<Identite> {
        let h = hwnd(cible);
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(h, Some(&mut pid));
        let mut tampon = [0u16; 256];
        let longueur = GetClassNameW(h, &mut tampon);
        let classe = if longueur > 0 {
            String::from_utf16_lossy(&tampon[..longueur as usize])
        } else {
            String::new()
        };
        Some(Identite { pid, tid, classe })
    }

    /// Vrai si la fenêtre peut être restaurée : identité inchangée, ou
    /// fenêtre disparue (dans ce cas `appliquer` ne fera rien et le
    /// journalisera). Faux si le HWND a été recyclé (journalisé si demandé).
    unsafe fn identite_verifiee(cible: isize, inscrite: &Identite, journaliser: bool) -> bool {
        match lire_identite(cible) {
            None => true,
            Some(courante) if meme_fenetre(inscrite, &courante) => true,
            Some(courante) => {
                if journaliser {
                    logging::avertir(&format!(
                        "Restauration de {cible:#x} ignorée : HWND recyclé (inscrit : {inscrite} ; \
                         courant : {courante}). Aucune modification."
                    ));
                }
                false
            }
        }
    }

    /// Résultat d'une tentative de masquage.
    pub enum ResultatMasquage {
        /// Fenêtre masquée (à l'alpha demandé), état d'origine mémorisé.
        Masquee(EtatLayered),
        /// Fenêtre volontairement non masquée (pas un échec de Windows).
        NonMasquable(Refus),
        /// Windows a refusé la modification (état d'origine déjà restauré).
        Echec(String),
    }

    fn hwnd(brut: isize) -> HWND {
        HWND(brut as *mut core::ffi::c_void)
    }

    /// Lit l'état layered courant (`None` si la fenêtre n'existe plus).
    unsafe fn lire_etat(cible: isize) -> Option<EtatLayered> {
        let h = hwnd(cible);
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let exstyle = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let mut etat = EtatLayered {
            exstyle,
            attributs: None,
        };
        if etat.est_layered() {
            let mut cle = COLORREF(0);
            let mut alpha = 0u8;
            let mut drapeaux = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            if GetLayeredWindowAttributes(h, Some(&mut cle), Some(&mut alpha), Some(&mut drapeaux))
                .is_ok()
            {
                etat.attributs = Some(Attributs {
                    cle: cle.0,
                    alpha,
                    drapeaux: drapeaux.0,
                });
            }
        }
        Some(etat)
    }

    /// Inscrit `cible` — test d'existence et insertion sous le MÊME verrou.
    /// Ne remplace JAMAIS une inscription existante : refus
    /// [`Refus::DejaMasqueeParNous`] (l'état d'origine mémorisé par la
    /// première animation doit rester intact).
    fn inscrire(
        cible: isize,
        origine: EtatLayered,
        identite: Identite,
        alpha_pose: u8,
    ) -> Result<(), Refus> {
        let mut liste = MASQUEES.lock().unwrap_or_else(PoisonError::into_inner);
        inscription_possible(liste.iter().map(|i| i.hwnd), cible)?;
        liste.push(Inscription {
            hwnd: cible,
            origine,
            identite,
            alpha_pose,
            essais: 0,
        });
        synchroniser_miroir(&liste);
        Ok(())
    }

    /// Réinscrit une fenêtre dont la restauration a échoué (alpha non nul
    /// uniquement), dans la limite de [`MAX_ESSAIS_RESTAURATION`]
    /// tentatives. Verrou en `try_lock` (appelable depuis le crochet de
    /// panique) ; jamais de remplacement d'une inscription existante.
    fn reinscrire(mut inscription: Inscription, journaliser: bool) {
        inscription.essais = inscription.essais.saturating_add(1);
        let cible = inscription.hwnd;
        let essais = inscription.essais;
        if !reinscrire_apres_echec(inscription.alpha_pose, essais) {
            if journaliser {
                logging::erreur(&format!(
                    "Restauration de {cible:#x} abandonnée après {essais} tentative(s) : la \
                     fenêtre peut rester quasi invisible (alpha {}). La fermer puis la rouvrir.",
                    inscription.alpha_pose
                ));
            }
            return;
        }
        let mut liste = match MASQUEES.try_lock() {
            Ok(l) => l,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if journaliser {
                    logging::erreur(&format!(
                        "Restauration de {cible:#x} en échec et liste occupée : non réinscrite."
                    ));
                }
                return;
            }
        };
        if inscription_possible(liste.iter().map(|i| i.hwnd), cible).is_err() {
            return;
        }
        liste.push(inscription);
        synchroniser_miroir(&liste);
        drop(liste);
        if journaliser {
            logging::avertir(&format!(
                "Restauration de {cible:#x} en échec (tentative {essais}/{MAX_ESSAIS_RESTAURATION}) \
                 : fenêtre réinscrite pour un nouvel essai."
            ));
        }
    }

    /// Nombre de fenêtres réinscrites après un échec de restauration, en
    /// attente d'un nouvel essai.
    pub fn restaurations_en_attente() -> usize {
        match MASQUEES.try_lock() {
            Ok(l) => l.iter().filter(|i| i.essais > 0).count(),
            Err(TryLockError::Poisoned(p)) => {
                p.into_inner().iter().filter(|i| i.essais > 0).count()
            }
            Err(TryLockError::WouldBlock) => 0,
        }
    }

    /// Nouvel essai de restauration des fenêtres réinscrites après un échec
    /// (fil de la boucle de messages). Renvoie le nombre de fenêtres encore
    /// en attente ensuite.
    pub fn reessayer_restaurations() -> usize {
        let cibles: Vec<isize> = match MASQUEES.try_lock() {
            Ok(l) => l.iter().filter(|i| i.essais > 0).map(|i| i.hwnd).collect(),
            Err(TryLockError::Poisoned(p)) => p
                .into_inner()
                .iter()
                .filter(|i| i.essais > 0)
                .map(|i| i.hwnd)
                .collect(),
            Err(TryLockError::WouldBlock) => return 0,
        };
        for cible in cibles {
            unsafe {
                let _ = restaurer(cible);
            }
        }
        restaurations_en_attente()
    }

    /// État layered courant de `cible` (`None` si la fenêtre n'existe plus).
    pub unsafe fn etat_courant(cible: isize) -> Option<EtatLayered> {
        lire_etat(cible)
    }

    fn desinscrire(cible: isize) -> Option<Inscription> {
        let mut liste = MASQUEES.lock().unwrap_or_else(PoisonError::into_inner);
        let pos = liste.iter().position(|i| i.hwnd == cible)?;
        marquer_demasquee(cible);
        let inscription = liste.swap_remove(pos);
        synchroniser_miroir(&liste);
        Some(inscription)
    }

    /// Masque `cible` par transparence temporaire à l'alpha `alpha`
    /// (ouverture : `ALPHA_OUVERTURE` = 1 depuis la v0.4.5 ; effet Jelly :
    /// 1, fenêtre cliquable). L'état d'origine est inscrit dans `MASQUEES`
    /// AVANT toute modification ; en cas d'échec, la fenêtre est restaurée
    /// et retirée de la liste avant de rendre la main. (La fonction
    /// historique `masquer`, alpha 0, a été retirée en v0.4.5 : plus
    /// aucun appelant.)
    pub unsafe fn masquer_avec(cible: isize, alpha: u8) -> ResultatMasquage {
        let Some(origine) = lire_etat(cible) else {
            return ResultatMasquage::Echec("fenêtre disparue avant le masquage".into());
        };
        // alpha 0 : chemin historique (plus utilisé depuis la v0.4.5).
        let possible = if alpha == 0 {
            masquage_possible(&origine)
        } else {
            masquage_possible_avec(&origine, alpha)
        };
        if let Err(refus) = possible {
            return ResultatMasquage::NonMasquable(refus);
        }
        let Some(identite) = lire_identite(cible) else {
            return ResultatMasquage::Echec("fenêtre disparue avant le masquage".into());
        };
        // Déjà masquée par une autre animation : refus, rien n'est modifié
        // (l'inscription existante et son état d'origine sont conservés).
        if let Err(refus) = inscrire(cible, origine, identite, alpha) {
            return ResultatMasquage::NonMasquable(refus);
        }
        let h = hwnd(cible);
        if !origine.est_layered() {
            let _ = SetWindowLongPtrW(h, GWL_EXSTYLE, origine.exstyle | super::WS_EX_LAYERED);
            // SetWindowLongPtrW ne signale pas toujours un refus de façon
            // fiable (UIPI, fenêtre qui filtre WM_STYLECHANGING) : on relit.
            let relu = GetWindowLongPtrW(h, GWL_EXSTYLE);
            if relu & super::WS_EX_LAYERED == 0 {
                let _ = desinscrire(cible);
                return ResultatMasquage::Echec(format!(
                    "WS_EX_LAYERED non appliqué (exstyle relu {relu:#x})"
                ));
            }
        }
        if let Err(e) = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA) {
            let message = format!(
                "SetLayeredWindowAttributes(alpha {alpha}) a échoué : {} (HRESULT {:#010x})",
                e.message(),
                e.code().0 as u32
            );
            // Restauration immédiate (l'alpha n'a pas été posé : on remet
            // simplement l'état d'origine).
            let _ = desinscrire(cible);
            if !origine.est_layered() {
                let courant = GetWindowLongPtrW(h, GWL_EXSTYLE);
                let _ = SetWindowLongPtrW(h, GWL_EXSTYLE, courant & !super::WS_EX_LAYERED);
            }
            return ResultatMasquage::Echec(message);
        }
        ResultatMasquage::Masquee(origine)
    }

    /// Applique la restauration décidée. Masquage à alpha 0 : chemin
    /// historique inchangé (jamais [`Issue::Echec`]). Alpha non nul (Jelly,
    /// ouverture depuis la v0.4.5) :
    /// un échec (`SetLayeredWindowAttributes` en erreur, ou fenêtre toujours
    /// à l'alpha posé après le retrait du style) est réessayé une fois, puis
    /// signalé par [`Issue::Echec`].
    unsafe fn appliquer(
        cible: isize,
        origine: &EtatLayered,
        alpha_pose: u8,
        journaliser: bool,
        conserver_layered: bool,
    ) -> Issue {
        let courant = lire_etat(cible);
        // alpha 0 : chemin historique inchangé (ouverture <= v0.4.4).
        let action = if alpha_pose == 0 {
            decision_restauration(origine, courant.as_ref())
        } else {
            decision_restauration_avec(origine, courant.as_ref(), alpha_pose)
        };
        let h = hwnd(cible);
        let tenter = |essai: u32| -> bool {
            match action {
                Action::RestaurerAttributs(a) => {
                    let r = SetLayeredWindowAttributes(
                        h,
                        COLORREF(a.cle),
                        a.alpha,
                        LAYERED_WINDOW_ATTRIBUTES_FLAGS(a.drapeaux),
                    );
                    if journaliser {
                        if let Err(e) = &r {
                            // Alpha 0 : message historique inchangé.
                            let essai_txt = if alpha_pose == 0 {
                                String::new()
                            } else {
                                format!(" (essai {essai})")
                            };
                            logging::avertir(&format!(
                                "Restauration de {cible:#x}{essai_txt} : \
                                 SetLayeredWindowAttributes a échoué : {} (HRESULT {:#010x})",
                                e.message(),
                                e.code().0 as u32
                            ));
                        }
                    }
                    r.is_ok()
                }
                Action::RetirerLayered { exstyle } => {
                    // Opaque d'abord (pas de trou si le retrait du style est
                    // lent), puis retrait du style et repeinture demandée.
                    let r255 = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
                    if conserver_layered {
                        // v0.4.3 : on NE retire PAS WS_EX_LAYERED. Le retrait
                        // fait recréer par DWM la surface de redirection de
                        // la fenêtre, vide jusqu'à ce que l'application se
                        // repeigne : flash sombre visible juste après la
                        // surimpression. Layered + alpha 255 est visuellement
                        // identique à non layered. Simple invalidation (sans
                        // effacement, sans attente de l'application).
                        let _ = RedrawWindow(
                            Some(h),
                            None,
                            None,
                            RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN,
                        );
                        if alpha_pose == 0 {
                            return true;
                        }
                        // Alpha non nul (Jelly) : vérification et réessais
                        // conservés (échec = fenêtre encore à l'alpha posé).
                        let ok = r255.is_ok()
                            && !encore_masquee(lire_etat(cible).as_ref(), alpha_pose);
                        if !ok && journaliser {
                            logging::avertir(&format!(
                                "Restauration de {cible:#x} (essai {essai}) : fenêtre toujours \
                                 à l'alpha {alpha_pose} (remise à 255 en échec)."
                            ));
                        }
                        return ok;
                    }
                    let _ = SetWindowLongPtrW(h, GWL_EXSTYLE, exstyle);
                    let _ = RedrawWindow(
                        Some(h),
                        None,
                        None,
                        RDW_ERASE | RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN,
                    );
                    if alpha_pose == 0 {
                        return true;
                    }
                    // Alpha non nul : vérification (style retiré OU fenêtre
                    // redevenue opaque suffit à la rendre visible).
                    let ok = !encore_masquee(lire_etat(cible).as_ref(), alpha_pose);
                    if !ok && journaliser {
                        logging::avertir(&format!(
                            "Restauration de {cible:#x} (essai {essai}) : fenêtre toujours à \
                             l'alpha {alpha_pose} après le retrait de WS_EX_LAYERED."
                        ));
                    }
                    ok
                }
                Action::Aucune(_) => false,
            }
        };
        let issue = match action {
            Action::Aucune(_) => Issue::Rien,
            _ if alpha_pose == 0 => {
                if tenter(1) {
                    Issue::Modifiee
                } else {
                    Issue::Rien
                }
            }
            _ => {
                if tenter(1) || tenter(2) {
                    Issue::Modifiee
                } else {
                    Issue::Echec
                }
            }
        };
        if journaliser {
            let courant_txt = courant
                .map(|c| c.to_string())
                .unwrap_or_else(|| "fenêtre disparue".into());
            logging::info(&format!(
                "Restauration de {cible:#x} : origine = {origine} ; courant = {courant_txt} ; \
                 action = {action:?}{}{}.",
                if conserver_layered && matches!(action, Action::RetirerLayered { .. }) {
                    " (v0.4.3 : WS_EX_LAYERED conservé, alpha 255, pour éviter le flash vide)"
                } else {
                    ""
                },
                if issue == Issue::Echec { " — ÉCHEC" } else { "" }
            ));
            if matches!(action, Action::Aucune(super::RaisonAucune::ModifieParTiers { .. })) {
                logging::info(&format!(
                    "Restauration de {cible:#x} ignorée : l'opacité a été modifiée entre-temps \
                     par un tiers (module de transparence / profils / application)."
                ));
            }
        }
        issue
    }

    /// Restaure `cible` si elle est masquée par nous (sinon ne fait rien et
    /// renvoie faux). Attend la composition DWM si la fenêtre a changé,
    /// pour éviter un clignotement quand l'appelant détruit ensuite la
    /// surimpression. Fil de la boucle de messages uniquement.
    ///
    /// Alpha non nul (Jelly) : en cas d'échec persistant, la fenêtre est
    /// réinscrite (voir [`MAX_ESSAIS_RESTAURATION`]) ; l'appelant peut
    /// consulter [`restaurations_en_attente`] pour programmer un réessai.
    pub unsafe fn restaurer(cible: isize) -> bool {
        restaurer_interne(cible)
    }

    unsafe fn restaurer_interne(cible: isize) -> bool {
        let Some(inscription) = desinscrire(cible) else {
            return false;
        };
        if !identite_verifiee(cible, &inscription.identite, true) {
            return true;
        }
        let origine = inscription.origine;
        match appliquer(cible, &origine, inscription.alpha_pose, true, true) {
            Issue::Modifiee => {
                // Une composition pour l'alpha (le style layered n'est plus
                // retiré : pas de nouvelle surface à attendre, v0.4.3).
                let _ = DwmFlush();
            }
            Issue::Rien => {}
            Issue::Echec => reinscrire(inscription, true),
        }
        true
    }

    /// Restaure toutes les fenêtres masquées par nous (arrêt, fin de
    /// session, panique). Verrou en `try_lock` : jamais d'interblocage si
    /// la panique survient pendant qu'il est tenu ; aucun `RefCell`.
    /// `journaliser = false` depuis le crochet de panique.
    pub fn restaurer_tout(journaliser: bool) {
        let liste: Vec<Inscription> = match MASQUEES.try_lock() {
            Ok(mut verrou) => {
                verrou.iter().for_each(|i| marquer_demasquee(i.hwnd));
                let liste = std::mem::take(&mut *verrou);
                synchroniser_miroir(&verrou);
                liste
            }
            Err(TryLockError::Poisoned(p)) => {
                let mut verrou = p.into_inner();
                verrou.iter().for_each(|i| marquer_demasquee(i.hwnd));
                let liste = std::mem::take(&mut *verrou);
                synchroniser_miroir(&verrou);
                liste
            }
            Err(TryLockError::WouldBlock) => return,
        };
        if journaliser && !liste.is_empty() {
            logging::avertir(&format!(
                "Restauration d'urgence de {} fenêtre(s) masquée(s) par transparence temporaire.",
                liste.len()
            ));
        }
        for inscription in liste {
            unsafe {
                if identite_verifiee(inscription.hwnd, &inscription.identite, journaliser)
                    && appliquer(
                        inscription.hwnd,
                        &inscription.origine,
                        inscription.alpha_pose,
                        journaliser,
                        false,
                    ) == Issue::Echec
                {
                    // Alpha non nul uniquement : nouvel essai possible plus
                    // tard (borné par MAX_ESSAIS_RESTAURATION).
                    reinscrire(inscription, journaliser);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn grace_demasquage() {
        // Emplacement vide : jamais de grâce.
        assert!(!en_grace(0, 10));
        // Dans la période.
        assert!(en_grace(1000, 1000));
        assert!(en_grace(1000, 1000 + GRACE_DEMASQUAGE_MS - 1));
        // Expirée.
        assert!(!en_grace(1000, 1000 + GRACE_DEMASQUAGE_MS));
        // Horloge qui recule : écart nul, donc grâce.
        assert!(en_grace(1000, 900));
    }

    use super::*;

    const EXSTYLE_NORMAL: isize = 0x0000_0100; // WS_EX_WINDOWEDGE

    fn non_layered() -> EtatLayered {
        EtatLayered {
            exstyle: EXSTYLE_NORMAL,
            attributs: None,
        }
    }

    fn layered(alpha: u8, drapeaux: u32) -> EtatLayered {
        EtatLayered {
            exstyle: EXSTYLE_NORMAL | WS_EX_LAYERED,
            attributs: Some(Attributs {
                cle: 0x00FF_00FF,
                alpha,
                drapeaux,
            }),
        }
    }

    /// État posé par notre masquage.
    fn masque(exstyle: isize) -> EtatLayered {
        EtatLayered {
            exstyle: exstyle | WS_EX_LAYERED,
            attributs: Some(Attributs {
                cle: 0,
                alpha: 0,
                drapeaux: LWA_ALPHA,
            }),
        }
    }

    #[test]
    fn non_layered_d_origine_retire_le_style() {
        let o = non_layered();
        assert_eq!(
            decision_restauration(&o, Some(&masque(o.exstyle))),
            Action::RetirerLayered {
                exstyle: EXSTYLE_NORMAL
            }
        );
    }

    #[test]
    fn retrait_conserve_les_autres_changements_de_style() {
        let o = non_layered();
        let topmost = 0x8;
        let c = masque(o.exstyle | topmost);
        assert_eq!(
            decision_restauration(&o, Some(&c)),
            Action::RetirerLayered {
                exstyle: EXSTYLE_NORMAL | topmost
            }
        );
    }

    #[test]
    fn layered_d_origine_restaure_les_attributs() {
        let o = layered(200, LWA_ALPHA);
        assert_eq!(
            decision_restauration(&o, Some(&masque(o.exstyle))),
            Action::RestaurerAttributs(o.attributs.unwrap())
        );
        // Clé de couleur seule : restaurée telle quelle.
        let o = layered(0, LWA_COLORKEY);
        assert_eq!(
            decision_restauration(&o, Some(&masque(o.exstyle))),
            Action::RestaurerAttributs(o.attributs.unwrap())
        );
    }

    #[test]
    fn alpha_modifie_par_un_tiers_rien() {
        let o = non_layered();
        let c = layered(204, LWA_ALPHA); // module de transparence : 80 %
        assert_eq!(
            decision_restauration(&o, Some(&c)),
            Action::Aucune(RaisonAucune::ModifieParTiers {
                alpha: 204,
                drapeaux: LWA_ALPHA
            })
        );
        // Alpha 0 mais sans LWA_ALPHA : ce n'est pas notre état.
        let c = layered(0, LWA_COLORKEY);
        assert!(matches!(
            decision_restauration(&o, Some(&c)),
            Action::Aucune(RaisonAucune::ModifieParTiers { .. })
        ));
    }

    #[test]
    fn fenetre_disparue_ou_style_retire_rien() {
        let o = non_layered();
        assert_eq!(
            decision_restauration(&o, None),
            Action::Aucune(RaisonAucune::FenetreDisparue)
        );
        assert_eq!(
            decision_restauration(&o, Some(&non_layered())),
            Action::Aucune(RaisonAucune::LayeredRetireParTiers)
        );
        let illisible = EtatLayered {
            exstyle: EXSTYLE_NORMAL | WS_EX_LAYERED,
            attributs: None,
        };
        assert_eq!(
            decision_restauration(&o, Some(&illisible)),
            Action::Aucune(RaisonAucune::AttributsIllisibles)
        );
    }

    #[test]
    fn masquage_possible_selon_origine() {
        assert_eq!(masquage_possible(&non_layered()), Ok(()));
        assert_eq!(masquage_possible(&layered(204, LWA_ALPHA)), Ok(()));
        assert_eq!(masquage_possible(&layered(0, LWA_COLORKEY)), Ok(()));
        assert_eq!(
            masquage_possible(&layered(0, LWA_ALPHA)),
            Err(Refus::DejaInvisible)
        );
        let ulw = EtatLayered {
            exstyle: EXSTYLE_NORMAL | WS_EX_LAYERED,
            attributs: None,
        };
        assert_eq!(masquage_possible(&ulw), Err(Refus::LayeredSansAttributs));
    }

    fn identite(pid: u32, tid: u32, classe: &str) -> Identite {
        Identite {
            pid,
            tid,
            classe: classe.to_string(),
        }
    }

    #[test]
    fn hwnd_recycle_detecte() {
        let inscrite = identite(1234, 5678, "Notepad");
        assert!(meme_fenetre(&inscrite, &identite(1234, 5678, "Notepad")));
        // Autre processus, autre fil ou autre classe : HWND recyclé.
        assert!(!meme_fenetre(&inscrite, &identite(4321, 5678, "Notepad")));
        assert!(!meme_fenetre(&inscrite, &identite(1234, 8765, "Notepad")));
        assert!(!meme_fenetre(&inscrite, &identite(1234, 5678, "CabinetWClass")));
        // La classe est comparée à l'identique (sensible à la casse).
        assert!(!meme_fenetre(&inscrite, &identite(1234, 5678, "notepad")));
        assert!(identite(1, 2, "X").to_string().contains("pid 1"));
    }

    /// État posé par un masquage à l'alpha `alpha` (effet Jelly : 1).
    fn masque_alpha(exstyle: isize, alpha: u8) -> EtatLayered {
        EtatLayered {
            exstyle: exstyle | WS_EX_LAYERED,
            attributs: Some(Attributs {
                cle: 0,
                alpha,
                drapeaux: LWA_ALPHA,
            }),
        }
    }

    #[test]
    fn alpha_pose_1_restaure() {
        let o = non_layered();
        assert_eq!(
            decision_restauration_avec(&o, Some(&masque_alpha(o.exstyle, 1)), 1),
            Action::RetirerLayered {
                exstyle: EXSTYLE_NORMAL
            }
        );
        let o = layered(200, LWA_ALPHA);
        assert_eq!(
            decision_restauration_avec(&o, Some(&masque_alpha(o.exstyle, 1)), 1),
            Action::RestaurerAttributs(o.attributs.unwrap())
        );
    }

    #[test]
    fn alpha_pose_1_tiers_alpha_0_rien() {
        let o = non_layered();
        assert_eq!(
            decision_restauration_avec(&o, Some(&masque_alpha(o.exstyle, 0)), 1),
            Action::Aucune(RaisonAucune::ModifieParTiers {
                alpha: 0,
                drapeaux: LWA_ALPHA
            })
        );
        assert!(matches!(
            decision_restauration_avec(&o, Some(&layered(204, LWA_ALPHA)), 1),
            Action::Aucune(RaisonAucune::ModifieParTiers { alpha: 204, .. })
        ));
    }

    #[test]
    fn possible_avec_alpha_pose() {
        assert_eq!(masquage_possible_avec(&non_layered(), 1), Ok(()));
        assert_eq!(masquage_possible_avec(&layered(204, LWA_ALPHA), 1), Ok(()));
        assert_eq!(masquage_possible_avec(&layered(2, LWA_ALPHA), 1), Ok(()));
        assert_eq!(
            masquage_possible_avec(&layered(1, LWA_ALPHA), 1),
            Err(Refus::DejaInvisible)
        );
        assert_eq!(
            masquage_possible_avec(&layered(0, LWA_ALPHA), 1),
            Err(Refus::DejaInvisible)
        );
        assert_eq!(masquage_possible_avec(&layered(0, LWA_COLORKEY), 1), Ok(()));
    }

    #[test]
    fn enveloppes_identiques() {
        let origines = [
            non_layered(),
            layered(204, LWA_ALPHA),
            layered(0, LWA_ALPHA),
            layered(1, LWA_ALPHA),
            layered(0, LWA_COLORKEY),
            EtatLayered {
                exstyle: EXSTYLE_NORMAL | WS_EX_LAYERED,
                attributs: None,
            },
        ];
        for o in &origines {
            assert_eq!(masquage_possible(o), masquage_possible_avec(o, 0));
            let courants = [
                None,
                Some(non_layered()),
                Some(masque(o.exstyle)),
                Some(masque_alpha(o.exstyle, 1)),
                Some(layered(204, LWA_ALPHA)),
                Some(layered(0, LWA_COLORKEY)),
            ];
            for c in &courants {
                assert_eq!(
                    decision_restauration(o, c.as_ref()),
                    decision_restauration_avec(o, c.as_ref(), 0)
                );
            }
        }
    }

    #[test]
    fn double_masquage_refuse() {
        // Liste vide ou autres fenêtres : inscription possible.
        assert_eq!(inscription_possible(Vec::<isize>::new(), 0x10), Ok(()));
        assert_eq!(inscription_possible([0x20, 0x30], 0x10), Ok(()));
        // Déjà inscrite (ex. Jelly à alpha 1 puis ouverture à alpha 0) :
        // refus, l'inscription existante n'est jamais remplacée.
        assert_eq!(
            inscription_possible([0x20, 0x10], 0x10),
            Err(Refus::DejaMasqueeParNous)
        );
        assert!(Refus::DejaMasqueeParNous.to_string().contains("déjà masquée"));
    }

    #[test]
    fn reinscription_bornee_et_jamais_pour_alpha_0() {
        // Alpha 0 (ouverture) : chemin historique, jamais de réinscription.
        for n in 0..=MAX_ESSAIS_RESTAURATION + 1 {
            assert!(!reinscrire_apres_echec(0, n));
        }
        // Alpha 1 (Jelly) : réinscrite tant que le nombre maximal n'est pas
        // atteint, puis abandon (pas de boucle infinie).
        assert!(reinscrire_apres_echec(1, 1));
        assert!(reinscrire_apres_echec(1, MAX_ESSAIS_RESTAURATION - 1));
        assert!(!reinscrire_apres_echec(1, MAX_ESSAIS_RESTAURATION));
        assert!(!reinscrire_apres_echec(1, u8::MAX));
        // Simulation : un échec permanent produit exactement MAX tentatives.
        let mut essais = 0u8;
        let mut tentatives = 0;
        loop {
            tentatives += 1;
            essais = essais.saturating_add(1);
            if !reinscrire_apres_echec(1, essais) {
                break;
            }
        }
        assert_eq!(tentatives, MAX_ESSAIS_RESTAURATION as usize);
    }

    #[test]
    fn encore_masquee_selon_etat() {
        let o = non_layered();
        assert!(encore_masquee(Some(&masque_alpha(o.exstyle, 1)), 1));
        // Opaque (alpha 255) ou style retiré : visible, restauration effective.
        assert!(!encore_masquee(Some(&masque_alpha(o.exstyle, 255)), 1));
        assert!(!encore_masquee(Some(&non_layered()), 1));
        assert!(!encore_masquee(None, 1));
        // Alpha différent posé par un tiers : ce n'est plus notre masquage.
        assert!(!encore_masquee(Some(&layered(204, LWA_ALPHA)), 1));
        assert!(!encore_masquee(Some(&layered(1, LWA_COLORKEY)), 1));
    }

    #[test]
    fn ouverture_a_l_alpha_1() {
        // v0.4.5 : l'ouverture masque à l'alpha 1.
        assert_eq!(ALPHA_OUVERTURE, 1);
        // Fenêtre déjà à alpha 0 ou 1 d'origine : jamais masquée.
        for a in [0u8, 1] {
            assert_eq!(
                masquage_possible_avec(&layered(a, LWA_ALPHA), ALPHA_OUVERTURE),
                Err(Refus::DejaInvisible)
            );
        }
        assert_eq!(masquage_possible_avec(&non_layered(), ALPHA_OUVERTURE), Ok(()));
        assert_eq!(
            masquage_possible_avec(&layered(255, LWA_ALPHA), ALPHA_OUVERTURE),
            Ok(())
        );
        // Restauration : l'état posé (alpha 1) est reconnu ; un état à
        // alpha 0 n'est PLUS pris pour le nôtre (tiers).
        let o = non_layered();
        assert_eq!(
            decision_restauration_avec(&o, Some(&masque_alpha(o.exstyle, ALPHA_OUVERTURE)), 1),
            Action::RetirerLayered {
                exstyle: EXSTYLE_NORMAL
            }
        );
        assert!(matches!(
            decision_restauration_avec(&o, Some(&masque(o.exstyle)), ALPHA_OUVERTURE),
            Action::Aucune(RaisonAucune::ModifieParTiers { alpha: 0, .. })
        ));
        // Échec de restauration à l'alpha 1 : réinscription bornée.
        assert!(reinscrire_apres_echec(ALPHA_OUVERTURE, 1));
        assert!(!reinscrire_apres_echec(ALPHA_OUVERTURE, MAX_ESSAIS_RESTAURATION));
        assert!(encore_masquee(Some(&masque_alpha(o.exstyle, ALPHA_OUVERTURE)), 1));
    }

    #[test]
    fn affichage_lisible() {
        assert!(non_layered().to_string().starts_with("non layered"));
        assert!(layered(10, LWA_ALPHA).to_string().contains("alpha 10"));
    }
}
