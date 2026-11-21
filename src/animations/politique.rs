//! Politiques pures du suivi de fermeture (testables hors Windows) :
//! - [`Lru`] : sessions de capture des fenêtres récemment actives, bornées ;
//! - [`rejet_transitoire`] / [`reessai_autorise`] : réessais différés d'une
//!   fenêtre au premier plan rejetée pour une raison passagère ;
//! - [`decision_disparition`] : règle HIDE / DESTROY d'une fenêtre suivie ;
//! - [`MemoireBornee`] : anti-spam du journal (une fois par fenêtre).

use std::collections::VecDeque;

/// Liste « la plus récente d'abord » d'éléments associés à une fenêtre
/// (valeur brute du HWND), au plus `max` éléments, une seule entrée par
/// fenêtre. Les éléments évincés sont RENDUS à l'appelant (qui les libère
/// hors de tout emprunt de l'état).
pub struct Lru<T> {
    elements: VecDeque<(isize, T)>,
    max: usize,
}

impl<T> Lru<T> {
    pub fn new(max: usize) -> Self {
        Lru {
            elements: VecDeque::with_capacity(max + 1),
            max,
        }
    }

    /// Ajoute `valeur` en tête ; renvoie l'ancienne valeur de la même
    /// fenêtre (doublon) et les éléments évincés au-delà de la borne.
    pub fn pousser(&mut self, cle: isize, valeur: T) -> Vec<T> {
        let mut rendus = Vec::new();
        if let Some(v) = self.retirer(cle) {
            rendus.push(v);
        }
        self.elements.push_front((cle, valeur));
        while self.elements.len() > self.max {
            if let Some((_, v)) = self.elements.pop_back() {
                rendus.push(v);
            }
        }
        rendus
    }

    /// Retire l'élément de la fenêtre `cle`.
    pub fn retirer(&mut self, cle: isize) -> Option<T> {
        let position = self.elements.iter().position(|(c, _)| *c == cle)?;
        self.elements.remove(position).map(|(_, v)| v)
    }

    #[cfg(test)]
    pub fn contient(&self, cle: isize) -> bool {
        self.elements.iter().any(|(c, _)| *c == cle)
    }

    pub fn obtenir(&self, cle: isize) -> Option<&T> {
        self.elements.iter().find(|(c, _)| *c == cle).map(|(_, v)| v)
    }

    /// Fenêtres présentes, de la plus récente à la plus ancienne.
    pub fn cles(&self) -> Vec<isize> {
        self.elements.iter().map(|(c, _)| *c).collect()
    }

