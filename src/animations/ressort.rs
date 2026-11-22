//! Simulation physique de l'effet « Jelly » (module pur, sans `windows`).
//!
//! La fenêtre est découpée en une grille de tuiles ; chaque sommet de la
//! grille est une masse (masse 1) reliée par un ressort amorti à sa
//! position « rigide » (position de repos + translation de la fenêtre) :
//!
//! ```text
//! p̈ = −k_i (p − T_i) − c_i ṗ        c_i = 2 ζ √k_i
//! k_i = k_loin + (k_proche − k_loin) (1 − r_i / r_max)²
//! ```
//!
//! où `r_i` est la distance (au repos) entre le sommet et le point saisi
//! par la souris. Le sommet le plus proche de la prise est épinglé
//! (`p = T`) : la fenêtre « suit » la souris à cet endroit et traîne
//! ailleurs, puis rebondit de façon amortie à l'arrêt.
//!
//! Intégration : Euler semi-implicite, sous-pas ≤ 4 ms, pas total borné à
//! 50 ms ; la cible rigide est interpolée linéairement sur les sous-pas.
//! Le déplacement de chaque sommet par rapport à sa position rigide est
//! borné à `deplacement_max_px` (la surimpression a une marge finie).

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};
use std::time::Duration;

/// Raideur des sommets proches de la prise.
const RAIDEUR_PROCHE: f32 = 2500.0;
/// Raideur des sommets les plus éloignés : de 1200 (puissance faible) à 90
/// (puissance maximale).
const RAIDEUR_LOIN_MIN_PUISSANCE: f32 = 1200.0;
const RAIDEUR_LOIN_MAX_PUISSANCE: f32 = 90.0;
/// Taux d'amortissement ζ : de 0,90 (puissance faible) à 0,30 (maximale).
const AMORTISSEMENT_MIN_PUISSANCE: f32 = 0.90;
const AMORTISSEMENT_MAX_PUISSANCE: f32 = 0.30;
/// Déplacement maximal : fraction du plus petit côté, plafonnée.
const FRACTION_DEPLACEMENT_MAX: f32 = 0.30;
const PLAFOND_DEPLACEMENT_PX: f32 = 240.0;

/// Taille visée d'une tuile (pixels) et bornes du nombre de tuiles par axe.
const TAILLE_TUILE_PX: f32 = 128.0;
const TUILES_MIN: u32 = 3;
const TUILES_MAX: u32 = 10;

/// Sous-pas d'intégration maximal et pas total maximal (secondes).
const SOUS_PAS_MAX_S: f32 = 0.004;
const PAS_MAX_S: f32 = 0.050;

/// Seuils du repos : déplacement (px) et vitesse relative (px/s).
const REPOS_DEPLACEMENT_PX: f32 = 0.5;
const REPOS_VITESSE_PX_S: f32 = 10.0;

/// Puissance de l'effet, en % (0 à 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Puissance(u8);

impl Puissance {
    /// Puissance bornée à 100 %.
    pub fn new(valeur: u8) -> Self {
        Puissance(valeur.min(100))
    }

    pub fn valeur(self) -> u8 {
        self.0
    }

    /// Puissance nulle : aucun mouvement (l'effet n'est pas joué).
    pub fn est_nulle(self) -> bool {
        self.0 == 0
    }

    /// Fraction `s = valeur / 100` (0,0 à 1,0).
    pub fn fraction(self) -> f32 {
        self.0 as f32 / 100.0
    }
}

/// Vecteur 2D (pixels ou pixels/s).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }

    pub fn norme(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn scalaire(self, autre: Vec2) -> f32 {
        self.x * autre.x + self.y * autre.y
    }

    pub fn est_fini(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a + (b - a) * s
}

/// Paramètres physiques dérivés de la puissance et de la taille.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParametresRessort {
    pub raideur_proche: f32,
    pub raideur_loin: f32,
    /// Taux d'amortissement ζ (sans unité).
    pub amortissement: f32,
    pub deplacement_max_px: f32,
}

/// Paramètres pour une puissance et une fenêtre `largeur` × `hauteur`
/// (pixels). `None` si la puissance est nulle ou la taille nulle.
pub fn parametres(puissance: Puissance, largeur: u32, hauteur: u32) -> Option<ParametresRessort> {
    if puissance.est_nulle() || largeur == 0 || hauteur == 0 {
        return None;
    }
    let s = puissance.fraction();
    let cote = largeur.min(hauteur) as f32;
    Some(ParametresRessort {
        raideur_proche: RAIDEUR_PROCHE,
        raideur_loin: lerp(RAIDEUR_LOIN_MIN_PUISSANCE, RAIDEUR_LOIN_MAX_PUISSANCE, s),
        amortissement: lerp(AMORTISSEMENT_MIN_PUISSANCE, AMORTISSEMENT_MAX_PUISSANCE, s),
        deplacement_max_px: (FRACTION_DEPLACEMENT_MAX * s * cote).min(PLAFOND_DEPLACEMENT_PX),
    })
}

