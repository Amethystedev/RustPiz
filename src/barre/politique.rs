//! Politique pure de réessai et de surveillance de l'intégration de la barre
//! des tâches (v0.6.0, correctifs d'audit).
//!
//! Aucune dépendance Win32 ni horloge : le temps est passé en millisecondes
//! (horloge monotone de l'appelant). Entièrement testable hors de Windows.
//!
//! Règles :
//! - les échecs sont mémorisés par couple (pid d'explorer, souhait) ; tout
//!   changement de l'un ou de l'autre remet la politique à zéro ;
//! - échec **passager** : nouvel essai après 30 s, puis 2 min, puis 10 min,
//!   puis plus aucun essai ([`DELAIS_REESSAI_MS`]) ;
//! - échec **durable** (conflit XAML, build trop ancien, SAC/WDAC, thème
//!   invalide, disjoncteur déclenché) : aucun nouvel essai tant que le pid et
//!   le souhait ne changent pas ;
//! - la surveillance ne réapplique que si explorer a changé de pid, si la
//!   session est morte APRÈS une application réussie (au plus
//!   [`MAX_PERTES_SESSION`] fois par couple), ou si un réessai est échu.

use super::decision::Souhait;

/// Délais entre un échec passager et l'essai suivant (30 s, 2 min, 10 min).
/// Après le dernier, plus aucun essai pour ce couple (pid, souhait).
pub const DELAIS_REESSAI_MS: [u64; 3] = [30_000, 120_000, 600_000];

/// Nombre maximal de réapplications après une perte de session (pour un même
/// couple pid/souhait), même si chaque réapplication réussit : évite une
/// boucle d'injection si la session meurt systématiquement.
pub const MAX_PERTES_SESSION: u32 = 3;

/// Le disjoncteur du runtime (`tripped`) est relu tous les N tours de
/// surveillance (un tour = 3 s). Le rapport d'état exécute un travail sur
/// chaque fil XAML d'explorer et écrit dans `runtime.log` : à chaque tour, ce
/// journal grossirait sans limite.
pub const TOURS_PAR_CONTROLE_DISJONCTEUR: u32 = 10;

/// Nature d'un échec d'application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nature {
    /// Peut se résoudre seul (réseau, runtime lent, fichier verrouillé…).
    Passager,
    /// Ne se résoudra pas sans changement du réglage ou d'explorer.
    Durable,
}

/// Conséquence d'un échec, pour le journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    /// Nouvel essai prévu dans `dans_ms`.
    Reessai { dans_ms: u64, essai: u32 },
    /// Échec passager répété : plus aucun essai pour ce couple.
    Abandon { echecs: u32 },
    /// Échec durable : aucun essai tant que pid et souhait ne changent pas.
    Durable,
}

impl Suite {
    /// Texte du journal décrivant la suite donnée à un échec.
    pub fn decrire(self) -> String {
        match self {
            Suite::Reessai { dans_ms, essai } => format!(
                "nouvel essai (n° {essai}) dans {} s.",
                dans_ms / 1000
            ),
            Suite::Abandon { echecs } => format!(
                "{echecs} échecs : plus aucun essai tant que le réglage ou l'Explorateur ne \
                 change pas."
            ),
            Suite::Durable => "échec durable : aucun nouvel essai tant que le réglage ou \
                               l'Explorateur ne change pas."
                .to_owned(),
        }
    }
}

/// Mémoire des échecs pour le couple (pid, souhait) courant.
#[derive(Debug, Default, Clone)]
pub struct Reessais {
    cle: Option<(u32, Souhait)>,
    /// Échecs passagers consécutifs.
    echecs: u32,
    /// Instant (ms) du dernier échec passager.
    dernier_echec_ms: u64,
    /// Un échec durable a été constaté.
    durable: bool,
    /// Réapplications après perte de session déjà accordées.
    pertes: u32,
}

impl Reessais {
    fn concerne(&self, pid: u32, souhait: &Souhait) -> bool {
        matches!(&self.cle, Some((p, s)) if *p == pid && s == souhait)
    }

    /// Remet tout à zéro si le couple change.
    fn aligner(&mut self, pid: u32, souhait: &Souhait) {
        if !self.concerne(pid, souhait) {
            *self = Reessais {
                cle: Some((pid, souhait.clone())),
                ..Reessais::default()
            };
        }
    }

    /// Enregistre un échec et renvoie la suite qui lui est donnée.
    pub fn noter_echec(
        &mut self,
        pid: u32,
        souhait: &Souhait,
        nature: Nature,
        maintenant_ms: u64,
    ) -> Suite {
        self.aligner(pid, souhait);
        if nature == Nature::Durable {
            self.durable = true;
        }
        if self.durable {
            return Suite::Durable;
        }
        self.echecs = self.echecs.saturating_add(1);
        self.dernier_echec_ms = maintenant_ms;
        match DELAIS_REESSAI_MS.get(self.echecs as usize - 1) {
            Some(&dans_ms) => Suite::Reessai {
                dans_ms,
                essai: self.echecs + 1,
            },
            None => Suite::Abandon {
                echecs: self.echecs,
            },
        }
    }