    /// Vide la liste et rend tous les éléments.
    pub fn vider(&mut self) -> Vec<T> {
        self.elements.drain(..).map(|(_, v)| v).collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

/// Raisons de rejet d'éligibilité passagères au moment de
/// `EVENT_SYSTEM_FOREGROUND` : une fenêtre neuve n'est souvent pas encore
/// visible / dimensionnée / « décloakée », une fenêtre restaurée peut être
/// encore réduite. Préfixes des textes produits par
/// `eligibilite::verifier` et `window::raison_exclusion_silencieuse`.
const RAISONS_TRANSITOIRES: [&str; 6] = [
    "fenêtre invisible",
    "fenêtre réduite",
    "masquée par DWM",
    "bornes illisibles",
    "taille hors limites",
    "état de l'application indisponible",
];

/// Vrai si le rejet peut disparaître de lui-même dans les instants qui
/// suivent (un réessai différé a un sens).
pub fn rejet_transitoire(raison: &str) -> bool {
    RAISONS_TRANSITOIRES.iter().any(|p| raison.starts_with(p))
}

/// Faut-il programmer un nouvel essai ? `essai` = nombre de réessais déjà
/// effectués (0 au premier rejet).
pub fn reessai_autorise(transitoire: bool, essai: u32, max: u32) -> bool {
    transitoire && essai < max
}

/// Décision pour une fenêtre suivie qui reçoit HIDE ou DESTROY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disparition {
    /// Masquage parasite : le suivi continue (motif pour le journal).
    Ignorer(&'static str),
    /// Fermeture : on rejoue la dernière image.
    Animer,
}

/// Règle HIDE / DESTROY : on anime au premier des deux, SAUF un HIDE pour
/// lequel la fenêtre existe encore ET est réduite ou de nouveau visible.
pub fn decision_disparition(detruite: bool, existe: bool, reduite: bool, visible: bool) -> Disparition {
    if detruite || !existe {
        return Disparition::Animer;
    }
    if reduite {
        Disparition::Ignorer("réduite")
    } else if visible {
        Disparition::Ignorer("de nouveau visible")
    } else {
        Disparition::Animer
    }
}

/// Ensemble borné de fenêtres (les plus anciennes sont oubliées).
pub struct MemoireBornee {
    elements: VecDeque<isize>,
    max: usize,
}

impl MemoireBornee {
    pub fn new(max: usize) -> Self {
        MemoireBornee {
            elements: VecDeque::with_capacity(max),
            max,
        }
    }

    /// Mémorise `h` ; `true` si elle n'y était pas déjà.
    pub fn marquer(&mut self, h: isize) -> bool {
        if self.elements.contains(&h) {
            return false;
        }
        if self.elements.len() >= self.max {
            self.elements.pop_front();
        }
        self.elements.push_back(h);
        true
    }

    /// Oublie `h` ; `true` si elle était mémorisée.
    pub fn retirer(&mut self, h: isize) -> bool {
        match self.elements.iter().position(|e| *e == h) {
            Some(p) => {
                self.elements.remove(p);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub fn contient(&self, h: isize) -> bool {
        self.elements.contains(&h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_borne_et_ordre() {
        let mut l = Lru::new(2);
        assert!(l.pousser(1, "a").is_empty());
        assert!(l.pousser(2, "b").is_empty());
        assert_eq!(l.cles(), vec![2, 1]);
        // Troisième : la plus ancienne (1) est évincée et rendue.
        assert_eq!(l.pousser(3, "c"), vec!["a"]);
        assert_eq!(l.cles(), vec![3, 2]);
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn lru_doublon_remplace_et_remonte() {
        let mut l = Lru::new(3);
        l.pousser(1, 10);
        l.pousser(2, 20);
        assert_eq!(l.pousser(1, 11), vec![10]);
        assert_eq!(l.cles(), vec![1, 2]);
        assert_eq!(l.obtenir(1), Some(&11));
    }

    #[test]
    fn lru_retirer_et_vider() {
        let mut l = Lru::new(2);
        l.pousser(5, 'x');
        l.pousser(6, 'y');
        assert!(l.contient(5));
        assert_eq!(l.retirer(5), Some('x'));
        assert!(!l.contient(5));
        assert_eq!(l.retirer(5), None);
        assert_eq!(l.vider(), vec!['y']);
        assert!(l.is_empty());
    }

    #[test]
    fn lru_borne_nulle() {
        let mut l = Lru::new(0);
        assert_eq!(l.pousser(1, 1), vec![1]);
        assert!(l.is_empty());
    }

    #[test]
    fn rejets_transitoires() {
        assert!(rejet_transitoire("fenêtre invisible"));
        assert!(rejet_transitoire("fenêtre réduite"));
        assert!(rejet_transitoire("masquée par DWM (DWMWA_CLOAKED = 0x2)"));
        assert!(rejet_transitoire("bornes illisibles"));
        assert!(rejet_transitoire(
            "taille hors limites (style 0x14cf0000, exstyle 0x00000100, 0x0)"
        ));
        assert!(!rejet_transitoire("fenêtre de RustPiz (style …)"));
        assert!(!rejet_transitoire("fenêtre d'outil (WS_EX_TOOLWINDOW) (style …)"));
        assert!(!rejet_transitoire("classe exclue (exclusions_classes)"));
        assert!(!rejet_transitoire("protégée contre la capture (affinité 0x11)"));
        assert!(!rejet_transitoire("handle invalide ou fenêtre détruite"));
    }

    #[test]
    fn reessais_bornes() {
        assert!(reessai_autorise(true, 0, 10));
        assert!(reessai_autorise(true, 9, 10));
        assert!(!reessai_autorise(true, 10, 10));
        assert!(!reessai_autorise(false, 0, 10));
    }

    #[test]
    fn regle_hide_destroy() {
        use Disparition::*;
        // DESTROY : toujours animé.
        assert_eq!(decision_disparition(true, true, true, true), Animer);
        // HIDE d'une fenêtre déjà détruite : animé.
        assert_eq!(decision_disparition(false, false, false, false), Animer);
        // HIDE parasite : réduction ou réaffichage.
        assert_eq!(decision_disparition(false, true, true, false), Ignorer("réduite"));
        assert_eq!(
            decision_disparition(false, true, false, true),
            Ignorer("de nouveau visible")
        );
        // HIDE d'une fenêtre masquée encore existante : animé.
        assert_eq!(decision_disparition(false, true, false, false), Animer);
    }

    #[test]
    fn memoire_bornee() {
        let mut m = MemoireBornee::new(2);
        assert!(m.marquer(1));
        assert!(!m.marquer(1));
        assert!(m.marquer(2));
        assert!(m.marquer(3)); // 1 oubliée
        assert!(!m.contient(1));
        assert!(m.retirer(2));
        assert!(!m.retirer(2));
        assert!(m.contient(3));
    }
}
