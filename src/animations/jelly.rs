//! Effet « Jelly » : la fenêtre déplacée par glisser de sa barre de titre
//! se déforme comme une gelée (le point saisi suit la souris, le reste
//! traîne), puis rebondit de façon amortie une fois relâchée.
//!
//! Chaîne (aucune injection, aucun rendu CPU) :
//! - détection : crochet `SetWinEventHook` DÉDIÉ, hors contexte,
//!   `EVENT_SYSTEM_MOVESIZESTART` / `EVENT_SYSTEM_MOVESIZEEND` ; le rappel
//!   poste un seul message à la fenêtre de contrôle du Jelly ;
//! - contenu : une image Windows.Graphics.Capture prise au début du
//!   glisser (texture GPU) ;
//! - simulation : masses-ressorts ([`ressort`](super::ressort), pur) ;
//! - rendu (v0.4.1) : maillage courbe Direct3D 11 dans une swapchain de
//!   composition ([`grille_rendu`](super::grille_rendu), pur ;
//!   [`maillage_gpu`](super::maillage_gpu)), ENTIÈREMENT sur le GPU
//!   matériel. Aucun rendu de repli : si le pipeline, la swapchain, la
//!   composition ou le périphérique échouent, le Jelly est désactivé pour la
//!   session (notification unique, réarmé en recochant la case) ;
//! - cadence : un fil de battement ([`metronome`](super::metronome)),
//!   présent seulement pendant l'effet, attend chaque image du compositeur
//!   (`DCompositionWaitForCompositorClock`, sinon `DwmFlush`) et poste un
//!   message coalescé ; le pas (simulation + rendu) reste dans le fil
//!   principal. Repli : minuteur `SetTimer` 15 ms si le fil ne démarre pas ;
//! - la vraie fenêtre est rendue quasi invisible pendant l'effet par
//!   transparence temporaire à alpha 1 (et non 0 : une fenêtre layered à
//!   alpha 0 laisse passer les clics, à alpha 1 elle reste cliquable), voir
//!   [`masquage`](super::masquage) ; restauration garantie (fin, arrêt,
//!   fin de session, panique via `masquage::restaurer_tout`).
//!
//! Cycle : `Repos` → (MOVESIZESTART, fenêtre éligible, capture démarrée)
//! `Armement` → (déplacement ≥ 2 px + image reçue, surimpression posée
//! PUIS fenêtre masquée) `Actif` → (fin du glisser) retour amorti →
//! (repos, clic, délai, disparition, redimensionnement…) `Repos`.
//!
//! La partie décisionnelle ci-dessous est pure (testable hors Windows) ;
//! la partie Win32 est dans le sous-module `win32`.

use std::fmt;

/// Durée maximale du retour amorti après la fin du glisser.
pub const DUREE_RETOUR_MAX_MS: u64 = 1200;
/// Durée maximale d'un effet (glisser compris) avant restauration forcée.
/// Un glisser plus long termine l'effet (la fenêtre redevient normale et
/// continue de suivre la souris sans déformation).
///
/// LIMITE CONNUE : si notre processus est tué (Gestionnaire des tâches,
/// plantage hors panique Rust) pendant l'effet, aucune restauration n'a
/// lieu et la fenêtre glissée reste layered à alpha 1, donc quasi
/// invisible (mais cliquable). La ramener : relancer RustPiz
/// n'y suffit pas (état non persistant) ; il faut fermer / rouvrir la
/// fenêtre. Ce garde-fou court limite seulement la durée d'exposition.
pub const GARDE_FOU_MS: u64 = 30_000;
/// Durée maximale de l'armement (glisser commencé, aucun déplacement
/// détecté, fin non reçue) avant abandon.
pub const DELAI_ARMEMENT_MS: u64 = 2_000;
/// Attente maximale de la création de la capture d'armement (fil
/// principal bloqué au plus ce temps, au début d'un glisser).
pub const DELAI_CAPTURE_MS: u64 = 250;

/// Rectangle d'écran (pixels physiques).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectEcran {
    pub gauche: i32,
    pub haut: i32,
    pub droite: i32,
    pub bas: i32,
}

impl RectEcran {
    pub fn largeur(&self) -> i32 {
        self.droite - self.gauche
    }
    pub fn hauteur(&self) -> i32 {
        self.bas - self.haut
    }
}

impl fmt::Display for RectEcran {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, {}) {}x{}",
            self.gauche,
            self.haut,
            self.largeur(),
            self.hauteur()
        )
    }
}

/// Nature du geste en cours (déplacement ou redimensionnement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geste {
    /// Pas encore assez de mouvement pour trancher.
    Indetermine,
    Deplacement,
    Redimensionnement,
}

/// Classe le geste d'après les bornes initiales et courantes : taille
/// changée de plus d'1 px -> redimensionnement ; sinon décalage d'au moins
/// `seuil_px` sur un axe -> déplacement ; sinon indéterminé.
pub fn classer_geste(initial: &RectEcran, courant: &RectEcran, seuil_px: i32) -> Geste {
    if (courant.largeur() - initial.largeur()).abs() > 1
        || (courant.hauteur() - initial.hauteur()).abs() > 1
    {
        return Geste::Redimensionnement;
    }
    let dx = (courant.gauche - initial.gauche).abs();
    let dy = (courant.haut - initial.haut).abs();
    if dx.max(dy) >= seuil_px.max(1) {
        Geste::Deplacement
    } else {
        Geste::Indetermine
    }
}

/// Motif de fin d'un effet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotifFin {
    FenetreDisparue,
    MasqueeOuReduite,
    Redimensionnee,
    Clic,
    Repos,
    DelaiRetour,
    GardeFou,
    Arret,
    Erreur,
    NouvelleCible,
}

impl fmt::Display for MotifFin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let texte = match self {
            MotifFin::FenetreDisparue => "fenêtre disparue",
            MotifFin::MasqueeOuReduite => "fenêtre masquée ou réduite",
            MotifFin::Redimensionnee => "fenêtre redimensionnée (ancrage, agrandissement, DPI…)",
            MotifFin::Clic => "clic pendant le retour",
            MotifFin::Repos => "retour au repos",
            MotifFin::DelaiRetour => "délai maximal du retour atteint",
            MotifFin::GardeFou => "garde-fou",
            MotifFin::Arret => "arrêt / réglages modifiés",
            MotifFin::Erreur => "erreur",
            MotifFin::NouvelleCible => "glisser d'une autre fenêtre",
        };
        f.write_str(texte)
    }
}

/// Décision prise à chaque pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Continuer,
    CommencerRetour,
    Terminer(MotifFin),
}

/// Observations relevées à chaque pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    pub existe: bool,
    pub visible_non_reduite: bool,
    pub taille_changee: bool,
    /// Glisser toujours en cours (ni MOVESIZEEND reçu, ni fin constatée).
    pub en_deplacement: bool,
    pub bouton_presse: bool,
    pub au_repos: bool,
    pub en_retour: bool,
    pub duree_retour_ms: u64,
    pub duree_totale_ms: u64,
}

/// Décision d'après les observations. Priorités : disparue > masquée /
/// réduite > redimensionnée > garde-fou > (en retour : clic > repos >
/// délai) > (glisser fini -> début du retour).
pub fn decider(o: &Observation) -> Decision {
    if !o.existe {
        return Decision::Terminer(MotifFin::FenetreDisparue);
    }
    if !o.visible_non_reduite {
        return Decision::Terminer(MotifFin::MasqueeOuReduite);
    }
    if o.taille_changee {
        return Decision::Terminer(MotifFin::Redimensionnee);
    }
    if o.duree_totale_ms >= GARDE_FOU_MS {
        return Decision::Terminer(MotifFin::GardeFou);
    }
    if o.en_retour {
        if o.bouton_presse {
            return Decision::Terminer(MotifFin::Clic);
        }
        if o.au_repos {
            return Decision::Terminer(MotifFin::Repos);
        }
        if o.duree_retour_ms >= DUREE_RETOUR_MAX_MS {
            return Decision::Terminer(MotifFin::DelaiRetour);
        }
        return Decision::Continuer;
    }
    if !o.en_deplacement {
        return Decision::CommencerRetour;
    }
    Decision::Continuer
}

/// Nature d'un échec du rendu Direct3D du Jelly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatureEchec {
    /// Propre à cette fenêtre / cette image (taille, ressource, fenêtre de
    /// surimpression, composition...) : seul cet effet est abandonné.
    Ponctuel,
    /// Périphérique graphique perdu (`DXGI_ERROR_DEVICE_REMOVED`, `RESET`,
    /// `HUNG`, erreur interne du pilote) : l'effet est terminé et le
    /// contexte GPU partagé recréé ; le Jelly reste actif.
    PeripheriquePerdu,
    /// Pipeline (compilateur HLSL, shaders, états) ou adaptateur (WARP
    /// refusé) : le Jelly est désactivé pour la session.
    Systeme,
}

