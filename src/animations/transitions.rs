//! Désactivation temporaire de l'animation native d'apparition de DWM
//! (`DWMWA_TRANSITIONS_FORCEDISABLED`) pendant une ouverture animée (v0.4.7).
//!
//! **Constat** (vidéo 60 i/s + journal v0.4.6) : à l'ouverture, la vraie
//! fenêtre apparaît en fondu ~50 ms après `EVENT_OBJECT_SHOW`, reste visible
//! ~200 ms, puis disparaît jusqu'au lancement de notre effet (« double
//! ouverture »), alors que le masquage alpha 1/255 est posé 2-15 ms après le
//! SHOW. **Hypothèse (non prouvée)** : l'animation native d'apparition de
//! DWM (~250 ms) ne respecte pas l'alpha layered.
//!
//! **Correctif** : `DwmSetWindowAttribute(DWMWA_TRANSITIONS_FORCEDISABLED,
//! TRUE)` le plus tôt possible — dès `EVENT_OBJECT_CREATE` (crochet dédié,
//! voir `evenements`), puis de nouveau au SHOW avant le masquage précoce —
//! et rétablissement de la valeur d'origine sur TOUS les chemins (fin
//! d'ouverture, annulations, échec, arrêt, fin de session, panique), via un
//! registre statique protégé par verrou (modèle `masquage.rs`, `try_lock`
//! en panique). Les poses non liées à une ouverture sont rétablies après
//! [`DELAI_ORPHELIN_MS`] (fenêtre créée mais jamais affichée).
//!
//! **Sondes** : quelques relevés (visible, layered, alpha, cloak,
//! transitions) aux paliers [`PALIERS_SONDES_MS`] après le SHOW, pour
//! confirmer ou infirmer l'hypothèse dans le journal.
//!
//! La partie pure (registre, sondes, textes) est testable hors Windows ;
//! le module `win32` interroge DWM. Aucun message n'est envoyé à
//! l'application cible (pas de `SendMessage`).
//!
//! Correctif audit v0.4.7 : un rétablissement refusé par DWM (hors arrêt,
//! fin de session et panique) réinscrit la pose avec un compteur d'essais
//! ([`MAX_ESSAIS_RETABLISSEMENT`] essais au total) ; le balayage périodique
//! la retente sans attendre [`DELAI_ORPHELIN_MS`].

/// Nombre maximal de poses mémorisées (au-delà : pas de pose).
pub const CAPACITE_REGISTRE: usize = 64;
/// Âge au-delà duquel une pose non liée à une ouverture est rétablie.
pub const DELAI_ORPHELIN_MS: u64 = 3000;
/// Période du balayage des poses orphelines (tant que le registre n'est pas vide).
pub const PERIODE_BALAYAGE_MS: u32 = 1000;
/// Lignes de journal « désactivée à la création » au plus par session.
pub const MAX_JOURNAL_CREATIONS: u32 = 20;
/// Paliers des sondes (ms après le SHOW).
pub const PALIERS_SONDES_MS: [u64; 6] = [0, 30, 60, 120, 200, 300];
/// Nombre total d'essais de rétablissement d'une pose (le premier compris)
/// avant abandon, hors arrêt / fin de session / panique (un seul essai).
pub const MAX_ESSAIS_RETABLISSEMENT: u8 = 3;

/// Moment de la pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// À `EVENT_OBJECT_CREATE`.
    Creation,
    /// À `EVENT_OBJECT_SHOW`.
    Affichage,
}

/// Lecture de `DWMWA_TRANSITIONS_FORCEDISABLED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lecture {
    Valeur(bool),
    /// Lecture refusée (HRESULT).
    Illisible(u32),
}

/// Pose mémorisée (valeur d'origine à rétablir).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pose {
    pub hwnd: isize,
    /// Identité à la pose (détection d'un HWND recyclé).
    pub pid: u32,
    pub tid: u32,
    /// Valeur lue AVANT la pose.
    pub origine: Lecture,
    pub moment: Moment,
    /// Instant de la pose (ms monotones).
    pub pose_ms: u64,
    /// Horodatage de l'événement (`dwmsEventTime`, base `GetTickCount`).
    pub evenement_ms: Option<u32>,
    /// Rétablissements déjà refusés (0 : aucun). Une pose dont `essais > 0`
    /// est retentée au prochain balayage si elle n'est pas liée.
    pub essais: u8,
}

