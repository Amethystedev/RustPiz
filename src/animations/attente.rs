//! Attente de l'image d'ouverture (v0.4.6) — logique pure, testable hors
//! Windows (aucune dépendance à la crate `windows`).
//!
//! **Diagnostic prouvé (vidéo + journal v0.4.5)** : la 1re image WGC d'une
//! fenêtre qui s'ouvre arrive 13 à 30 ms après `EVENT_OBJECT_SHOW`, AVANT
//! que l'application ait peint son contenu (Bloc-notes : cadre vide avec
//! l'icône, texte apparu ~0,3 s après l'effet ; Photos : image initiale
//! transparente). Les versions 0.4.2 à 0.4.5 jouaient l'effet sur cette
//! image incomplète, puis tentaient de « maintenir » la surimpression
//! jusqu'au repaint (comparaison à l'image initiale incomplète : jamais
//! convergé, surimpression tenue jusqu'au plafond de 1,6 s).
//!
//! **Politique v0.4.6** : la fenêtre reste masquée (alpha 1/255, elle
//! continue de se peindre sous le masque) et la session WGC d'ouverture
//! reste ouverte ; l'effet n'est lancé qu'avec l'image LA PLUS RÉCENTE une
//! fois la fenêtre réellement peinte :
//! 1. **stabilité temporelle d'abord** : au moins une image reçue et plus
//!    aucune nouvelle image depuis [`DELAI_STABILITE_MS`] (WGC ne livre une
//!    image que si la fenêtre change ; intervalle minimal de capture
//!    100 ms) — [`DELAI_STABILITE_PREMIERE_MS`] si une seule image a été
//!    reçue (la 1re image précède souvent le 1er paint, c'est le cas prouvé) ;
//! 2. **garde-fou de contenu** : l'image retenue ne doit pas être « vide »
//!    ([`ContenuImage::Transparente`] : presque aucun pixel visible, comme
//!    l'image initiale de Photos ou une image à l'alpha 1 du masque ;
//!    [`ContenuImage::Uniforme`] : tous les pixels visibles échantillonnés
//!    de la même couleur, comme un cadre pas encore peint). Une fenêtre
//!    sombre n'est PAS vide : seul compte l'écart entre pixels ;
//! 3. **plafond** [`PLAFOND_ATTENTE_MS`] compté depuis le SHOW : dernière
//!    image non vide (même encore changeante) -> effet joué avec elle ;
//!    dernière image [`ContenuImage::Uniforme`] (non transparente) ET
//!    stable -> effet joué aussi (fenêtre réellement unie, rien de mieux
//!    ne viendra) ; aucune image, dernière image transparente, ou
//!    uniforme encore changeante -> effet NON joué, fenêtre restaurée
//!    immédiatement.
//!
//! L'image n'est analysée (copie GPU de quelques blocs, une lecture) qu'au
//! moment d'une décision (stabilité ou plafond atteints), jamais à chaque
//! image reçue ; elle n'est copiée dans la surimpression qu'une fois retenue.

/// Période de la scrutation de l'attente (minuteur Win32).
pub const TICK_ATTENTE_MS: u32 = 25;
/// Silence (aucune nouvelle image) exigé pour déclarer la fenêtre peinte
/// quand au moins deux images ont été reçues. Supérieur à l'intervalle
/// minimal de capture WGC (100 ms) avec une marge pour la gigue.
pub const DELAI_STABILITE_MS: u64 = 150;
/// Silence exigé quand UNE SEULE image a été reçue : la 1re image arrive
/// souvent avant le 1er paint (prouvé par le journal v0.4.5), on laisse à
/// l'application le temps d'en produire une deuxième.
pub const DELAI_STABILITE_PREMIERE_MS: u64 = 250;
/// Plafond de l'attente (masquage précoce), compté depuis le SHOW : la
/// fenêtre est invisible au plus pendant ce temps avant l'effet ou sa
/// restauration.
pub const PLAFOND_ATTENTE_MS: u64 = 750;
/// Plafond de l'attente de la 1re image en mode « capture d'abord »
/// (fenêtre visible pendant ce temps ; l'effet part dès la 1re image).
pub const PLAFOND_PREMIERE_IMAGE_MS: u64 = 200;