/// Grille de tuiles couvrant la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grille {
    pub tuiles_x: u32,
    pub tuiles_y: u32,
    pub largeur: u32,
    pub hauteur: u32,
}

impl Grille {
    /// Grille pour une fenêtre `largeur` × `hauteur` : tuiles d'environ
    /// 128 px, de 3 à 10 par axe. `None` si la taille est nulle.
    pub fn pour(largeur: u32, hauteur: u32) -> Option<Grille> {
        if largeur == 0 || hauteur == 0 {
            return None;
        }
        let nombre = |cote: u32| {
            ((cote as f32 / TAILLE_TUILE_PX).round() as u32).clamp(TUILES_MIN, TUILES_MAX)
        };
        Some(Grille {
            tuiles_x: nombre(largeur),
            tuiles_y: nombre(hauteur),
            largeur,
            hauteur,
        })
    }

    /// Nombre de sommets.
    pub fn noeuds(&self) -> usize {
        ((self.tuiles_x + 1) * (self.tuiles_y + 1)) as usize
    }

    /// Index du sommet (colonne `ix`, ligne `iy`) — utilisé par les tests
    /// (l'ordre ligne par ligne est celui de `position_repos`).
    #[cfg(test)]
    pub fn index(&self, ix: u32, iy: u32) -> usize {
        (iy * (self.tuiles_x + 1) + ix) as usize
    }

    /// Position de repos du sommet `i` (coordonnées de l'image).
    pub fn position_repos(&self, i: usize) -> Vec2 {
        let colonnes = (self.tuiles_x + 1) as usize;
        let ix = (i % colonnes) as f32;
        let iy = (i / colonnes) as f32;
        Vec2::new(
            self.largeur as f32 * ix / self.tuiles_x as f32,
            self.hauteur as f32 * iy / self.tuiles_y as f32,
        )
    }
}

/// Simulation masses-ressorts de la grille.
#[derive(Debug, Clone)]
pub struct Simulation {
    params: ParametresRessort,
    grille: Grille,
    repos: Vec<Vec2>,
    /// Positions absolues (repère de la position initiale de la fenêtre).
    positions: Vec<Vec2>,
    vitesses: Vec<Vec2>,
    raideurs: Vec<f32>,
    amortissements: Vec<f32>,
    epingle: usize,
    /// Translation rigide cumulée de la fenêtre.
    cible: Vec2,
    /// Vitesse de la cible au dernier sous-pas.
    vitesse_cible: Vec2,
    cible_immobile: bool,
    amplitude_max: f32,
}

impl Simulation {
    /// Nouvelle simulation au repos ; `prise` = point saisi (coordonnées
    /// de l'image).
    pub fn nouvelle(params: ParametresRessort, grille: Grille, prise: Vec2) -> Simulation {
        let n = grille.noeuds();
        let repos: Vec<Vec2> = (0..n).map(|i| grille.position_repos(i)).collect();
        let mut sim = Simulation {
            params,
            grille,
            positions: repos.clone(),
            repos,
            vitesses: vec![Vec2::ZERO; n],
            raideurs: vec![params.raideur_loin; n],
            amortissements: vec![0.0; n],
            epingle: 0,
            cible: Vec2::ZERO,
            vitesse_cible: Vec2::ZERO,
            cible_immobile: true,
            amplitude_max: 0.0,
        };
        sim.configurer_prise(prise);
        sim
    }

    /// Grille simulée.
    pub fn grille(&self) -> &Grille {
        &self.grille
    }

    /// Calcule raideurs, amortissements et sommet épinglé pour une prise.
    fn configurer_prise(&mut self, prise: Vec2) {
        let prise = if prise.est_fini() {
            Vec2::new(
                prise.x.clamp(0.0, self.grille.largeur as f32),
                prise.y.clamp(0.0, self.grille.hauteur as f32),
            )
        } else {
            Vec2::new(self.grille.largeur as f32 / 2.0, 0.0)
        };
        let distances: Vec<f32> = self.repos.iter().map(|r| (*r - prise).norme()).collect();
        let r_max = distances.iter().cloned().fold(0.0f32, f32::max).max(1.0);
        let p = self.params;
        let mut plus_proche = 0;
        for (i, r) in distances.iter().enumerate() {
            let t = (1.0 - r / r_max).clamp(0.0, 1.0);
            let k = p.raideur_loin + (p.raideur_proche - p.raideur_loin) * t * t;
            self.raideurs[i] = k;
            self.amortissements[i] = 2.0 * p.amortissement * k.max(0.0).sqrt();
            if *r < distances[plus_proche] {
                plus_proche = i;
            }
        }
        self.epingle = plus_proche;
    }