    /// Application réussie : les échecs sont oubliés (les pertes de session
    /// restent comptées pour ce couple).
    pub fn noter_succes(&mut self, pid: u32, souhait: &Souhait) {
        self.aligner(pid, souhait);
        self.echecs = 0;
        self.dernier_echec_ms = 0;
        self.durable = false;
    }

    /// Un réessai est-il échu pour ce couple ? Faux sans échec mémorisé (rien
    /// à réessayer), après un échec durable ou après l'abandon.
    pub fn reessai_du(&self, pid: u32, souhait: &Souhait, maintenant_ms: u64) -> bool {
        if !self.concerne(pid, souhait) || self.durable || self.echecs == 0 {
            return false;
        }
        match DELAIS_REESSAI_MS.get(self.echecs as usize - 1) {
            Some(&delai) => maintenant_ms.saturating_sub(self.dernier_echec_ms) >= delai,
            None => false,
        }
    }

    /// Session perdue après une application réussie : renvoie vrai si une
    /// réapplication est encore permise pour ce couple (et la compte).
    pub fn accorder_perte_session(&mut self, pid: u32, souhait: &Souhait) -> bool {
        self.aligner(pid, souhait);
        if self.durable || self.pertes >= MAX_PERTES_SESSION {
            return false;
        }
        self.pertes += 1;
        true
    }
}

/// Motif d'une réapplication décidée par la surveillance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motif {
    /// Explorer a changé de pid (redémarré) ou vient d'apparaître.
    ExplorerChange,
    /// Un réessai après échec passager est échu.
    Reessai,
}

/// Décision d'un tour de surveillance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surveillance {
    /// Rien à faire.
    Rien,
    /// Explorer introuvable : oublier le pid et l'état appliqué.
    Oublier,
    /// Application réussie dans ce pid : vérifier que la session vit encore.
    VerifierSession,
    /// Réappliquer le souhait.
    Reappliquer(Motif),
}

/// Décide le tour de surveillance.
///
/// `pid_actuel` : explorer qui détient la barre ; `pid_memorise` : celui de la
/// dernière tentative ; `applique` : la dernière application dans ce pid a
/// réussi ; `reessai_du` : [`Reessais::reessai_du`] pour (pid_actuel, souhait).
pub fn decider_surveillance(
    pid_actuel: Option<u32>,
    pid_memorise: Option<u32>,
    applique: bool,
    reessai_du: bool,
) -> Surveillance {
    match pid_actuel {
        None => Surveillance::Oublier,
        Some(pid) if pid_memorise != Some(pid) => Surveillance::Reappliquer(Motif::ExplorerChange),
        Some(_) if applique => Surveillance::VerifierSession,
        Some(_) if reessai_du => Surveillance::Reappliquer(Motif::Reessai),
        Some(_) => Surveillance::Rien,
    }
}

/// Le disjoncteur doit-il être relu à ce tour (compté à partir de 1) ?
pub fn controle_disjoncteur_du(tour: u32) -> bool {
    tour != 0 && tour.is_multiple_of(TOURS_PAR_CONTROLE_DISJONCTEUR)
}