/// Largeur d'un bloc échantillonné (pixels).
pub const BLOC_LARGEUR: u32 = 16;
/// Hauteur d'un bloc échantillonné (pixels).
pub const BLOC_HAUTEUR: u32 = 4;
/// Grille des centres des blocs échantillonnés (colonnes x lignes), à 10,
/// 30, 50, 70 et 90 % de la largeur / hauteur.
pub const GRILLE_ECHANTILLONS: u32 = 5;
/// Alpha en dessous duquel (inclus) un pixel est considéré comme invisible
/// (fenêtre masquée à l'alpha 1 dont WGC appliquerait l'alpha, fond
/// transparent d'une application pas encore peinte).
pub const ALPHA_INVISIBLE: u8 = 32;
/// Fraction minimale de pixels visibles pour qu'une image ne soit pas
/// « transparente ».
pub const FRACTION_VISIBLE_MIN: f32 = 0.05;
/// Écart maximal par canal (B, G, R, A) entre pixels visibles d'une image
/// « uniforme ».
pub const TOLERANCE_UNIFORME: u8 = 6;

/// Rectangle d'échantillonnage dans l'image source (pixels, bornes
/// exclusives à droite / en bas, comme `D3D11_BOX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bloc {
    pub gauche: u32,
    pub haut: u32,
    pub droite: u32,
    pub bas: u32,
}

impl Bloc {
    pub fn largeur(&self) -> u32 {
        self.droite - self.gauche
    }
    pub fn hauteur(&self) -> u32 {
        self.bas - self.haut
    }
}

/// Blocs à échantillonner dans une image `largeur` x `hauteur` : grille de
/// [`GRILLE_ECHANTILLONS`]² centres répartis sur toute l'image. Chaque bloc
/// est rogné à l'image ; les doublons (petites images) sont retirés ; image
/// sans pixel -> aucun bloc. Les blocs sont empilés verticalement dans la
/// texture de lecture (ligne de départ du bloc `k` = `k * BLOC_HAUTEUR`).
pub fn blocs_echantillons(largeur: u32, hauteur: u32) -> Vec<Bloc> {
    if largeur == 0 || hauteur == 0 {
        return Vec::new();
    }
    let l = BLOC_LARGEUR.min(largeur);
    let h = BLOC_HAUTEUR.min(hauteur);
    let n = u64::from(GRILLE_ECHANTILLONS);
    let mut blocs: Vec<Bloc> = Vec::with_capacity((n * n) as usize);
    for fy in 0..n {
        for fx in 0..n {
            // Centre à (2k + 1) / (2n) de la dimension.
            let cx = (u64::from(largeur) * (2 * fx + 1) / (2 * n)) as u32;
            let cy = (u64::from(hauteur) * (2 * fy + 1) / (2 * n)) as u32;
            let gauche = cx.saturating_sub(l / 2).min(largeur - l);
            let haut = cy.saturating_sub(h / 2).min(hauteur - h);
            let bloc = Bloc {
                gauche,
                haut,
                droite: gauche + l,
                bas: haut + h,
            };
            if !blocs.contains(&bloc) {
                blocs.push(bloc);
            }
        }
    }
    blocs
}

/// Nature du contenu d'une image (garde-fou, voir l'en-tête du module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContenuImage {
    /// Presque aucun pixel visible (alpha <= [`ALPHA_INVISIBLE`]).
    Transparente,
    /// Tous les pixels visibles échantillonnés ont la même couleur.
    Uniforme,
    /// Contenu réel (au moins deux couleurs visibles distinctes).
    Contenu,
}

impl ContenuImage {
    /// Vrai si l'image ne peut pas servir à l'effet.
    pub fn vide(self) -> bool {
        self != ContenuImage::Contenu
    }

    /// Texte pour le journal.
    pub fn texte(self) -> &'static str {
        match self {
            ContenuImage::Transparente => "transparente",
            ContenuImage::Uniforme => "uniforme (une seule couleur)",
            ContenuImage::Contenu => "contenu présent",
        }
    }
}

