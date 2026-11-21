//! Masquage précoce de l'ouverture (v0.4.2) — logique pure, testable hors
//! Windows (aucune dépendance à la crate `windows`).
//!
//! Problème corrigé : en v0.4.1, à l'affichage d'une fenêtre, la capture
//! WGC démarrait fenêtre visible et le masquage n'avait lieu qu'à la 1re
//! image ; l'utilisateur voyait donc la vraie fenêtre s'ouvrir, disparaître,
//! puis l'animation se jouer (« double ouverture »).
//!
//! Mode « précoce » : la fenêtre est masquée (layered, alpha 0 ; alpha
//! 1/255 depuis la v0.4.5) DÈS
//! `EVENT_OBJECT_SHOW`, avant la création de la session de capture. On
//! ignore toutefois si WGC applique l'alpha layered à l'image capturée :
//! si c'est le cas, l'image reçue est entièrement transparente et rien ne
//! peut être animé. Ce module décide quand basculer définitivement (pour
//! la session, réarmable) vers l'ancien mode « capture d'abord ».
//!
//! v0.4.6 : l'analyse des pixels (image transparente / uniforme) et la
//! politique d'attente de l'image d'ouverture sont dans [`super::attente`] ;
//! ce module n'est plus nourri qu'à la DÉCISION finale de l'attente (image
//! retenue non vide, dernière image transparente au plafond, ou aucune
//! image au plafond).

use super::attente::DELAI_STABILITE_PREMIERE_MS;

/// Images vides PROBANTES ([`vide_probante`]) CONSÉCUTIVES (mode précoce)
/// avant la bascule en mode « capture d'abord ». 2 et non 1 : une fenêtre affichée mais pas encore
/// peinte pourrait produire une 1re image transparente sans que WGC
/// n'applique l'alpha layered ; une seule occurrence ne doit pas désactiver
/// définitivement le mode précoce.
pub const SEUIL_IMAGES_VIDES: u32 = 2;
/// Attentes expirées SANS AUCUNE image, CONSÉCUTIVES (mode précoce), avant
/// la bascule : WGC pourrait ne livrer aucune image d'une fenêtre masquée ;
/// chaque expiration coûte alors une fenêtre invisible pendant le plafond
/// d'attente, sans animation.
pub const SEUIL_DELAIS_EXPIRES: u32 = 3;

/// Vrai si une image transparente obtenue avec ce silence (âge de la
/// dernière image reçue, ms) est probante pour la bascule : la fenêtre
/// était réellement stable (aucune nouvelle image depuis au moins
/// [`DELAI_STABILITE_PREMIERE_MS`]). Sinon l'application se peignait
/// encore (abandon au plafond d'une appli lente) : non compté.
pub fn vide_probante(silence_ms: Option<u64>) -> bool {
    silence_ms.is_some_and(|s| s >= DELAI_STABILITE_PREMIERE_MS)
}

/// Mode d'ouverture courant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Masquage à `EVENT_OBJECT_SHOW`, avant la capture (v0.4.2).
    Precoce,
    /// Ancien comportement : capture fenêtre visible, masquage à la 1re
    /// image (v0.3.1 – v0.4.1).
    CaptureDabord,
}

/// Raison de la bascule en mode « capture d'abord ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisonBascule {
    /// WGC renvoie des images transparentes d'une fenêtre à alpha 0.
    ImagesVides,
    /// WGC ne livre pas d'image d'une fenêtre à alpha 0 dans le délai.
    AucuneImage,
}

/// Adaptation du mode d'ouverture (compteurs d'échecs consécutifs du mode
/// précoce). Réinitialisée en recochant « Animer l'ouverture ».
#[derive(Debug, Clone, Copy, Default)]
pub struct Adaptation {
    images_vides: u32,
    delais_expires: u32,
    bascule: Option<RaisonBascule>,
}

impl Adaptation {
    /// Mode à utiliser pour la prochaine ouverture.
    pub fn mode(&self) -> Mode {
        if self.bascule.is_some() {
            Mode::CaptureDabord
        } else {
            Mode::Precoce
        }
    }

    /// Raison de la bascule (`None` = mode précoce toujours actif).
    pub fn bascule(&self) -> Option<RaisonBascule> {
        self.bascule
    }

    /// Compteurs (images vides, délais expirés) pour le journal.
    pub fn compteurs(&self) -> (u32, u32) {
        (self.images_vides, self.delais_expires)
    }