impl Pose {
    /// Valeur à rétablir : TRUE seulement si TRUE a été lu à l'origine ;
    /// FALSE sinon (valeur par défaut de DWM, y compris si illisible).
    pub fn valeur_a_restaurer(&self) -> bool {
        matches!(self.origine, Lecture::Valeur(true))
    }

    /// Pose à réinscrire après un rétablissement refusé : compteur
    /// d'essais incrémenté ; `None` si [`MAX_ESSAIS_RETABLISSEMENT`] essais
    /// ont été faits (abandon).
    pub fn apres_echec_retablissement(&self) -> Option<Pose> {
        let essais = self.essais.saturating_add(1);
        (essais < MAX_ESSAIS_RETABLISSEMENT).then_some(Pose { essais, ..*self })
    }
}

/// Résultat d'une inscription dans le registre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inscription {
    Nouvelle,
    /// Déjà inscrite : l'inscription d'origine est conservée.
    DejaPresente,
    /// Registre plein ([`CAPACITE_REGISTRE`]) : rien inscrit.
    Pleine,
}

/// Compteurs de la session (journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Compteurs {
    pub creations_vues: u32,
    pub posees_creation: u32,
    pub posees_affichage: u32,
    pub refus: u32,
    pub deja_desactivees: u32,
    pub retablies: u32,
    pub orphelines: u32,
    /// Rétablissements refusés puis réinscrits pour un nouvel essai.
    pub reessais: u32,
    /// Rétablissements abandonnés après [`MAX_ESSAIS_RETABLISSEMENT`] refus.
    pub abandons: u32,
}

impl Compteurs {
    const fn zero() -> Compteurs {
        Compteurs {
            creations_vues: 0,
            posees_creation: 0,
            posees_affichage: 0,
            refus: 0,
            deja_desactivees: 0,
            retablies: 0,
            orphelines: 0,
            reessais: 0,
            abandons: 0,
        }
    }

    /// Texte pour le journal.
    pub fn texte(&self) -> String {
        format!(
            "créations vues {}, posées à la CREATE {}, posées au SHOW {}, refus {}, déjà \
             désactivées {}, rétablies {}, orphelines {}, rétablissements réessayés {}, \
             abandonnés {}",
            self.creations_vues,
            self.posees_creation,
            self.posees_affichage,
            self.refus,
            self.deja_desactivees,
            self.retablies,
            self.orphelines,
            self.reessais,
            self.abandons
        )
    }
}

/// Registre des poses (une par fenêtre).
#[derive(Debug)]
pub struct Registre {
    poses: Vec<Pose>,
    pub compteurs: Compteurs,
}

impl Registre {
    pub const fn new() -> Registre {
        Registre {
            poses: Vec::new(),
            compteurs: Compteurs::zero(),
        }
    }

    /// Inscrit une pose ; ne remplace jamais une inscription existante.
    pub fn inscrire(&mut self, pose: Pose) -> Inscription {
        if self.poses.iter().any(|p| p.hwnd == pose.hwnd) {
            return Inscription::DejaPresente;
        }
        if self.poses.len() >= CAPACITE_REGISTRE {
            return Inscription::Pleine;
        }
        self.poses.push(pose);
        Inscription::Nouvelle
    }

    pub fn obtenir(&self, hwnd: isize) -> Option<&Pose> {
        self.poses.iter().find(|p| p.hwnd == hwnd)
    }

    /// Remet à zéro le compteur d'essais (pose renouvelée avec succès).
    pub fn reinitialiser_essais(&mut self, hwnd: isize) {
        if let Some(p) = self.poses.iter_mut().find(|p| p.hwnd == hwnd) {
            p.essais = 0;
        }
    }

    pub fn retirer(&mut self, hwnd: isize) -> Option<Pose> {
        let pos = self.poses.iter().position(|p| p.hwnd == hwnd)?;
        Some(self.poses.swap_remove(pos))
    }

