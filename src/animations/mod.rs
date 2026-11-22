//! Animations GPU d'ouverture / fermeture des fenêtres des autres applications.
//!
//! Chaîne complète (aucune injection, aucun rendu CPU / GDI) :
//! - détection : `SetWinEventHook` hors contexte ([`evenements`]) ;
//! - contenu : Windows.Graphics.Capture, textures D3D11 restant sur le GPU
//!   ([`capture`]) ;
//! - rendu : fenêtre de surimpression DirectComposition dont les propriétés
//!   sont animées par le compositeur ([`surimpression`]) ;
//! - courbes / plans : modules purs ([`courbes`], [`plan`]) ;
//! - masquage temporaire de la vraie fenêtre : [`masquage`] (pur + Win32) ;
//! - effet « Jelly » pendant le déplacement d'une fenêtre (v0.4.0) :
//!   [`jelly`] (crochet dédié, cycle de l'effet), [`ressort`],
//!   [`deformation`], [`grille_rendu`] et [`cadence`] (purs),
//!   [`maillage_gpu`] (rendu Direct3D 11 du maillage, v0.4.1) et
//!   [`metronome`] (fil de battement synchronisé sur le compositeur).
//!
//! **Fermeture** : une session WGC suit en continu la fenêtre éligible au
//! premier plan (`suivi`) ; quand elle est masquée / détruite, sa dernière
//! image est rejouée avec l'effet de fermeture. Les sessions des
//! [`RECENTS_MAX`] fenêtres précédemment actives sont conservées (`recents`,
//! LRU borné, ~10 i/s au plus chacune et seulement quand leur contenu
//! change) : à la fermeture, Windows peut activer la fenêtre suivante AVANT
//! de signaler le masquage de celle qui se ferme, et une fenêtre peut être
//! fermée depuis la barre des tâches sans être au premier plan.
//!
//! v0.3.2 (retour de test : fermeture seule inopérante) :
//! - les sessions de suivi sont créées de façon ASYNCHRONE par le fil de
//!   capture ([`capture::Atelier`]) : le fil principal ne peut plus être
//!   bloqué par WGC (en v0.3.1 il n'est jamais revenu de la création de la
//!   session du Bloc-notes : journal arrêté net, plus aucun traitement) ;
//! - une fenêtre au premier plan rejetée pour une raison passagère
//!   (invisible, réduite, cloakée...) est réessayée toutes les
//!   [`DELAI_REESSAI_MS`] ms, au plus [`MAX_REESSAIS`] fois, et
//!   `EVENT_OBJECT_SHOW` réévalue la fenêtre au premier plan même quand
//!   l'ouverture n'est pas animée ;
//! - journal détaillé du chemin de fermeture (HIDE/DESTROY reçus, session
//!   présente ou non, image disponible et son âge, changements de suivi).
//!
//! Règle HIDE / DESTROY (fenêtre suivie) : une fermeture normale produit
//! `EVENT_OBJECT_HIDE` puis `EVENT_OBJECT_DESTROY`. On anime dès le premier
//! des deux, SAUF un HIDE pour lequel la fenêtre existe encore ET est
//! réduite (`IsIconic`) ou de nouveau visible (`IsWindowVisible`) : c'est
//! un masquage parasite (réduction, masquage/réaffichage éclair), ignoré —
//! le suivi continue. Une fenêtre masquée sans être détruite (« réduire
//! dans la zone de notification ») est animée comme une fermeture : elle
//! disparaît bien de l'écran.
//!
//! **Ouverture** (v0.3.1, « capture d'abord, masquage ensuite ») : à
//! l'affichage, une session WGC dédiée démarre SANS toucher à la fenêtre ;
//! à la première image, la fenêtre est masquée par transparence temporaire
//! (layered, alpha 0 — voir [`masquage`]), la surimpression est posée et
//! l'effet joué, puis l'état d'origine est restauré exactement. La
//! restauration est garantie dans tous les chemins (fin, délai d'attente,
//! garde-fou, destruction, arrêt, fin de session, panique). Un clignotement
//! de 1 à 3 images (fenêtre visible pendant la latence de l'événement et de
//! la première image) est assumé.
//!
//! **Ouverture, masquage précoce** (v0.4.2, mode par défaut) : retour de
//! test v0.4.1, « double ouverture » (la vraie fenêtre apparaît, disparaît,
//! l'animation se joue, puis elle réapparaît) car la fenêtre restait visible
//! entre `EVENT_OBJECT_SHOW` et la 1re image. La fenêtre est désormais
//! masquée (alpha 0 ; alpha 1 depuis la v0.4.5, [`masquage::masquer_avec`])
//! DÈS le SHOW, AVANT la création
//! de la session de capture ; `OuvertureEnAttente::masquee` le mémorise et
//! tous les chemins d'abandon la restaurent. Comme on ignore si WGC applique
//! l'alpha layered à l'image capturée, l'image retenue est échantillonnée
//! ([`capture::analyser_trame`], décision pure dans [`attente`]) ; après
//! [`precoce::SEUIL_IMAGES_VIDES`] abandons sur image transparente d'une
//! fenêtre stable ([`precoce::vide_probante`]) (ou
//! [`precoce::SEUIL_DELAIS_EXPIRES`] attentes sans aucune image) consécutifs, retour
//! à l'ancien mode « capture d'abord » pour la session (réarmé en recochant
//! « Animer l'ouverture »).
//!
//! **v0.4.2 à v0.4.5 (approches abandonnées sur ce point)** : l'effet
//! était joué sur la 1re image WGC, puis la surimpression était
//! « maintenue » après l'effet jusqu'à un repaint détecté (v0.4.4/v0.4.5 :
//! signatures, coups de pouce de relayout, redimensionnement 1 px).
//!
//! **v0.4.6 — attente de l'image peinte** ([`attente`]) : prouvé par vidéo +
//! journal v0.4.5, la 1re image arrive 13-30 ms après le SHOW, AVANT le
//! 1er paint (image incomplète rejouée par l'effet, maintien jamais
//! convergé et surimpression tenue 1,6 s). Désormais, en masquage précoce,
//! la fenêtre reste masquée (alpha 1, elle se peint sous le masque) et la
//! session d'ouverture reste ouverte ; `MINUTEUR_ATTENTE` scrute la session
//! toutes les [`attente::TICK_ATTENTE_MS`] ms ([`examiner_ouverture`]) :
//! effet lancé avec l'image LA PLUS RÉCENTE dès qu'elle est stable (aucune
//! nouvelle image depuis [`attente::DELAI_STABILITE_MS`] ms,
//! [`attente::DELAI_STABILITE_PREMIERE_MS`] ms si une seule image) et non
//! vide ; au plafond [`attente::PLAFOND_ATTENTE_MS`] (depuis le SHOW) :
//! dernière image non vide -> effet joué avec elle (image uniforme mais
//! non transparente et stable : jouée aussi), sinon effet abandonné
//! et fenêtre restaurée immédiatement. Fin de l'effet : restauration
//! immédiate et levée instantanée de la surimpression (comme en v0.4.3).
//!
//! **v0.4.7 — transition native de DWM** ([`transitions`]) : vidéo 60 i/s +
//! journal v0.4.6, la vraie fenêtre apparaît en fondu ~50 ms après le SHOW
//! malgré le masquage alpha 1 posé à T+2..15 ms (« double ouverture »).
//! Hypothèse (non prouvée) : l'animation d'apparition de DWM ignore l'alpha
//! layered. `DWMWA_TRANSITIONS_FORCEDISABLED` est posé dès
//! `EVENT_OBJECT_CREATE` (crochet installé seulement si l'ouverture est
//! active et la clé cachée `desactiver_transition_dwm` vraie), de nouveau
//! au SHOW avant le masquage précoce, et rétabli sur tous les chemins
//! (fin d'ouverture, annulations, échec, poses orphelines, arrêt, fin de
//! session, panique). Sondes (visible / layered / alpha / cloak /
//! transitions) aux paliers [`transitions::PALIERS_SONDES_MS`] après le SHOW.

//! RÈGLE ANTI-RÉENTRANCE : l'état est un `RefCell` thread-local. On en
//! extrait toujours les objets nécessaires (`take` / `remove` / clone) sous
//! un emprunt COURT, on relâche l'emprunt, PUIS on appelle Win32 / WinRT
//! (DestroyWindow, SetLayeredWindowAttributes, Commit, PostMessage, ...) ou
//! `window::raison_exclusion_silencieuse`. Tous les emprunts passent par
//! `try_borrow(_mut)` : en cas d'échec on journalise et on ignore.
//! Ce module n'appelle jamais `window::avec_etat`.

mod attente;
mod cadence;
mod capture;
mod catalogue;
mod courbes;
mod d3d_commun;
mod deformation;
mod effet_gpu;
mod effets_shader;
mod eligibilite;
mod erreur;
mod evenements;
mod gpu;
mod grille_rendu;
mod hlsl_effets;
mod jelly;
mod maillage_gpu;
mod masquage;
mod metronome;
mod plan;
mod politique;
mod precoce;
mod ressort;
mod surimpression;
mod transitions;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub use catalogue::EffetAnimation;
pub use jelly::{jelly_actif, jelly_desactive};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetAncestor, GetForegroundWindow, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, KillTimer, RegisterClassW, SendMessageTimeoutW, SetTimer,
    GA_ROOT, HWND_MESSAGE, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_APP, WM_NULL, WM_TIMER,
    WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

use capture::{RefusAtelier, SessionCapture};
use erreur::ErreurAnimation;
use evenements::{Crochets, Evenement};
use gpu::ContexteGpu;
use attente::{Action, Attente, ContenuImage, ModeAttente, Observation, RaisonAbandon, RaisonChoix};
pub use catalogue::Moteur;
use effet_gpu::{EchecEffet, EtatRendu};
use masquage::{ResultatMasquage, ALPHA_OUVERTURE};
use metronome::Metronome;
use plan::{Plan, Sens, Taille};
use politique::{Disparition, Lru, MemoireBornee};
use precoce::{Adaptation, Mode, RaisonBascule};
use surimpression::AnimationActive;
use transitions::{texte_lecture, Moment, Sondes};

use crate::config::Config;
use crate::logging;
use crate::tray;
use crate::window;

/// Message posté par le rappel WinEvent : des événements attendent en file.
pub(crate) const WM_EVENEMENTS: u32 = WM_APP + 20;
/// Message posté par le fil WGC : première image disponible (wparam = id).
pub(crate) const WM_TRAME_PRETE: u32 = WM_APP + 21;
/// Message posté par le fil de capture : session(s) de suivi créée(s)
/// (résultats relevés par `capture::recuperer_resultats`).
pub(crate) const WM_SESSION_CREEE: u32 = WM_APP + 22;
/// Message posté par le fil de battement des effets à shader (v0.4.7) :
/// une image composée par DWM (wparam = génération du fil).
const WM_EFFET_BATTEMENT: u32 = WM_APP + 23;

/// Nombre maximal d'animations simultanées (en attente d'image + en cours).
const MAX_SIMULTANEES: usize = 4;
/// Garde-fou de l'attente d'une ouverture (depuis le SHOW) et durée
/// maximale d'une fermeture, avant restauration / destruction forcée.
/// Couvre le plafond d'attente [`attente::PLAFOND_ATTENTE_MS`].
const GARDE_FOU_MS: u32 = 1500;
const _: () = assert!(attente::PLAFOND_ATTENTE_MS + 250 < GARDE_FOU_MS as u64);
/// Garde-fou de l'effet d'ouverture une fois lancé : durée de l'effet +
/// [`MARGE_FIN_MS`] + cette marge.
const MARGE_GARDE_EFFET_MS: u32 = 500;
/// Marge ajoutée à la durée de l'effet avant de détruire la surimpression.
const MARGE_FIN_MS: u32 = 80;
/// Sessions de suivi conservées pour les fenêtres récemment actives (en
/// plus de celle du premier plan) : 3 sessions au plus au total.
const RECENTS_MAX: usize = 2;
/// Réessai d'une fenêtre au premier plan rejetée pour une raison passagère.
const DELAI_REESSAI_MS: u32 = 150;
const MAX_REESSAIS: u32 = 10;
/// Surveillance d'une création de session en cours sur le fil de capture.
const DELAI_SURVEILLANCE_MS: u32 = 2000;
/// Alertes « création bloquée » au plus par création.
const MAX_ALERTES_BLOCAGE: u32 = 5;
/// Fenêtres « vues » (éligibles ou rejetées au premier plan) pour lesquelles
/// un HIDE/DESTROY sans session est journalisé (une fois chacune).
const MEMOIRE_VUES: usize = 64;
/// Échecs consécutifs du masquage avant désactivation de l'ouverture.
const SEUIL_ECHECS_MASQUAGE: u32 = 3;
/// Nombre de rejets mémorisés pour éviter de les journaliser en boucle.
const MEMOIRE_REJETS: usize = 64;
/// Anti-rebond : un SHOW du même HWND moins de ce délai après la
/// restauration d'une ouverture précoce est ignoré pour l'ouverture.
const REBOND_OUVERTURE_MS: u128 = 500;
/// Taille maximale de la table des dernières restaurations précoces.
const MEMOIRE_REBOND: usize = 16;
/// Délai de création de la session de capture en mode précoce (fenêtre
/// invisible pendant ce temps au plus).
const DELAI_SESSION_PRECOCE_MS: u64 = 400;
/// Délai de réponse exigé de la fenêtre avant son masquage précoce.
const DELAI_REPONSE_MS: u32 = 50;

/// Genres de minuteurs : identifiant Win32 = id d'animation fois
/// [`NB_GENRES`], plus le genre (8 depuis la v0.4.7 : place pour
/// [`MINUTEUR_TRANSITIONS`] sans jamais utiliser l'identifiant 0).
const NB_GENRES: usize = 8;
const MINUTEUR_FIN: usize = 0;
const MINUTEUR_ATTENTE: usize = 1;
const MINUTEUR_GARDE: usize = 2;
/// Minuteurs globaux (id 0 : les animations commencent à 1).
/// Surveillance du fil de capture.
const MINUTEUR_SURVEILLANCE: usize = 2;
/// Réessai d'éligibilité de la fenêtre au premier plan.
const MINUTEUR_REESSAI: usize = 3;
/// Nouvel essai des restaurations d'ouverture en échec (fenêtres masquées
/// à l'alpha 1 réinscrites par `masquage`, v0.4.5).
const MINUTEUR_RESTAURATIONS: usize = 1;
/// Période de ce nouvel essai.
const PERIODE_RESTAURATIONS_MS: u32 = 500;
/// Balayage des poses `DWMWA_TRANSITIONS_FORCEDISABLED` orphelines (v0.4.7,
/// toutes les [`transitions::PERIODE_BALAYAGE_MS`] ms tant que le registre
/// n'est pas vide).
const MINUTEUR_TRANSITIONS: usize = 4;
const _: () = assert!(MINUTEUR_TRANSITIONS < NB_GENRES);
/// Battement de secours des effets à shader (v0.4.7) si le fil de
/// battement ne démarre pas : reprogrammé à chaque tick.
const MINUTEUR_BATTEMENT_SECOURS: usize = 5;
const _: () = assert!(MINUTEUR_BATTEMENT_SECOURS < NB_GENRES);
/// Période du battement de secours (ms).
const PERIODE_BATTEMENT_SECOURS_MS: u32 = 15;

/// Réglages courants des animations.
#[derive(Debug, Clone, Copy)]
struct Reglages {
    effet: EffetAnimation,
    ouverture: bool,
    fermeture: bool,
    /// Clé cachée `desactiver_transition_dwm` (v0.4.7).
    sans_transition_dwm: bool,
}

impl Reglages {
    fn fermeture_active(&self) -> bool {
        self.fermeture && self.effet != EffetAnimation::Aucun
    }
    fn ouverture_active(&self) -> bool {
        self.ouverture && self.effet != EffetAnimation::Aucun
    }
}

/// Disponibilité du contexte GPU (créé paresseusement).
enum Disponibilite {
    NonInitialise,
    Prete(ContexteGpu),
    Indisponible(ErreurAnimation),
}

/// Cadence du rendu des effets à shader (v0.4.7).
enum Battement {
    /// Fil synchronisé sur l'horloge du compositeur ([`metronome`]).
    Fil(Metronome),
    /// Repli : minuteur `SetTimer` de [`PERIODE_BATTEMENT_SECOURS_MS`] ms.
    Minuteur,
}