    /// Image retenue par l'attente en mode précoce (`vide` = dernière image
    /// transparente au plafond ; `silence_ms` = âge de la dernière image
    /// reçue au moment de la décision). Renvoie `Some(raison)` si cette
    /// image provoque la bascule. Une image non vide remet les deux
    /// compteurs à zéro (le mode précoce fonctionne). Une image vide ne
    /// compte que si elle est PROBANTE ([`vide_probante`] : fenêtre stable
    /// depuis [`DELAI_STABILITE_PREMIERE_MS`]) ; une image vide encore
    /// changeante au plafond (application lente à se peindre) ne prouve
    /// rien sur WGC : compteurs inchangés.
    pub fn image_recue(&mut self, vide: bool, silence_ms: Option<u64>) -> Option<RaisonBascule> {
        if self.bascule.is_some() {
            return None;
        }
        if !vide {
            self.images_vides = 0;
            self.delais_expires = 0;
            return None;
        }
        if !vide_probante(silence_ms) {
            return None;
        }
        self.images_vides = self.images_vides.saturating_add(1);
        if self.images_vides >= SEUIL_IMAGES_VIDES {
            self.bascule = Some(RaisonBascule::ImagesVides);
            return self.bascule;
        }
        None
    }

    /// Attente expirée sans aucune image en mode précoce. Renvoie
    /// `Some(raison)` si cette expiration provoque la bascule. (Aucune
    /// image du tout pendant le plafond n'est pas un cas d'« appli lente » :
    /// WGC livre au moins une image dès qu'une fenêtre visible change.)
    pub fn delai_expire(&mut self) -> Option<RaisonBascule> {
        if self.bascule.is_some() {
            return None;
        }
        self.delais_expires = self.delais_expires.saturating_add(1);
        if self.delais_expires >= SEUIL_DELAIS_EXPIRES {
            self.bascule = Some(RaisonBascule::AucuneImage);
            return self.bascule;
        }
        None
    }

    /// Retour au mode précoce (case « Animer l'ouverture » recochée).
    pub fn reinitialiser(&mut self) {
        *self = Adaptation::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence probant (fenêtre stable).
    const STABLE: Option<u64> = Some(DELAI_STABILITE_PREMIERE_MS);

    #[test]
    fn adaptation_bascule_apres_images_vides_consecutives() {
        let mut a = Adaptation::default();
        assert_eq!(a.mode(), Mode::Precoce);
        assert_eq!(a.image_recue(true, STABLE), None);
        assert_eq!(a.mode(), Mode::Precoce);
        // Une image valide remet à zéro.
        assert_eq!(a.image_recue(false, None), None);
        assert_eq!(a.compteurs(), (0, 0));
        assert_eq!(a.image_recue(true, STABLE), None);
        assert_eq!(a.image_recue(true, Some(5000)), Some(RaisonBascule::ImagesVides));
        assert_eq!(a.mode(), Mode::CaptureDabord);
        // Plus de nouvelle bascule signalée ensuite.
        assert_eq!(a.image_recue(true, STABLE), None);
        assert_eq!(a.delai_expire(), None);
        a.reinitialiser();
        assert_eq!(a.mode(), Mode::Precoce);
        assert_eq!(a.bascule(), None);
    }

    #[test]
    fn image_vide_non_stable_ne_compte_pas() {
        let mut a = Adaptation::default();
        // Appli lente : images vides encore changeantes au plafond.
        for silence in [None, Some(0), Some(40), Some(DELAI_STABILITE_PREMIERE_MS - 1)] {
            assert!(!vide_probante(silence));
            assert_eq!(a.image_recue(true, silence), None);
            assert_eq!(a.compteurs(), (0, 0));
        }
        assert_eq!(a.mode(), Mode::Precoce);
        // Ni remise à zéro : une image probante précédente reste comptée.
        assert_eq!(a.image_recue(true, STABLE), None);
        assert_eq!(a.compteurs(), (1, 0));
        assert_eq!(a.image_recue(true, Some(10)), None);
        assert_eq!(a.compteurs(), (1, 0));
        assert_eq!(a.image_recue(true, STABLE), Some(RaisonBascule::ImagesVides));
    }

    #[test]
    fn adaptation_bascule_apres_delais_expires() {
        let mut a = Adaptation::default();
        assert_eq!(a.delai_expire(), None);
        assert_eq!(a.delai_expire(), None);
        // Une image valide remet aussi le compteur de délais à zéro.
        assert_eq!(a.image_recue(false, None), None);
        assert_eq!(a.compteurs(), (0, 0));
        for _ in 1..SEUIL_DELAIS_EXPIRES {
            assert_eq!(a.delai_expire(), None);
        }
        assert_eq!(a.delai_expire(), Some(RaisonBascule::AucuneImage));
        assert_eq!(a.mode(), Mode::CaptureDabord);
        assert_eq!(a.bascule(), Some(RaisonBascule::AucuneImage));
    }
}