    /// Retire et rend les poses NON liées (`liees` = fenêtres d'une
    /// ouverture en attente ou en cours) âgées d'au moins
    /// [`DELAI_ORPHELIN_MS`], ou déjà en échec de rétablissement
    /// (`essais > 0`, retentées sans délai). `maintenant_ms = u64::MAX` rend
    /// toutes les poses non liées.
    pub fn orphelines(&mut self, maintenant_ms: u64, liees: &[isize]) -> Vec<Pose> {
        let mut rendues = Vec::new();
        self.poses.retain(|p| {
            let orpheline = !liees.contains(&p.hwnd)
                && (p.essais > 0
                    || maintenant_ms.saturating_sub(p.pose_ms) >= DELAI_ORPHELIN_MS);
            if orpheline {
                rendues.push(*p);
            }
            !orpheline
        });
        rendues
    }

    pub fn tout_retirer(&mut self) -> Vec<Pose> {
        std::mem::take(&mut self.poses)
    }

    pub fn len(&self) -> usize {
        self.poses.len()
    }
}

/// Issue d'une demande de désactivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssuePose {
    /// Nouvelle pose inscrite et appliquée.
    Posee,
    /// Déjà inscrite (par exemple à la CREATE) : appel renouvelé.
    DejaInscrite,
    /// Déjà désactivée par l'application : rien inscrit, rien posé.
    DejaDesactivee,
    /// Registre plein : rien posé.
    Pleine,
    /// `DwmSetWindowAttribute` refusé (HRESULT) : rien inscrit.
    Refus(u32),
    /// Fenêtre disparue (ou registre inaccessible).
    Impossible,
}

/// Rapport d'une demande de désactivation (journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RapportPose {
    /// Valeur d'origine (celle de l'inscription si déjà inscrite).
    pub origine: Lecture,
    /// Résultat de `DwmSetWindowAttribute(TRUE)` (`None` : pas d'appel).
    pub appel: Option<Result<(), u32>>,
    pub issue: IssuePose,
}

impl RapportPose {
    /// « S_OK », « 0x80070005 » ou « non fait (raison) ».
    pub fn texte_appel(&self) -> String {
        match self.appel {
            Some(Ok(())) => "S_OK".to_string(),
            Some(Err(hr)) => format!("{hr:#010X}"),
            None => match self.issue {
                IssuePose::DejaDesactivee => "non fait (déjà désactivée par l'application)".into(),
                IssuePose::Pleine => "non fait (registre plein)".into(),
                _ => "non fait (fenêtre disparue ou registre inaccessible)".into(),
            },
        }
    }
}

/// Texte d'une lecture : « TRUE », « FALSE », « illisible (0x…) ».
pub fn texte_lecture(l: Lecture) -> String {
    match l {
        Lecture::Valeur(true) => "TRUE".to_string(),
        Lecture::Valeur(false) => "FALSE".to_string(),
        Lecture::Illisible(hr) => format!("illisible ({hr:#010X})"),
    }
}

/// Délai CREATE -> SHOW (horodatages `GetTickCount`, débordement géré).
pub fn delai_creation_affichage(creation_ms: u32, show_ms: u32) -> u32 {
    show_ms.wrapping_sub(creation_ms)
}

/// Relevé d'une sonde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Releve {
    /// Ms après le SHOW.
    pub ecoule_ms: u64,
    pub visible: bool,
    pub layered: bool,
    pub alpha: Option<u8>,
    pub cloak: Option<u32>,
    pub transitions: Lecture,
}

impl Releve {
    fn texte(&self) -> String {
        format!(
            "T+{} : visible={}, layered={}, alpha={}, cloak={}, transitions={}",
            self.ecoule_ms,
            if self.visible { "oui" } else { "non" },
            if self.layered { "oui" } else { "non" },
            self.alpha
                .map(|a| a.to_string())
                .unwrap_or_else(|| "?".into()),
            self.cloak
                .map(|c| format!("{c:#x}"))
                .unwrap_or_else(|| "?".into()),
            texte_lecture(self.transitions)
        )
    }
}

/// Sondes d'une ouverture (paliers [`PALIERS_SONDES_MS`]).
#[derive(Debug, Clone, Default)]
pub struct Sondes {
    /// Index du prochain palier à relever.
    prochain: usize,
    releves: Vec<Releve>,
}

impl Sondes {
    /// Vrai si un relevé est dû à `ecoule_ms`.
    pub fn due(&self, ecoule_ms: u64) -> bool {
        PALIERS_SONDES_MS
            .get(self.prochain)
            .is_some_and(|&p| ecoule_ms >= p)
    }