/// Ouverture en attente de l'image peinte (v0.4.6, voir [`attente`]).
struct OuvertureEnAttente {
    cible: isize,
    /// Session WGC d'ouverture : conserve l'image la plus récente (aucune
    /// copie avant la décision).
    session: SessionCapture,
    /// Vrai si la fenêtre a été masquée (alpha [`ALPHA_OUVERTURE`]) par CETTE
    /// ouverture dès `EVENT_OBJECT_SHOW` (mode précoce) : tout abandon doit
    /// la restaurer.
    masquee: bool,
    /// Instant du traitement de `EVENT_OBJECT_SHOW` (plafond, journal).
    debut: Instant,
    /// État pur de l'attente (dernière image analysée).
    attente: Attente,
    /// Sondes de l'hypothèse « transition DWM » (v0.4.7).
    sondes: Sondes,
}

/// État du module (fil de la boucle de messages uniquement).
struct EtatAnimations {
    reglages: Reglages,
    gpu: Disponibilite,
    /// Vrai si le contexte GPU a déjà été recréé une fois après une perte.
    gpu_recree: bool,
    /// Fenêtre de contrôle message-only (0 = pas encore créée).
    controle: isize,
    crochets: Option<Crochets>,
    /// Suivi de la fenêtre au premier plan (fermeture animée).
    suivi: Option<SessionCapture>,
    /// Sessions des fenêtres récemment actives (LRU borné).
    recents: Lru<SessionCapture>,
    /// Créations asynchrones en cours (jeton -> fenêtre, début).
    en_creation: HashMap<u64, (isize, Instant)>,
    /// Jeton de la création destinée à devenir `suivi` (fenêtre au premier
    /// plan) ; les autres résultats rejoignent `recents`.
    demande: Option<u64>,
    prochain_jeton: u64,
    /// Réessai programmé : (fenêtre, réessais déjà effectués).
    reessai: Option<(isize, u32)>,
    /// Fenêtres vues au premier plan (anti-spam du journal HIDE/DESTROY).
    vues: MemoireBornee,
    /// Alertes « création bloquée » : (jeton, nombre déjà journalisé).
    alertes_blocage: (u64, u32),
    ouvertures: HashMap<u32, OuvertureEnAttente>,
    actives: HashMap<u32, AnimationActive>,
    /// Ouverture animée désactivée pour la session après
    /// [`SEUIL_ECHECS_MASQUAGE`] échecs consécutifs du masquage par
    /// transparence temporaire (réarmée en recochant « Animer l'ouverture »).
    ouverture_desactivee: bool,
    /// Échecs consécutifs du masquage (un succès remet à 0).
    echecs_masquage: u32,
    /// Dernières restaurations d'ouvertures précoces (HWND, instant) :
    /// anti-rebond des SHOW provoqués par la restauration.
    restaurations: VecDeque<(isize, Instant)>,
    /// Mode d'ouverture (masquage précoce / capture d'abord) et compteurs
    /// d'échecs du mode précoce ; « masquage précoce inutilisable » =
    /// `adaptation.mode() == Mode::CaptureDabord`. Réarmé en recochant
    /// « Animer l'ouverture ».
    adaptation: Adaptation,
    prochain_id: u32,
    /// Derniers rejets journalisés (anti-spam).
    rejets: VecDeque<(isize, String)>,
    /// Lignes « transition DWM désactivée à la création » déjà journalisées.
    creations_journalisees: u32,
    /// Vrai si `MINUTEUR_TRANSITIONS` est programmé.
    balayage_programme: bool,
    /// Cadence du rendu des effets à shader (présente seulement tant
    /// qu'une animation à shader a des images à rendre).
    battement: Option<Battement>,
    /// Génération du dernier fil de battement (messages périmés ignorés).
    generation_battement: u32,
    /// Effets à shader dont l'indisponibilité a déjà été notifiée (une
    /// notification par effet et par session).
    effets_notifies: Vec<EffetAnimation>,
    /// Effets dont le repli « Fondu » (effet indisponible) a déjà été
    /// journalisé (une ligne par effet et par session).
    replis_journalises: Vec<EffetAnimation>,
}

impl EtatAnimations {
    fn nouveau(reglages: Reglages) -> Self {
        EtatAnimations {
            reglages,
            gpu: Disponibilite::NonInitialise,
            gpu_recree: false,
            controle: 0,
            crochets: None,
            suivi: None,
            recents: Lru::new(RECENTS_MAX),
            en_creation: HashMap::new(),
            demande: None,
            prochain_jeton: 1,
            reessai: None,
            vues: MemoireBornee::new(MEMOIRE_VUES),
            alertes_blocage: (0, 0),
            ouvertures: HashMap::new(),
            actives: HashMap::new(),
            ouverture_desactivee: false,
            echecs_masquage: 0,
            restaurations: VecDeque::with_capacity(MEMOIRE_REBOND),
            adaptation: Adaptation::default(),
            prochain_id: 1,
            rejets: VecDeque::with_capacity(MEMOIRE_REJETS),
            creations_journalisees: 0,
            balayage_programme: false,
            battement: None,
            generation_battement: 0,
            effets_notifies: Vec::new(),
            replis_journalises: Vec::new(),
        }
    }
}

/// Sessions de suivi extraites de l'état (à libérer HORS emprunt).
type SuivisExtraits = (Option<SessionCapture>, Vec<SessionCapture>);

impl EtatAnimations {
    /// Retire toutes les sessions de suivi et oublie les créations en
    /// cours (leurs résultats seront libérés à l'arrivée).
    fn extraire_suivis(&mut self) -> SuivisExtraits {
        self.en_creation.clear();
        self.demande = None;
        self.reessai = None;
        (self.suivi.take(), self.recents.vider())
    }

    /// Jeton d'une création en cours pour `h`. Les créations plus anciennes
    /// que `capture::SEUIL_BLOCAGE_MS` sont oubliées (fil de capture bloqué,
    /// relancé ou non) : une nouvelle demande pourra être faite ; leur
    /// résultat éventuel sera libéré à l'arrivée.
    fn creation_pour(&mut self, h: isize) -> Option<u64> {
        let perimees: Vec<u64> = self
            .en_creation
            .iter()
            .filter(|(_, (f, debut))| {
                *f == h && debut.elapsed().as_millis() as u64 >= capture::SEUIL_BLOCAGE_MS
            })
            .map(|(j, _)| *j)
            .collect();
        for j in perimees {
            self.en_creation.remove(&j);
            if self.demande == Some(j) {
                self.demande = None;
            }
        }
        self.en_creation
            .iter()
            .find(|(_, (f, _))| *f == h)
            .map(|(j, _)| *j)
    }

    /// Texte « suivi=…, récentes=[…], en création=[…] » pour le journal.
    fn texte_suivis(&self) -> String {
        let suivi = self
            .suivi
            .as_ref()
            .map(|c| format!("{:#x}", c.hwnd))
            .unwrap_or_else(|| "aucun".into());
        let recents: Vec<String> = self.recents.cles().iter().map(|h| format!("{h:#x}")).collect();
        let mut creation: Vec<String> = self
            .en_creation
            .iter()
            .map(|(j, (h, _))| {
                format!("{h:#x}{}", if self.demande == Some(*j) { "*" } else { "" })
            })
            .collect();
        creation.sort();
        format!(
            "suivi={suivi}, récentes=[{}], en création=[{}]",
            recents.join(", "),
            creation.join(", ")
        )
    }
}

thread_local! {
    static ETAT: RefCell<Option<EtatAnimations>> = const { RefCell::new(None) };
    /// WinRT initialisé et classe de surimpression enregistrée.
    static INFRA_PRETE: Cell<bool> = const { Cell::new(false) };
    /// Crochets qui n'ont pas pu être remis dans l'état parce qu'il était
    /// emprunté (audit v0.4.7) : ils restent installés ici et sont repris
    /// au prochain [`reevaluer`] ou traitement d'événements (jetés à l'arrêt).
    static CROCHETS_EN_SUSPENS: RefCell<Option<Crochets>> = const { RefCell::new(None) };
}

/// Remet des crochets dans l'état. État absent (arrêt) : ils sont détruits
/// (crochets retirés). État occupé : mis en suspens (jamais perdus). État
/// qui possède déjà des crochets : les nouveaux sont détruits.
fn ranger_crochets(c: Crochets) {
    let mut tenue = Some(c);
    let occupe = ETAT.with(|cellule| match cellule.try_borrow_mut() {
        Ok(mut g) => {
            if let Some(s) = g.as_mut() {
                if s.crochets.is_none() {
                    s.crochets = tenue.take();
                }
            }
            false
        }
        Err(_) => true,
    });
    if let Some(c) = tenue {
        if occupe {
            logging::avertir("Animations : état occupé, crochets mis en suspens (repris plus tard).");
            let reste = CROCHETS_EN_SUSPENS.with(|e| match e.try_borrow_mut() {
                Ok(mut e) => e.replace(c),
                Err(_) => Some(c),
            });
            drop(reste);
        } else {
            drop(c);
        }
    }
}

/// Reprend les crochets mis en suspens par [`ranger_crochets`].
fn reprendre_crochets_en_suspens() {
    let suspens = CROCHETS_EN_SUSPENS.with(|e| e.try_borrow_mut().ok().and_then(|mut e| e.take()));
    if let Some(c) = suspens {
        logging::info("Animations : crochets en suspens repris.");
        ranger_crochets(c);
    }
}

/// Détruit les crochets en suspens (arrêt, indisponibilité).
fn jeter_crochets_en_suspens() {
    let suspens = CROCHETS_EN_SUSPENS.with(|e| e.try_borrow_mut().ok().and_then(|mut e| e.take()));
    drop(suspens);
}

/// Exécute `f` sur l'état sous un emprunt court (jamais de panique :
/// `None` si l'état est absent ou déjà emprunté).
fn avec<R>(f: impl FnOnce(&mut EtatAnimations) -> R) -> Option<R> {
    ETAT.with(|cellule| match cellule.try_borrow_mut() {
        Ok(mut garde) => garde.as_mut().map(f),
        Err(_) => {
            logging::avertir("Animations : état déjà emprunté (réentrance évitée).");
            None
        }
    })
}

/// Extrait les réglages d'une configuration (effet invalide -> `Aucun`).
fn reglages_depuis(config: &Config) -> Reglages {
    Reglages {
        effet: EffetAnimation::depuis_nom(&config.effet_fenetre).unwrap_or(EffetAnimation::Aucun),
        ouverture: config.effet_ouverture,
        fermeture: config.effet_fermeture,
        sans_transition_dwm: config.desactiver_transition_dwm,
    }
}

/// Lit les réglages courants (valeurs par défaut si non initialisé).
fn lire() -> Reglages {
    let defaut = Reglages {
        effet: EffetAnimation::Zoom,
        ouverture: false,
        fermeture: false,
        sans_transition_dwm: true,
    };
    ETAT.with(|e| match e.try_borrow() {
        Ok(g) => g.as_ref().map(|s| s.reglages).unwrap_or(defaut),
        Err(_) => defaut,
    })
}

/// Remplace les réglages (crée l'état si nécessaire, conserve le reste).
fn ecrire(reglages: Reglages) {
    ETAT.with(|e| match e.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(etat) => etat.reglages = reglages,
            None => *g = Some(EtatAnimations::nouveau(reglages)),
        },
        Err(_) => logging::avertir("Animations : réglages non appliqués (état occupé)."),
    });
}

/// Initialise le module depuis la configuration chargée.
pub fn initialiser(config: &Config) {
    ecrire(reglages_depuis(config));
    reevaluer("initialisation");
    jelly::appliquer_config(config, "initialisation");
}

/// Applique une configuration rechargée (menu / fenêtre de paramètres).
pub fn appliquer_config(config: &Config) {
    ecrire(reglages_depuis(config));
    reevaluer("configuration rechargée");
    jelly::appliquer_config(config, "configuration rechargée");
}

/// Animation d'ouverture activée ?
pub fn ouverture_active() -> bool {
    lire().ouverture
}

/// Réarmements explicites demandés à la validation de la fenêtre de
/// paramètres (équivalents de ceux de l'ancien menu), à appeler AVANT
/// [`appliquer_config`] (qui réévalue ensuite l'infrastructure) :
/// - `effet_choisi` : l'utilisateur a choisi un effet dans la liste ->
///   les échecs mémorisés des effets à shader sont oubliés (ancien
///   `definir_effet` : compilation retentée au prochain préchauffage) ;
/// - `ouverture` : « Animer l'ouverture » (ré)activée -> l'ouverture
///   désactivée après des échecs de masquage est réarmée et le mode
///   adaptatif réinitialisé (ancien `basculer_ouverture`) ;
/// - `jelly` : « Activer Jelly » (ré)activé -> Jelly réarmé (ancien
///   `basculer_jelly`).
///
/// Aucune écriture de configuration ici (la fenêtre de paramètres écrit
/// `config.toml` elle-même, une seule fois).
pub fn rearmer(effet_choisi: bool, ouverture: bool, jelly: bool) {
    if effet_choisi {
        for e in EffetAnimation::TOUS {
            if e.moteur() == Moteur::Shader {
                effet_gpu::oublier_echec(e);
            }
        }
    }
    if ouverture {
        avec(|s| {
            s.ouverture_desactivee = false;
            s.echecs_masquage = 0;
            s.adaptation.reinitialiser();
        });
    }
    if jelly {
        jelly::rearmer();
    }
    if effet_choisi || ouverture || jelly {
        logging::info(&format!(
            "Animations : réarmement demandé par les paramètres (effet={}, ouverture={}, jelly={}).",
            oui_non(effet_choisi),
            oui_non(ouverture),
            oui_non(jelly)
        ));
    }
}

/// Raison pour laquelle les animations sont indisponibles (`None` = OK ou
/// pas encore initialisées).
pub fn indisponibilite() -> Option<String> {
    ETAT.with(|e| match e.try_borrow() {
        Ok(g) => match g.as_ref().map(|s| &s.gpu) {
            Some(Disponibilite::Indisponible(err)) => Some(err.to_string()),
            _ => None,
        },
        Err(_) => None,
    })
}

/// Vrai si l'effet peut être rendu (le flou exige `IDCompositionDevice3` ;
/// un effet à shader ne doit pas être connu indisponible). Tant que le GPU
/// n'est pas initialisé, on répond « oui ».
pub fn effet_disponible(effet: EffetAnimation) -> bool {
    if effet.moteur() == Moteur::Shader {
        return !effet_gpu::est_indisponible(effet);
    }
    if !effet.exige_device3() {
        return true;
    }
    ETAT.with(|e| match e.try_borrow() {
        Ok(g) => match g.as_ref().map(|s| &s.gpu) {
            Some(Disponibilite::Prete(gpu)) => gpu.dcomp3.is_some(),
            _ => true,
        },
        Err(_) => true,
    })
}

// ---------------------------------------------------------------------------
// Journalisation
// ---------------------------------------------------------------------------

fn oui_non(v: bool) -> &'static str {
    if v {
        "oui"
    } else {
        "non"
    }
}

/// Une ligne de synthèse de l'état du module.
fn journaliser_etat(contexte: &str) {
    let ligne = avec(|s| {
        let gpu = match &s.gpu {
            Disponibilite::NonInitialise => "non initialisé".to_string(),
            Disponibilite::Prete(g) => format!("prêt (flou {})", oui_non(g.dcomp3.is_some())),
            Disponibilite::Indisponible(e) => format!("indisponible ({e})"),
        };
        format!(
            "Animations ({contexte}) : effet={}, ouverture={}{} [mode {}], fermeture={}, \
             GPU={gpu}, crochets={}, {} ; transitions DWM : désactivation={}, crochet \
             création={}, {} ; {}.",
            s.reglages.effet.nom_config(),
            oui_non(s.reglages.ouverture),
            if s.ouverture_desactivee {
                " (désactivée : masquage par transparence temporaire impossible)"
            } else {
                ""
            },
            texte_mode(&s.adaptation),
            oui_non(s.reglages.fermeture),
            if s.crochets.is_some() {
                "installés"
            } else {
                "absents"
            },
            s.texte_suivis(),
            oui_non(s.reglages.sans_transition_dwm),
            oui_non(s.crochets.as_ref().is_some_and(|c| c.creations_actives())),
            transitions::compteurs().texte(),
            evenements::compteurs_creations().texte(),
        )
    });
    if let Some(l) = ligne {
        logging::info(&l);
    }
}