    /// Change le point saisi (nouvelle prise pendant le rebond) ; les
    /// positions et vitesses courantes sont conservées.
    pub fn changer_prise(&mut self, prise: Vec2) {
        self.configurer_prise(prise);
    }

    /// Avance la simulation : la fenêtre s'est déplacée de `delta` pixels
    /// pendant `dt`. Un `delta` non fini est ignoré ; `dt` est borné à
    /// 50 ms.
    pub fn avancer(&mut self, delta: Vec2, dt: Duration) {
        let delta = if delta.est_fini() { delta } else { Vec2::ZERO };
        let dt = dt.as_secs_f32().min(PAS_MAX_S);
        let depart = self.cible;
        if dt <= 1e-6 {
            // Pas de temps : saut instantané de la cible.
            self.cible = depart + delta;
            self.vitesse_cible = Vec2::ZERO;
            self.epingler();
            self.contraindre();
        } else {
            let n = ((dt / SOUS_PAS_MAX_S).ceil() as usize).max(1);
            let h = dt / n as f32;
            let vitesse_cible = delta * (1.0 / dt);
            for s in 1..=n {
                let cible = depart + delta * (s as f32 / n as f32);
                for i in 0..self.positions.len() {
                    if i == self.epingle {
                        continue;
                    }
                    let t = self.repos[i] + cible;
                    let acceleration = (t - self.positions[i]) * self.raideurs[i]
                        - self.vitesses[i] * self.amortissements[i];
                    self.vitesses[i] += acceleration * h;
                    self.positions[i] += self.vitesses[i] * h;
                }
                self.cible = cible;
                self.vitesse_cible = vitesse_cible;
                self.epingler();
                self.contraindre();
            }
        }
        self.cible_immobile = delta.norme() <= 1e-6;
        let a = self.amplitude();
        if a > self.amplitude_max {
            self.amplitude_max = a;
        }
    }

    /// Le sommet épinglé suit exactement la cible.
    fn epingler(&mut self) {
        let e = self.epingle;
        if e < self.positions.len() {
            self.positions[e] = self.repos[e] + self.cible;
            self.vitesses[e] = self.vitesse_cible;
        }
    }

    /// Borne `|p − T|` à `deplacement_max_px` ; la composante radiale
    /// sortante de la vitesse relative est annulée.
    fn contraindre(&mut self) {
        let max = self.params.deplacement_max_px.max(0.0);
        for i in 0..self.positions.len() {
            let t = self.repos[i] + self.cible;
            let d = self.positions[i] - t;
            let n = d.norme();
            if !n.is_finite() {
                // Filet de sécurité numérique : retour au repos.
                self.positions[i] = t;
                self.vitesses[i] = self.vitesse_cible;
                continue;
            }
            if n > max && n > 0.0 {
                let u = d * (1.0 / n);
                self.positions[i] = t + u * max;
                let relative = self.vitesses[i] - self.vitesse_cible;
                let radiale = relative.scalaire(u);
                if radiale > 0.0 {
                    self.vitesses[i] -= u * radiale;
                }
            }
        }
    }

    /// Déplacements des sommets par rapport à leur position rigide.
    pub fn deplacements(&self) -> Vec<Vec2> {
        self.positions
            .iter()
            .zip(self.repos.iter())
            .map(|(p, r)| *p - (*r + self.cible))
            .collect()
    }

    /// Plus grand déplacement courant (px).
    pub fn amplitude(&self) -> f32 {
        self.deplacements()
            .iter()
            .map(|d| d.norme())
            .fold(0.0f32, f32::max)
    }

    /// Plus grand déplacement observé depuis le début (px).
    pub fn amplitude_max_vue(&self) -> f32 {
        self.amplitude_max
    }