/// Codes d'échec d'`InitializeXamlDiagnosticsEx` qui signalent un blocage
/// par une politique d'intégrité du code (Smart App Control, WDAC) : la DLL
/// non signée est refusée, réessayer ne sert à rien.
pub fn code_bloque_par_politique(hresult: i32) -> bool {
    const CODES: [u32; 5] = [
        0x8007_04EC, // ERROR_ACCESS_DISABLED_BY_POLICY
        0x8007_0241, // ERROR_INVALID_IMAGE_HASH
        0xC000_0428, // STATUS_INVALID_IMAGE_HASH
        0x8007_11C7, // ERROR_SYSTEM_INTEGRITY_POLICY_VIOLATION (4551)
        0xC0E9_0002, // STATUS_SYSTEM_INTEGRITY_POLICY_VIOLATION
    ];
    CODES.contains(&(hresult as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(theme: &str) -> Souhait {
        Souhait {
            theme: Some(theme.to_owned()),
            miniatures: None,
        }
    }

    #[test]
    fn backoff_30s_2min_10min_puis_abandon() {
        let mut r = Reessais::default();
        let a = s("surface");
        assert!(!r.reessai_du(10, &a, 0), "aucun échec : rien à réessayer");
        assert_eq!(
            r.noter_echec(10, &a, Nature::Passager, 1_000),
            Suite::Reessai { dans_ms: 30_000, essai: 2 }
        );
        assert!(!r.reessai_du(10, &a, 30_999));
        assert!(r.reessai_du(10, &a, 31_000));
        assert_eq!(
            r.noter_echec(10, &a, Nature::Passager, 31_000),
            Suite::Reessai { dans_ms: 120_000, essai: 3 }
        );
        assert!(!r.reessai_du(10, &a, 150_999));
        assert!(r.reessai_du(10, &a, 151_000));
        assert_eq!(
            r.noter_echec(10, &a, Nature::Passager, 151_000),
            Suite::Reessai { dans_ms: 600_000, essai: 4 }
        );
        assert!(r.reessai_du(10, &a, 751_000));
        assert_eq!(
            r.noter_echec(10, &a, Nature::Passager, 751_000),
            Suite::Abandon { echecs: 4 }
        );
        assert!(!r.reessai_du(10, &a, u64::MAX));
    }

    #[test]
    fn durable_bloque_jusqu_au_changement() {
        let mut r = Reessais::default();
        let a = s("surface");
        assert_eq!(r.noter_echec(10, &a, Nature::Durable, 0), Suite::Durable);
        // Un échec passager ultérieur ne débloque rien.
        assert_eq!(r.noter_echec(10, &a, Nature::Passager, 5), Suite::Durable);
        assert!(!r.reessai_du(10, &a, u64::MAX));
        // Changement de souhait ou de pid : remise à zéro.
        assert_eq!(
            r.noter_echec(10, &s("bubbles"), Nature::Passager, 0),
            Suite::Reessai { dans_ms: 30_000, essai: 2 }
        );
        assert_eq!(
            r.noter_echec(11, &a, Nature::Passager, 0),
            Suite::Reessai { dans_ms: 30_000, essai: 2 }
        );
    }

    #[test]
    fn succes_remet_les_echecs_a_zero() {
        let mut r = Reessais::default();
        let a = s("surface");
        r.noter_echec(10, &a, Nature::Passager, 0);
        r.noter_echec(10, &a, Nature::Passager, 40_000);
        r.noter_succes(10, &a);
        assert!(!r.reessai_du(10, &a, u64::MAX));
        assert_eq!(
            r.noter_echec(10, &a, Nature::Passager, 0),
            Suite::Reessai { dans_ms: 30_000, essai: 2 }
        );
    }

    #[test]
    fn pertes_de_session_bornees() {
        let mut r = Reessais::default();
        let a = s("surface");
        for _ in 0..MAX_PERTES_SESSION {
            assert!(r.accorder_perte_session(10, &a));
            r.noter_succes(10, &a);
        }
        assert!(!r.accorder_perte_session(10, &a), "boucle évitée");
        // Nouveau pid : de nouveau permis.
        assert!(r.accorder_perte_session(11, &a));
        // Après un échec durable : non.
        r.noter_echec(11, &a, Nature::Durable, 0);
        assert!(!r.accorder_perte_session(11, &a));
    }

    #[test]
    fn surveillance_ne_reapplique_pas_sur_simple_echec() {
        // Le défaut de l'audit : `applique == None` suffisait à réappliquer.
        assert_eq!(decider_surveillance(Some(10), Some(10), false, false), Surveillance::Rien);
        assert_eq!(
            decider_surveillance(Some(10), Some(10), false, true),
            Surveillance::Reappliquer(Motif::Reessai)
        );
        assert_eq!(
            decider_surveillance(Some(10), Some(10), true, false),
            Surveillance::VerifierSession
        );
        assert_eq!(
            decider_surveillance(Some(11), Some(10), false, false),
            Surveillance::Reappliquer(Motif::ExplorerChange)
        );
        assert_eq!(
            decider_surveillance(Some(11), None, true, false),
            Surveillance::Reappliquer(Motif::ExplorerChange)
        );
        assert_eq!(decider_surveillance(None, Some(10), true, true), Surveillance::Oublier);
    }

    #[test]
    fn disjoncteur_relu_periodiquement() {
        assert!(!controle_disjoncteur_du(0));
        assert!(!controle_disjoncteur_du(1));
        assert!(controle_disjoncteur_du(TOURS_PAR_CONTROLE_DISJONCTEUR));
        assert!(controle_disjoncteur_du(2 * TOURS_PAR_CONTROLE_DISJONCTEUR));
    }

    #[test]
    fn codes_de_politique_reconnus() {
        assert!(code_bloque_par_politique(0x8007_04ECu32 as i32));
        assert!(code_bloque_par_politique(0x8007_0241u32 as i32));
        assert!(code_bloque_par_politique(0x8007_11C7u32 as i32));
        assert!(!code_bloque_par_politique(0x8007_0005u32 as i32));
        assert!(!code_bloque_par_politique(0));
    }

    #[test]
    fn suites_decrites() {
        assert!(Suite::Reessai { dans_ms: 30_000, essai: 2 }.decrire().contains("30 s"));
        assert!(Suite::Durable.decrire().contains("durable"));
        assert!(Suite::Abandon { echecs: 4 }.decrire().contains('4'));
    }
}