/// Classe les pixels échantillonnés. `octets` : lignes B8G8R8A8 de la
/// texture de lecture, `pas` octets par ligne ; `blocs` : (largeur,
/// hauteur) de chaque bloc, le bloc `k` commençant à la ligne
/// `k * BLOC_HAUTEUR`, colonne 0. Données incohérentes (pas trop petit,
/// tampon trop court, bloc plus grand que [`BLOC_LARGEUR`] x
/// [`BLOC_HAUTEUR`]) ou aucun pixel : `None` (aucune conclusion sans preuve).
pub fn classer(octets: &[u8], pas: usize, blocs: &[(u32, u32)]) -> Option<ContenuImage> {
    if blocs.is_empty() {
        return None;
    }
    let mut total = 0usize;
    let mut visibles = 0usize;
    let mut reference: Option<[u8; 4]> = None;
    let mut uniforme = true;
    for (k, &(l, h)) in blocs.iter().enumerate() {
        if l == 0 || h == 0 || l > BLOC_LARGEUR || h > BLOC_HAUTEUR {
            return None;
        }
        let utile = l as usize * 4;
        if pas < utile {
            return None;
        }
        for y in 0..h as usize {
            let ligne = k * BLOC_HAUTEUR as usize + y;
            let debut = ligne.checked_mul(pas)?;
            let fin = debut.checked_add(utile)?;
            let pixels = octets.get(debut..fin)?;
            let (morceaux, _) = pixels.as_chunks::<4>();
            for p in morceaux {
                total += 1;
                if p[3] <= ALPHA_INVISIBLE {
                    continue;
                }
                visibles += 1;
                let px = [p[0], p[1], p[2], p[3]];
                match reference {
                    None => reference = Some(px),
                    Some(r) => {
                        if r.iter()
                            .zip(px.iter())
                            .any(|(a, b)| a.abs_diff(*b) > TOLERANCE_UNIFORME)
                        {
                            uniforme = false;
                        }
                    }
                }
            }
        }
    }
    if total == 0 {
        return None;
    }
    if (visibles as f32) < FRACTION_VISIBLE_MIN * total as f32 || visibles == 0 {
        return Some(ContenuImage::Transparente);
    }
    Some(if uniforme {
        ContenuImage::Uniforme
    } else {
        ContenuImage::Contenu
    })
}

/// Politique applicable à l'attente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeAttente {
    /// Masquage précoce : attente de la stabilité (plafond
    /// [`PLAFOND_ATTENTE_MS`]).
    Stabilite,
    /// « Capture d'abord » (fenêtre visible) : décision dès la 1re image
    /// (plafond [`PLAFOND_PREMIERE_IMAGE_MS`]).
    PremiereImage,
}

impl ModeAttente {
    pub fn plafond_ms(self) -> u64 {
        match self {
            ModeAttente::Stabilite => PLAFOND_ATTENTE_MS,
            ModeAttente::PremiereImage => PLAFOND_PREMIERE_IMAGE_MS,
        }
    }
}

/// Observation de la session de capture à un instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// Temps écoulé depuis le traitement du SHOW (ms).
    pub ecoule_ms: u64,
    /// Images reçues par la session (numéro de la plus récente).
    pub images: u64,
    /// Âge de la dernière image reçue (ms) ; `None` si aucune.
    pub silence_ms: Option<u64>,
}

/// Raison du choix de l'image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisonChoix {
    /// Plus aucune nouvelle image depuis le seuil de stabilité.
    Stable,
    /// Plafond atteint, image non vide mais encore changeante.
    Plafond,
    /// Mode « capture d'abord » : 1re image.
    PremiereImage,
    /// Plafond atteint, image uniforme mais non transparente et stable.
    PlafondUniforme,
}

/// Raison de l'abandon (effet non joué, fenêtre restaurée).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisonAbandon {
    /// Aucune image reçue avant le plafond.
    AucuneImage,
    /// Dernière image vide au plafond (ou dès la 1re image en « capture
    /// d'abord »).
    ImageVide(ContenuImage),
}

/// Décision à un tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Continuer d'attendre (tick suivant).
    Attendre,
    /// Analyser l'image la plus récente (puis redécider).
    Analyser,
    /// Lancer l'effet avec l'image la plus récente (déjà analysée).
    Jouer(RaisonChoix),
    /// Ne pas jouer l'effet ; restaurer la fenêtre.
    Abandonner(RaisonAbandon),
}