/// Réaction à un échec du rendu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    /// Désactiver le Jelly pour la session.
    pub desactiver: bool,
    /// Notifier l'utilisateur (une seule fois par désactivation).
    pub notifier: bool,
}

/// Réaction à un échec de nature `nature`, le Jelly étant déjà désactivé
/// ou non. Seuls les échecs systémiques désactivent, et notifient une fois.
pub fn reaction_echec(nature: NatureEchec, deja_desactive: bool) -> Reaction {
    match nature {
        NatureEchec::Systeme => Reaction {
            desactiver: true,
            notifier: !deja_desactive,
        },
        NatureEchec::Ponctuel | NatureEchec::PeripheriquePerdu => Reaction {
            desactiver: false,
            notifier: false,
        },
    }
}

/// HRESULT signalant la perte du périphérique graphique :
/// `DXGI_ERROR_DEVICE_REMOVED`, `DXGI_ERROR_DEVICE_HUNG`,
/// `DXGI_ERROR_DEVICE_RESET`, `DXGI_ERROR_DRIVER_INTERNAL_ERROR`.
pub const HRESULTS_PERIPHERIQUE_PERDU: [u32; 4] =
    [0x887A_0005, 0x887A_0006, 0x887A_0007, 0x887A_0020];

/// Vrai si le HRESULT (brut) signale un périphérique graphique perdu.
pub fn hresult_peripherique_perdu(code: u32) -> bool {
    HRESULTS_PERIPHERIQUE_PERDU.contains(&code)
}

/// Taille maximale (pixels) d'une texture 2D / swapchain selon le niveau de
/// fonctionnalité Direct3D (valeur brute `D3D_FEATURE_LEVEL`) : 16384 en
/// 11_x et plus, 8192 en 10_x, 2048 en dessous (non pris en charge par le
/// pipeline du Jelly de toute façon).
pub fn taille_max_texture(niveau: i32) -> u32 {
    if niveau >= 0xb000 {
        16384
    } else if niveau >= 0xa000 {
        8192
    } else {
        2048
    }
}

#[cfg(windows)]
pub use win32::{jelly_actif, jelly_desactive};

#[cfg(windows)]
pub(super) use win32::{appliquer_config, arreter, rearmer, reprendre};

#[cfg(windows)]
mod win32 {
    use super::{
        classer_geste, decider, reaction_echec, Decision, Geste, MotifFin, NatureEchec,
        Observation, RectEcran,
    };