    /// Vrai si la grille est revenue au repos : déplacements < 0,5 px,
    /// vitesses relatives < 10 px/s, cible immobile au dernier pas.
    pub fn au_repos(&self) -> bool {
        if !self.cible_immobile {
            return false;
        }
        self.positions.iter().enumerate().all(|(i, p)| {
            let d = *p - (self.repos[i] + self.cible);
            let v = self.vitesses[i] - self.vitesse_cible;
            d.norme() < REPOS_DEPLACEMENT_PX && v.norme() < REPOS_VITESSE_PX_S
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAS: Duration = Duration::from_millis(16);

    fn simulation(puissance: u8, l: u32, h: u32, prise: Vec2) -> Simulation {
        let p = parametres(Puissance::new(puissance), l, h).expect("paramètres");
        let g = Grille::pour(l, h).expect("grille");
        Simulation::nouvelle(p, g, prise)
    }

    fn glisser(sim: &mut Simulation, vitesse_px_s: f32, pas: usize) {
        let delta = Vec2::new(vitesse_px_s * PAS.as_secs_f32(), 0.0);
        for _ in 0..pas {
            sim.avancer(delta, PAS);
        }
    }

    #[test]
    fn puissance_nulle_aucun_parametre() {
        assert!(parametres(Puissance::new(0), 800, 600).is_none());
        assert!(parametres(Puissance::new(50), 0, 600).is_none());
        assert!(parametres(Puissance::new(50), 800, 0).is_none());
        assert!(Puissance::new(0).est_nulle());
    }

    #[test]
    fn puissance_bornee_et_continue() {
        assert_eq!(Puissance::new(150).valeur(), 100);
        // Toute valeur 0-100 est honorée telle quelle (curseur au pas de 1).
        assert_eq!(Puissance::new(63).valeur(), 63);
        assert!((Puissance::new(25).fraction() - 0.25).abs() < 1e-6);
        assert!((Puissance::new(37).fraction() - 0.37).abs() < 1e-6);
        assert!(!Puissance::new(1).est_nulle());
        assert!(Puissance::new(0).est_nulle());
    }

    #[test]
    fn parametres_monotones() {
        let mut precedent: Option<ParametresRessort> = None;
        for v in [25u8, 50, 75, 100] {
            let p = parametres(Puissance::new(v), 1000, 700).expect("paramètres");
            if let Some(a) = precedent {
                assert!(p.raideur_loin < a.raideur_loin, "raideur décroissante");
                assert!(
                    p.amortissement < a.amortissement,
                    "amortissement décroissant"
                );
                assert!(
                    p.deplacement_max_px > a.deplacement_max_px,
                    "déplacement croissant"
                );
                assert_eq!(p.raideur_proche, a.raideur_proche);
            }
            assert!(p.raideur_loin > 0.0 && p.amortissement > 0.0);
            precedent = Some(p);
        }
    }

    #[test]
    fn deplacement_max_plafonne() {
        let p = parametres(Puissance::new(100), 16384, 16384).expect("paramètres");
        assert!(p.deplacement_max_px <= 240.0);
        assert!((p.deplacement_max_px - 240.0).abs() < 1e-3);
    }

    #[test]
    fn grille_bornee() {
        let g = Grille::pour(100, 100).expect("grille");
        assert_eq!((g.tuiles_x, g.tuiles_y), (3, 3));
        let g = Grille::pour(16384, 16384).expect("grille");
        assert_eq!((g.tuiles_x, g.tuiles_y), (10, 10));
        let g = Grille::pour(800, 600).expect("grille");
        assert!((3..=10).contains(&g.tuiles_x) && (3..=10).contains(&g.tuiles_y));
        assert_eq!(g.noeuds(), ((g.tuiles_x + 1) * (g.tuiles_y + 1)) as usize);
        assert!(Grille::pour(0, 100).is_none());
        assert!(Grille::pour(100, 0).is_none());
        // Sommets extrêmes aux coins de l'image.
        let dernier = g.position_repos(g.noeuds() - 1);
        assert_eq!(dernier, Vec2::new(800.0, 600.0));
        assert_eq!(g.position_repos(g.index(0, 0)), Vec2::ZERO);
    }

    #[test]
    fn immobile_reste_au_repos() {
        let mut sim = simulation(100, 800, 600, Vec2::new(400.0, 15.0));
        for _ in 0..100 {
            sim.avancer(Vec2::ZERO, PAS);
        }
        assert!(sim.au_repos());
        assert!(sim.amplitude() < 1e-6);
    }

    #[test]
    fn noeud_saisi_epingle() {
        let mut sim = simulation(100, 800, 600, Vec2::new(400.0, 15.0));
        glisser(&mut sim, 2000.0, 30);
        let g = *sim.grille();
        let e = g.index(g.tuiles_x / 2, 0);
        let d = sim.deplacements();
        assert!(d[e].norme() < 1e-3, "sommet saisi : {:?}", d[e]);
        assert!(sim.amplitude() > 1.0);
    }

    #[test]
    fn glisser_constant_produit_une_trainee() {
        let mut sim = simulation(50, 800, 600, Vec2::new(400.0, 15.0));
        glisser(&mut sim, 1000.0, 60);
        let g = *sim.grille();
        let d = sim.deplacements();
        let colonne = g.tuiles_x / 2;
        let mut precedent = 0.0f32;
        for iy in 1..=g.tuiles_y {
            let v = d[g.index(colonne, iy)];
            assert!(
                v.x < 0.0,
                "le sommet doit traîner derrière (iy={iy}, d={v:?})"
            );
            assert!(
                v.norme() > precedent,
                "|d| croît avec la distance (iy={iy})"
            );
            precedent = v.norme();
        }
        // Coins inférieurs aussi en retard.
        assert!(d[g.index(0, g.tuiles_y)].x < 0.0);
        assert!(d[g.index(g.tuiles_x, g.tuiles_y)].x < 0.0);
    }

    fn rebond(puissance: u8) -> (bool, f32) {
        let mut sim = simulation(puissance, 800, 600, Vec2::new(400.0, 15.0));
        glisser(&mut sim, 600.0, 40);
        let g = *sim.grille();
        let coin = g.index(g.tuiles_x, g.tuiles_y);
        let initial = sim.deplacements()[coin].x;
        assert!(initial < 0.0);
        let mut change_de_signe = false;
        let mut t = 0.0f32;
        while t < 5.0 {
            sim.avancer(Vec2::ZERO, PAS);
            t += PAS.as_secs_f32();
            if sim.deplacements()[coin].x > 0.05 {
                change_de_signe = true;
            }
            if sim.au_repos() {
                return (change_de_signe, t);
            }
        }
        (change_de_signe, f32::INFINITY)
    }

    #[test]
    fn arret_rebond_amorti_100() {
        let (signe, t) = rebond(100);
        assert!(signe, "rebond attendu à 100 %");
        assert!(t < 2.0, "repos en {t} s");
    }

    #[test]
    fn arret_rebond_amorti_25() {
        let (_, t) = rebond(25);
        assert!(t < 1.2, "repos en {t} s");
    }

    #[test]
    fn amplitude_bornee_apres_saut() {
        let mut sim = simulation(100, 800, 600, Vec2::new(400.0, 15.0));
        let max = parametres(Puissance::new(100), 800, 600)
            .expect("paramètres")
            .deplacement_max_px;
        sim.avancer(Vec2::new(5000.0, 0.0), PAS);
        assert!(sim.amplitude() <= max + 1e-3);
        for _ in 0..200 {
            sim.avancer(Vec2::ZERO, PAS);
            assert!(sim.amplitude() <= max + 1e-3);
        }
        assert!(sim.amplitude_max_vue() <= max + 1e-3);
        assert!(sim.amplitude_max_vue() > 0.0);
    }

    #[test]
    fn dt_extremes_sans_nan() {
        let mut sim = simulation(100, 800, 600, Vec2::new(10.0, 10.0));
        sim.avancer(Vec2::new(50.0, -20.0), Duration::ZERO);
        sim.avancer(Vec2::new(50.0, -20.0), Duration::from_secs(3600));
        sim.avancer(Vec2::new(f32::NAN, 1.0), PAS);
        sim.avancer(Vec2::new(f32::INFINITY, 1.0), PAS);
        sim.avancer(Vec2::new(1e30, -1e30), Duration::from_nanos(1));
        for d in sim.deplacements() {
            assert!(d.est_fini(), "déplacement non fini : {d:?}");
        }
        assert!(sim.amplitude().is_finite());
    }

    #[test]
    fn changer_prise_continu() {
        let mut sim = simulation(75, 800, 600, Vec2::new(400.0, 15.0));
        glisser(&mut sim, 800.0, 20);
        let avant = sim.deplacements();
        let max = parametres(Puissance::new(75), 800, 600)
            .expect("paramètres")
            .deplacement_max_px;
        sim.changer_prise(Vec2::new(50.0, 550.0));
        assert_eq!(
            sim.deplacements(),
            avant,
            "aucun saut au changement de prise"
        );
        sim.avancer(Vec2::new(2.0, 0.0), PAS);
        for (a, b) in avant.iter().zip(sim.deplacements().iter()) {
            assert!(b.est_fini());
            assert!((*b - *a).norme() <= max + 1e-3);
        }
        // Le nouveau sommet saisi est épinglé.
        let g = *sim.grille();
        assert!(sim.deplacements()[g.index(0, g.tuiles_y)].norme() < 1e-3);
    }
}