/// Mode d'ouverture lisible pour le journal.
fn texte_mode(a: &Adaptation) -> String {
    let (vides, delais) = a.compteurs();
    match a.bascule() {
        None => format!(
            "masquage précoce (images vides probantes consécutives {vides}/{}, attentes expirées \
             consécutives {delais}/{})",
            precoce::SEUIL_IMAGES_VIDES,
            precoce::SEUIL_DELAIS_EXPIRES
        ),
        Some(r) => format!("capture d'abord ({})", texte_raison(r)),
    }
}

fn texte_raison(r: RaisonBascule) -> &'static str {
    match r {
        RaisonBascule::ImagesVides => {
            "masquage précoce inutilisable : WGC renvoie une image transparente d'une fenêtre \
             masquée (alpha quasi nul)"
        }
        RaisonBascule::AucuneImage => {
            "masquage précoce inutilisable : WGC ne livre pas d'image d'une fenêtre masquée \
             (alpha quasi nul) dans le délai"
        }
    }
}

/// Bascule (définitive pour la session) en mode « capture d'abord ».
fn journaliser_bascule(r: RaisonBascule) {
    logging::avertir(&format!(
        "Ouverture : bascule en mode « capture d'abord » ({}) — la fenêtre restera visible \
         jusqu'à la 1re image (ancien comportement). Décocher puis recocher « Animer \
         l'ouverture » pour réessayer le masquage précoce.",
        texte_raison(r)
    ));
    journaliser_etat("bascule du mode d'ouverture");
}

/// Délai (ms) entre l'horodatage d'un événement WinEvent (`dwmsEventTime`,
/// base `GetTickCount`) et maintenant ; `None` si incohérent.
fn latence_evenement(horodatage: u32) -> Option<u32> {
    if horodatage == 0 {
        return None;
    }
    let ecart = unsafe { GetTickCount() }.wrapping_sub(horodatage);
    (ecart < 60_000).then_some(ecart)
}

/// Abrège un texte à `max` caractères.
fn abreger(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        texte.to_string()
    } else {
        let mut t: String = texte.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// « 0x1234 [processus | classe | « titre »] » pour le journal.
fn decrire(h: isize) -> String {
    let (processus, classe, titre) = window::infos_fenetre(HWND(h as *mut core::ffi::c_void));
    // Textes d'une autre application : caractères de contrôle neutralisés.
    format!(
        "{h:#x} [{} | {} | « {} »]",
        abreger(&logging::sans_controle(&processus), 32),
        abreger(&logging::sans_controle(&classe), 40),
        abreger(&logging::sans_controle(&titre), 40)
    )
}

/// Journalise un rejet d'éligibilité, une seule fois par couple
/// (fenêtre, raison) parmi les [`MEMOIRE_REJETS`] derniers.
fn journaliser_rejet(evenement: &str, h: isize, raison: &str) {
    let nouveau = avec(|s| {
        if s.rejets.iter().any(|(f, r)| *f == h && r == raison) {
            return false;
        }
        if s.rejets.len() >= MEMOIRE_REJETS {
            s.rejets.pop_front();
        }
        s.rejets.push_back((h, raison.to_string()));
        true
    })
    .unwrap_or(false);
    if nouveau {
        logging::info(&format!(
            "{evenement} : {} non éligible — {raison}.",
            decrire(h)
        ));
    }
}

/// Filtre anti-spam des événements d'affichage : on ne journalise les
/// rejets que pour les fenêtres racines visibles et titrées (les bulles,
/// menus et contrôles enfants affichés en permanence sont ignorés).
fn affichage_digne_du_journal(h: isize) -> bool {
    let hwnd = HWND(h as *mut core::ffi::c_void);
    unsafe {
        IsWindowVisible(hwnd).as_bool()
            && GetAncestor(hwnd, GA_ROOT) == hwnd
            && GetWindowTextLengthW(hwnd) > 0
    }
}

// ---------------------------------------------------------------------------
// Infrastructure : GPU, fenêtre de contrôle, crochets
// ---------------------------------------------------------------------------

/// Crée (une fois) WinRT, la classe de surimpression et le contexte GPU.
fn initialiser_gpu() {
    if !INFRA_PRETE.with(|c| c.get()) {
        // Fil de la boucle de messages : WinRT en STA, AVANT toute
        // création d'objet WGC dans ce fil (repli de création directe ;
        // normalement les sessions sont créées par le fil de capture MTA).
        gpu::initialiser_winrt();
        if let Err(e) = unsafe { surimpression::enregistrer_classe() } {
            logging::erreur(&format!("Classe de surimpression : {e}"));
            avec(|s| s.gpu = Disponibilite::Indisponible(e));
            return;
        }
        INFRA_PRETE.with(|c| c.set(true));
    }
    let disponibilite = match ContexteGpu::creer() {
        Ok(gpu) => Disponibilite::Prete(gpu),
        Err(e) => {
            logging::erreur(&format!("Animations indisponibles : {e}"));
            Disponibilite::Indisponible(e)
        }
    };
    avec(|s| s.gpu = disponibilite);
}

/// Crée la fenêtre de contrôle message-only si nécessaire ; renvoie son HWND brut.
fn assurer_controle() -> isize {
    let existant = avec(|s| s.controle).unwrap_or(0);
    if existant != 0 {
        return existant;
    }
    let cree = unsafe { creer_controle() };
    match cree {
        Ok(hwnd) => {
            let brut = hwnd.0 as isize;
            avec(|s| s.controle = brut);
            brut
        }
        Err(e) => {
            logging::erreur(&format!("Fenêtre de contrôle des animations : {e}"));
            0
        }
    }
}

unsafe fn creer_controle() -> windows::core::Result<HWND> {
    let instance = GetModuleHandleW(None)?;
    let nom = w!("TransparentWindows_Animations");
    let classe = WNDCLASSW {
        lpfnWndProc: Some(procedure_controle),
        hInstance: instance.into(),
        lpszClassName: nom,
        ..Default::default()
    };
    // Échec toléré (classe déjà enregistrée lors d'une tentative
    // précédente) : CreateWindowExW signalera la vraie erreur le cas échéant.
    let _ = RegisterClassW(&classe);
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        nom,
        nom,
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(instance.into()),
        None,
    )
}

/// Réévalue l'infrastructure après chaque changement de réglages :
/// création paresseuse du GPU, installation / retrait des crochets,
/// arrêt du suivi ou des ouvertures devenus inutiles, et démarrage du
/// suivi sur la fenêtre DÉJÀ au premier plan quand la fermeture est
/// (ré)activée (sinon aucun suivi n'existerait avant le prochain
/// changement de premier plan).
fn reevaluer(contexte: &str) {
    reprendre_crochets_en_suspens();
    let Some((r, gpu_a_creer)) =
        avec(|s| (s.reglages, matches!(s.gpu, Disponibilite::NonInitialise)))
    else {
        return;
    };
    let voulu = r.ouverture_active() || r.fermeture_active();
    let mut controle = 0;
    if voulu {
        if gpu_a_creer {
            initialiser_gpu();
        }
        controle = assurer_controle();
    }
    let (pret, desactivee) = avec(|s| (matches!(s.gpu, Disponibilite::Prete(_)), s.ouverture_desactivee))
        .unwrap_or((false, true));
    let actif = voulu && pret && controle != 0;
    // Crochet EVENT_OBJECT_CREATE (v0.4.7) : seulement si l'ouverture
    // animée est réellement active et la clé cachée vraie.
    let creations_voulues =
        actif && r.ouverture_active() && r.sans_transition_dwm && !desactivee;

    let extrait = avec(|s| {
        let crochets = if actif { None } else { s.crochets.take() };
        let suivis = if actif && r.fermeture_active() {
            (None, Vec::new())
        } else {
            s.extraire_suivis()
        };
        let ouvertures: Vec<(u32, OuvertureEnAttente)> = if actif && r.ouverture_active() {
            Vec::new()
        } else {
            s.ouvertures.drain().collect()
        };
        let a_installer = actif && s.crochets.is_none();
        (s.controle, crochets, suivis, ouvertures, a_installer)
    });
    let Some((controle, crochets, suivis, ouvertures, a_installer)) = extrait else {
        return;
    };
    // Hors emprunt : retrait des crochets, fermeture des captures,
    // restauration des ouvertures annulées.
    if crochets.is_some() {
        logging::info(&format!("Animations : crochets d'événements retirés ({contexte})."));
    }
    drop(crochets);
    drop(suivis);
    for (id, o) in ouvertures {
        annuler_ouverture(controle, id, o, "réglages modifiés");
    }
    if a_installer {
        match Crochets::installer(HWND(controle as *mut core::ffi::c_void), creations_voulues) {
            Ok(c) => {
                logging::info(&format!(
                    "Animations : crochets d'événements installés ({contexte}) — \
                     SHOW/HIDE/DESTROY + premier plan + fin de déplacement{}.",
                    if c.creations_actives() {
                        " + CREATE (transition DWM)"
                    } else {
                        ""
                    }
                ));
                // État occupé : mis en suspens (jamais perdus).
                ranger_crochets(c);
            }
            Err(e) => logging::erreur(&format!("SetWinEventHook a échoué : {e}")),
        }
    }
    synchroniser_crochet_creations(creations_voulues, contexte);

    // Fermeture (ré)activée : suivi immédiat de la fenêtre au premier plan.
    let besoin_suivi = avec(|s| {
        actif
            && s.reglages.fermeture_active()
            && s.crochets.is_some()
            && s.suivi.is_none()
            && s.demande.is_none()
    })
    .unwrap_or(false);
    if besoin_suivi {
        let premier_plan = unsafe { GetForegroundWindow() };
        if premier_plan.0.is_null() {
            logging::info("Fermeture : aucune fenêtre au premier plan à suivre pour l'instant.");
        } else {
            sur_premier_plan(premier_plan.0 as isize, "fenêtre déjà active");
        }
    }
    // Effet à shader choisi : compilation synchrone maintenant plutôt qu'à
    // la première animation (la fenêtre attendrait la compilation).
    if actif && r.effet.moteur() == Moteur::Shader {
        prechauffer_effet(r.effet);
    }
    journaliser_etat(contexte);
}

/// Installe / retire le crochet `EVENT_OBJECT_CREATE` selon `voulu` (les
/// crochets sont extraits sous emprunt court, modifiés HORS emprunt, puis
/// remis) ; crochet non voulu : les poses de transition DWM non liées à une
/// ouverture sont rétablies tout de suite.
fn synchroniser_crochet_creations(voulu: bool, contexte: &str) {
    if let Some(Some(mut c)) = avec(|s| s.crochets.take()) {
        match c.definir_creations(voulu) {
            Ok(true) => logging::info(&format!(
                "Animations : crochet EVENT_OBJECT_CREATE (transition DWM) {} ({contexte}).",
                if voulu { "installé" } else { "retiré" }
            )),
            Ok(false) => {}
            Err(e) => logging::avertir(&format!(
                "Animations : crochet EVENT_OBJECT_CREATE impossible ({e}) — transition DWM \
                 désactivée au SHOW seulement ({contexte})."
            )),
        }
        // État occupé : mis en suspens (repris au prochain `reevaluer` ou
        // traitement d'événements), jamais perdus.
        ranger_crochets(c);
    }
    if !voulu {
        if let Some(liees) = cibles_liees() {
            let rendues = unsafe { transitions::retablir_non_liees(&liees, true) };
            journaliser_retablissements(&rendues, &format!("désactivation ({contexte})"));
        }
    }
}

/// Fenêtres liées à une ouverture (en attente d'image ou effet en cours) ;
/// `None` si l'état est inaccessible (aucune pose ne doit alors être
/// considérée comme orpheline).
fn cibles_liees() -> Option<Vec<isize>> {
    avec(|s| {
        s.ouvertures
            .values()
            .map(|o| o.cible)
            .chain(
                s.actives
                    .values()
                    .filter(|a| a.sens == Sens::Ouverture)
                    .map(|a| a.cible),
            )
            .collect()
    })
}

/// Une ligne de journal pour des poses rétablies en lot.
fn journaliser_retablissements(rendues: &[(isize, transitions::Retablissement)], motif: &str) {
    if rendues.is_empty() {
        return;
    }
    let details: Vec<String> = rendues
        .iter()
        .map(|(h, r)| format!("{h:#x} {}", r.texte(motif)))
        .collect();
    logging::info(&format!(
        "Transitions DWM : {} pose(s) non liée(s) traitée(s) — {}.",
        rendues.len(),
        details.join(" ; ")
    ));
}

/// Rétablit la transition DWM de `cible` si elle a été désactivée par nous.
/// Refus de DWM : la pose est réinscrite (compteur d'essais) et le
/// balayage programmé pour la retenter.
fn liberer_transition(cible: isize, motif: &str) {
    if let Some(r) = unsafe { transitions::retablir(cible) } {
        logging::info(&format!("Transition DWM de {cible:#x} {}.", r.texte(motif)));
        if matches!(r, transitions::Retablissement::Reessai(..)) {
            if let Some(controle) = ETAT.with(|c| {
                c.try_borrow().ok().and_then(|g| g.as_ref().map(|s| s.controle))
            }) {
                programmer_balayage(controle);
            }
        }
    }
}

/// Programme le balayage des poses orphelines s'il y en a et qu'il ne
/// l'est pas déjà.
fn programmer_balayage(controle: isize) {
    if controle == 0 || transitions::registre_vide() {
        return;
    }
    let deja = avec(|s| std::mem::replace(&mut s.balayage_programme, true)).unwrap_or(true);
    if !deja && !programmer(controle, 0, MINUTEUR_TRANSITIONS, transitions::PERIODE_BALAYAGE_MS) {
        avec(|s| s.balayage_programme = false);
    }
}

/// `MINUTEUR_TRANSITIONS` : rétablit les poses orphelines (non liées,
/// plus vieilles que [`transitions::DELAI_ORPHELIN_MS`]) puis reprogramme.
fn balayer_transitions(controle: isize) {
    avec(|s| s.balayage_programme = false);
    if let Some(liees) = cibles_liees() {
        let rendues = unsafe { transitions::retablir_non_liees(&liees, false) };
        journaliser_retablissements(
            &rendues,
            &format!(
                "orpheline (aucune ouverture liée depuis {} ms)",
                transitions::DELAI_ORPHELIN_MS
            ),
        );
    }
    programmer_balayage(controle);
}

/// Relève une sonde si un palier est dû (ms depuis le SHOW).
fn sonder_si_du(sondes: &mut Sondes, cible: isize, debut: Instant) {
    let ecoule = debut.elapsed().as_millis() as u64;
    if sondes.due(ecoule) {
        sondes.noter(unsafe { transitions::sonder(cible, ecoule) });
    }
}

/// Fenêtre d'application créée (`EVENT_OBJECT_CREATE`, filtre de style
/// déjà passé dans le rappel) : désactivation au plus tôt de la transition
/// native d'apparition de DWM.
fn sur_creation(h: isize, horodatage: u32) {
    let Some(i) = instantane() else {
        return;
    };
    if i.gpu.is_none()
        || !i.reglages.ouverture_active()
        || !i.reglages.sans_transition_dwm
        || i.ouverture_desactivee
        || i.controle == 0
    {
        return;
    }
    let hwnd = HWND(h as *mut core::ffi::c_void);
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return;
    }
    transitions::noter_creation_vue();
    let rapport = unsafe { transitions::desactiver(h, Moment::Creation, Some(horodatage)) };
    let rang = avec(|s| {
        s.creations_journalisees = s.creations_journalisees.saturating_add(1);
        s.creations_journalisees
    })
    .unwrap_or(u32::MAX);
    if rang <= transitions::MAX_JOURNAL_CREATIONS {
        let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
        logging::info(&format!(
            "Transition DWM désactivée à la création de {} : Set = {} ; origine {} ; visible {}{}.",
            decrire(h),
            rapport.texte_appel(),
            texte_lecture(rapport.origine),
            oui_non(visible),
            if rang == transitions::MAX_JOURNAL_CREATIONS {
                " (dernière ligne de ce type pour la session ; voir les compteurs de l'état)"
            } else {
                ""
            }
        ));
    }
    programmer_balayage(i.controle);
}

/// Après le traitement d'un SHOW : une pose de transition DWM non liée à
/// une ouverture (fenêtre non animée) est rétablie tout de suite.
fn apres_affichage(h: isize) {
    if transitions::pose(h).is_none() {
        return;
    }
    if let Some(liees) = cibles_liees() {
        if !liees.contains(&h) {
            liberer_transition(h, "affichée sans ouverture animée");
        }
    }
}