/// Seuil de silence applicable selon le nombre d'images reçues.
pub fn seuil_stabilite(images: u64) -> u64 {
    if images <= 1 {
        DELAI_STABILITE_PREMIERE_MS
    } else {
        DELAI_STABILITE_MS
    }
}

/// Vrai si l'image la plus récente est stable (au moins une image et plus
/// rien depuis le seuil).
pub fn est_stable(obs: &Observation) -> bool {
    obs.images > 0 && obs.silence_ms.is_some_and(|s| s >= seuil_stabilite(obs.images))
}

/// État pur de l'attente : dernière image analysée (numéro, contenu).
#[derive(Debug, Clone, Copy, Default)]
pub struct Attente {
    analyse: Option<(u64, ContenuImage)>,
    /// Nombre d'analyses effectuées (journal).
    pub analyses: u32,
}

impl Attente {
    /// Mémorise l'analyse de l'image numéro `numero`.
    pub fn noter_analyse(&mut self, numero: u64, contenu: ContenuImage) {
        self.analyse = Some((numero, contenu));
        self.analyses = self.analyses.saturating_add(1);
    }

    /// Numéro de la dernière image analysée.
    pub fn numero_analyse(&self) -> Option<u64> {
        self.analyse.map(|(n, _)| n)
    }

    /// Contenu de la dernière image analysée.
    pub fn contenu_analyse(&self) -> Option<ContenuImage> {
        self.analyse.map(|(_, c)| c)
    }