    /// Enregistre un relevé ; avance au-delà de tous les paliers déjà
    /// atteints (un relevé tardif couvre plusieurs paliers).
    pub fn noter(&mut self, r: Releve) {
        self.releves.push(r);
        while PALIERS_SONDES_MS
            .get(self.prochain)
            .is_some_and(|&p| p <= r.ecoule_ms)
        {
            self.prochain += 1;
        }
    }

    /// Texte des relevés puis des paliers manqués (« non relevé (fin à N ms) »).
    pub fn texte(&self, fin_ms: u64) -> String {
        let mut parties: Vec<String> = self.releves.iter().map(Releve::texte).collect();
        for p in PALIERS_SONDES_MS.iter().skip(self.prochain) {
            parties.push(format!("T+{p} : non relevé (fin à {fin_ms} ms)"));
        }
        if parties.is_empty() {
            "aucune".to_string()
        } else {
            parties.join(" ; ")
        }
    }
}

#[cfg(windows)]
pub use win32::{
    compteurs, desactiver, noter_creation_vue, oublier, pose, registre_vide, retablir,
    retablir_non_liees, retablir_tout, sonder, Retablissement,
};

#[cfg(windows)]
mod win32 {
    use super::super::masquage::WS_EX_LAYERED;
    use super::{
        Inscription, IssuePose, Lecture, Moment, Pose, RapportPose, Registre, Releve,
        MAX_ESSAIS_RETABLISSEMENT,
    };

    use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
    use std::time::Instant;