/// Rend les animations indisponibles pour la session (erreur globale).
fn rendre_indisponible(erreur: ErreurAnimation) {
    logging::erreur(&format!("Animations désactivées : {erreur}"));
    let extrait = avec(|s| {
        s.gpu = Disponibilite::Indisponible(erreur);
        (
            s.controle,
            s.crochets.take(),
            s.extraire_suivis(),
            s.ouvertures.drain().collect::<Vec<_>>(),
        )
    });
    jeter_crochets_en_suspens();
    if let Some((controle, crochets, suivis, ouvertures)) = extrait {
        drop(crochets);
        drop(suivis);
        for (id, o) in ouvertures {
            annuler_ouverture(controle, id, o, "animations indisponibles");
        }
    }
    journaliser_etat("indisponibilité");
}

/// Journalise une erreur et en tire les conséquences (indisponibilité
/// globale, ou recréation du contexte GPU en cas de perte du périphérique).
fn gerer_erreur(erreur: ErreurAnimation, quoi: &str) {
    if erreur.est_globale() {
        rendre_indisponible(erreur);
        return;
    }
    logging::avertir(&format!("Animations — {quoi} : {erreur}"));
    if matches!(erreur, ErreurAnimation::Win32(_)) {
        verifier_perte_gpu();
    }
}

/// Contexte GPU partagé avec l'effet Jelly, créé paresseusement (et la
/// classe de surimpression enregistrée) si nécessaire. Ne JAMAIS appeler
/// sous un emprunt de l'état (`initialiser_gpu` emprunte l'état).
pub(super) fn contexte_gpu_partage() -> Result<ContexteGpu, String> {
    let lire_gpu = || {
        avec(|s| match &s.gpu {
            Disponibilite::Prete(g) => Ok(Some(g.clone())),
            Disponibilite::NonInitialise => Ok(None),
            Disponibilite::Indisponible(e) => Err(e.to_string()),
        })
    };
    match lire_gpu() {
        None => Err("état des animations indisponible".to_string()),
        Some(Err(e)) => Err(e),
        Some(Ok(Some(g))) => Ok(g),
        Some(Ok(None)) => {
            initialiser_gpu();
            match lire_gpu() {
                Some(Ok(Some(g))) => Ok(g),
                Some(Err(e)) => Err(e),
                _ => Err("contexte GPU non créé".to_string()),
            }
        }
    }
}

/// Erreur rencontrée par l'effet Jelly : mêmes conséquences qu'une erreur
/// d'animation (indisponibilité globale, recréation du GPU perdu).
pub(super) fn signaler_erreur(erreur: ErreurAnimation, quoi: &str) {
    gerer_erreur(erreur, quoi);
}

/// Périphérique perdu : recrée le contexte une fois, sinon indisponible.
fn verifier_perte_gpu() {
    let Some(Some((gpu, deja_recree))) = avec(|s| match &s.gpu {
        Disponibilite::Prete(g) => Some((g.clone(), s.gpu_recree)),
        _ => None,
    }) else {
        return;
    };
    if !gpu.est_perdu() {
        return;
    }
    drop(gpu);
    if deja_recree {
        rendre_indisponible(ErreurAnimation::GpuMaterielAbsent);
        return;
    }
    logging::avertir("Périphérique graphique perdu : recréation du contexte GPU.");
    match ContexteGpu::creer() {
        Ok(nouveau) => {
            // Les ouvertures en attente tiennent des sessions de capture de
            // l'ancien périphérique : annulées (fenêtre restaurée si masquée
            // par le mode précoce), HORS emprunt de l'état, comme dans
            // `rendre_indisponible`.
            let extrait = avec(|s| {
                s.gpu = Disponibilite::Prete(nouveau);
                s.gpu_recree = true;
                (
                    s.controle,
                    s.extraire_suivis(),
                    s.ouvertures.drain().collect::<Vec<_>>(),
                )
            });
            if let Some((controle, suivis, ouvertures)) = extrait {
                drop(suivis);
                for (id, o) in ouvertures {
                    annuler_ouverture(controle, id, o, "GPU recréé");
                }
            }
        }
        Err(e) => rendre_indisponible(e),
    }
}

// ---------------------------------------------------------------------------
// Minuteurs et stockage sûr
// ---------------------------------------------------------------------------

fn id_minuteur(id: u32, genre: usize) -> usize {
    (id as usize) * NB_GENRES + genre
}

/// Programme un minuteur ; `false` (et avertissement) si `SetTimer` échoue.
fn programmer(controle: isize, id: u32, genre: usize, ms: u32) -> bool {
    unsafe {
        if SetTimer(
            Some(HWND(controle as *mut core::ffi::c_void)),
            id_minuteur(id, genre),
            ms,
            None,
        ) == 0
        {
            logging::avertir(&format!(
                "Animations : SetTimer a échoué ({}).",
                windows::core::Error::from_thread()
            ));
            return false;
        }
    }
    true
}

fn tuer(controle: isize, id: u32, genre: usize) {
    unsafe {
        let _ = KillTimer(
            Some(HWND(controle as *mut core::ffi::c_void)),
            id_minuteur(id, genre),
        );
    }
}

fn tuer_tous(controle: isize, id: u32) {
    for genre in [MINUTEUR_FIN, MINUTEUR_ATTENTE, MINUTEUR_GARDE] {
        tuer(controle, id, genre);
    }
}

fn nouvel_id() -> Option<u32> {
    avec(|s| {
        let id = s.prochain_id;
        s.prochain_id = if id >= 1_000_000 { 1 } else { id + 1 };
        id
    })
}

/// Range une animation active ; si l'état est inaccessible, elle est
/// terminée immédiatement (jamais de surimpression orpheline ni de
/// fenêtre laissée invisible).
fn stocker_active(anim: AnimationActive) {
    let reste = ETAT.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(s) => s.actives.insert(anim.id, anim),
            None => Some(anim),
        },
        Err(_) => Some(anim),
    });
    if let Some(a) = reste {
        let ouverture = (a.sens == Sens::Ouverture).then_some((a.id, a.cible));
        unsafe { surimpression::terminer(a) };
        // Ouverture non rangée : sa pose de transition DWM ne serait liée à
        // rien (audit v0.4.7) -> rétablie tout de suite.
        if let Some((id, cible)) = ouverture {
            liberer_transition(cible, &format!("ouverture #{id} non rangée (état occupé)"));
        }
    }
}

/// Range une ouverture en attente ; en cas d'échec, elle est abandonnée
/// (et la fenêtre restaurée si elle a été masquée par le mode précoce).
fn stocker_ouverture(controle: isize, id: u32, ouverture: OuvertureEnAttente) {
    let reste = ETAT.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(s) => s.ouvertures.insert(id, ouverture),
            None => Some(ouverture),
        },
        Err(_) => Some(ouverture),
    });
    if let Some(o) = reste {
        annuler_ouverture(controle, id, o, "état indisponible");
    }
}

/// Annule une ouverture en attente : minuteurs, restauration de la
/// fenêtre si CETTE ouverture l'a masquée (mode précoce), puis capture.
/// Une ouverture non masquée ne restaure rien : la fenêtre peut être
/// masquée par une AUTRE animation (effet Jelly) qu'il ne faut pas
/// interrompre (`masquage::restaurer` ne distingue pas les animations).
fn annuler_ouverture(controle: isize, id: u32, ouverture: OuvertureEnAttente, raison: &str) {
    tuer(controle, id, MINUTEUR_ATTENTE);
    tuer(controle, id, MINUTEUR_GARDE);
    let cible = ouverture.cible;
    let restauree = ouverture.masquee && unsafe { masquage::restaurer(cible) };
    if ouverture.masquee {
        noter_restauration_precoce(cible);
        surveiller_restaurations(controle);
    }
    logging::info(&format!(
        "Ouverture #{id} de {cible:#x} annulée : {raison}{}.",
        if !ouverture.masquee {
            ""
        } else if restauree {
            " — fenêtre restaurée (masquage précoce)"
        } else {
            " — masquage précoce déjà levé (rien à restaurer)"
        }
    ));
    let fin = ouverture.debut.elapsed().as_millis() as u64;
    logging::info(&format!(
        "Ouverture #{id} — sondes (hypothèse transition DWM ; ouverture annulée à T+{fin}) : {}.",
        ouverture.sondes.texte(fin)
    ));
    liberer_transition(cible, &format!("ouverture #{id} annulée"));
    drop(ouverture.session);
}

/// Mémorise l'instant de la restauration d'une ouverture précoce (table
/// bornée) : le SHOW provoqué par la restauration sera ignoré.
fn noter_restauration_precoce(h: isize) {
    let _ = avec(|s| {
        s.restaurations.retain(|(c, _)| *c != h);
        if s.restaurations.len() >= MEMOIRE_REBOND {
            s.restaurations.pop_front();
        }
        s.restaurations.push_back((h, Instant::now()));
    });
}

/// Vrai si une ouverture précoce de `h` vient d'être restaurée (< 500 ms).
fn rebond_recent(h: isize) -> bool {
    avec(|s| {
        s.restaurations
            .iter()
            .any(|(c, t)| *c == h && t.elapsed().as_millis() < REBOND_OUVERTURE_MS)
    })
    .unwrap_or(false)
}

/// Vrai si la fenêtre répond aux messages dans [`DELAI_REPONSE_MS`] (pas
/// figée) : le masquage (`SetWindowLongPtrW`) est synchrone vers l'appli.
fn fenetre_repond(h: isize) -> bool {
    let mut resultat = 0usize;
    let r = unsafe {
        SendMessageTimeoutW(
            HWND(h as *mut core::ffi::c_void),
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            DELAI_REPONSE_MS,
            Some(&mut resultat),
        )
    };
    r.0 != 0
}

/// Termine une animation active (minuteurs + restauration + destruction).
/// Ouverture (v0.4.6, comme en v0.4.3) : la vraie fenêtre est restaurée
/// tout de suite (alpha 255, `WS_EX_LAYERED` conservé, invalidation légère)
/// et la surimpression levée instantanément — plus aucun maintien.
fn finir_active(controle: isize, anim: AnimationActive) {
    tuer_tous(controle, anim.id);
    let ouverture = (anim.sens == Sens::Ouverture).then_some((anim.cible, anim.id));
    unsafe { surimpression::terminer(anim) };
    // Après la restauration de la fenêtre par `terminer`.
    if let Some((cible, id)) = ouverture {
        liberer_transition(cible, &format!("fin de l'ouverture #{id}"));
    }
    surveiller_restaurations(controle);
}

/// La fenêtre ouverte a pu passer au premier plan pendant l'animation : on
/// s'assure qu'elle est suivie (fermeture animée).
fn suivre_si_premier_plan(cible: isize) {
    let premier_plan = unsafe { GetForegroundWindow() };
    if premier_plan.0 as isize == cible {
        sur_premier_plan(cible, "fin d'ouverture");
    }
}

/// Programme un nouvel essai des restaurations en échec (fenêtres masquées
/// à l'alpha [`ALPHA_OUVERTURE`] réinscrites par `masquage`), s'il y en a.
/// À appeler après toute restauration d'ouverture.
fn surveiller_restaurations(controle: isize) {
    if controle != 0
        && masquage::restaurations_en_attente() > 0
        && programmer(controle, 0, MINUTEUR_RESTAURATIONS, PERIODE_RESTAURATIONS_MS)
    {
        logging::avertir(&format!(
            "Ouverture : restauration en échec — nouvel essai dans {PERIODE_RESTAURATIONS_MS} \
             ms."
        ));
    }
}

/// Expiration de `MINUTEUR_FIN` : fin de l'effet (restauration immédiate de
/// la vraie fenêtre et levée de la surimpression).
fn fin_effet(controle: isize, anim: AnimationActive) {
    let ouverture = (anim.sens == Sens::Ouverture).then_some(anim.cible);
    finir_active(controle, anim);
    if let Some(cible) = ouverture {
        // Restauration de la fenêtre en fin d'effet : un SHOW éventuel
        // qu'elle provoquerait est ignoré (comme pour les annulations).
        noter_restauration_precoce(cible);
        suivre_si_premier_plan(cible);
    }
}

// ---------------------------------------------------------------------------
// Fenêtre de contrôle et flux des événements
// ---------------------------------------------------------------------------