    use std::cell::{Cell, RefCell};
    use std::time::Instant;

    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetCursorPos, GetGUIThreadInfo, GetSystemMetrics,
        GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible, KillTimer,
        PostMessageW, RegisterClassW, SetTimer, CHILDID_SELF, EVENT_SYSTEM_MOVESIZEEND,
        EVENT_SYSTEM_MOVESIZESTART, GUITHREADINFO, GUI_INMOVESIZE, HWND_MESSAGE, OBJID_WINDOW,
        SM_SWAPBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT,
        WINEVENT_SKIPOWNPROCESS, WM_APP, WM_TIMER, WNDCLASSW,
    };

    use super::super::cadence::{frequence_hz, ModeCadence, PERIODE_MINUTEUR_MS};
    use super::super::capture::{self, DerniereTrame, SessionCapture};
    use super::super::deformation;
    use super::super::eligibilite;
    use super::super::gpu::ContexteGpu;
    use super::super::maillage_gpu::{self, EchecMaillage, MaillageGpu};
    use super::super::metronome::{Metronome, PoigneeBattement};
    use super::super::masquage::{self, ResultatMasquage};
    use super::super::politique::MemoireBornee;
    use super::super::ressort::{self, Grille, Puissance, Simulation, Vec2};
    use super::super::{contexte_gpu_partage, decrire, oui_non, signaler_erreur, WM_TRAME_PRETE};
    use crate::config::Config;
    use crate::logging;
    use crate::tray;

    /// Message posté par le rappel WinEvent du Jelly (wparam = événement,
    /// lparam = HWND).
    const WM_JELLY_EVENEMENT: u32 = WM_APP + 30;
    /// Message posté par le fil de battement (wparam = génération).
    const WM_JELLY_BATTEMENT: u32 = WM_APP + 31;
    /// Minuteur de repli des pas (actif UNIQUEMENT pendant l'effet, et
    /// seulement si le fil de battement n'a pas pu démarrer).
    const ID_PAS: usize = 1;
    /// Minuteur de réessai des restaurations en échec (fenêtre restée à
    /// alpha 1 et réinscrite par `masquage`), actif seulement dans ce cas.
    const ID_REESSAI_RESTAURATION: usize = 2;
    /// Période du réessai des restaurations en échec.
    const PERIODE_REESSAI_MS: u32 = 500;
    /// Délai maximal d'attente de la première image capturée.
    const DELAI_IMAGE_MS: u128 = 400;
    /// Décalage minimal (px) pour reconnaître un déplacement.
    const SEUIL_GESTE_PX: i32 = 2;
    /// Alpha posé sur la vraie fenêtre pendant l'effet (1 : reste cliquable).
    const ALPHA_MASQUAGE: u8 = 1;
    /// Échecs consécutifs du masquage avant désactivation pour la session.
    const SEUIL_ECHECS_MASQUAGE: u32 = 3;
    /// Fenêtres rejetées mémorisées (journal une fois par fenêtre).
    const MEMOIRE_REJETS: usize = 64;
    /// Période minimale du battement de journal pendant l'effet.
    const BATTEMENT_MS: u128 = 1000;
    /// Fenêtre de référence pour journaliser les paramètres.
    const REFERENCE: (u32, u32) = (1000, 700);

    /// Réglages du Jelly.
    #[derive(Debug, Clone, Copy)]
    struct Reglages {
        actif: bool,
        puissance: Puissance,
    }

    impl Reglages {
        fn defaut() -> Self {
            Reglages {
                actif: false,
                puissance: Puissance::new(50),
            }
        }
    }

    /// Crochet WinEvent dédié (retiré à la destruction).
    struct CrochetJelly(HWINEVENTHOOK);

    impl Drop for CrochetJelly {
        fn drop(&mut self) {
            unsafe {
                let _ = UnhookWinEvent(self.0);
            }
        }
    }

    /// Glisser détecté, en attente d'un déplacement et d'une image.
    struct Armement {
        id: u32,
        cible: isize,
        gpu: ContexteGpu,
        session: SessionCapture,
        trame: Option<DerniereTrame>,
        puissance: Puissance,
        bornes_initiales: RectEcran,
        prise: Vec2,
        debut: Instant,
        image_ms: Option<u128>,
        geste: Geste,
        fin_recue: bool,
    }

    /// Statistiques du battement de journal.
    struct Battement {
        depuis: Instant,
        pas: u32,
        somme_ms: f64,
        max_ms: f64,
    }

    impl Battement {
        fn nouveau() -> Self {
            Battement {
                depuis: Instant::now(),
                pas: 0,
                somme_ms: 0.0,
                max_ms: 0.0,
            }
        }
    }

    /// Source des pas pendant l'effet (aucune quand l'effet est inactif).
    enum Cadence {
        /// Fil de battement synchronisé sur le compositeur.
        Fil(Metronome),
        /// Repli : minuteur `SetTimer` de la fenêtre de contrôle.
        Minuteur { debut: Instant },
    }

    impl Cadence {
        fn mode(&self) -> ModeCadence {
            match self {
                Cadence::Fil(m) => m.compteurs().mode.unwrap_or(ModeCadence::DwmFlush),
                Cadence::Minuteur { .. } => ModeCadence::Minuteur,
            }
        }

        /// Résumé pour le journal (mode + compteurs du fil).
        fn resume(&self) -> String {
            match self {
                Cadence::Fil(m) => {
                    let c = m.compteurs();
                    format!(
                        "cadence {} — {} battements, {} coalescés, {} attentes sans tic{}{}{}",
                        self.mode().libelle(),
                        c.postes,
                        c.coalesces,
                        c.delais,
                        if c.debloquages > 0 {
                            format!(", {} drapeaux débloqués par le garde-fou", c.debloquages)
                        } else {
                            String::new()
                        },
                        if c.echecs_post > 0 {
                            format!(", {} PostMessage en échec", c.echecs_post)
                        } else {
                            String::new()
                        },
                        if c.dernier_echec != 0 {
                            format!(", dernier échec d'attente {:#010x}", c.dernier_echec)
                        } else {
                            String::new()
                        }
                    )
                }
                Cadence::Minuteur { .. } => format!("cadence {}", ModeCadence::Minuteur.libelle()),
            }
        }

        fn duree_ms(&self) -> u128 {
            match self {
                Cadence::Fil(m) => m.duree().as_millis(),
                Cadence::Minuteur { debut } => debut.elapsed().as_millis(),
            }
        }
    }

    /// Effet en cours (vraie fenêtre masquée à alpha 1, surimpression posée).
    struct EffetActif {
        id: u32,
        cible: isize,
        tid: u32,
        rendu: Box<MaillageGpu>,
        simulation: Simulation,
        bornes: RectEcran,
        taille: (i32, i32),
        fin_recue: bool,
        debut_glisser: Instant,
        debut: Instant,
        debut_retour: Option<Instant>,
        dernier_pas: Instant,
        pas: u64,
        battement: Battement,
    }

    enum Phase {
        Repos,
        Armement(Box<Armement>),
        Actif(Box<EffetActif>),
    }

    struct EtatJelly {
        reglages: Reglages,
        /// Fenêtre de contrôle message-only (0 = pas encore créée).
        controle: isize,
        crochet: Option<CrochetJelly>,
        phase: Phase,
        prochain_id: u32,
        echecs_masquage: u32,
        /// Désactivé pour la session (échecs de masquage ou rendu Direct3D
        /// impossible) ; réarmé en recochant « Effet Jelly ».
        desactive: bool,
        /// Raison de la désactivation (journal).
        motif_desactivation: Option<String>,
        /// Source des pas, présente uniquement pendant l'effet.
        cadence: Option<Cadence>,
        /// Génération du fil de battement courant (messages périmés ignorés).
        generation_cadence: u32,
        /// Mode de cadence déjà journalisé (une ligne par changement).
        dernier_mode_journalise: Option<ModeCadence>,
        /// Posé par [`arreter`] (fermeture de l'application ou de la
        /// session) ; effacé par [`reprendre`] / [`appliquer_config`]. Tant
        /// qu'il est posé, aucun nouvel effet ne démarre.
        arrete: bool,
        rejetes: MemoireBornee,
    }

    impl EtatJelly {
        fn nouveau(reglages: Reglages) -> Self {
            EtatJelly {
                reglages,
                controle: 0,
                crochet: None,
                phase: Phase::Repos,
                prochain_id: 1,
                echecs_masquage: 0,
                desactive: false,
                motif_desactivation: None,
                cadence: None,
                generation_cadence: 0,
                dernier_mode_journalise: None,
                arrete: false,
                rejetes: MemoireBornee::new(MEMOIRE_REJETS),
            }
        }
    }

    thread_local! {
        static ETAT_JELLY: RefCell<Option<EtatJelly>> = const { RefCell::new(None) };
        /// Fenêtre de contrôle destinataire du rappel WinEvent.
        static CONTROLE_JELLY: Cell<isize> = const { Cell::new(0) };
        /// Poignée du fil de battement courant, HORS de `ETAT_JELLY` : le
        /// battement est pris en charge (drapeau de coalescence libéré) même
        /// si l'état est momentanément emprunté.
        static BATTEMENT_COURANT: Cell<Option<PoigneeBattement>> = const { Cell::new(None) };
    }

    /// Exécute `f` sur l'état sous un emprunt court (`None` si absent ou
    /// déjà emprunté ; jamais de panique).
    fn avec<R>(f: impl FnOnce(&mut EtatJelly) -> R) -> Option<R> {
        ETAT_JELLY.with(|cellule| match cellule.try_borrow_mut() {
            Ok(mut garde) => garde.as_mut().map(f),
            Err(_) => {
                logging::avertir("Jelly : état déjà emprunté (réentrance évitée).");
                None
            }
        })
    }

    fn lire() -> Reglages {
        ETAT_JELLY.with(|e| match e.try_borrow() {
            Ok(g) => g
                .as_ref()
                .map(|s| s.reglages)
                .unwrap_or_else(Reglages::defaut),
            Err(_) => Reglages::defaut(),
        })
    }

    fn ecrire(reglages: Reglages) {
        ETAT_JELLY.with(|e| match e.try_borrow_mut() {
            Ok(mut g) => match g.as_mut() {
                Some(etat) => etat.reglages = reglages,
                None => *g = Some(EtatJelly::nouveau(reglages)),
            },
            Err(_) => logging::avertir("Jelly : réglages non appliqués (état occupé)."),
        });
    }

    /// Extrait la phase courante (remplacée par `Repos`) sous emprunt court.
    fn prendre_phase() -> Option<Phase> {
        avec(|s| std::mem::replace(&mut s.phase, Phase::Repos))
    }

    /// Remet une phase dans l'état ; renvoie la phase non rangée (état
    /// indisponible, occupé par une autre phase, Jelly arrêté ou fenêtre de
    /// contrôle absente) : l'appelant doit alors la terminer immédiatement
    /// (restauration).
    #[must_use]
    fn remettre_phase(phase: Phase) -> Option<Phase> {
        ETAT_JELLY.with(|c| match c.try_borrow_mut() {
            Ok(mut g) => match g.as_mut() {
                Some(s) if matches!(s.phase, Phase::Repos) && !s.arrete && s.controle != 0 => {
                    s.phase = phase;
                    None
                }
                _ => Some(phase),
            },
            Err(_) => Some(phase),
        })
    }

    /// Remet la phase ou, à défaut, la termine sur-le-champ.
    fn ranger(phase: Phase) {
        if let Some(reste) = remettre_phase(phase) {
            logging::avertir(
                "Jelly : état indisponible ou Jelly arrêté, effet terminé immédiatement.",
            );
            terminer_phase(reste, MotifFin::Erreur);
        }
    }

    fn nouvel_id() -> Option<u32> {
        avec(|s| {
            let id = s.prochain_id;
            s.prochain_id = if id >= 1_000_000 { 1 } else { id + 1 };
            id
        })
    }

    // -----------------------------------------------------------------------
    // API publique (menu) et cycle de vie
    // -----------------------------------------------------------------------

    /// Effet Jelly activé (case du menu) ?
    pub fn jelly_actif() -> bool {
        lire().actif
    }

    /// Jelly désactivé pour la session (échecs de masquage ou rendu
    /// Direct3D impossible) ? Réarmé en recochant « Effet Jelly ».
    pub fn jelly_desactive() -> bool {
        ETAT_JELLY.with(|e| match e.try_borrow() {
            Ok(g) => g.as_ref().map(|s| s.desactive).unwrap_or(false),
            Err(_) => false,
        })
    }

    /// Réarme le Jelly s'il avait été désactivé pour la session (échecs de
    /// masquage ou pipeline Direct3D en échec) : équivalent de l'ancien
    /// « décocher puis recocher » du menu. Appelé par la fenêtre de
    /// paramètres AVANT [`appliquer_config`] (qui réévalue ensuite).
    pub fn rearmer() {
        avec(|s| {
            s.desactive = false;
            s.motif_desactivation = None;
            s.echecs_masquage = 0;
        });
        // Le pipeline Direct3D sera retenté au prochain glisser.
        maillage_gpu::oublier_echec_pipeline();
    }

    /// Applique une configuration (démarrage, rechargement).
    pub fn appliquer_config(config: &Config, contexte: &str) {
        ecrire(Reglages {
            actif: config.jelly_actif,
            puissance: Puissance::new(config.jelly_puissance),
        });
        avec(|s| s.arrete = false);
        reevaluer(contexte);
    }

    /// Arrêt : termine l'effet en cours (restauration), retire le crochet,
    /// arrête le minuteur.
    pub fn arreter() {
        let extrait = ETAT_JELLY.with(|c| match c.try_borrow_mut() {
            Ok(mut g) => g.as_mut().map(|s| {
                s.arrete = true;
                (
                    s.controle,
                    s.crochet.take(),
                    std::mem::replace(&mut s.phase, Phase::Repos),
                )
            }),
            Err(_) => {
                logging::avertir("Arrêt du Jelly : état occupé (restauration d'urgence ensuite).");
                None
            }
        });
        if let Some((controle, crochet, phase)) = extrait {
            arreter_cadence(controle);
            if crochet.is_some() {
                logging::info("Jelly : crochet de déplacement retiré (arrêt).");
            }
            drop(crochet);
            terminer_phase(phase, MotifFin::Arret);
        }
    }

    /// Reprise après une fermeture de session annulée.
    pub fn reprendre() {
        avec(|s| s.arrete = false);
        reevaluer("fermeture de session annulée");
    }

    /// Installe / retire le crochet selon les réglages. Désactivé ou
    /// puissance nulle : aucun crochet, aucun minuteur, aucune capture, pas
    /// de GPU créé pour le Jelly.
    fn reevaluer(contexte: &str) {
        let Some((reglages, desactive)) = avec(|s| (s.reglages, s.desactive)) else {
            return;
        };
        let voulu = reglages.actif && !reglages.puissance.est_nulle() && !desactive;
        let mut texte_gpu = "non demandé".to_string();
        let mut gpu_pret = false;
        let mut controle = 0;
        if voulu {
            match contexte_gpu_partage() {
                Ok(gpu) => {
                    gpu_pret = true;
                    texte_gpu = "prêt".to_string();
                    drop(gpu);
                }
                Err(e) => texte_gpu = format!("indisponible ({e})"),
            }
            if gpu_pret {
                controle = assurer_controle();
            }
        }
        let effectif = voulu && gpu_pret && controle != 0;
        let extrait = avec(|s| {
            let crochet = if effectif { None } else { s.crochet.take() };
            let phase = if effectif {
                Phase::Repos
            } else {
                std::mem::replace(&mut s.phase, Phase::Repos)
            };
            (s.controle, crochet, phase, effectif && s.crochet.is_none())
        });
        let Some((controle, crochet, phase, a_installer)) = extrait else {
            return;
        };
        if !effectif {
            arreter_cadence(controle);
        }
        if crochet.is_some() {
            logging::info(&format!(
                "Jelly : crochet de déplacement retiré ({contexte})."
            ));
        }
        drop(crochet);
        terminer_phase(phase, MotifFin::Arret);
        if a_installer {
            match installer_crochet(controle) {
                Ok(c) => {
                    logging::info(&format!(
                        "Jelly : crochet de déplacement installé ({contexte}) — \
                         EVENT_SYSTEM_MOVESIZESTART / MOVESIZEEND, hors contexte."
                    ));
                    // État occupé : `c` est détruit (crochet retiré).
                    avec(move |s| s.crochet = Some(c));
                }
                Err(e) => logging::erreur(&format!("Jelly : SetWinEventHook a échoué : {e}")),
            }
        }
        journaliser_etat(contexte, &texte_gpu, effectif);
    }

    fn journaliser_etat(contexte: &str, gpu: &str, effectif: bool) {
        let Some((r, desactive, motif, crochet)) = avec(|s| {
            (
                s.reglages,
                s.desactive,
                s.motif_desactivation.clone(),
                s.crochet.is_some(),
            )
        }) else {
            return;
        };
        let parametres = match ressort::parametres(r.puissance, REFERENCE.0, REFERENCE.1) {
            Some(p) => format!(
                "paramètres (fenêtre {}x{}) : raideur proche {:.0}, raideur loin {:.0}, \
                 amortissement ζ {:.2}, déplacement max {:.0} px",
                REFERENCE.0,
                REFERENCE.1,
                p.raideur_proche,
                p.raideur_loin,
                p.amortissement,
                p.deplacement_max_px
            ),
            None => "paramètres : aucun (puissance nulle)".to_string(),
        };
        logging::info(&format!(
            "Jelly ({contexte}) : actif={}, puissance={} %, effectif={}{}, GPU={gpu}, crochet={}, \
             {parametres}.",
            oui_non(r.actif),
            r.puissance.valeur(),
            oui_non(effectif),
            if desactive {
                format!(
                    " (désactivé pour la session : {})",
                    motif.as_deref().unwrap_or("raison inconnue")
                )
            } else {
                String::new()
            },
            if crochet { "installé" } else { "absent" },
        ));
    }

    // -----------------------------------------------------------------------
    // Infrastructure : fenêtre de contrôle, crochet, minuteur
    // -----------------------------------------------------------------------

    fn assurer_controle() -> isize {
        let existant = avec(|s| s.controle).unwrap_or(0);
        if existant != 0 {
            return existant;
        }
        match unsafe { creer_controle() } {
            Ok(hwnd) => {
                let brut = hwnd.0 as isize;
                avec(|s| s.controle = brut);
                CONTROLE_JELLY.with(|c| c.set(brut));
                brut
            }
            Err(e) => {
                logging::erreur(&format!("Fenêtre de contrôle du Jelly : {e}"));
                0
            }
        }
    }

    unsafe fn creer_controle() -> windows::core::Result<HWND> {
        let instance = GetModuleHandleW(None)?;
        let nom = w!("TransparentWindows_Jelly");
        let classe = WNDCLASSW {
            lpfnWndProc: Some(procedure_controle),
            hInstance: instance.into(),
            lpszClassName: nom,
            ..Default::default()
        };
        // Échec toléré (classe déjà enregistrée) : CreateWindowExW signalera
        // la vraie erreur le cas échéant.
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

    fn installer_crochet(controle: isize) -> windows::core::Result<CrochetJelly> {
        CONTROLE_JELLY.with(|c| c.set(controle));
        let crochet = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_MOVESIZESTART,
                EVENT_SYSTEM_MOVESIZEEND,
                None,
                Some(rappel),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if crochet.0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(CrochetJelly(crochet))
    }

    /// Rappel WinEvent : filtre puis UN `PostMessageW`. Ne panique jamais.
    unsafe extern "system" fn rappel(
        _crochet: HWINEVENTHOOK,
        evenement: u32,
        hwnd: HWND,
        id_objet: i32,
        id_enfant: i32,
        _fil: u32,
        _horodatage: u32,
    ) {
        if hwnd.0.is_null() || id_objet != OBJID_WINDOW.0 || id_enfant != CHILDID_SELF as i32 {
            return;
        }
        if evenement != EVENT_SYSTEM_MOVESIZESTART && evenement != EVENT_SYSTEM_MOVESIZEEND {
            return;
        }
        let controle = CONTROLE_JELLY.with(|c| c.get());
        if controle == 0 {
            return;
        }
        let _ = PostMessageW(
            Some(HWND(controle as *mut core::ffi::c_void)),
            WM_JELLY_EVENEMENT,
            WPARAM(evenement as usize),
            LPARAM(hwnd.0 as isize),
        );
    }

    /// Assure une source de pas pendant l'effet : fil de battement
    /// synchronisé sur le compositeur, sinon minuteur de repli. Sans effet
    /// si une cadence existe déjà. `false` si aucune source de pas n'a pu
    /// être obtenue : l'appelant termine alors l'effet (restauration).
    #[must_use]
    fn assurer_cadence(controle: isize) -> bool {
        if controle == 0 {
            return false;
        }
        let Some((existe, generation)) = avec(|s| {
            if s.cadence.is_some() {
                (true, 0)
            } else {
                s.generation_cadence = s.generation_cadence.wrapping_add(1);
                (false, s.generation_cadence)
            }
        }) else {
            return false;
        };
        if existe {
            return true;
        }
        let cadence = match Metronome::demarrer(controle, WM_JELLY_BATTEMENT, generation) {
            Ok(m) => {
                if let Some(motif) = &m.motif_repli {
                    journaliser_mode_une_fois(
                        ModeCadence::DwmFlush,
                        &format!("horloge du compositeur indisponible : {motif}"),
                    );
                } else {
                    journaliser_mode_une_fois(ModeCadence::HorlogeCompositeur, "");
                }
                Cadence::Fil(m)
            }
            Err(e) => {
                logging::avertir(&format!(
                    "Jelly : fil de battement impossible ({e}) — repli sur un minuteur SetTimer \
                     {PERIODE_MINUTEUR_MS} ms."
                ));
                let ok = unsafe {
                    SetTimer(Some(hwnd(controle)), ID_PAS, PERIODE_MINUTEUR_MS, None) != 0
                };
                if !ok {
                    logging::erreur(&format!(
                        "Jelly : SetTimer a échoué ({}) — aucun pas possible, effet terminé.",
                        windows::core::Error::from_thread()
                    ));
                    return false;
                }
                journaliser_mode_une_fois(ModeCadence::Minuteur, &e);
                Cadence::Minuteur {
                    debut: Instant::now(),
                }
            }
        };
        let poignee = match &cadence {
            Cadence::Fil(m) => Some(m.poignee()),
            Cadence::Minuteur { .. } => None,
        };
        // État indisponible ou cadence apparue entre-temps : celle-ci est
        // arrêtée aussitôt.
        let reste = avec(move |s| {
            if s.cadence.is_none() {
                s.cadence = Some(cadence);
                None
            } else {
                Some(cadence)
            }
        });
        match reste {
            Some(None) => {
                if poignee.is_some() {
                    BATTEMENT_COURANT.with(|c| c.set(poignee));
                }
                true
            }
            // Une autre cadence est déjà active : elle fournit les pas.
            Some(Some(c)) => {
                liberer_cadence(controle, c);
                true
            }
            None => {
                // `cadence` a été libérée avec la fermeture non exécutée :
                // seul le minuteur éventuel reste à tuer.
                unsafe {
                    let _ = KillTimer(Some(hwnd(controle)), ID_PAS);
                }
                false
            }
        }
    }

    /// Démarre la cadence ; à défaut, termine la phase en cours
    /// (restauration de la fenêtre si elle était masquée).
    fn assurer_cadence_ou_terminer(controle: isize) {
        if !assurer_cadence(controle) {
            if let Some(phase) = prendre_phase() {
                terminer_phase(phase, MotifFin::Erreur);
            }
        }
    }

    /// Journalise le mode de cadence s'il a changé depuis la dernière fois.
    fn journaliser_mode_une_fois(mode: ModeCadence, detail: &str) {
        let nouveau = avec(|s| {
            let n = s.dernier_mode_journalise != Some(mode);
            s.dernier_mode_journalise = Some(mode);
            n
        })
        .unwrap_or(true);
        if nouveau {
            logging::info(&format!(
                "Jelly : cadence des pas = {}{}.",
                mode.libelle(),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                }
            ));
        }
    }

    /// Arrête la cadence (fil ou minuteur) hors de tout emprunt. Idempotent.
    fn arreter_cadence(controle: isize) {
        let cadence = avec(|s| s.cadence.take()).flatten();
        match cadence {
            Some(c) => liberer_cadence(controle, c),
            None => {
                // Minuteur de repli éventuellement orphelin (état occupé).
                if controle != 0 {
                    unsafe {
                        let _ = KillTimer(Some(hwnd(controle)), ID_PAS);
                    }
                }
            }
        }
    }

    fn liberer_cadence(controle: isize, cadence: Cadence) {
        if let Cadence::Fil(m) = &cadence {
            let generation = m.generation();
            BATTEMENT_COURANT.with(|c| {
                let p = c.take();
                if p.as_ref().is_some_and(|p| p.generation() != generation) {
                    c.set(p);
                }
            });
        }
        let resume = cadence.resume();
        let duree = cadence.duree_ms();
        if let Cadence::Fil(m) = &cadence {
            if let Some(m2) = m.compteurs().mode {
                let initial = if m.motif_repli.is_none() {
                    ModeCadence::HorlogeCompositeur
                } else {
                    ModeCadence::DwmFlush
                };
                if m2 != initial {
                    journaliser_mode_une_fois(m2, "repli en cours d'effet après un échec d'attente");
                }
            }
        }
        match cadence {
            Cadence::Fil(m) => drop(m), // arrêt borné (drapeau + événement)
            Cadence::Minuteur { .. } => {
                if controle != 0 {
                    unsafe {
                        let _ = KillTimer(Some(hwnd(controle)), ID_PAS);
                    }
                }
            }
        }
        logging::info(&format!("Jelly : battement arrêté après {duree} ms ({resume})."));
    }

    /// Battement reçu du fil (wparam = génération) : pris en charge PUIS pas.
    /// La prise en charge (drapeau de coalescence) est faite d'abord via
    /// `BATTEMENT_COURANT`, indépendamment de l'emprunt de l'état : un état
    /// occupé ne bloque jamais le fil de battement.
    fn battement(generation: u32) {
        BATTEMENT_COURANT.with(|c| {
            let p = c.take();
            if let Some(p) = &p {
                if p.generation() == generation {
                    p.prendre_en_charge();
                }
            }
            c.set(p);
        });
        let courant = avec(|s| match &s.cadence {
            Some(Cadence::Fil(m)) if m.generation() == generation => {
                m.prendre_en_charge();
                true
            }
            _ => false,
        })
        .unwrap_or(false);
        if courant {
            pas();
        }
    }

    /// Procédure de la fenêtre de contrôle (hors de tout emprunt).
    unsafe extern "system" fn procedure_controle(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_JELLY_EVENEMENT => {
                let h = lparam.0;
                match wparam.0 as u32 {
                    EVENT_SYSTEM_MOVESIZESTART => debut_glisser(h),
                    EVENT_SYSTEM_MOVESIZEEND => fin_glisser(h),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAME_PRETE => {
                trame_prete(wparam.0 as u32);
                LRESULT(0)
            }
            WM_JELLY_BATTEMENT => {
                battement(wparam.0 as u32);
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == ID_PAS {
                    pas();
                } else if wparam.0 == ID_REESSAI_RESTAURATION {
                    reessayer_restaurations(hwnd.0 as isize);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    // -----------------------------------------------------------------------
    // Outils Win32
    // -----------------------------------------------------------------------

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    fn rect_ecran(r: &RECT) -> RectEcran {
        RectEcran {
            gauche: r.left,
            haut: r.top,
            droite: r.right,
            bas: r.bottom,
        }
    }

    fn rect_win32(r: &RectEcran) -> RECT {
        RECT {
            left: r.gauche,
            top: r.haut,
            right: r.droite,
            bottom: r.bas,
        }
    }

    /// Bornes visibles (`DWMWA_EXTENDED_FRAME_BOUNDS`, pixels physiques).
    fn lire_bornes(h: isize) -> Option<RectEcran> {
        unsafe { eligibilite::bornes_fenetre(hwnd(h)) }.map(|r| rect_ecran(&r))
    }

    fn utilisable(h: isize) -> (bool, bool) {
        unsafe {
            let w = hwnd(h);
            let existe = IsWindow(Some(w)).as_bool();
            let visible = existe && IsWindowVisible(w).as_bool() && !IsIconic(w).as_bool();
            (existe, visible)
        }
    }

    /// Point saisi, relatif aux bornes : position du curseur (contexte DPI
    /// per-monitor v2 : pixels physiques, comme les bornes DWM). Hors de la
    /// fenêtre : milieu de la barre de titre (l/2, 15).
    fn calculer_prise(bornes: &RectEcran) -> Vec2 {
        let mut p = POINT::default();
        let lu = unsafe {
            let ancien = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let r = GetCursorPos(&mut p);
            if !ancien.0.is_null() {
                SetThreadDpiAwarenessContext(ancien);
            }
            r.is_ok()
        };
        let l = bornes.largeur();
        let h = bornes.hauteur();
        let x = p.x - bornes.gauche;
        let y = p.y - bornes.haut;
        if lu && (0..=l).contains(&x) && (0..=h).contains(&y) {
            Vec2::new(x as f32, y as f32)
        } else {
            Vec2::new(l as f32 / 2.0, 15.0)
        }
    }

    /// Bouton principal de la souris enfoncé (boutons inversés gérés).
    fn bouton_presse() -> bool {
        unsafe {
            let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                VK_RBUTTON
            } else {
                VK_LBUTTON
            };
            (GetAsyncKeyState(vk.0 as i32) as u16) & 0x8000 != 0
        }
    }

    /// Le fil de la fenêtre est-il dans une boucle de déplacement /
    /// redimensionnement ? `None` si l'information est illisible.
    fn fil_en_deplacement(tid: u32) -> Option<bool> {
        if tid == 0 {
            return None;
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetGUIThreadInfo(tid, &mut info) }
            .ok()
            .map(|()| info.flags.0 & GUI_INMOVESIZE.0 != 0)
    }

    // -----------------------------------------------------------------------
    // Cycle de l'effet
    // -----------------------------------------------------------------------

    /// Termine une phase quelconque (hors emprunt).
    fn terminer_phase(phase: Phase, motif: MotifFin) {
        match phase {
            Phase::Repos => {}
            Phase::Armement(a) => abandonner(a, &motif.to_string()),
            Phase::Actif(e) => terminer(e, motif),
        }
    }

    /// Abandon avant activation : la fenêtre n'a pas été modifiée.
    fn abandonner(a: Box<Armement>, motif: &str) {
        logging::info(&format!(
            "Jelly #{} abandonné pour {:#x} après {} ms : {motif} (fenêtre non modifiée).",
            a.id,
            a.cible,
            a.debut.elapsed().as_millis()
        ));
        drop(a);
    }

    /// Fin d'un effet actif : restauration de la vraie fenêtre (avec
    /// attente de la composition) PUIS destruction de la surimpression.
    fn terminer(e: Box<EffetActif>, motif: MotifFin) {
        let EffetActif {
            id,
            cible,
            rendu,
            simulation,
            debut_glisser,
            debut,
            debut_retour,
            pas,
            ..
        } = *e;
        logging::info(&format!(
            "Jelly #{id} terminé pour {cible:#x} : {motif} — effet {} ms (glisser compris {} ms, \
             retour {}), {pas} pas, amplitude max {:.1} px.",
            debut.elapsed().as_millis(),
            debut_glisser.elapsed().as_millis(),
            debut_retour
                .map(|d| format!("{} ms", d.elapsed().as_millis()))
                .unwrap_or_else(|| "non commencé".into()),
            simulation.amplitude_max_vue()
        ));
        let _ = unsafe { masquage::restaurer(cible) };
        drop(rendu);
        // Restauration en échec (fenêtre réinscrite) : nouvel essai périodique.
        if masquage::restaurations_en_attente() > 0 {
            programmer_reessai_restauration();
        }
    }

    /// Programme le minuteur de réessai des restaurations en échec.
    fn programmer_reessai_restauration() {
        let controle = avec(|s| s.controle).unwrap_or(0);
        if controle == 0 {
            return;
        }
        let ok = unsafe {
            SetTimer(
                Some(hwnd(controle)),
                ID_REESSAI_RESTAURATION,
                PERIODE_REESSAI_MS,
                None,
            ) != 0
        };
        if ok {
            logging::avertir(&format!(
                "Jelly : restauration en échec — nouvel essai dans {PERIODE_REESSAI_MS} ms."
            ));
        } else {
            logging::erreur(&format!(
                "Jelly : SetTimer (réessai de restauration) a échoué ({}) — nouvel essai à \
                 l'arrêt.",
                windows::core::Error::from_thread()
            ));
        }
    }

    /// Minuteur de réessai : tente de nouveau les restaurations en échec
    /// (bornées par `masquage::MAX_ESSAIS_RESTAURATION`) ; le minuteur
    /// s'arrête quand plus aucune n'est en attente.
    fn reessayer_restaurations(controle: isize) {
        if masquage::reessayer_restaurations() == 0 {
            unsafe {
                let _ = KillTimer(Some(hwnd(controle)), ID_REESSAI_RESTAURATION);
            }
        }
    }

    /// `EVENT_SYSTEM_MOVESIZESTART`.
    fn debut_glisser(h: isize) {
        let Some((reglages, desactive, arrete, controle)) =
            avec(|s| (s.reglages, s.desactive, s.arrete, s.controle))
        else {
            return;
        };
        if arrete {
            logging::info(&format!(
                "Jelly : MOVESIZESTART de {h:#x} ignoré — effet arrêté (fermeture en cours)."
            ));
            return;
        }
        if !reglages.actif || reglages.puissance.est_nulle() || desactive || controle == 0 {
            return;
        }
        let Some(phase) = prendre_phase() else {
            return;
        };
        match phase {
            Phase::Actif(mut e) if e.cible == h => {
                // Nouvelle prise pendant le rebond : on continue l'effet.
                let bornes = lire_bornes(h).unwrap_or(e.bornes);
                let prise = calculer_prise(&bornes);
                e.simulation.changer_prise(prise);
                e.fin_recue = false;
                e.debut_retour = None;
                // Le garde-fou repart de cette nouvelle prise.
                e.debut_glisser = Instant::now();
                logging::info(&format!(
                    "Jelly #{} : nouvelle prise de {h:#x} pendant l'effet, prise ({:.0}, {:.0}).",
                    e.id, prise.x, prise.y
                ));
                ranger(Phase::Actif(e));
                assurer_cadence_ou_terminer(controle);
                return;
            }
            autre => terminer_phase(autre, MotifFin::NouvelleCible),
        }

        if let Err(raison) = unsafe { eligibilite::verifier(hwnd(h)) } {
            if avec(|s| s.rejetes.marquer(h)).unwrap_or(false) {
                logging::info(&format!(
                    "Jelly : MOVESIZESTART de {} non éligible — {raison}.",
                    decrire(h)
                ));
            }
            return;
        }
        if masquage::est_masquee(h) {
            logging::info(&format!(
                "Jelly : MOVESIZESTART de {h:#x} ignoré — fenêtre déjà masquée par une animation."
            ));
            return;
        }
        let Some(bornes) = lire_bornes(h) else {
            logging::info(&format!(
                "Jelly : MOVESIZESTART de {h:#x} ignoré — bornes illisibles."
            ));
            return;
        };
        let prise = calculer_prise(&bornes);
        let gpu = match contexte_gpu_partage() {
            Ok(g) => g,
            Err(e) => {
                logging::avertir(&format!("Jelly : GPU indisponible ({e}) — pas d'effet."));
                return;
            }
        };
        let Some(id) = nouvel_id() else {
            return;
        };
        logging::info(&format!(
            "Jelly #{id} : MOVESIZESTART de {} — éligible ; bornes {bornes}, prise ({:.0}, {:.0}), \
             puissance {} % ; capture démarrée.",
            decrire(h),
            prise.x,
            prise.y,
            reglages.puissance.valeur()
        ));
        // Attente bornée à DELAI_CAPTURE_MS, sans repli de création directe
        // (non bornée) dans le fil principal.
        match capture::demarrer_session_bornee(
            &gpu,
            h,
            Some((controle, id)),
            &format!("jelly #{id}"),
            super::DELAI_CAPTURE_MS,
        ) {
            Ok(session) => {
                let a = Armement {
                    id,
                    cible: h,
                    gpu,
                    session,
                    trame: None,
                    puissance: reglages.puissance,
                    bornes_initiales: bornes,
                    prise,
                    debut: Instant::now(),
                    image_ms: None,
                    geste: Geste::Indetermine,
                    fin_recue: false,
                };
                ranger(Phase::Armement(Box::new(a)));
                assurer_cadence_ou_terminer(controle);
            }
            Err(e) => signaler_erreur(e, "capture pour l'effet Jelly"),
        }
    }

    /// `EVENT_SYSTEM_MOVESIZEEND` : le pas suivant commence le retour (ou
    /// termine si la taille a changé : ancrage Snap, agrandissement).
    fn fin_glisser(h: isize) {
        let info = avec(|s| match &mut s.phase {
            Phase::Armement(a) if a.cible == h => {
                a.fin_recue = true;
                None
            }
            Phase::Actif(e) if e.cible == h && !e.fin_recue => {
                e.fin_recue = true;
                Some(e.id)
            }
            _ => None,
        })
        .flatten();
        if let Some(id) = info {
            logging::info(&format!(
                "Jelly #{id} : fin du glisser de {h:#x} (source : crochet MOVESIZEEND)."
            ));
        }
    }

    /// Première image de la capture d'armement.
    fn trame_prete(id: u32) {
        let Some(phase) = prendre_phase() else {
            return;
        };
        let phase = match phase {
            Phase::Armement(mut a) if a.id == id && a.image_ms.is_none() => {
                let ms = a.debut.elapsed().as_millis();
                a.image_ms = Some(ms);
                if let Some(t) = a.session.prendre_trame() {
                    logging::info(&format!(
                        "Jelly #{id} : 1re image après {ms} ms ({}x{}).",
                        t.taille.0, t.taille.1
                    ));
                    a.trame = Some(t);
                }
                Phase::Armement(a)
            }
            autre => autre,
        };
        ranger(phase);
    }

    /// Pas (battement synchronisé sur le compositeur, ou minuteur de
    /// repli) : armement ou simulation.
    fn pas() {
        let Some(phase) = prendre_phase() else {
            return;
        };
        let controle = avec(|s| s.controle).unwrap_or(0);
        let suite = match phase {
            Phase::Repos => None,
            Phase::Armement(a) => pas_armement(a),
            Phase::Actif(e) => pas_actif(e),
        };
        match suite {
            Some(p) => ranger(p),
            None => arreter_cadence(controle),
        }
    }

    fn pas_armement(mut a: Box<Armement>) -> Option<Phase> {
        let ecoule = a.debut.elapsed().as_millis();
        if a.fin_recue {
            abandonner(a, "glisser terminé avant tout déplacement");
            return None;
        }
        let Some(bornes) = lire_bornes(a.cible) else {
            abandonner(a, "fenêtre disparue");
            return None;
        };
        let (existe, visible) = utilisable(a.cible);
        if !existe || !visible {
            abandonner(a, "fenêtre détruite, masquée ou réduite");
            return None;
        }
        let geste = classer_geste(&a.bornes_initiales, &bornes, SEUIL_GESTE_PX);
        if geste != a.geste && geste != Geste::Indetermine {
            logging::info(&format!(
                "Jelly #{} : geste classé « {geste:?} » après {ecoule} ms (bornes {} -> {bornes}).",
                a.id, a.bornes_initiales
            ));
            a.geste = geste;
        }
        if geste == Geste::Redimensionnement {
            abandonner(a, "redimensionnement (pas d'effet Jelly)");
            return None;
        }
        if geste == Geste::Indetermine && ecoule >= super::DELAI_ARMEMENT_MS as u128 {
            abandonner(
                a,
                &format!(
                    "aucun déplacement détecté en {} ms (fin du glisser non reçue)",
                    super::DELAI_ARMEMENT_MS
                ),
            );
            return None;
        }
        let image = a.image_ms.is_some() || a.trame.is_some() || a.session.a_une_image();
        if !image {
            if ecoule > DELAI_IMAGE_MS {
                abandonner(a, &format!("aucune image reçue en {DELAI_IMAGE_MS} ms"));
                return None;
            }
            return Some(Phase::Armement(a));
        }
        if geste == Geste::Deplacement {
            return activer(a, bornes);
        }
        Some(Phase::Armement(a))
    }

    /// Échec du masquage : compteur ; au-delà du seuil, Jelly désactivé
    /// pour la session (notification unique).
    fn enregistrer_echec_masquage(cible: isize, message: &str) {
        let (n, desactiver) = avec(|s| {
            s.echecs_masquage = s.echecs_masquage.saturating_add(1);
            let n = s.echecs_masquage;
            let desactiver = n >= SEUIL_ECHECS_MASQUAGE && !s.desactive;
            if desactiver {
                s.desactive = true;
            }
            (n, desactiver)
        })
        .unwrap_or((0, false));
        logging::avertir(&format!(
            "Jelly : masquage par transparence temporaire refusé sur {cible:#x} : {message} \
             (échec consécutif {n}/{SEUIL_ECHECS_MASQUAGE}) — surimpression retirée, pas d'effet."
        ));
        if desactiver {
            logging::erreur(&format!(
                "Effet Jelly désactivé pour la session : {SEUIL_ECHECS_MASQUAGE} échecs consécutifs \
                 du masquage par transparence temporaire."
            ));
            avec(|s| {
                s.motif_desactivation =
                    Some("masquage par transparence temporaire impossible".into())
            });
            tray::notifier(
                "L'effet Jelly est désactivé : Windows refuse le masquage par transparence \
                 temporaire des fenêtres des autres applications. Décochez puis recochez \
                 « Effet Jelly » pour réessayer.",
            );
            reevaluer("Jelly désactivé après des échecs de masquage");
        }
    }

    /// Échec du rendu Direct3D. Systémique (pipeline, adaptateur, HLSL) :
    /// Jelly désactivé pour la session, notification unique, réarmé en
    /// recochant la case (aucun rendu de repli). Périphérique perdu : l'effet
    /// est terminé (par l'appelant, restauration d'abord), le contexte GPU
    /// est recréé par `signaler_erreur`, le Jelly reste actif. Ponctuel :
    /// seul cet effet est abandonné.
    fn echec_rendu(id: u32, echec: EchecMaillage, quoi: &str) {
        let nature = echec.nature();
        match nature {
            NatureEchec::Systeme => {
                let detail = echec.detail().to_string();
                let notifier = avec(|s| {
                    let r = reaction_echec(nature, s.desactive);
                    if r.desactiver {
                        s.desactive = true;
                        s.motif_desactivation =
                            Some(format!("rendu Direct3D impossible — {detail}"));
                    }
                    r.notifier
                })
                .unwrap_or(false);
                logging::erreur(&format!(
                    "Effet Jelly désactivé pour la session (Jelly #{id}, {quoi}) : rendu Direct3D \
                     impossible — {detail}. Aucun rendu de repli ; recocher « Effet Jelly » pour \
                     réessayer."
                ));
                if notifier {
                    tray::notifier(
                        "L'effet Jelly est désactivé : le rendu par le processeur graphique \
                         (Direct3D 11) n'a pas pu être préparé (détails dans journal.log). Décochez \
                         puis recochez « Effet Jelly » pour réessayer.",
                    );
                    reevaluer("Jelly désactivé : rendu Direct3D impossible");
                }
            }
            NatureEchec::PeripheriquePerdu => {
                logging::avertir(&format!(
                    "Jelly #{id} — {quoi} : périphérique graphique perdu ({echec}) — effet \
                     terminé, Jelly maintenu, recréation du contexte GPU."
                ));
            }
            NatureEchec::Ponctuel => {
                logging::avertir(&format!(
                    "Jelly #{id} — {quoi} : {echec} (propre à cette fenêtre, Jelly maintenu)."
                ));
            }
        }
        // Perte du périphérique : recréation du contexte GPU partagé.
        signaler_erreur(echec.en_erreur(), quoi);
    }

    /// Activation : surimpression posée AVANT le masquage (pas de trou),
    /// puis vraie fenêtre masquée à alpha 1.
    fn activer(a: Box<Armement>, bornes: RectEcran) -> Option<Phase> {
        let Armement {
            id,
            cible,
            gpu,
            session,
            trame,
            puissance,
            prise,
            debut,
            ..
        } = *a;
        let abandon = |motif: &str| {
            logging::info(&format!(
                "Jelly #{id} abandonné pour {cible:#x} après {} ms : {motif} (fenêtre non modifiée).",
                debut.elapsed().as_millis()
            ));
        };
        // Image la plus récente (l'ancienne éventuelle est libérée).
        let Some(mut trame) = session.prendre_trame().or(trame) else {
            abandon("image absente");
            return None;
        };
        trame.bornes = rect_win32(&bornes);
        let (tl, th) = trame.taille;
        if (tl as i32 - bornes.largeur()).abs() > 1 || (th as i32 - bornes.hauteur()).abs() > 1 {
            abandon(&format!(
                "taille de l'image {tl}x{th} différente des bornes {bornes}"
            ));
            return None;
        }
        let Some(params) = ressort::parametres(puissance, tl, th) else {
            abandon("paramètres impossibles (puissance nulle ou taille nulle)");
            return None;
        };
        let Some(grille) = Grille::pour(tl, th) else {
            abandon("grille impossible");
            return None;
        };
        let simulation = Simulation::nouvelle(params, grille, prise);
        let marge = deformation::marge_px(&params);

        let Some(origine) = (unsafe { masquage::etat_courant(cible) }) else {
            abandon("fenêtre disparue");
            return None;
        };
        if let Err(refus) = masquage::masquage_possible_avec(&origine, ALPHA_MASQUAGE) {
            abandon(&format!("non masquable : {refus}"));
            return None;
        }
        let opacite = match origine.attributs {
            Some(at) if origine.est_layered() && at.drapeaux & masquage::LWA_ALPHA != 0 => {
                at.alpha as f32 / 255.0
            }
            _ => 1.0,
        };
        if unsafe { IsHungAppWindow(hwnd(cible)) }.as_bool() {
            abandon("l'application ne répond pas");
            return None;
        }

        // Maillage Direct3D : seul rendu (aucun repli). Échec systémique
        // (pipeline, adaptateur) -> Jelly désactivé pour la session ;
        // périphérique perdu -> contexte GPU recréé ; échec propre à cette
        // image (ressources, swapchain, fenêtre) -> pas d'effet.
        let rendu = match unsafe { MaillageGpu::creer(&gpu, &trame, &grille, marge, opacite, id) } {
            Ok(m) => Box::new(m),
            Err(echec) => {
                abandon(&format!("surimpression Direct3D impossible ({echec})"));
                drop(trame);
                drop(session);
                echec_rendu(id, echec, "création de la surimpression Jelly");
                return None;
            }
        };
        match unsafe { masquage::masquer_avec(cible, ALPHA_MASQUAGE) } {
            ResultatMasquage::Masquee(o) => {
                avec(|s| s.echecs_masquage = 0);
                logging::info(&format!(
                    "Jelly #{id} : {cible:#x} masquée par transparence temporaire (alpha \
                     {ALPHA_MASQUAGE}, origine : {o})."
                ));
            }
            ResultatMasquage::NonMasquable(refus) => {
                drop(rendu);
                abandon(&format!("non masquable : {refus}"));
                return None;
            }
            ResultatMasquage::Echec(message) => {
                drop(rendu);
                enregistrer_echec_masquage(cible, &message);
                return None;
            }
        }
        // La session n'est fermée qu'après la copie de l'image.
        drop(trame);
        drop(session);

        let tid = unsafe { GetWindowThreadProcessId(hwnd(cible), None) };
        let (x, y) = rendu.position();
        let g = *simulation.grille();
        logging::info(&format!(
            "Jelly #{id} : activé après {} ms — image {tl}x{th}, grille {}x{}, rendu : maillage \
             courbe Direct3D (GPU), marge {marge} px, surimpression ({x}, {y}) {}x{}, opacité \
             {opacite:.2} ; raideur loin {:.0}, ζ {:.2}, déplacement max {:.0} px.",
            debut.elapsed().as_millis(),
            g.tuiles_x,
            g.tuiles_y,
            tl as i32 + 2 * marge,
            th as i32 + 2 * marge,
            params.raideur_loin,
            params.amortissement,
            params.deplacement_max_px
        ));
        let maintenant = Instant::now();
        Some(Phase::Actif(Box::new(EffetActif {
            id,
            cible,
            tid,
            rendu,
            simulation,
            bornes,
            taille: (tl as i32, th as i32),
            fin_recue: false,
            debut_glisser: debut,
            debut: maintenant,
            debut_retour: None,
            dernier_pas: maintenant,
            pas: 0,
            battement: Battement::nouveau(),
        })))
    }

    fn pas_actif(mut e: Box<EffetActif>) -> Option<Phase> {
        let maintenant = Instant::now();
        let (existe, visible) = utilisable(e.cible);
        let bornes = if existe { lire_bornes(e.cible) } else { None };
        let taille_changee = bornes
            .map(|b| (b.largeur() - e.taille.0).abs() > 1 || (b.hauteur() - e.taille.1).abs() > 1)
            .unwrap_or(false);
        let existe = existe && bornes.is_some();

        if existe && visible && !taille_changee {
            if let Some(b) = bornes {
                // Fin du glisser constatée par l'état du fil de la fenêtre
                // (MOVESIZEEND perdu ou en retard).
                if !e.fin_recue {
                    let fini = match fil_en_deplacement(e.tid) {
                        Some(en_cours) => (!en_cours).then_some("GUI_INMOVESIZE"),
                        None => (!bouton_presse()).then_some("bouton de la souris relâché"),
                    };
                    if let Some(source) = fini {
                        e.fin_recue = true;
                        logging::info(&format!(
                            "Jelly #{} : fin du glisser de {:#x} (source : {source}).",
                            e.id, e.cible
                        ));
                    }
                }
                let delta = Vec2::new(
                    (b.gauche - e.bornes.gauche) as f32,
                    (b.haut - e.bornes.haut) as f32,
                );
                let dt = maintenant.saturating_duration_since(e.dernier_pas);
                e.simulation.avancer(delta, dt);
                e.bornes = b;
                e.dernier_pas = maintenant;
                let deplacements = e.simulation.deplacements();
                if let Err(echec) = unsafe { e.rendu.mettre_a_jour(&rect_win32(&b), &deplacements) } {
                    let id = e.id;
                    terminer(e, MotifFin::Erreur);
                    echec_rendu(id, echec, &format!("mise à jour de la surimpression Jelly #{id}"));
                    return None;
                }
                e.pas += 1;
                let ms = dt.as_secs_f64() * 1000.0;
                let bat = &mut e.battement;
                bat.pas += 1;
                bat.somme_ms += ms;
                if ms > bat.max_ms {
                    bat.max_ms = ms;
                }
                if bat.depuis.elapsed().as_millis() >= BATTEMENT_MS {
                    let moyen = bat.somme_ms / bat.pas.max(1) as f64;
                    let cadence = avec(|s| s.cadence.as_ref().map(Cadence::resume))
                        .flatten()
                        .unwrap_or_else(|| "cadence absente".into());
                    logging::info(&format!(
                        "Jelly #{} : {} pas en {} ms (intervalle moyen {:.1} ms soit {:.0} Hz, max \
                         {:.1} ms ; {cadence}), amplitude {:.1} px (max {:.1} px){}.",
                        e.id,
                        bat.pas,
                        bat.depuis.elapsed().as_millis(),
                        moyen,
                        frequence_hz(moyen),
                        bat.max_ms,
                        e.simulation.amplitude(),
                        e.simulation.amplitude_max_vue(),
                        if e.debut_retour.is_some() {
                            ", en retour"
                        } else {
                            ""
                        }
                    ));
                    e.battement = Battement::nouveau();
                }
            }
        }

        let observation = Observation {
            existe,
            visible_non_reduite: visible,
            taille_changee,
            en_deplacement: !e.fin_recue,
            bouton_presse: bouton_presse(),
            au_repos: e.simulation.au_repos(),
            en_retour: e.debut_retour.is_some(),
            duree_retour_ms: e
                .debut_retour
                .map(|d| d.elapsed().as_millis() as u64)
                .unwrap_or(0),
            duree_totale_ms: e.debut_glisser.elapsed().as_millis() as u64,
        };
        match decider(&observation) {
            Decision::Continuer => Some(Phase::Actif(e)),
            Decision::CommencerRetour => {
                logging::info(&format!(
                    "Jelly #{} : début du retour amorti (amplitude {:.1} px, max {:.1} px).",
                    e.id,
                    e.simulation.amplitude(),
                    e.simulation.amplitude_max_vue()
                ));
                e.debut_retour = Some(maintenant);
                Some(Phase::Actif(e))
            }
            Decision::Terminer(motif) => {
                terminer(e, motif);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(g: i32, h: i32, l: i32, ht: i32) -> RectEcran {
        RectEcran {
            gauche: g,
            haut: h,
            droite: g + l,
            bas: h + ht,
        }
    }

    #[test]
    fn geste_deplacement() {
        let i = r(100, 100, 800, 600);
        assert_eq!(
            classer_geste(&i, &r(102, 100, 800, 600), 2),
            Geste::Deplacement
        );
        assert_eq!(
            classer_geste(&i, &r(100, 40, 800, 600), 2),
            Geste::Deplacement
        );
        // Variation d'1 px de la taille tolérée (arrondis DPI).
        assert_eq!(
            classer_geste(&i, &r(150, 100, 801, 600), 2),
            Geste::Deplacement
        );
    }

    #[test]
    fn geste_redimensionnement() {
        let i = r(100, 100, 800, 600);
        assert_eq!(
            classer_geste(&i, &r(100, 100, 810, 600), 2),
            Geste::Redimensionnement
        );
        assert_eq!(
            classer_geste(&i, &r(90, 100, 810, 600), 2),
            Geste::Redimensionnement
        );
        assert_eq!(
            classer_geste(&i, &r(100, 100, 800, 598), 2),
            Geste::Redimensionnement
        );
    }

    #[test]
    fn geste_indetermine() {
        let i = r(100, 100, 800, 600);
        assert_eq!(classer_geste(&i, &i, 2), Geste::Indetermine);
        assert_eq!(
            classer_geste(&i, &r(101, 101, 800, 600), 2),
            Geste::Indetermine
        );
    }

    fn base() -> Observation {
        Observation {
            existe: true,
            visible_non_reduite: true,
            taille_changee: false,
            en_deplacement: true,
            bouton_presse: true,
            au_repos: false,
            en_retour: false,
            duree_retour_ms: 0,
            duree_totale_ms: 500,
        }
    }

    #[test]
    fn decider_disparue_prioritaire() {
        let o = Observation {
            existe: false,
            visible_non_reduite: false,
            taille_changee: true,
            duree_totale_ms: GARDE_FOU_MS + 1,
            ..base()
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::FenetreDisparue));
        let o = Observation {
            visible_non_reduite: false,
            taille_changee: true,
            ..base()
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::MasqueeOuReduite));
    }

    #[test]
    fn decider_redimension() {
        let o = Observation {
            taille_changee: true,
            en_deplacement: false,
            ..base()
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::Redimensionnee));
    }

    #[test]
    fn decider_fin_de_glisser_commence_le_retour() {
        assert_eq!(decider(&base()), Decision::Continuer);
        let o = Observation {
            en_deplacement: false,
            bouton_presse: false,
            ..base()
        };
        assert_eq!(decider(&o), Decision::CommencerRetour);
    }

    #[test]
    fn decider_retour_repos_clic_delai() {
        let retour = Observation {
            en_deplacement: false,
            bouton_presse: false,
            en_retour: true,
            duree_retour_ms: 100,
            ..base()
        };
        assert_eq!(decider(&retour), Decision::Continuer);
        let o = Observation {
            au_repos: true,
            ..retour
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::Repos));
        // Le clic est prioritaire sur le repos.
        let o = Observation {
            au_repos: true,
            bouton_presse: true,
            ..retour
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::Clic));
        let o = Observation {
            duree_retour_ms: DUREE_RETOUR_MAX_MS,
            ..retour
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::DelaiRetour));
    }

    #[test]
    fn decider_garde_fou() {
        let o = Observation {
            duree_totale_ms: GARDE_FOU_MS,
            ..base()
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::GardeFou));
        let o = Observation {
            duree_totale_ms: GARDE_FOU_MS,
            en_retour: true,
            au_repos: true,
            ..base()
        };
        assert_eq!(decider(&o), Decision::Terminer(MotifFin::GardeFou));
    }

    #[test]
    fn seuls_les_echecs_systemiques_desactivent() {
        let r = reaction_echec(NatureEchec::Systeme, false);
        assert!(r.desactiver && r.notifier);
        // Déjà désactivé : pas de seconde notification.
        let r = reaction_echec(NatureEchec::Systeme, true);
        assert!(r.desactiver && !r.notifier);
        for deja in [false, true] {
            for n in [NatureEchec::Ponctuel, NatureEchec::PeripheriquePerdu] {
                let r = reaction_echec(n, deja);
                assert!(!r.desactiver && !r.notifier, "{n:?}");
            }
        }
    }

    #[test]
    fn hresults_de_peripherique_perdu() {
        assert!(hresult_peripherique_perdu(0x887A_0005)); // DEVICE_REMOVED
        assert!(hresult_peripherique_perdu(0x887A_0007)); // DEVICE_RESET
        assert!(hresult_peripherique_perdu(0x887A_0006)); // DEVICE_HUNG
        assert!(hresult_peripherique_perdu(0x887A_0020)); // DRIVER_INTERNAL_ERROR
        assert!(!hresult_peripherique_perdu(0x8007_000E)); // E_OUTOFMEMORY
        assert!(!hresult_peripherique_perdu(0x8007_0057)); // E_INVALIDARG
        assert!(!hresult_peripherique_perdu(0x887A_0001)); // DXGI_ERROR_INVALID_CALL
        assert!(!hresult_peripherique_perdu(0));
    }

    #[test]
    fn taille_max_selon_niveau() {
        assert_eq!(taille_max_texture(0xb000), 16384); // 11_0
        assert_eq!(taille_max_texture(0xb100), 16384); // 11_1
        assert_eq!(taille_max_texture(0xc100), 16384); // 12_1
        assert_eq!(taille_max_texture(0xa000), 8192); // 10_0
        assert_eq!(taille_max_texture(0xa100), 8192); // 10_1
        assert_eq!(taille_max_texture(0x9300), 2048); // 9_3
    }

    #[test]
    fn affichages_lisibles() {
        assert_eq!(r(1, 2, 3, 4).to_string(), "(1, 2) 3x4");
        assert!(MotifFin::Repos.to_string().contains("repos"));
    }
}