    /// Décide à partir de l'observation courante. L'image la plus récente
    /// (numéro `obs.images`) doit avoir été analysée avant toute décision
    /// finale ; sinon [`Action::Analyser`] est demandé.
    pub fn decider(&self, obs: Observation, mode: ModeAttente) -> Action {
        let plafond = obs.ecoule_ms >= mode.plafond_ms();
        if obs.images == 0 {
            return if plafond {
                Action::Abandonner(RaisonAbandon::AucuneImage)
            } else {
                Action::Attendre
            };
        }
        let prete = match mode {
            ModeAttente::PremiereImage => true,
            ModeAttente::Stabilite => est_stable(&obs),
        };
        if !prete && !plafond {
            return Action::Attendre;
        }
        let contenu = match self.analyse {
            Some((n, c)) if n == obs.images => c,
            _ => return Action::Analyser,
        };
        if !contenu.vide() {
            return Action::Jouer(match mode {
                ModeAttente::PremiereImage => RaisonChoix::PremiereImage,
                ModeAttente::Stabilite if prete => RaisonChoix::Stable,
                ModeAttente::Stabilite => RaisonChoix::Plafond,
            });
        }
        // Image vide : en « capture d'abord », pas de seconde chance (la
        // fenêtre est visible) ; en masquage précoce, on attend une image
        // plus récente jusqu'au plafond. Au plafond, une image uniforme
        // (non transparente) et stable est jouée : la fenêtre est
        // réellement unie et ne changera plus.
        if mode == ModeAttente::Stabilite
            && plafond
            && prete
            && contenu == ContenuImage::Uniforme
        {
            return Action::Jouer(RaisonChoix::PlafondUniforme);
        }
        if mode == ModeAttente::PremiereImage || plafond {
            Action::Abandonner(RaisonAbandon::ImageVide(contenu))
        } else {
            Action::Attendre
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ecoule_ms: u64, images: u64, silence_ms: Option<u64>) -> Observation {
        Observation {
            ecoule_ms,
            images,
            silence_ms,
        }
    }

    /// Texture de lecture : `n` blocs de 16x4 empilés, pixels donnés par
    /// `pixel(k, x, y)`.
    fn texture(n: usize, pas: usize, pixel: impl Fn(usize, usize, usize) -> [u8; 4]) -> Vec<u8> {
        let lignes = n * BLOC_HAUTEUR as usize;
        let mut v = vec![0u8; pas * lignes];
        for k in 0..n {
            for y in 0..BLOC_HAUTEUR as usize {
                for x in 0..BLOC_LARGEUR as usize {
                    let i = (k * BLOC_HAUTEUR as usize + y) * pas + x * 4;
                    v[i..i + 4].copy_from_slice(&pixel(k, x, y));
                }
            }
        }
        v
    }

    fn dims(n: usize) -> Vec<(u32, u32)> {
        vec![(BLOC_LARGEUR, BLOC_HAUTEUR); n]
    }

    #[test]
    fn image_transparente() {
        let t = texture(25, 64, |_, _, _| [0, 0, 0, 0]);
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Transparente));
        // Alpha 1 (WGC appliquant l'alpha du masque) : transparente aussi.
        let t = texture(25, 64, |_, _, _| [1, 1, 1, 1]);
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Transparente));
        // Quelques pixels visibles isolés (< 5 %) : toujours transparente.
        let t = texture(25, 64, |k, x, y| {
            if k == 0 && y == 0 && x < 4 {
                [200, 10, 10, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Transparente));
    }

    #[test]
    fn image_uniforme_meme_sombre() {
        // Cadre pas encore peint : une seule couleur opaque.
        let t = texture(25, 64, |_, _, _| [255, 255, 255, 255]);
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Uniforme));
        // Petites variations sous la tolérance : uniforme.
        let t = texture(25, 64, |k, _, _| [30 + (k % 3) as u8, 30, 30, 255]);
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Uniforme));
        // Fond transparent + une seule couleur visible : uniforme.
        let t = texture(25, 64, |k, _, _| {
            if k % 2 == 0 {
                [40, 40, 40, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Uniforme));
    }

    #[test]
    fn fenetre_sombre_avec_contenu_n_est_pas_vide() {
        // Thème sombre : fond 32/32/32 et un peu de texte gris clair.
        let t = texture(25, 64, |k, x, _| {
            if k == 7 && x == 3 {
                [200, 200, 200, 255]
            } else {
                [32, 32, 32, 255]
            }
        });
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Contenu));
        // Deux zones de teintes différentes (volet / contenu).
        let t = texture(25, 64, |k, _, _| {
            if k % 5 == 0 {
                [43, 43, 43, 255]
            } else {
                [25, 25, 25, 255]
            }
        });
        assert_eq!(classer(&t, 64, &dims(25)), Some(ContenuImage::Contenu));
    }

    #[test]
    fn remplissage_de_ligne_ignore() {
        // Pas de 128 octets : les octets au-delà des 64 utiles (ici des
        // pixels « colorés ») ne comptent pas.
        let mut t = texture(2, 128, |_, _, _| [10, 10, 10, 255]);
        for ligne in 0..(2 * BLOC_HAUTEUR as usize) {
            for i in 64..128 {
                t[ligne * 128 + i] = (i * 7) as u8;
            }
        }
        assert_eq!(classer(&t, 128, &dims(2)), Some(ContenuImage::Uniforme));
    }

    #[test]
    fn donnees_incoherentes_sans_conclusion() {
        assert_eq!(classer(&[], 64, &[]), None);
        assert_eq!(classer(&[0u8; 64 * 4], 32, &dims(1)), None);
        // Tampon trop court.
        assert_eq!(classer(&[0u8; 64 * 3 + 60], 64, &dims(1)), None);
        // Bloc trop grand.
        assert_eq!(classer(&[0u8; 4096], 64, &[(17, 4)]), None);
        assert_eq!(classer(&[0u8; 4096], 64, &[(0, 4)]), None);
        // Dernière ligne sans remplissage : accepté.
        assert_eq!(
            classer(&[0u8; 80 * 3 + 64], 80, &dims(1)),
            Some(ContenuImage::Transparente)
        );
    }

    #[test]
    fn blocs_dans_l_image() {
        for (l, h) in [(1u32, 1u32), (3, 2), (16, 4), (17, 5), (800, 600), (3840, 2160)] {
            let blocs = blocs_echantillons(l, h);
            assert!(!blocs.is_empty(), "{l}x{h}");
            assert!(blocs.len() <= (GRILLE_ECHANTILLONS * GRILLE_ECHANTILLONS) as usize);
            for b in &blocs {
                assert!(b.droite <= l && b.bas <= h, "{l}x{h} : {b:?}");
                assert!(b.largeur() >= 1 && b.hauteur() >= 1);
                assert!(b.largeur() <= BLOC_LARGEUR && b.hauteur() <= BLOC_HAUTEUR);
            }
        }
        assert!(blocs_echantillons(0, 10).is_empty());
        assert!(blocs_echantillons(10, 0).is_empty());
        let grands = blocs_echantillons(800, 600);
        assert_eq!(grands.len(), 25);
        // Premier centre à 10 % / 10 % : (80, 60).
        assert_eq!(
            grands[0],
            Bloc {
                gauche: 72,
                haut: 58,
                droite: 88,
                bas: 62
            }
        );
        // Centre de l'image (indice 12).
        assert_eq!(grands[12].gauche, 392);
        assert_eq!(grands[12].haut, 298);
        assert_eq!(blocs_echantillons(1, 1).len(), 1);
    }

    #[test]
    fn premiere_image_seule_exige_un_silence_plus_long() {
        let a = Attente::default();
        // 1re image reçue à 20 ms, silence 150 ms : pas encore stable.
        assert_eq!(a.decider(obs(170, 1, Some(150)), ModeAttente::Stabilite), Action::Attendre);
        // Silence 250 ms : stable -> analyse demandée.
        assert_eq!(a.decider(obs(270, 1, Some(250)), ModeAttente::Stabilite), Action::Analyser);
        // Deux images : 150 ms suffisent.
        assert_eq!(a.decider(obs(300, 2, Some(150)), ModeAttente::Stabilite), Action::Analyser);
        assert_eq!(a.decider(obs(300, 2, Some(149)), ModeAttente::Stabilite), Action::Attendre);
    }

    #[test]
    fn images_qui_changent_encore_on_attend() {
        let a = Attente::default();
        for (t, n) in [(30u64, 1u64), (130, 2), (230, 3), (330, 4), (430, 5)] {
            assert_eq!(
                a.decider(obs(t + 50, n, Some(50)), ModeAttente::Stabilite),
                Action::Attendre,
                "t={t}"
            );
        }
    }

    #[test]
    fn stable_et_non_vide_on_joue_avec_la_plus_recente() {
        let mut a = Attente::default();
        let o = obs(420, 4, Some(160));
        assert_eq!(a.decider(o, ModeAttente::Stabilite), Action::Analyser);
        // Une analyse d'une image PLUS ANCIENNE ne suffit pas.
        a.noter_analyse(3, ContenuImage::Contenu);
        assert_eq!(a.decider(o, ModeAttente::Stabilite), Action::Analyser);
        a.noter_analyse(4, ContenuImage::Contenu);
        assert_eq!(
            a.decider(o, ModeAttente::Stabilite),
            Action::Jouer(RaisonChoix::Stable)
        );
        assert_eq!(a.numero_analyse(), Some(4));
        assert_eq!(a.analyses, 2);
        // Une nouvelle image arrive : nouvelle attente de stabilité.
        assert_eq!(a.decider(obs(440, 5, Some(5)), ModeAttente::Stabilite), Action::Attendre);
    }

    #[test]
    fn stable_mais_vide_on_attend_jusqu_au_plafond() {
        let mut a = Attente::default();
        a.noter_analyse(1, ContenuImage::Transparente);
        assert_eq!(a.decider(obs(300, 1, Some(280)), ModeAttente::Stabilite), Action::Attendre);
        a.noter_analyse(1, ContenuImage::Uniforme);
        assert_eq!(a.decider(obs(600, 1, Some(580)), ModeAttente::Stabilite), Action::Attendre);
        // Plafond, uniforme mais encore changeante : abandon.
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 1, Some(100)), ModeAttente::Stabilite),
            Action::Abandonner(RaisonAbandon::ImageVide(ContenuImage::Uniforme))
        );
        // Plafond, uniforme (non transparente) et stable : effet joué.
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 1, Some(730)), ModeAttente::Stabilite),
            Action::Jouer(RaisonChoix::PlafondUniforme)
        );
        // Plafond, transparente même stable : abandon.
        a.noter_analyse(1, ContenuImage::Transparente);
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 1, Some(730)), ModeAttente::Stabilite),
            Action::Abandonner(RaisonAbandon::ImageVide(ContenuImage::Transparente))
        );
        // Une image plus récente au plafond est d'abord analysée.
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 2, Some(10)), ModeAttente::Stabilite),
            Action::Analyser
        );
    }

    #[test]
    fn plafond_image_changeante_non_vide_on_joue() {
        let mut a = Attente::default();
        let o = obs(PLAFOND_ATTENTE_MS + 5, 8, Some(40));
        assert_eq!(a.decider(o, ModeAttente::Stabilite), Action::Analyser);
        a.noter_analyse(8, ContenuImage::Contenu);
        assert_eq!(
            a.decider(o, ModeAttente::Stabilite),
            Action::Jouer(RaisonChoix::Plafond)
        );
    }

    #[test]
    fn plafond_uniforme_stable_selon_le_nombre_d_images() {
        let mut a = Attente::default();
        a.noter_analyse(3, ContenuImage::Uniforme);
        // Plusieurs images : 150 ms de silence suffisent.
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 3, Some(DELAI_STABILITE_MS)), ModeAttente::Stabilite),
            Action::Jouer(RaisonChoix::PlafondUniforme)
        );
        assert_eq!(
            a.decider(
                obs(PLAFOND_ATTENTE_MS, 3, Some(DELAI_STABILITE_MS - 1)),
                ModeAttente::Stabilite
            ),
            Action::Abandonner(RaisonAbandon::ImageVide(ContenuImage::Uniforme))
        );
        // Avant le plafond, une image uniforme stable reste en attente.
        assert_eq!(a.decider(obs(500, 3, Some(300)), ModeAttente::Stabilite), Action::Attendre);
        // « Capture d'abord » : uniforme = abandon immédiat (inchangé).
        a.noter_analyse(1, ContenuImage::Uniforme);
        assert_eq!(
            a.decider(obs(40, 1, Some(0)), ModeAttente::PremiereImage),
            Action::Abandonner(RaisonAbandon::ImageVide(ContenuImage::Uniforme))
        );
    }

    #[test]
    fn plafond_sans_image_on_abandonne() {
        let a = Attente::default();
        assert_eq!(a.decider(obs(500, 0, None), ModeAttente::Stabilite), Action::Attendre);
        assert_eq!(
            a.decider(obs(PLAFOND_ATTENTE_MS, 0, None), ModeAttente::Stabilite),
            Action::Abandonner(RaisonAbandon::AucuneImage)
        );
    }

    #[test]
    fn capture_d_abord_decide_des_la_premiere_image() {
        let mut a = Attente::default();
        let m = ModeAttente::PremiereImage;
        assert_eq!(a.decider(obs(50, 0, None), m), Action::Attendre);
        assert_eq!(
            a.decider(obs(PLAFOND_PREMIERE_IMAGE_MS, 0, None), m),
            Action::Abandonner(RaisonAbandon::AucuneImage)
        );
        assert_eq!(a.decider(obs(40, 1, Some(0)), m), Action::Analyser);
        a.noter_analyse(1, ContenuImage::Contenu);
        assert_eq!(
            a.decider(obs(40, 1, Some(0)), m),
            Action::Jouer(RaisonChoix::PremiereImage)
        );
        a.noter_analyse(1, ContenuImage::Transparente);
        assert_eq!(
            a.decider(obs(40, 1, Some(0)), m),
            Action::Abandonner(RaisonAbandon::ImageVide(ContenuImage::Transparente))
        );
    }

    #[test]
    fn delai_borne_par_le_plafond() {
        // Quel que soit le flux d'images, la décision finale tombe au plus
        // tard au plafond (aucun « Attendre » au-delà, hors analyse).
        let mut a = Attente::default();
        for n in 1..20u64 {
            a.noter_analyse(n, ContenuImage::Uniforme);
            let o = obs(PLAFOND_ATTENTE_MS + n, n, Some(n % 7));
            assert!(matches!(a.decider(o, ModeAttente::Stabilite), Action::Abandonner(_)));
            a.noter_analyse(n, ContenuImage::Contenu);
            assert_eq!(
                a.decider(o, ModeAttente::Stabilite),
                Action::Jouer(RaisonChoix::Plafond)
            );
        }
        assert!(PLAFOND_ATTENTE_MS > DELAI_STABILITE_PREMIERE_MS + 300);
        assert!(DELAI_STABILITE_MS > 100, "doit dépasser l'intervalle minimal WGC");
    }
}