/// Procédure de la fenêtre de contrôle (message-only). Appelée par la
/// boucle de messages, hors de tout emprunt de l'état.
unsafe extern "system" fn procedure_controle(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_EVENEMENTS => {
            reprendre_crochets_en_suspens();
            for ev in evenements::vider_file() {
                traiter_evenement(ev);
            }
            LRESULT(0)
        }
        WM_TRAME_PRETE => {
            trame_prete(hwnd.0 as isize, wparam.0 as u32);
            LRESULT(0)
        }
        WM_SESSION_CREEE => {
            sessions_creees();
            LRESULT(0)
        }
        WM_EFFET_BATTEMENT => {
            battement_effets(hwnd.0 as isize, Some(wparam.0 as u32));
            LRESULT(0)
        }
        WM_TIMER => {
            let _ = KillTimer(Some(hwnd), wparam.0);
            minuteur(hwnd.0 as isize, wparam.0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Instantané des informations nécessaires au traitement d'un événement.
struct Instantane {
    reglages: Reglages,
    gpu: Option<ContexteGpu>,
    controle: isize,
    ouverture_desactivee: bool,
    /// Mode d'ouverture (masquage précoce ou capture d'abord).
    mode: Mode,
    occupees: usize,
    suivi: Option<isize>,
}

fn instantane() -> Option<Instantane> {
    avec(|s| Instantane {
        reglages: s.reglages,
        gpu: match &s.gpu {
            Disponibilite::Prete(g) => Some(g.clone()),
            _ => None,
        },
        controle: s.controle,
        ouverture_desactivee: s.ouverture_desactivee,
        mode: s.adaptation.mode(),
        occupees: s.actives.len() + s.ouvertures.len(),
        suivi: s.suivi.as_ref().map(|c| c.hwnd),
    })
}

fn traiter_evenement(ev: Evenement) {
    match ev {
        Evenement::PremierPlan(h) => sur_premier_plan(h, "EVENT_SYSTEM_FOREGROUND"),
        Evenement::Masquee(h) => sur_disparition(h, false),
        Evenement::Detruite(h) => {
            // Fenêtre détruite : la pose de transition DWM est oubliée sans
            // appel (HWND bientôt recyclable).
            transitions::oublier(h);
            sur_disparition(h, true)
        }
        Evenement::Affichee(h, horodatage) => sur_affichage(h, horodatage),
        Evenement::Creee(h, horodatage) => sur_creation(h, horodatage),
        // Le redimensionnement est géré par la capture (recréation du pool) ;
        // un simple déplacement ne produit pas d'image WGC (contenu inchangé),
        // donc les bornes mémorisées de la session suivie sont relues ici.
        Evenement::DeplacementFini(h) => {
            avec(|s| {
                if let Some(c) = s.suivi.as_ref().filter(|c| c.hwnd == h) {
                    c.rafraichir_bornes();
                } else if let Some(c) = s.recents.obtenir(h) {
                    c.rafraichir_bornes();
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Suivi de fermeture (fenêtre au premier plan + fenêtres récentes)
// ---------------------------------------------------------------------------

/// Nouvelle fenêtre au premier plan : on (re)cible le suivi de fermeture.
fn sur_premier_plan(h: isize, origine: &str) {
    sur_premier_plan_essai(h, origine, 0);
}

/// Abandonne tout le suivi (fermeture inactive ou GPU indisponible).
fn abandonner_suivis(controle: isize) {
    let extrait = avec(|s| s.extraire_suivis());
    if controle != 0 {
        tuer(controle, 0, MINUTEUR_REESSAI);
    }
    drop(extrait);
}

/// Remplace la session `suivi` (l'ancienne rejoint les récentes, la plus
/// ancienne des récentes est libérée HORS emprunt) et la création attendue.
/// Journalise le changement (avant -> après) s'il y en a un.
fn changer_suivi(nouvelle: Option<SessionCapture>, demande: Option<u64>, contexte: &str) {
    let resultat = avec(move |s| {
        let ancien = s.suivi.as_ref().map(|c| c.hwnd);
        let nouveau = nouvelle.as_ref().map(|c| c.hwnd);
        if ancien.is_none() && nouveau.is_none() && s.demande == demande {
            return None; // rien ne change
        }
        let avant = s.texte_suivis();
        s.demande = demande;
        let ancienne = std::mem::replace(&mut s.suivi, nouvelle);
        let rendus = match ancienne {
            Some(a) => {
                let h = a.hwnd;
                s.recents.pousser(h, a)
            }
            None => Vec::new(),
        };
        // Une même fenêtre ne doit pas avoir deux sessions conservées.
        let doublon = nouveau.and_then(|h| s.recents.retirer(h));
        Some((avant, s.texte_suivis(), rendus, doublon))
    })
    .flatten();
    if let Some((avant, apres, rendus, doublon)) = resultat {
        logging::info(&format!("Suivi de fermeture ({contexte}) : {avant} -> {apres}."));
        drop(rendus);
        drop(doublon);
    }
}

/// Libère les sessions récentes dont la fenêtre n'existe plus (HIDE /
/// DESTROY manqué). `IsWindow` est appelé hors emprunt.
fn purger_recents() {
    let cles = avec(|s| s.recents.cles()).unwrap_or_default();
    let mortes: Vec<isize> = cles
        .into_iter()
        .filter(|h| unsafe { !IsWindow(Some(HWND(*h as *mut core::ffi::c_void))).as_bool() })
        .collect();
    if mortes.is_empty() {
        return;
    }
    let liberees = avec(|s| {
        mortes
            .iter()
            .filter_map(|h| s.recents.retirer(*h))
            .collect::<Vec<_>>()
    });
    let texte: Vec<String> = mortes.iter().map(|h| format!("{h:#x}")).collect();
    logging::info(&format!(
        "Suivi de fermeture : sessions récentes de fenêtres disparues libérées ({}).",
        texte.join(", ")
    ));
    drop(liberees);
}

/// Cœur du changement de premier plan. `essai` = nombre de réessais déjà
/// effectués pour cette fenêtre (0 = événement réel).
fn sur_premier_plan_essai(h: isize, origine: &str, essai: u32) {
    let Some(i) = instantane() else {
        return;
    };
    let gpu = match (&i.gpu, i.reglages.fermeture_active()) {
        (Some(g), true) => g.clone(),
        _ => {
            abandonner_suivis(i.controle);
            return;
        }
    };
    if h == 0 {
        return;
    }
    // Un autre premier plan annule le réessai en cours.
    let annule = avec(|s| match s.reessai {
        Some((c, n)) if c != h => {
            s.reessai = None;
            Some((c, n))
        }
        _ => None,
    })
    .flatten();
    if let Some((c, n)) = annule {
        tuer(i.controle, 0, MINUTEUR_REESSAI);
        logging::info(&format!(
            "Premier plan : réessai de {c:#x} annulé après {n} essai(s) — {h:#x} est passée au \
             premier plan."
        ));
    }
    purger_recents();
    if i.suivi == Some(h) {
        // Déjà suivie : on met seulement les bornes à jour.
        avec(|s| {
            if let Some(c) = s.suivi.as_ref() {
                c.rafraichir_bornes();
            }
        });
        return;
    }
    // Création déjà en cours pour cette fenêtre : elle redevient la cible.
    let en_cours = avec(|s| s.creation_pour(h)).flatten();
    if let Some(jeton) = en_cours {
        changer_suivi(None, Some(jeton), &format!("premier plan {h:#x}, création déjà en cours"));
        return;
    }
    // Retour sur une fenêtre récente : on reprend sa session.
    let reprise = avec(|s| s.recents.retirer(h)).flatten();
    if let Some(session) = reprise {
        changer_suivi(
            Some(session),
            None,
            &format!("premier plan {h:#x} ({origine}), session récente reprise"),
        );
        return;
    }
    let hwnd = HWND(h as *mut core::ffi::c_void);
    match unsafe { eligibilite::verifier(hwnd) } {
        Ok(()) => {
            let reessai_vise = avec(|s| {
                s.vues.marquer(h);
                if s.reessai.map(|(c, _)| c) == Some(h) {
                    s.reessai = None;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if reessai_vise {
                tuer(i.controle, 0, MINUTEUR_REESSAI);
            }
            logging::info(&format!(
                "Premier plan ({origine}) : {} éligible{} — suivi de fermeture demandé.",
                decrire(h),
                if essai > 0 {
                    format!(" après {essai} réessai(s)")
                } else {
                    String::new()
                }
            ));
            demander_suivi(&gpu, i.controle, h, origine);
        }
        Err(raison) => {
            journaliser_rejet(&format!("Premier plan ({origine})"), h, &raison);
            let transitoire = politique::rejet_transitoire(&raison);
            if transitoire {
                avec(|s| s.vues.marquer(h));
            }
            // L'ancienne fenêtre suivie n'est plus au premier plan : sa
            // session rejoint les récentes (fermeture toujours animable).
            changer_suivi(None, None, &format!("premier plan {h:#x} non éligible"));
            if politique::reessai_autorise(transitoire, essai, MAX_REESSAIS) {
                avec(|s| s.reessai = Some((h, essai + 1)));
                if programmer(i.controle, 0, MINUTEUR_REESSAI, DELAI_REESSAI_MS) && essai == 0 {
                    logging::info(&format!(
                        "Premier plan : {h:#x} rejetée pour une raison passagère ({raison}) — \
                         réessai toutes les {DELAI_REESSAI_MS} ms ({MAX_REESSAIS} au plus)."
                    ));
                }
            } else if transitoire {
                logging::info(&format!(
                    "Premier plan : {h:#x} toujours non éligible après {essai} réessai(s) \
                     ({raison}) — abandon (EVENT_OBJECT_SHOW la réévaluera)."
                ));
            }
        }
    }
}

/// Demande la création de la session de suivi de `h` au fil de capture
/// (asynchrone) ; repli sur la création directe si le fil est arrêté.
fn demander_suivi(gpu: &ContexteGpu, controle: isize, h: isize, origine: &str) {
    let Some(jeton) = avec(|s| {
        let j = s.prochain_jeton;
        s.prochain_jeton = s.prochain_jeton.wrapping_add(1).max(1);
        j
    }) else {
        return;
    };
    if let Some(atelier) = gpu.atelier.as_ref() {
        match atelier.demander(jeton, h, "suivi", controle) {
            Ok(()) => {
                avec(|s| {
                    s.en_creation.insert(jeton, (h, Instant::now()));
                });
                changer_suivi(
                    None,
                    Some(jeton),
                    &format!("premier plan {h:#x} ({origine}), création demandée"),
                );
                programmer(controle, 0, MINUTEUR_SURVEILLANCE, DELAI_SURVEILLANCE_MS);
                return;
            }
            Err(RefusAtelier::Occupe(t)) => {
                logging::avertir(&format!(
                    "Suivi de fermeture de {h:#x} impossible pour l'instant : {t}."
                ));
                changer_suivi(None, None, &format!("premier plan {h:#x}, fil de capture occupé"));
                return;
            }
            Err(RefusAtelier::Arrete) => logging::avertir(
                "Fil de capture WGC arrêté : session de suivi créée dans le fil principal (repli).",
            ),
        }
    }
    match capture::demarrer_directement(gpu, h, "suivi") {
        Ok(session) => changer_suivi(
            Some(session),
            None,
            &format!("premier plan {h:#x} ({origine}), création directe"),
        ),
        Err(e) => {
            changer_suivi(None, None, &format!("premier plan {h:#x}, capture impossible"));
            gerer_erreur(e, "capture de la fenêtre au premier plan");
        }
    }
}

/// Résultats des créations asynchrones (message `WM_SESSION_CREEE`).
fn sessions_creees() {
    for (jeton, resultat) in capture::recuperer_resultats() {
        let info = avec(|s| {
            let connue = s.en_creation.remove(&jeton);
            let attendue = s.demande == Some(jeton);
            if attendue {
                s.demande = None;
            }
            (connue, attendue, s.reglages.fermeture_active())
        });
        let Some((connue, attendue, fermeture)) = info else {
            drop(resultat);
            continue;
        };
        let session = match resultat {
            Ok(session) => session,
            Err(e) => {
                // Déjà journalisée (étape + HRESULT) par le fil de capture.
                if connue.is_some() && fermeture {
                    gerer_erreur(e, "capture de la fenêtre au premier plan");
                }
                continue;
            }
        };
        let Some((h, debut)) = connue else {
            logging::info(&format!(
                "Session de suivi de {:#x} arrivée après l'abandon du suivi : libérée.",
                session.hwnd
            ));
            drop(session);
            continue;
        };
        if !fermeture {
            drop(session);
            continue;
        }
        let delai = debut.elapsed().as_millis();
        let hwnd = HWND(h as *mut core::ffi::c_void);
        let (existe, visible) = unsafe {
            let existe = IsWindow(Some(hwnd)).as_bool();
            (existe, existe && IsWindowVisible(hwnd).as_bool())
        };
        if !existe || !visible {
            logging::info(&format!(
                "Session de suivi de {h:#x} prête après {delai} ms mais fenêtre {} entre-temps : \
                 libérée.",
                if existe { "masquée" } else { "détruite" }
            ));
            drop(session);
            continue;
        }
        if attendue {
            changer_suivi(Some(session), None, &format!("session de {h:#x} créée en {delai} ms"));
        } else {
            let rendus = avec(move |s| s.recents.pousser(h, session));
            logging::info(&format!(
                "Session de suivi de {h:#x} prête après {delai} ms ; le premier plan a changé \
                 entre-temps : rangée parmi les récentes."
            ));
            drop(rendus);
        }
    }
}

/// Minuteur de réessai : la fenêtre est réévaluée si elle est toujours au
/// premier plan.
fn reessayer() {
    let Some(Some((h, n))) = avec(|s| s.reessai.take()) else {
        return;
    };
    let premier_plan = unsafe { GetForegroundWindow() };
    if premier_plan.0 as isize != h {
        logging::info(&format!(
            "Premier plan : réessai {n}/{MAX_REESSAIS} de {h:#x} abandonné (n'est plus au premier \
             plan)."
        ));
        return;
    }
    sur_premier_plan_essai(h, &format!("réessai {n}/{MAX_REESSAIS}"), n);
}

/// Minuteur de surveillance : signale une création bloquée sur le fil de
/// capture (sans jamais bloquer le fil principal).
fn surveiller(controle: isize) {
    let Some(i) = instantane() else {
        return;
    };
    let Some(atelier) = i.gpu.as_ref().and_then(|g| g.atelier.clone()) else {
        return;
    };
    let Some(c) = atelier.creation_en_cours() else {
        avec(|s| s.alertes_blocage = (0, 0));
        return;
    };
    let n = avec(|s| {
        if s.alertes_blocage.0 != c.jeton {
            s.alertes_blocage = (c.jeton, 0);
        }
        s.alertes_blocage.1 = s.alertes_blocage.1.saturating_add(1);
        s.alertes_blocage.1
    })
    .unwrap_or(MAX_ALERTES_BLOCAGE);
    if c.depuis_ms >= capture::SEUIL_BLOCAGE_MS && n <= MAX_ALERTES_BLOCAGE {
        logging::avertir(&format!(
            "Fil de capture WGC : création sur {:#x} en cours depuis {} ms, à l'étape « {} » \
             depuis {} ms ({} demande(s) en file) — blocage probable.",
            c.hwnd, c.depuis_ms, c.etape, c.etape_depuis_ms, c.en_file
        ));
    }
    if n < MAX_ALERTES_BLOCAGE {
        programmer(controle, 0, MINUTEUR_SURVEILLANCE, DELAI_SURVEILLANCE_MS);
    }
}

/// Fenêtre masquée ou détruite : abandonne ses ouvertures, puis lance la
/// fermeture animée si une session de suivi (premier plan ou récente)
/// existe pour elle (voir la règle HIDE / DESTROY en tête de module).
fn sur_disparition(h: isize, detruite: bool) {
    let nom_evt = if detruite { "DESTROY" } else { "HIDE" };
    let extrait = avec(|s| {
        let ids: Vec<u32> = s
            .ouvertures
            .iter()
            .filter(|(_, o)| o.cible == h)
            .map(|(id, _)| *id)
            .collect();
        let ouvertures: Vec<(u32, OuvertureEnAttente)> = ids
            .into_iter()
            .filter_map(|id| s.ouvertures.remove(&id).map(|o| (id, o)))
            .collect();
        let ids: Vec<u32> = s
            .actives
            .iter()
            .filter(|(_, a)| a.cible == h && a.sens == Sens::Ouverture)
            .map(|(id, _)| *id)
            .collect();
        let actives: Vec<AnimationActive> =
            ids.into_iter().filter_map(|id| s.actives.remove(&id)).collect();
        let session = if s.suivi.as_ref().map(|c| c.hwnd) == Some(h) {
            s.suivi.as_ref().map(|c| ("suivie", c))
        } else {
            s.recents.obtenir(h).map(|c| ("récente", c))
        };
        // (rôle, image disponible, âge de la dernière image, images reçues, âge de la session)
        let etat_session = session.map(|(quelle, c)| {
            (
                quelle,
                c.a_une_image(),
                c.age_derniere_image_ms(),
                c.trames_recues(),
                c.age_ms(),
            )
        });
        let en_creation = s
            .en_creation
            .values()
            .find(|(f, _)| *f == h)
            .map(|(_, d)| d.elapsed().as_millis());
        // Une fenêtre suivie reste « vue » (un HIDE parasite ne l'oublie pas).
        let vue = etat_session.is_none() && s.vues.retirer(h);
        (
            s.controle,
            ouvertures,
            actives,
            etat_session,
            en_creation,
            vue,
            s.texte_suivis(),
        )
    });
    let Some((controle, ouvertures, actives, etat_session, en_creation, vue, suivis)) = extrait
    else {
        return;
    };
    for (id, o) in ouvertures {
        annuler_ouverture(controle, id, o, &format!("fenêtre {nom_evt}"));
    }
    for a in actives {
        logging::info(&format!(
            "Animation d'ouverture #{} de {h:#x} interrompue ({nom_evt}).",
            a.id
        ));
        finir_active(controle, a);
    }
    let Some((quelle, image, age_image, recues, age_session)) = etat_session else {
        if let Some(ms) = en_creation {
            logging::info(&format!(
                "Fermeture : {nom_evt} de {h:#x} — session de suivi encore en création depuis \
                 {ms} ms : fermeture non animée."
            ));
        } else if vue {
            // Une seule ligne par fenêtre (elle a été retirée de `vues`).
            logging::info(&format!(
                "Fermeture : {nom_evt} de {h:#x} ignoré — aucune session de suivi pour cette \
                 fenêtre ({suivis})."
            ));
        }
        return;
    };
    let fenetre = if detruite {
        format!("{h:#x} (détruite)")
    } else {
        decrire(h)
    };
    logging::info(&format!(
        "Fermeture : {nom_evt} reçu pour {fenetre} — session {quelle} présente, image disponible : \
         {}, {recues} image(s) reçue(s), session âgée de {age_session} ms.",
        match (image, age_image) {
            (true, Some(age)) => format!("oui (âge {age} ms)"),
            (true, None) => "oui".to_string(),
            (false, _) => "non".to_string(),
        }
    ));
    if !detruite {
        let hwnd = HWND(h as *mut core::ffi::c_void);
        let (existe, reduite, visible) = unsafe {
            let existe = IsWindow(Some(hwnd)).as_bool();
            (
                existe,
                existe && IsIconic(hwnd).as_bool(),
                existe && IsWindowVisible(hwnd).as_bool(),
            )
        };
        match politique::decision_disparition(false, existe, reduite, visible) {
            Disparition::Ignorer(motif) => {
                logging::info(&format!(
                    "Fermeture : HIDE de {h:#x} ignoré (fenêtre {motif}) — suivi conservé."
                ));
                return;
            }
            Disparition::Animer => logging::info(&format!(
                "Fermeture : HIDE de {h:#x} (fenêtre {}).",
                if existe {
                    "masquée, encore existante"
                } else {
                    "déjà détruite"
                }
            )),
        }
    }
    let session = avec(|s| {
        if s.suivi.as_ref().map(|c| c.hwnd) == Some(h) {
            s.suivi.take().map(|c| (c, "suivie"))
        } else {
            s.recents.retirer(h).map(|c| (c, "récente"))
        }
    })
    .flatten();
    if let Some((session, quelle)) = session {
        logging::info(&format!(
            "Fermeture : déclenchement pour {h:#x} sur {nom_evt} (session {quelle})."
        ));
        lancer_fermeture(h, session);
    }
}

/// Rejoue la dernière image de la fenêtre suivie avec l'effet de fermeture.
/// La session reste ouverte jusqu'à ce que l'image ait été copiée dans la
/// surimpression (fermer le pool avant invaliderait la texture de l'image).
fn lancer_fermeture(h: isize, session: SessionCapture) {
    // Bornes à jour si la fenêtre existe encore, sinon les dernières connues.
    session.rafraichir_bornes();
    let recues = session.trames_recues();
    let mut trame = session.prendre_trame();
    if let Some(t) = trame.as_mut() {
        t.bornes = session.bornes();
    }
    let Some(i) = instantane() else {
        return;
    };
    let Some(gpu) = i.gpu else {
        return;
    };
    if !i.reglages.fermeture_active() {
        return;
    }
    if i.occupees >= MAX_SIMULTANEES {
        logging::info(&format!(
            "Fermeture de {h:#x} non animée : {} animation(s) déjà en cours.",
            i.occupees
        ));
        return;
    }
    let Some(trame) = trame else {
        logging::info(&format!(
            "Fermeture de {h:#x} non animée : aucune image ({recues} image(s) reçue(s) par la \
             session, âgée de {} ms).",
            session.age_ms()
        ));
        return;
    };
    logging::info(&format!(
        "Fermeture de {h:#x} : image disponible {}x{} (âge {} ms ; {recues} image(s) reçue(s) par \
         la session).",
        trame.taille.0,
        trame.taille.1,
        session
            .age_derniere_image_ms()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into())
    ));
    let taille = Taille {
        largeur: trame.taille.0,
        hauteur: trame.taille.1,
    };
    let Some(plan) = plan::construire_plan(i.reglages.effet, Sens::Fermeture, taille) else {
        logging::info(&format!(
            "Fermeture de {h:#x} non animée : plan impossible (effet {}, {}x{}).",
            i.reglages.effet.nom_config(),
            taille.largeur,
            taille.hauteur
        ));
        return;
    };
    logging::info(&format!(
        "Fermeture de {h:#x} : plan construit (effet {}, {:.0} ms{}).",
        i.reglages.effet.nom_config(),
        plan.duree() * 1000.0,
        if matches!(plan, Plan::Shader(_)) {
            ", shader"
        } else {
            ""
        }
    ));
    let Some(id) = nouvel_id() else {
        return;
    };
    let resultat = lancer_avec_repli(&gpu, id, Sens::Fermeture, h, &trame, plan, taille);
    drop(trame);
    drop(session);
    match resultat {
        Ok((anim, duree)) => {
            // Garde-fou : la surimpression est détruite au plus tard après
            // GARDE_FOU_MS (ou la durée de l'effet + marges si elle est plus
            // longue), même si le minuteur de fin est perdu. Les 7 effets de
            // composition durent moins de GARDE_FOU_MS : inchangé pour eux.
            let fin = duree_ms(duree) + MARGE_FIN_MS;
            programmer(i.controle, id, MINUTEUR_GARDE, GARDE_FOU_MS.max(fin + MARGE_GARDE_EFFET_MS));
            if programmer(i.controle, id, MINUTEUR_FIN, fin) {
                let shader = anim.est_shader();
                stocker_active(anim);
                if shader {
                    ajuster_battement(i.controle);
                }
            } else {
                // Pas de minuteur de fin : on termine tout de suite.
                finir_active(i.controle, anim);
            }
        }
        Err(e) => gerer_erreur(e, "animation de fermeture"),
    }
}

// ---------------------------------------------------------------------------
// Effets à shader (v0.4.7) : lancement avec repli, battement, préchauffage
// ---------------------------------------------------------------------------

/// Lance la surimpression du plan. Effet à shader indisponible (shader non
/// compilé, base du pipeline impossible) ou échec ponctuel : repli
/// immédiat sur « Fondu » pour CETTE animation, avec la même image (la
/// configuration n'est pas réécrite) ; indisponibilité notifiée une fois
/// par effet et par session. Périphérique perdu : erreur (l'appelant
/// appelle `gerer_erreur`, qui recrée le contexte GPU). Renvoie aussi la
/// durée réelle de l'effet joué (s).
fn lancer_avec_repli(
    gpu: &ContexteGpu,
    id: u32,
    sens: Sens,
    cible: isize,
    trame: &capture::DerniereTrame,
    plan: Plan,
    taille: Taille,
) -> Result<(AnimationActive, f64), ErreurAnimation> {
    let quoi = match sens {
        Sens::Ouverture => "Ouverture",
        Sens::Fermeture => "Fermeture",
    };
    let plan_shader = match plan {
        Plan::Composition(p) => {
            let anim = unsafe { surimpression::lancer(gpu, id, sens, cible, trame, &p)? };
            return Ok((anim, p.duree));
        }
        Plan::Shader(p) => p,
    };
    let effet = plan_shader.effet;
    if effet_gpu::est_indisponible(effet) {
        // Une seule ligne par effet et par session (sinon une par animation).
        let premiere = avec(|s| {
            if s.replis_journalises.contains(&effet) {
                false
            } else {
                s.replis_journalises.push(effet);
                true
            }
        })
        .unwrap_or(false);
        if premiere {
            logging::info(&format!(
                "{quoi} #{id} de {cible:#x} : repli Fondu : effet « {} » indisponible (échec \
                 mémorisé, voir plus haut dans le journal ; ligne unique pour cet effet dans la \
                 session).",
                effet.nom()
            ));
        }
        notifier_indisponible(effet);
    } else {
        let graine = effets_shader::graine(id, cible);
        let resultat = unsafe {
            surimpression::lancer_shader(gpu, id, sens, cible, trame, &plan_shader, graine)
        };
        match resultat {
            Ok(anim) => return Ok((anim, plan_shader.duree)),
            Err(EchecEffet::PeripheriquePerdu(e)) => {
                logging::avertir(&format!(
                    "{quoi} #{id} de {cible:#x} : effet « {} » — périphérique graphique perdu ({e}).",
                    effet.nom()
                ));
                return Err(e);
            }
            Err(e @ EchecEffet::Indisponible { .. }) => {
                logging::avertir(&format!(
                    "{quoi} #{id} de {cible:#x} : {e} — repli sur « Fondu » pour cette animation."
                ));
                notifier_indisponible(effet);
            }
            Err(e @ EchecEffet::Ponctuel { .. }) => {
                logging::avertir(&format!(
                    "{quoi} #{id} de {cible:#x} : effet « {} », {e} — repli sur « Fondu » pour \
                     cette animation.",
                    effet.nom()
                ));
            }
        }
    }
    let Some(fondu) = plan::construire(EffetAnimation::Fondu, sens, taille) else {
        return Err(ErreurAnimation::Shader(format!(
            "repli « Fondu » impossible ({}x{})",
            taille.largeur, taille.hauteur
        )));
    };
    let anim = unsafe { surimpression::lancer(gpu, id, sens, cible, trame, &fondu)? };
    Ok((anim, fondu.duree))
}

/// Notifie (une fois par effet et par session) qu'un effet à shader est
/// indisponible. La notification est affichée HORS emprunt de l'état.
fn notifier_indisponible(effet: EffetAnimation) {
    let nouveau = avec(|s| {
        if s.effets_notifies.contains(&effet) {
            false
        } else {
            s.effets_notifies.push(effet);
            true
        }
    })
    .unwrap_or(false);
    if nouveau {
        tray::notifier(&format!(
            "L'effet « {} » est indisponible (shader non compilé) : repli sur « Fondu ». Voir \
             journal.",
            effet.nom()
        ));
    }
}

/// Compile tout de suite (de façon synchrone) le shader de l'effet choisi,
/// pour que la première animation ne paie pas la compilation. Échec :
/// effet marqué indisponible (par `effet_gpu`) et notification unique.
fn prechauffer_effet(effet: EffetAnimation) {
    let Some(Some(gpu)) = avec(|s| match &s.gpu {
        Disponibilite::Prete(g) => Some(g.clone()),
        _ => None,
    }) else {
        return;
    };
    if effet_gpu::est_pret(&gpu.d3d, effet) {
        return;
    }
    if effet_gpu::est_indisponible(effet) {
        logging::info(&format!(
            "Effet GPU « {} » : indisponible (échec mémorisé) — les animations utiliseront « Fondu ».",
            effet.nom()
        ));
        notifier_indisponible(effet);
        return;
    }
    match effet_gpu::prechauffer(&gpu, effet) {
        Ok(d) => logging::info(&format!(
            "Effet GPU « {} » : shader préchauffé (compilation synchrone) en {:.1} ms.",
            effet.nom(),
            d.as_secs_f64() * 1000.0
        )),
        Err(e @ EchecEffet::Indisponible { .. }) => {
            logging::avertir(&format!(
                "Effet GPU : préchauffage impossible — {e} ; les animations utiliseront « Fondu »."
            ));
            notifier_indisponible(effet);
        }
        Err(e @ EchecEffet::Ponctuel { .. }) => logging::avertir(&format!(
            "Effet GPU « {} » : préchauffage non terminé ({e}) — nouvel essai à la première \
             animation.",
            effet.nom()
        )),
        Err(EchecEffet::PeripheriquePerdu(e)) => {
            drop(gpu);
            gerer_erreur(e, "préchauffage d'un effet GPU");
        }
    }
}

/// Démarre la cadence du rendu des effets à shader si une animation à
/// shader a des images à rendre, l'arrête sinon.
fn ajuster_battement(controle: isize) {
    let besoin = avec(|s| s.actives.values().any(|a| a.besoin_rendu())).unwrap_or(false);
    if besoin {
        assurer_battement(controle);
    } else {
        arreter_battement(controle);
    }
}

/// Démarre le fil de battement (horloge du compositeur, [`metronome`]) ;
/// repli sur `MINUTEUR_BATTEMENT_SECOURS` s'il ne démarre pas.
fn assurer_battement(controle: isize) {
    if controle == 0 {
        return;
    }
    let Some((existe, generation)) = avec(|s| {
        if s.battement.is_some() {
            (true, 0)
        } else {
            s.generation_battement = s.generation_battement.wrapping_add(1);
            (false, s.generation_battement)
        }
    }) else {
        return;
    };
    if existe {
        return;
    }
    let battement = match Metronome::demarrer(controle, WM_EFFET_BATTEMENT, generation) {
        Ok(m) => {
            match &m.motif_repli {
                Some(motif) => logging::info(&format!(
                    "Effets GPU : battement démarré (génération {generation}) sur DwmFlush — \
                     horloge du compositeur indisponible : {motif}."
                )),
                None => logging::info(&format!(
                    "Effets GPU : battement démarré (génération {generation}) sur l'horloge du \
                     compositeur."
                )),
            }
            Battement::Fil(m)
        }
        Err(e) => {
            logging::avertir(&format!(
                "Effets GPU : fil de battement impossible ({e}) — repli sur un minuteur SetTimer \
                 {PERIODE_BATTEMENT_SECOURS_MS} ms."
            ));
            if !programmer(controle, 0, MINUTEUR_BATTEMENT_SECOURS, PERIODE_BATTEMENT_SECOURS_MS) {
                logging::erreur(
                    "Effets GPU : aucun battement possible (SetTimer) — les effets à shader en \
                     cours restent sur leur image courante jusqu'à leur fin.",
                );
                return;
            }
            Battement::Minuteur
        }
    };
    // État occupé ou cadence apparue entre-temps : celle-ci est arrêtée
    // aussitôt (Drop du Metronome, hors emprunt).
    let reste = avec(move |s| {
        if s.battement.is_none() {
            s.battement = Some(battement);
            None
        } else {
            Some(battement)
        }
    });
    drop(reste);
}

/// Arrête la cadence (Drop du fil, hors emprunt ; minuteur tué).
fn arreter_battement(controle: isize) {
    let Some(Some(b)) = avec(|s| s.battement.take()) else {
        return;
    };
    match b {
        Battement::Fil(m) => {
            let c = m.compteurs();
            let duree = m.duree().as_millis();
            drop(m);
            logging::info(&format!(
                "Effets GPU : battement arrêté après {duree} ms ({} battement(s) posté(s), {} \
                 coalescé(s), {} délai(s), {} échec(s) de PostMessage).",
                c.postes, c.coalesces, c.delais, c.echecs_post
            ));
        }
        Battement::Minuteur => {
            tuer(controle, 0, MINUTEUR_BATTEMENT_SECOURS);
            logging::info("Effets GPU : battement de secours (SetTimer) arrêté.");
        }
    }
}

/// Battement : rend l'image courante de chaque animation à shader.
/// `generation` = `Some` pour le fil (message périmé ignoré), `None` pour
/// le minuteur de secours. Les animations sont extraites de l'état sous un
/// emprunt court, rendues HORS emprunt, puis réinsérées. Un rendu en échec
/// répété (3 fois) ou un périphérique perdu termine l'animation (fenêtre
/// restaurée, pas de repli en cours d'effet) ; la fin normale reste pilotée
/// par `MINUTEUR_FIN` / `MINUTEUR_GARDE`.
fn battement_effets(controle: isize, generation: Option<u32>) {
    let valide = avec(|s| match (&s.battement, generation) {
        (Some(Battement::Fil(m)), Some(g)) if m.generation() == g => {
            m.prendre_en_charge();
            true
        }
        (Some(Battement::Minuteur), None) => true,
        _ => false,
    });
    match valide {
        Some(true) => {}
        Some(false) => return,
        None => {
            // État momentanément occupé : le minuteur de secours (déjà tué
            // par la procédure) est reprogrammé ; le fil, lui, repostera.
            if generation.is_none() {
                programmer(controle, 0, MINUTEUR_BATTEMENT_SECOURS, PERIODE_BATTEMENT_SECOURS_MS);
            }
            return;
        }
    }
    let anims: Vec<AnimationActive> = avec(|s| {
        let ids: Vec<u32> = s
            .actives
            .iter()
            .filter(|(_, a)| a.besoin_rendu())
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| s.actives.remove(&id))
            .collect()
    })
    .unwrap_or_default();
    for mut anim in anims {
        match unsafe { anim.rendre() } {
            Ok(EtatRendu::DerniereImage) => {
                logging::info(&format!(
                    "Effet GPU #{} : dernière image rendue après {} ms (prévu : {:.0} ms).",
                    anim.id,
                    anim.debut.elapsed().as_millis(),
                    anim.duree * 1000.0
                ));
                stocker_active(anim);
            }
            Ok(EtatRendu::EnCours | EtatRendu::DejaFini) => stocker_active(anim),
            Err(EchecEffet::PeripheriquePerdu(e)) => {
                logging::avertir(&format!(
                    "Effet GPU #{} terminé : périphérique perdu — fenêtre restaurée, surimpression \
                     levée.",
                    anim.id
                ));
                fin_effet(controle, anim);
                gerer_erreur(e, "rendu d'un effet GPU");
            }
            Err(e) => {
                logging::avertir(&format!(
                    "Effet GPU #{} terminé : {e} — fenêtre restaurée, surimpression levée.",
                    anim.id
                ));
                fin_effet(controle, anim);
            }
        }
    }
    let besoin = avec(|s| s.actives.values().any(|a| a.besoin_rendu())).unwrap_or(false);
    if besoin {
        assurer_battement(controle);
        let secours = avec(|s| matches!(s.battement, Some(Battement::Minuteur))).unwrap_or(false);
        if secours {
            programmer(controle, 0, MINUTEUR_BATTEMENT_SECOURS, PERIODE_BATTEMENT_SECOURS_MS);
        }
    } else {
        arreter_battement(controle);
    }
}

fn duree_ms(secondes: f64) -> u32 {
    (secondes * 1000.0).clamp(0.0, 10_000.0).ceil() as u32
}

/// Fenêtre affichée : ouverture animée éventuelle, puis réévaluation du
/// suivi de fermeture si la fenêtre est (déjà) au premier plan — même
/// quand l'ouverture n'est pas animée (la fenêtre a pu être rejetée comme
/// « invisible » à son `EVENT_SYSTEM_FOREGROUND`, arrivé avant le SHOW).
fn sur_affichage(h: isize, horodatage: u32) {
    ouverture_sur_affichage(h, horodatage);
    apres_affichage(h);
    suivi_sur_affichage(h);
}

/// Suivi de fermeture sur `EVENT_OBJECT_SHOW` (voir [`sur_affichage`]).
fn suivi_sur_affichage(h: isize) {
    let Some((actif, deja, essai)) = avec(|s| {
        (
            s.reglages.fermeture_active()
                && matches!(s.gpu, Disponibilite::Prete(_))
                && s.crochets.is_some(),
            s.suivi.as_ref().map(|c| c.hwnd) == Some(h) || s.creation_pour(h).is_some(),
            s.reessai.filter(|(c, _)| *c == h).map(|(_, n)| n).unwrap_or(0),
        )
    }) else {
        return;
    };
    if !actif || deja {
        return;
    }
    let premier_plan = unsafe { GetForegroundWindow() };
    if premier_plan.0 as isize != h {
        return;
    }
    sur_premier_plan_essai(h, "EVENT_OBJECT_SHOW", essai);
}

/// Fenêtre affichée : ouverture animée éventuelle.
///
/// Mode précoce (défaut, v0.4.2) : la fenêtre est masquée (alpha 1 depuis
/// la v0.4.5) TOUT DE SUITE, avant la création (lente) de la session de
/// capture, pour qu'elle n'apparaisse jamais avant l'animation ; tout échec
/// ultérieur la restaure. L'effet attend ensuite l'image peinte (v0.4.6,
/// [`examiner_ouverture`]).
/// Mode « capture d'abord » (après bascule) : capture dédiée SANS modifier
/// la fenêtre (elle reste visible), masquage et effet dès la 1re image non
/// vide.
fn ouverture_sur_affichage(h: isize, horodatage: u32) {
    let Some(i) = instantane() else {
        return;
    };
    let Some(gpu) = i.gpu else {
        return;
    };
    if !i.reglages.ouverture_active() || i.ouverture_desactivee || i.controle == 0 {
        return;
    }
    let deja = avec(|s| {
        s.ouvertures.values().any(|o| o.cible == h) || s.actives.values().any(|a| a.cible == h)
    })
    .unwrap_or(true);
    if deja {
        return;
    }
    if let Err(raison) = unsafe { eligibilite::verifier(HWND(h as *mut core::ffi::c_void)) } {
        if affichage_digne_du_journal(h) {
            journaliser_rejet("Affichage (EVENT_OBJECT_SHOW)", h, &raison);
        }
        return;
    }
    if rebond_recent(h) {
        logging::info(&format!(
            "Ouverture de {h:#x} ignorée : SHOW dans les {REBOND_OUVERTURE_MS} ms suivant la \
             restauration d'une ouverture précoce (anti-rebond)."
        ));
        return;
    }
    if i.occupees >= MAX_SIMULTANEES {
        logging::info(&format!(
            "Ouverture de {h:#x} non animée : {} animation(s) déjà en cours.",
            i.occupees
        ));
        return;
    }
    let Some(id) = nouvel_id() else {
        return;
    };
    let debut = Instant::now();
    let latence = latence_evenement(horodatage)
        .map(|ms| format!("{ms} ms après l'événement"))
        .unwrap_or_else(|| "délai depuis l'événement inconnu".into());

    // v0.4.7 : transition native d'apparition de DWM désactivée (de
    // nouveau) AVANT le masquage précoce ; rétablie à la fin de
    // l'ouverture ou, si elle n'est pas animée, par `apres_affichage`.
    let texte_transition = if i.reglages.sans_transition_dwm {
        let avant = transitions::pose(h);
        let rapport = unsafe { transitions::desactiver(h, Moment::Affichage, None) };
        let vue = match avant {
            Some(p) if p.moment == Moment::Creation => match p.evenement_ms {
                Some(c) if horodatage != 0 => format!(
                    "posée à la CREATE (CREATE→SHOW = {} ms)",
                    transitions::delai_creation_affichage(c, horodatage)
                ),
                _ => "posée à la CREATE (CREATE→SHOW inconnu)".to_string(),
            },
            Some(_) => "déjà posée à un SHOW précédent".to_string(),
            None => "non vue à la CREATE".to_string(),
        };
        programmer_balayage(i.controle);
        format!(
            " ; transition DWM : {vue} ; appel au SHOW = {} ; origine {}",
            rapport.texte_appel(),
            texte_lecture(rapport.origine)
        )
    } else {
        " ; transition DWM : non désactivée (desactiver_transition_dwm = false)".to_string()
    };

    let masquee = match i.mode {
        Mode::CaptureDabord => {
            logging::info(&format!(
                "Affichage (EVENT_OBJECT_SHOW, {latence}) : {} éligible — ouverture #{id} : \
                 mode « capture d'abord », fenêtre laissée visible{texte_transition}.",
                decrire(h)
            ));
            false
        }
        Mode::Precoce => {
            // Fenêtre déjà masquée par une autre animation (effet Jelly) :
            // pas d'ouverture animée (`masquer` le refuserait aussi).
            if masquage::est_masquee(h) {
                logging::info(&format!(
                    "Ouverture #{id} de {h:#x} non animée : fenêtre déjà masquée par une autre \
                     animation."
                ));
                return;
            }
            if !fenetre_repond(h) {
                logging::info(&format!(
                    "Ouverture #{id} de {h:#x} non animée (masquage précoce) : la fenêtre ne \
                     répond pas en {DELAI_REPONSE_MS} ms (application figée ?)."
                ));
                return;
            }
            // État d'origine inscrit dans la liste statique AVANT
            // modification (restauration garantie : arrêt, panique...).
            match unsafe { masquage::masquer_avec(h, ALPHA_OUVERTURE) } {
                ResultatMasquage::Masquee(origine) => {
                    avec(|s| s.echecs_masquage = 0);
                    logging::info(&format!(
                        "Affichage (EVENT_OBJECT_SHOW, {latence}) : {} éligible — ouverture \
                         #{id} : masquage précoce (alpha {ALPHA_OUVERTURE}/255) posé en {} ms \
                         (origine : {origine}) ; capture ensuite{texte_transition}.",
                        decrire(h),
                        debut.elapsed().as_millis()
                    ));
                    true
                }
                ResultatMasquage::NonMasquable(refus) => {
                    logging::info(&format!(
                        "Ouverture #{id} de {h:#x} non animée (masquage précoce) : {refus}."
                    ));
                    return;
                }
                ResultatMasquage::Echec(message) => {
                    enregistrer_echec_masquage(h, &message);
                    return;
                }
            }
        }
    };
    // Sonde du palier 0 (après le masquage précoce, ou après la pose en
    // « capture d'abord »).
    let mut sondes = Sondes::default();
    sonder_si_du(&mut sondes, h, debut);

    let role = format!("ouverture #{id}");
    let creation = if masquee {
        // Mode précoce : création bornée, sans repli non borné dans le fil
        // principal (la fenêtre est invisible pendant ce temps).
        capture::demarrer_session_bornee(
            &gpu,
            h,
            Some((i.controle, id)),
            &role,
            DELAI_SESSION_PRECOCE_MS,
        )
    } else {
        capture::demarrer_session(&gpu, h, Some((i.controle, id)), &role)
    };
    match creation {
        Ok(session) => {
            let mode_attente = if masquee {
                ModeAttente::Stabilite
            } else {
                ModeAttente::PremiereImage
            };
            logging::info(&format!(
                "Ouverture #{id} : session de capture créée {} ms après le SHOW ({}) ; attente \
                 de l'image peinte : {}.",
                debut.elapsed().as_millis(),
                if masquee {
                    "fenêtre masquée"
                } else {
                    "fenêtre visible"
                },
                texte_politique(mode_attente)
            ));
            let ouverture = OuvertureEnAttente {
                cible: h,
                session,
                masquee,
                debut,
                attente: Attente::default(),
                sondes,
            };
            if !programmer(i.controle, id, MINUTEUR_GARDE, GARDE_FOU_MS) {
                annuler_ouverture(i.controle, id, ouverture, "minuteur de garde impossible");
                return;
            }
            if !programmer(i.controle, id, MINUTEUR_ATTENTE, attente::TICK_ATTENTE_MS) {
                annuler_ouverture(i.controle, id, ouverture, "minuteur d'attente impossible");
                return;
            }
            stocker_ouverture(i.controle, id, ouverture);
        }
        Err(e) => {
            if masquee {
                let restauree = unsafe { masquage::restaurer(h) };
                noter_restauration_precoce(h);
                surveiller_restaurations(i.controle);
                logging::info(&format!(
                    "Ouverture #{id} de {h:#x} abandonnée : capture impossible {} ms après le \
                     SHOW — {}.",
                    debut.elapsed().as_millis(),
                    if restauree {
                        "fenêtre restaurée"
                    } else {
                        "masquage déjà levé"
                    }
                ));
            }
            gerer_erreur(e, "capture pour l'animation d'ouverture");
        }
    }
}

/// Échec du masquage par transparence temporaire : journal + compteur ;
/// au-delà de [`SEUIL_ECHECS_MASQUAGE`] échecs consécutifs, l'ouverture est
/// désactivée (une notification unique).
fn enregistrer_echec_masquage(cible: isize, message: &str) {
    // Fenêtre disparue : ce n'est pas un refus de Windows -> ni compteur,
    // ni notification, ni désactivation.
    let disparue = !unsafe { IsWindow(Some(HWND(cible as *mut core::ffi::c_void))) }.as_bool()
        || message.contains("disparue");
    if disparue {
        logging::info(&format!(
            "Masquage de {cible:#x} non effectué : {message} (fenêtre disparue, échec non \
             compté)."
        ));
        return;
    }
    let (n, desactiver) = avec(|s| {
        s.echecs_masquage = s.echecs_masquage.saturating_add(1);
        let n = s.echecs_masquage;
        let desactiver = n >= SEUIL_ECHECS_MASQUAGE && !s.ouverture_desactivee;
        if desactiver {
            s.ouverture_desactivee = true;
        }
        (n, desactiver)
    })
    .unwrap_or((0, false));
    logging::avertir(&format!(
        "Masquage par transparence temporaire refusé sur {cible:#x} : {message} \
         (échec consécutif {n}/{SEUIL_ECHECS_MASQUAGE}) — fenêtre restaurée, non animée."
    ));
    if desactiver {
        logging::erreur(&format!(
            "Animation d'ouverture désactivée : {SEUIL_ECHECS_MASQUAGE} échecs consécutifs du \
             masquage par transparence temporaire."
        ));
        tray::notifier(
            "L'animation d'ouverture est désactivée : Windows refuse le masquage par \
             transparence temporaire des fenêtres des autres applications. \
             Décochez puis recochez « Animer l'ouverture » pour réessayer.",
        );
        // Retire le crochet des créations et rétablit les poses de
        // transition DWM non liées (journalise aussi l'état).
        reevaluer("ouverture désactivée");
    }
}

/// Politique d'attente lisible pour le journal.
fn texte_politique(mode: ModeAttente) -> String {
    match mode {
        ModeAttente::Stabilite => format!(
            "image la plus récente stable (aucune nouvelle image depuis {} ms, {} ms si une \
             seule image) et non vide, plafond {} ms après le SHOW",
            attente::DELAI_STABILITE_MS,
            attente::DELAI_STABILITE_PREMIERE_MS,
            attente::PLAFOND_ATTENTE_MS
        ),
        ModeAttente::PremiereImage => format!(
            "1re image non vide (mode « capture d'abord »), {} ms au plus",
            attente::PLAFOND_PREMIERE_IMAGE_MS
        ),
    }
}

/// Raison du choix de l'image lisible pour le journal.
fn texte_choix(raison: RaisonChoix) -> String {
    match raison {
        RaisonChoix::Stable => "stable".to_string(),
        RaisonChoix::Plafond => format!(
            "plafond de {} ms atteint (image non vide mais encore changeante)",
            attente::PLAFOND_ATTENTE_MS
        ),
        RaisonChoix::PremiereImage => "1re image (mode « capture d'abord »)".to_string(),
        RaisonChoix::PlafondUniforme => format!(
            "plafond de {} ms atteint, image uniforme mais non transparente et stable",
            attente::PLAFOND_ATTENTE_MS
        ),
    }
}

/// Tick de l'attente d'une ouverture (`MINUTEUR_ATTENTE`, ou 1re image
/// signalée par `WM_TRAME_PRETE`) : décision pure ([`Attente::decider`]) ;
/// analyse de l'image la plus récente seulement quand une décision est
/// possible ; puis nouvelle attente, lancement de l'effet avec l'image la
/// plus récente, ou abandon (effet non joué, fenêtre restaurée).
fn examiner_ouverture(controle: isize, id: u32) {
    let mut ouverture = match avec(|s| s.ouvertures.remove(&id)) {
        Some(Some(o)) => o,
        Some(None) => return, // déjà lancée ou annulée (délai, destruction, arrêt...)
        None => {
            // État indisponible : l'ouverture n'est pas confirmée absente,
            // réessai court (le garde-fou reste en place).
            if !programmer(controle, id, MINUTEUR_ATTENTE, 50) {
                logging::avertir(&format!(
                    "Ouverture #{id} : attente non reprogrammable (état indisponible)."
                ));
            }
            return;
        }
    };
    tuer(controle, id, MINUTEUR_ATTENTE);
    let Some(i) = instantane() else {
        annuler_ouverture(controle, id, ouverture, "état indisponible");
        return;
    };
    let cible = ouverture.cible;
    let Some(gpu) = i.gpu.clone() else {
        annuler_ouverture(controle, id, ouverture, "GPU indisponible");
        return;
    };
    if !i.reglages.ouverture_active() || i.ouverture_desactivee {
        annuler_ouverture(controle, id, ouverture, "animation d'ouverture inactive");
        return;
    }
    let hwnd = HWND(cible as *mut core::ffi::c_void);
    let utilisable = unsafe {
        IsWindow(Some(hwnd)).as_bool()
            && IsWindowVisible(hwnd).as_bool()
            && !IsIconic(hwnd).as_bool()
    };
    if !utilisable {
        annuler_ouverture(
            controle,
            id,
            ouverture,
            "fenêtre détruite, masquée ou réduite pendant l'attente de l'image",
        );
        return;
    }
    if ouverture.masquee && !masquage::est_masquee(cible) {
        annuler_ouverture(controle, id, ouverture, "masquage précoce levé entre-temps");
        return;
    }
    sonder_si_du(&mut ouverture.sondes, cible, ouverture.debut);
    let mode = if ouverture.masquee {
        ModeAttente::Stabilite
    } else {
        ModeAttente::PremiereImage
    };

    // Image la plus récente prise en main pour l'analyse : rendue à la
    // session si l'attente continue, utilisée telle quelle si l'effet part.
    let mut en_main: Option<capture::DerniereTrame> = None;
    let mut action;
    let mut obs;
    let mut passes = 0;
    loop {
        obs = Observation {
            ecoule_ms: ouverture.debut.elapsed().as_millis() as u64,
            images: ouverture.session.trames_recues(),
            silence_ms: ouverture.session.age_derniere_image_ms(),
        };
        action = ouverture.attente.decider(obs, mode);
        if action != Action::Analyser {
            break;
        }
        passes += 1;
        if let Some(t) = en_main.take() {
            ouverture.session.remettre_trame(t);
        }
        // Au plus deux analyses par tick ; image pas encore déposée par le
        // fil WGC (compteur incrémenté juste avant) ou déjà analysée :
        // tick suivant.
        let trame = if passes <= 2 {
            ouverture.session.prendre_trame()
        } else {
            None
        };
        let Some(trame) = trame else {
            action = Action::Attendre;
            break;
        };
        if ouverture.attente.numero_analyse() == Some(trame.numero) {
            ouverture.session.remettre_trame(trame);
            action = Action::Attendre;
            break;
        }
        let contenu = match unsafe { capture::analyser_trame(&gpu, &trame) } {
            Ok(c) => c,
            Err(t) => {
                logging::avertir(&format!(
                    "Ouverture #{id} de {cible:#x} : analyse de l'image n°{} impossible ({t}) — \
                     image considérée comme porteuse de contenu.",
                    trame.numero
                ));
                ContenuImage::Contenu
            }
        };
        ouverture.attente.noter_analyse(trame.numero, contenu);
        en_main = Some(trame);
    }

    match action {
        Action::Attendre | Action::Analyser => {
            if let Some(t) = en_main.take() {
                ouverture.session.remettre_trame(t);
            }
            if programmer(controle, id, MINUTEUR_ATTENTE, attente::TICK_ATTENTE_MS) {
                stocker_ouverture(controle, id, ouverture);
            } else {
                annuler_ouverture(controle, id, ouverture, "minuteur d'attente impossible");
            }
        }
        Action::Abandonner(raison) => {
            drop(en_main);
            abandonner_attente(controle, id, ouverture, obs, raison);
        }
        Action::Jouer(raison) => {
            let trame = en_main.or_else(|| ouverture.session.prendre_trame());
            let Some(trame) = trame else {
                annuler_ouverture(controle, id, ouverture, "image absente au lancement de l'effet");
                return;
            };
            // Une image plus récente, non analysée, a pu arriver entre la
            // décision et la prise : elle sera analysée au tick suivant.
            if ouverture.attente.numero_analyse() != Some(trame.numero) {
                ouverture.session.remettre_trame(trame);
                if programmer(controle, id, MINUTEUR_ATTENTE, attente::TICK_ATTENTE_MS) {
                    stocker_ouverture(controle, id, ouverture);
                } else {
                    annuler_ouverture(controle, id, ouverture, "minuteur d'attente impossible");
                }
                return;
            }
            logging::info(&format!(
                "Ouverture #{id} de {cible:#x} : image retenue {} ms après le SHOW — image n°{} \
                 sur {} reçue(s), dernière image reçue il y a {} ms, raison : {} ; contenu : {} ; \
                 {} analyse(s) ; image {}x{}, effet {}.",
                obs.ecoule_ms,
                trame.numero,
                obs.images,
                obs.silence_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                texte_choix(raison),
                ouverture
                    .attente
                    .contenu_analyse()
                    .map(ContenuImage::texte)
                    .unwrap_or("non analysé"),
                ouverture.attente.analyses,
                trame.taille.0,
                trame.taille.1,
                i.reglages.effet.nom_config()
            ));
            let fin = ouverture.debut.elapsed().as_millis() as u64;
            logging::info(&format!(
                "Ouverture #{id} — sondes (hypothèse transition DWM ; effet lancé à T+{fin}) : {}.",
                ouverture.sondes.texte(fin)
            ));
            jouer_ouverture(&i, controle, id, ouverture, trame);
        }
    }
}

/// Fin de l'attente sans image acceptable : effet NON joué, fenêtre
/// restaurée immédiatement (si masquée par nous) ; compteurs du mode
/// précoce mis à jour (aucune image / image transparente).
fn abandonner_attente(
    controle: isize,
    id: u32,
    ouverture: OuvertureEnAttente,
    obs: Observation,
    raison: RaisonAbandon,
) {
    let masquee = ouverture.masquee;
    let analyses = ouverture.attente.analyses;
    let quoi = match raison {
        RaisonAbandon::AucuneImage => "aucune image reçue".to_string(),
        RaisonAbandon::ImageVide(c) => format!("dernière image {}", c.texte()),
    };
    annuler_ouverture(
        controle,
        id,
        ouverture,
        &format!(
            "effet NON joué — {quoi} ({} ms après le SHOW, {} image(s) reçue(s), {analyses} \
             analyse(s) ; politique : {})",
            obs.ecoule_ms,
            obs.images,
            texte_politique(if masquee {
                ModeAttente::Stabilite
            } else {
                ModeAttente::PremiereImage
            })
        ),
    );
    if !masquee {
        return;
    }
    let bascule = match raison {
        RaisonAbandon::AucuneImage => avec(|s| s.adaptation.delai_expire()).flatten(),
        RaisonAbandon::ImageVide(ContenuImage::Transparente) => {
            // Seule une image transparente d'une fenêtre STABLE est
            // probante ; une appli lente encore en train de se peindre au
            // plafond ne compte pas pour la bascule.
            if !precoce::vide_probante(obs.silence_ms) {
                logging::info(&format!(
                    "Ouverture #{id} : image transparente non probante (dernière image reçue \
                     il y a {} ms < {} ms : fenêtre encore changeante) — non comptée pour la \
                     bascule en « capture d'abord ».",
                    obs.silence_ms
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "?".into()),
                    attente::DELAI_STABILITE_PREMIERE_MS
                ));
            }
            avec(|s| s.adaptation.image_recue(true, obs.silence_ms)).flatten()
        }
        RaisonAbandon::ImageVide(_) => None,
    };
    match bascule {
        Some(r) => journaliser_bascule(r),
        None => {
            if let Some(a) = avec(|s| texte_mode(&s.adaptation)) {
                logging::info(&format!("Ouverture #{id} : mode d'ouverture = {a}."));
            }
        }
    }
}

/// Lance l'effet avec l'image retenue ; la session d'ouverture est fermée
/// après la copie de l'image dans la surimpression. Échec : fenêtre
/// restaurée immédiatement.
fn jouer_ouverture(
    i: &Instantane,
    controle: isize,
    id: u32,
    ouverture: OuvertureEnAttente,
    trame: capture::DerniereTrame,
) {
    let resultat = lancer_ouverture(i, id, &ouverture, trame);
    let OuvertureEnAttente {
        cible,
        session,
        masquee,
        ..
    } = ouverture;
    // L'image a été copiée (ou abandonnée) : la session peut être fermée.
    drop(session);
    match resultat {
        Ok(Some((anim, duree))) => {
            if masquee {
                avec(|s| s.adaptation.image_recue(false, None));
            }
            logging::info(&format!(
                "Ouverture #{id} de {cible:#x} : effet joué ({:.0} ms) ; restauration immédiate \
                 de la fenêtre et levée de la surimpression à la fin de l'effet.",
                duree * 1000.0
            ));
            let fin = duree_ms(duree) + MARGE_FIN_MS;
            // Garde-fou réarmé pour l'effet (remplace celui de l'attente).
            programmer(controle, id, MINUTEUR_GARDE, fin + MARGE_GARDE_EFFET_MS);
            if programmer(controle, id, MINUTEUR_FIN, fin) {
                let shader = anim.est_shader();
                stocker_active(anim);
                if shader {
                    ajuster_battement(controle);
                }
            } else {
                // Pas de minuteur de fin : fin immédiate (restaure la fenêtre).
                finir_active(controle, anim);
            }
        }
        autre => {
            tuer(controle, id, MINUTEUR_GARDE);
            // Mode précoce : la fenêtre ne doit JAMAIS rester invisible
            // (restauration idempotente : sans effet si déjà faite).
            if masquee {
                if unsafe { masquage::restaurer(cible) } {
                    logging::info(&format!(
                        "Ouverture #{id} de {cible:#x} abandonnée : fenêtre restaurée (masquage \
                         précoce)."
                    ));
                }
                noter_restauration_precoce(cible);
            }
            liberer_transition(cible, &format!("lancement de l'ouverture #{id} impossible"));
            surveiller_restaurations(controle);
            if let Err(e) = autre {
                gerer_erreur(e, "animation d'ouverture");
            }
        }
    }
}

/// Prépare et lance l'animation d'ouverture avec l'image retenue : relecture
/// des bornes, plan, masquage par transparence temporaire (sauf si déjà
/// masquée par CETTE ouverture en mode précoce), surimpression.
/// `Ok(None)` = ouverture abandonnée proprement (déjà journalisé ; en mode
/// précoce, l'APPELANT restaure la fenêtre — `masquage::restaurer` est
/// idempotent). En cas d'erreur, la fenêtre est restaurée ici.
fn lancer_ouverture(
    i: &Instantane,
    id: u32,
    ouverture: &OuvertureEnAttente,
    mut trame: capture::DerniereTrame,
) -> Result<Option<(AnimationActive, f64)>, ErreurAnimation> {
    let cible = ouverture.cible;
    let Some(gpu) = i.gpu.as_ref() else {
        return Ok(None);
    };
    ouverture.session.rafraichir_bornes();
    trame.bornes = ouverture.session.bornes();
    let taille = Taille {
        largeur: trame.taille.0,
        hauteur: trame.taille.1,
    };
    let Some(plan) = plan::construire_plan(i.reglages.effet, Sens::Ouverture, taille) else {
        logging::info(&format!(
            "Ouverture #{id} de {cible:#x} abandonnée : plan impossible ({}x{}).",
            taille.largeur, taille.hauteur
        ));
        return Ok(None);
    };

    if ouverture.masquee {
        // Mode précoce : la fenêtre doit être encore masquée par nous.
        if !masquage::est_masquee(cible) {
            logging::info(&format!(
                "Ouverture #{id} de {cible:#x} abandonnée : masquage précoce levé entre-temps."
            ));
            return Ok(None);
        }
        return lancer_surimpression(gpu, id, cible, &trame, plan, taille);
    }

    // Mode « capture d'abord ».
    // Défense en profondeur : fenêtre déjà masquée par une autre animation
    // (effet Jelly à alpha 1) -> pas d'ouverture animée (`masquer` le
    // refuserait aussi, sous verrou).
    if masquage::est_masquee(cible) {
        logging::info(&format!(
            "Ouverture #{id} de {cible:#x} non animée : fenêtre déjà masquée par une autre \
             animation."
        ));
        return Ok(None);
    }

    if !fenetre_repond(cible) {
        logging::info(&format!(
            "Ouverture #{id} de {cible:#x} non animée : la fenêtre ne répond pas en \
             {DELAI_REPONSE_MS} ms (application figée ?)."
        ));
        return Ok(None);
    }
    // Masquage par transparence temporaire (état d'origine inscrit dans la
    // liste statique AVANT modification).
    match unsafe { masquage::masquer_avec(cible, ALPHA_OUVERTURE) } {
        ResultatMasquage::Masquee(origine) => {
            avec(|s| s.echecs_masquage = 0);
            logging::info(&format!(
                "Ouverture #{id} : {cible:#x} masquée par transparence temporaire (alpha \
                 {ALPHA_OUVERTURE}/255, origine : {origine}) ; image {}x{}, effet {}, {:.0} ms.",
                taille.largeur,
                taille.hauteur,
                i.reglages.effet.nom_config(),
                plan.duree() * 1000.0
            ));
        }
        ResultatMasquage::NonMasquable(refus) => {
            logging::info(&format!("Ouverture #{id} de {cible:#x} non animée : {refus}."));
            return Ok(None);
        }
        ResultatMasquage::Echec(message) => {
            enregistrer_echec_masquage(cible, &message);
            return Ok(None);
        }
    }

    lancer_surimpression(gpu, id, cible, &trame, plan, taille)
}

/// Pose la surimpression d'ouverture au-dessus d'une fenêtre masquée par
/// nous ; en cas d'échec, la fenêtre est restaurée immédiatement.
fn lancer_surimpression(
    gpu: &ContexteGpu,
    id: u32,
    cible: isize,
    trame: &capture::DerniereTrame,
    plan: Plan,
    taille: Taille,
) -> Result<Option<(AnimationActive, f64)>, ErreurAnimation> {
    match lancer_avec_repli(gpu, id, Sens::Ouverture, cible, trame, plan, taille) {
        Ok((mut anim, duree)) => {
            // La vraie fenêtre reste invisible jusqu'à la fin de l'effet ;
            // `surimpression::terminer` la restaurera.
            anim.masquee_par_nous = true;
            Ok(Some((anim, duree)))
        }
        Err(e) => {
            // Restauration immédiate : jamais de fenêtre invisible sans
            // surimpression par-dessus.
            let _ = unsafe { masquage::restaurer(cible) };
            Err(e)
        }
    }
}

/// Première image d'une ouverture disponible (`WM_TRAME_PRETE`) : examen
/// immédiat (en « capture d'abord », l'effet part dès cette image ; en
/// masquage précoce, l'attente de stabilité continue).
fn trame_prete(controle: isize, id: u32) {
    examiner_ouverture(controle, id);
}

/// Minuteur expiré (déjà tué par la procédure de fenêtre).
fn minuteur(controle: isize, id_win32: usize) {
    let id = (id_win32 / NB_GENRES) as u32;
    let genre = id_win32 % NB_GENRES;
    if id == 0 {
        // Minuteurs globaux (les animations commencent à 1).
        match genre {
            MINUTEUR_REESSAI => reessayer(),
            MINUTEUR_SURVEILLANCE => surveiller(controle),
            MINUTEUR_TRANSITIONS => balayer_transitions(controle),
            MINUTEUR_BATTEMENT_SECOURS => battement_effets(controle, None),
            MINUTEUR_RESTAURATIONS => {
                let restantes = masquage::reessayer_restaurations();
                if restantes > 0
                    && !programmer(controle, 0, MINUTEUR_RESTAURATIONS, PERIODE_RESTAURATIONS_MS)
                {
                    logging::erreur(&format!(
                        "Ouverture : {restantes} restauration(s) en échec, nouvel essai \
                         impossible (SetTimer) — nouvel essai à l'arrêt."
                    ));
                }
            }
            _ => {}
        }
        return;
    }
    match genre {
        MINUTEUR_FIN => {
            let retrait = avec(|s| s.actives.remove(&id));
            if let Some(Some(anim)) = retrait {
                fin_effet(controle, anim);
                ajuster_battement(controle);
            } else if retrait.is_none() {
                // État momentanément indisponible : l'animation n'est pas
                // confirmée absente, on garde le garde-fou et on réessaie.
                if !programmer(controle, id, MINUTEUR_FIN, 50) {
                    logging::avertir("Animations : fin différée impossible (garde-fou conservé).");
                }
            } else {
                // Absence confirmée (déjà terminée) : le garde-fou est inutile.
                tuer(controle, id, MINUTEUR_GARDE);
            }
        }
        MINUTEUR_ATTENTE => examiner_ouverture(controle, id),
        MINUTEUR_GARDE => {
            let extrait = avec(|s| (s.ouvertures.remove(&id), s.actives.remove(&id)));
            if extrait.is_none() {
                // État momentanément indisponible : le garde-fou n'a rien
                // pu vérifier, on le reprogramme (comme MINUTEUR_FIN).
                if !programmer(controle, id, MINUTEUR_GARDE, 50) {
                    logging::avertir(&format!(
                        "Animations : garde-fou #{id} non reprogrammable (état indisponible)."
                    ));
                }
            }
            if let Some((o, a)) = extrait {
                if o.is_some() || a.is_some() {
                    logging::avertir(&format!(
                        "Animations : garde-fou déclenché (#{id}, {}) — restauration.",
                        if o.is_some() {
                            "attente de l'image d'ouverture"
                        } else {
                            "effet en cours"
                        }
                    ));
                }
                if let Some(o) = o {
                    annuler_ouverture(controle, id, o, "garde-fou");
                }
                if let Some(a) = a {
                    finir_active(controle, a);
                    ajuster_battement(controle);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Arrêt
// ---------------------------------------------------------------------------

/// Arrêt complet : décroche les crochets, termine les animations, restaure
/// toutes les fenêtres masquées et ferme les captures. Appelé à la
/// fermeture de l'application et de la session Windows.
pub fn arreter() {
    let extrait = ETAT.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(|s| {
            (
                s.controle,
                s.crochets.take(),
                s.extraire_suivis(),
                s.ouvertures.drain().collect::<Vec<_>>(),
                s.actives.drain().map(|(_, a)| a).collect::<Vec<_>>(),
            )
        }),
        Err(_) => {
            logging::avertir("Arrêt des animations : état occupé, restauration d'urgence.");
            None
        }
    });
    jeter_crochets_en_suspens();
    if let Some((controle, crochets, suivis, ouvertures, actives)) = extrait {
        if crochets.is_some() {
            logging::info(&format!(
                "Animations : crochets d'événements retirés (arrêt) — {}.",
                evenements::compteurs_creations().texte()
            ));
        }
        drop(crochets);
        drop(suivis);
        for (id, o) in ouvertures {
            annuler_ouverture(controle, id, o, "arrêt");
        }
        for a in actives {
            finir_active(controle, a);
        }
        // Effets à shader : cadence arrêtée (plus aucune animation).
        arreter_battement(controle);
    }
    // Effet Jelly : termine l'effet en cours (restauration), retire son
    // crochet et son minuteur.
    jelly::arreter();
    // Filet de sécurité : toute fenêtre encore masquée par nous (liste
    // statique, sans RefCell, en try_lock).
    masquage::restaurer_tout(true);
    // Transitions DWM désactivées par nous (registre statique, try_lock).
    transitions::retablir_tout(true);
}

/// Reprise après une fermeture de session annulée (réinstalle les crochets).
pub fn reprendre() {
    reevaluer("fermeture de session annulée");
    jelly::reprendre();
}

/// Vrai si `hwnd` est momentanément masquée (alpha 1 temporaire depuis la
/// v0.4.5) par une animation (ouverture, Jelly). Le module de transparence s'abstient alors de la modifier
/// (il réessaiera plus tard). Ne bloque jamais, n'emprunte aucun `RefCell` ;
/// verrou indisponible -> `true` par prudence (voir `masquage::est_masquee`).
pub fn fenetre_masquee_par_animation(hwnd: HWND) -> bool {
    masquage::est_masquee(hwnd.0 as isize)
}

/// Crochet de panique : restaure toutes les fenêtres masquées par nous.
/// N'emprunte aucun `RefCell` (liste statique, `try_lock`) et ne
/// journalise rien (le crochet a déjà écrit la panique).
pub fn urgence_restaurer() {
    masquage::restaurer_tout(false);
    transitions::retablir_tout(false);
}