    use windows::core::BOOL;
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAKED,
        DWMWA_TRANSITIONS_FORCEDISABLED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow,
        IsWindowVisible, GWL_EXSTYLE, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
    };

    use crate::logging;

    /// Registre statique (accessible depuis le crochet de panique en `try_lock`).
    static REGISTRE: Mutex<Registre> = Mutex::new(Registre::new());

    fn verrou() -> MutexGuard<'static, Registre> {
        REGISTRE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn maintenant_ms() -> u64 {
        static BASE: OnceLock<Instant> = OnceLock::new();
        BASE.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn hwnd(brut: isize) -> HWND {
        HWND(brut as *mut core::ffi::c_void)
    }

    /// Lit `DWMWA_TRANSITIONS_FORCEDISABLED`.
    unsafe fn lire(h: HWND) -> Lecture {
        let mut valeur = BOOL(0);
        match DwmGetWindowAttribute(
            h,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &mut valeur as *mut BOOL as *mut core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        ) {
            Ok(()) => Lecture::Valeur(valeur.as_bool()),
            Err(e) => Lecture::Illisible(e.code().0 as u32),
        }
    }

    /// Écrit `DWMWA_TRANSITIONS_FORCEDISABLED` (`Err(HRESULT)` si refusé).
    unsafe fn ecrire(h: HWND, valeur: bool) -> Result<(), u32> {
        let v = BOOL::from(valeur);
        DwmSetWindowAttribute(
            h,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &v as *const BOOL as *const core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        )
        .map_err(|e| e.code().0 as u32)
    }

    /// (pid, tid) courants ; `None` si la fenêtre n'existe plus.
    unsafe fn identite(h: HWND) -> Option<(u32, u32)> {
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(h, Some(&mut pid));
        (tid != 0).then_some((pid, tid))
    }

    /// Désactive la transition native d'apparition de `cible`. Origine lue
    /// d'abord ; déjà TRUE : rien inscrit ni posé. Sinon inscription PUIS
    /// pose ; échec de la pose : désinscription. Déjà inscrite : la pose est
    /// renouvelée, l'origine mémorisée conservée.
    pub unsafe fn desactiver(
        cible: isize,
        moment: Moment,
        evenement_ms: Option<u32>,
    ) -> RapportPose {
        let h = hwnd(cible);
        let Some((pid, tid)) = identite(h) else {
            return RapportPose {
                origine: Lecture::Illisible(0),
                appel: None,
                issue: IssuePose::Impossible,
            };
        };
        let deja = verrou().obtenir(cible).copied();
        if let Some(p) = deja.filter(|p| p.pid == pid && p.tid == tid) {
            let appel = ecrire(h, true);
            if appel.is_err() {
                verrou().compteurs.refus += 1;
            } else {
                // Pose renouvelée : un éventuel échec de rétablissement
                // antérieur est caduc.
                verrou().reinitialiser_essais(cible);
            }
            return RapportPose {
                origine: p.origine,
                appel: Some(appel),
                issue: IssuePose::DejaInscrite,
            };
        }
        if deja.is_some() {
            // HWND recyclé : l'ancienne inscription est caduque.
            verrou().retirer(cible);
        }
        let origine = lire(h);
        if origine == Lecture::Valeur(true) {
            verrou().compteurs.deja_desactivees += 1;
            return RapportPose {
                origine,
                appel: None,
                issue: IssuePose::DejaDesactivee,
            };
        }
        let inscription = verrou().inscrire(Pose {
            hwnd: cible,
            pid,
            tid,
            origine,
            moment,
            pose_ms: maintenant_ms(),
            evenement_ms,
            essais: 0,
        });
        if inscription == Inscription::Pleine {
            return RapportPose {
                origine,
                appel: None,
                issue: IssuePose::Pleine,
            };
        }
        let appel = ecrire(h, true);
        let mut r = verrou();
        let issue = match appel {
            Ok(()) => {
                match moment {
                    Moment::Creation => r.compteurs.posees_creation += 1,
                    Moment::Affichage => r.compteurs.posees_affichage += 1,
                }
                IssuePose::Posee
            }
            Err(hr) => {
                r.retirer(cible);
                r.compteurs.refus += 1;
                IssuePose::Refus(hr)
            }
        };
        RapportPose {
            origine,
            appel: Some(appel),
            issue,
        }
    }

    /// Issue d'un rétablissement.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Retablissement {
        /// Valeur rétablie (valeur écrite, résultat de l'appel).
        Fait(bool, Result<(), u32>),
        /// Fenêtre disparue : retirée sans appel.
        Disparue,
        /// HWND recyclé (pid/tid différents) : retirée sans appel.
        Recyclee,
        /// Refusé (valeur écrite, HRESULT) : pose réinscrite, essai n° `essai`
        /// fait sur [`MAX_ESSAIS_RETABLISSEMENT`], retentée au prochain balayage.
        Reessai(bool, u32, u8),
        /// Refusé (valeur écrite, HRESULT) après [`MAX_ESSAIS_RETABLISSEMENT`]
        /// essais (ou réinscription impossible) : abandon.
        Abandon(bool, u32, u8),
    }

    impl Retablissement {
        /// Texte pour le journal : « rétablie (FALSE) : motif — S_OK ».
        pub fn texte(&self, motif: &str) -> String {
            let v = |v: bool| if v { "TRUE" } else { "FALSE" };
            match self {
                Retablissement::Fait(b, Ok(())) => format!("rétablie ({}) : {motif} — S_OK", v(*b)),
                Retablissement::Fait(b, Err(hr)) => {
                    format!("non rétablie ({}) : {motif} — refus {hr:#010X}", v(*b))
                }
                Retablissement::Disparue => format!("oubliée : {motif} — fenêtre disparue"),
                Retablissement::Recyclee => {
                    format!("oubliée : {motif} — HWND recyclé, aucun appel")
                }
                Retablissement::Reessai(b, hr, n) => format!(
                    "non rétablie ({}) : {motif} — refus {hr:#010X}, essai {n}/{} ; nouvel \
                     essai au prochain balayage",
                    v(*b),
                    MAX_ESSAIS_RETABLISSEMENT
                ),
                Retablissement::Abandon(b, hr, n) => format!(
                    "non rétablie ({}) : {motif} — refus {hr:#010X}, abandon après {n} essai(s)",
                    v(*b)
                ),
            }
        }
    }

    unsafe fn appliquer_retablissement(p: &Pose) -> Retablissement {
        let h = hwnd(p.hwnd);
        match identite(h) {
            None => Retablissement::Disparue,
            Some((pid, tid)) if pid != p.pid || tid != p.tid => Retablissement::Recyclee,
            Some(_) => {
                let v = p.valeur_a_restaurer();
                Retablissement::Fait(v, ecrire(h, v))
            }
        }
    }

    /// Rétablissement hors arrêt / panique : un refus réinscrit la pose
    /// (compteur d'essais) pour un nouvel essai au balayage, jusqu'à
    /// [`MAX_ESSAIS_RETABLISSEMENT`] essais.
    unsafe fn retablir_avec_reessai(p: &Pose) -> Retablissement {
        let issue = appliquer_retablissement(p);
        let mut r = verrou();
        match issue {
            Retablissement::Fait(_, Ok(())) => {
                r.compteurs.retablies += 1;
                issue
            }
            Retablissement::Fait(v, Err(hr)) => {
                let fait = p.essais.saturating_add(1);
                match p.apres_echec_retablissement() {
                    Some(np) if r.inscrire(np) == Inscription::Nouvelle => {
                        r.compteurs.reessais += 1;
                        Retablissement::Reessai(v, hr, fait)
                    }
                    _ => {
                        r.compteurs.abandons += 1;
                        Retablissement::Abandon(v, hr, fait)
                    }
                }
            }
            autre => autre,
        }
    }

    /// Rétablit la valeur d'origine de `cible` si une pose est inscrite.
    pub unsafe fn retablir(cible: isize) -> Option<Retablissement> {
        let p = verrou().retirer(cible)?;
        Some(retablir_avec_reessai(&p))
    }

    /// Oublie la pose de `cible` sans appel (fenêtre détruite).
    pub fn oublier(cible: isize) {
        verrou().retirer(cible);
    }

    /// Rétablit les poses orphelines (non liées, plus vieilles que
    /// [`super::DELAI_ORPHELIN_MS`]) ; `toutes = true` : toutes les non liées.
    pub unsafe fn retablir_non_liees(
        liees: &[isize],
        toutes: bool,
    ) -> Vec<(isize, Retablissement)> {
        let maintenant = if toutes { u64::MAX } else { maintenant_ms() };
        let poses = verrou().orphelines(maintenant, liees);
        let mut sortie = Vec::with_capacity(poses.len());
        for p in poses {
            if p.essais == 0 {
                verrou().compteurs.orphelines += 1;
            }
            let issue = retablir_avec_reessai(&p);
            sortie.push((p.hwnd, issue));
        }
        sortie
    }

    /// Rétablit tout (arrêt, fin de session, panique). `try_lock` : jamais
    /// d'interblocage depuis le crochet de panique.
    pub fn retablir_tout(journaliser: bool) {
        let poses = match REGISTRE.try_lock() {
            Ok(mut r) => r.tout_retirer(),
            Err(TryLockError::Poisoned(p)) => p.into_inner().tout_retirer(),
            Err(TryLockError::WouldBlock) => {
                if journaliser {
                    logging::avertir(
                        "Transitions DWM : registre occupé, rétablissement impossible.",
                    );
                }
                return;
            }
        };
        for p in poses {
            let issue = unsafe { appliquer_retablissement(&p) };
            if journaliser {
                logging::info(&format!(
                    "Transition DWM de {:#x} {}.",
                    p.hwnd,
                    issue.texte("arrêt")
                ));
            }
        }
    }

    /// Pose inscrite pour `cible`.
    pub fn pose(cible: isize) -> Option<Pose> {
        verrou().obtenir(cible).copied()
    }

    /// Vrai si aucune pose n'est inscrite.
    pub fn registre_vide() -> bool {
        verrou().len() == 0
    }

    pub fn compteurs() -> super::Compteurs {
        verrou().compteurs
    }

    pub fn noter_creation_vue() {
        verrou().compteurs.creations_vues += 1;
    }

    /// Relevé de l'état visible de `cible` (lectures seules, aucun message).
    pub unsafe fn sonder(cible: isize, ecoule_ms: u64) -> Releve {
        let h = hwnd(cible);
        let visible = IsWindowVisible(h).as_bool();
        let layered = GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_LAYERED != 0;
        let alpha = if layered {
            let mut cle = COLORREF(0);
            let mut a = 0u8;
            let mut drapeaux = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            (GetLayeredWindowAttributes(h, Some(&mut cle), Some(&mut a), Some(&mut drapeaux))
                .is_ok()
                && drapeaux.0 & LWA_ALPHA.0 != 0)
                .then_some(a)
        } else {
            None
        };
        let mut cloak = 0u32;
        let cloak = DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            &mut cloak as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .ok()
        .map(|_| cloak);
        Releve {
            ecoule_ms,
            visible,
            layered,
            alpha,
            cloak,
            transitions: lire(h),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(hwnd: isize, pose_ms: u64) -> Pose {
        Pose {
            hwnd,
            pid: 10,
            tid: 20,
            origine: Lecture::Valeur(false),
            moment: Moment::Creation,
            pose_ms,
            evenement_ms: Some(1000),
            essais: 0,
        }
    }

    fn releve(ecoule_ms: u64) -> Releve {
        Releve {
            ecoule_ms,
            visible: true,
            layered: true,
            alpha: Some(1),
            cloak: Some(0),
            transitions: Lecture::Valeur(true),
        }
    }

    #[test]
    fn registre_inscrire_retirer() {
        let mut r = Registre::new();
        assert_eq!(r.inscrire(pose(1, 0)), Inscription::Nouvelle);
        assert_eq!(r.len(), 1);
        assert_eq!(r.obtenir(1).map(|p| p.pid), Some(10));
        assert_eq!(r.retirer(1).map(|p| p.hwnd), Some(1));
        assert_eq!(r.retirer(1), None);
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn registre_deja_present() {
        let mut r = Registre::new();
        r.inscrire(pose(1, 5));
        let mut autre = pose(1, 99);
        autre.origine = Lecture::Illisible(0x8007_0057);
        assert_eq!(r.inscrire(autre), Inscription::DejaPresente);
        // L'inscription d'origine est conservée.
        assert_eq!(r.obtenir(1).map(|p| p.pose_ms), Some(5));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn registre_plein_refuse() {
        let mut r = Registre::new();
        for k in 0..CAPACITE_REGISTRE {
            assert_eq!(r.inscrire(pose(k as isize + 1, 0)), Inscription::Nouvelle);
        }
        assert_eq!(r.inscrire(pose(1000, 0)), Inscription::Pleine);
        assert_eq!(r.len(), CAPACITE_REGISTRE);
        assert!(r.obtenir(1000).is_none());
    }

    #[test]
    fn orphelines_selon_delai_et_liees() {
        let mut r = Registre::new();
        r.inscrire(pose(1, 0)); // âge 3000 : rendue
        r.inscrire(pose(2, 1)); // âge 2999 : gardée
        r.inscrire(pose(3, 0)); // liée : jamais rendue
        let rendues = r.orphelines(DELAI_ORPHELIN_MS, &[3]);
        assert_eq!(rendues.iter().map(|p| p.hwnd).collect::<Vec<_>>(), vec![1]);
        assert_eq!(r.len(), 2);
        // Toutes les non liées, quel que soit l'âge.
        let rendues = r.orphelines(u64::MAX, &[3]);
        assert_eq!(rendues.iter().map(|p| p.hwnd).collect::<Vec<_>>(), vec![2]);
        assert!(r.obtenir(3).is_some());
        // Horloge antérieure à la pose : jamais orpheline (pas de débordement).
        let mut r = Registre::new();
        r.inscrire(pose(4, 10_000));
        assert!(r.orphelines(5, &[]).is_empty());
    }

    #[test]
    fn essais_de_retablissement_bornes() {
        let p = pose(1, 0);
        let p1 = p.apres_echec_retablissement().expect("1er refus : réinscrite");
        assert_eq!(p1.essais, 1);
        assert_eq!(p1.pose_ms, p.pose_ms);
        let p2 = p1.apres_echec_retablissement().expect("2e refus : réinscrite");
        assert_eq!(p2.essais, 2);
        // 3e refus : MAX_ESSAIS_RETABLISSEMENT essais faits, abandon.
        assert_eq!(MAX_ESSAIS_RETABLISSEMENT, 3);
        assert!(p2.apres_echec_retablissement().is_none());
        // Pas de débordement.
        let mut pmax = pose(2, 0);
        pmax.essais = u8::MAX;
        assert!(pmax.apres_echec_retablissement().is_none());
    }

    #[test]
    fn poses_en_reessai_retentees_sans_delai_sauf_si_liees() {
        let mut r = Registre::new();
        let mut p = pose(1, 1000);
        p.essais = 1;
        r.inscrire(p);
        let mut q = pose(2, 1000);
        q.essais = 2;
        r.inscrire(q);
        r.inscrire(pose(3, 1000)); // jeune, sans échec : gardée
        let rendues = r.orphelines(1001, &[2]);
        assert_eq!(rendues.iter().map(|p| p.hwnd).collect::<Vec<_>>(), vec![1]);
        assert_eq!(r.len(), 2);
        // Pose renouvelée : compteur remis à zéro, plus retentée sans délai.
        r.reinitialiser_essais(2);
        assert_eq!(r.obtenir(2).map(|p| p.essais), Some(0));
        assert!(r.orphelines(1001, &[]).is_empty());
    }

    #[test]
    fn tout_retirer_vide() {
        let mut r = Registre::new();
        r.inscrire(pose(1, 0));
        r.inscrire(pose(2, 0));
        assert_eq!(r.tout_retirer().len(), 2);
        assert_eq!(r.len(), 0);
        assert!(r.tout_retirer().is_empty());
    }

    #[test]
    fn valeur_a_restaurer_vraie_seulement_si_lue_vraie() {
        let mut p = pose(1, 0);
        p.origine = Lecture::Valeur(true);
        assert!(p.valeur_a_restaurer());
    }

    #[test]
    fn valeur_a_restaurer_fausse_si_faux_ou_illisible() {
        let mut p = pose(1, 0);
        p.origine = Lecture::Valeur(false);
        assert!(!p.valeur_a_restaurer());
        p.origine = Lecture::Illisible(0x8007_0057);
        assert!(!p.valeur_a_restaurer());
    }

    #[test]
    fn delai_creation_affichage_debordement() {
        assert_eq!(delai_creation_affichage(1000, 1042), 42);
        assert_eq!(delai_creation_affichage(u32::MAX - 5, 10), 16);
    }

    #[test]
    fn sondes_paliers_dans_l_ordre() {
        let mut s = Sondes::default();
        assert!(s.due(0));
        s.noter(releve(0));
        assert!(!s.due(29));
        assert!(s.due(30));
        s.noter(releve(31));
        assert!(!s.due(59));
        assert!(s.due(60));
        for t in [60, 120, 200, 300] {
            assert!(s.due(t));
            s.noter(releve(t));
        }
        assert!(!s.due(10_000));
        assert_eq!(s.releves.len(), 6);
    }

    #[test]
    fn sondes_saut_de_paliers_un_seul_releve() {
        let mut s = Sondes::default();
        s.noter(releve(0));
        // Tick tardif : couvre 30, 60 et 120.
        assert!(s.due(130));
        s.noter(releve(130));
        assert_eq!(s.releves.len(), 2);
        assert!(!s.due(199));
        assert!(s.due(200));
    }

    #[test]
    fn sondes_texte_paliers_manques() {
        let mut s = Sondes::default();
        s.noter(releve(0));
        s.noter(releve(31));
        let t = s.texte(297);
        assert!(
            t.starts_with("T+0 : visible=oui, layered=oui, alpha=1, cloak=0x0, transitions=TRUE")
        );
        assert!(t.contains("T+31 : "));
        assert!(t.contains("T+60 : non relevé (fin à 297 ms)"));
        assert!(t.contains("T+300 : non relevé (fin à 297 ms)"));
        assert_eq!(t.matches("non relevé").count(), 4);
        assert_eq!(Sondes::default().texte(5).matches("non relevé").count(), 6);
    }

    #[test]
    fn texte_lecture_formats() {
        assert_eq!(texte_lecture(Lecture::Valeur(true)), "TRUE");
        assert_eq!(texte_lecture(Lecture::Valeur(false)), "FALSE");
        assert_eq!(
            texte_lecture(Lecture::Illisible(0x8007_0057)),
            "illisible (0x80070057)"
        );
    }

    #[test]
    fn rapport_texte_appel() {
        let r = RapportPose {
            origine: Lecture::Valeur(false),
            appel: Some(Ok(())),
            issue: IssuePose::Posee,
        };
        assert_eq!(r.texte_appel(), "S_OK");
        let r = RapportPose {
            appel: Some(Err(0x8007_0006)),
            issue: IssuePose::Refus(0x8007_0006),
            ..r
        };
        assert_eq!(r.texte_appel(), "0x80070006");
        let r = RapportPose {
            appel: None,
            issue: IssuePose::DejaDesactivee,
            ..r
        };
        assert!(r.texte_appel().contains("déjà désactivée"));
    }
}
