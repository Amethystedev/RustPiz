//! Plans d'animation (module pur, sans Win32).
//!
//! Un [`PlanAnimation`] regroupe les pistes (opacité, échelles, décalage,
//! flou) d'un effet pour une fenêtre de taille donnée. On construit
//! toujours la chronologie de **fermeture**, puis on l'inverse dans le
//! temps pour obtenir l'**ouverture** : les deux sens restent ainsi
//! parfaitement symétriques.

use super::catalogue::{EffetAnimation, Moteur};
use super::courbes::{Courbe, Piste};
use super::effets_shader;

/// Sens de l'animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sens {
    /// La fenêtre apparaît.
    Ouverture,
    /// La fenêtre disparaît.
    Fermeture,
}

/// Taille de la fenêtre animée, en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taille {
    pub largeur: u32,
    pub hauteur: u32,
}

/// Rayon des coins arrondis de Windows 11 (pixels).
const RAYON_COINS: f32 = 8.0;
/// Échelle minimale de l'effet Zoom.
const ECHELLE_ZOOM: f32 = 0.85;
/// Échelle finale de l'effet Rétrécissement.
const ECHELLE_RETRECI: f32 = 0.05;
/// Écrasement vertical de l'effet Télévision (ligne fine).
const ECHELLE_TV_Y: f32 = 0.02;
/// Glissement : fraction de la hauteur, plafonnée.
const GLISSEMENT_FRACTION: f32 = 0.08;
const GLISSEMENT_MAX_PX: f32 = 80.0;
/// Écart-type maximal du flou gaussien.
const FLOU_MAX: f32 = 20.0;

/// Description complète d'une animation, prête à être convertie en
/// `IDCompositionAnimation`s. Les pistes absentes (`None`) gardent leur
/// valeur neutre (opacité 1, échelle 1, décalage 0, flou 0).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanAnimation {
    /// Durée totale (s).
    pub duree: f64,
    /// Marge (px) ajoutée autour de la fenêtre de surimpression, pour que
    /// le contenu déplacé ne soit pas rogné.
    pub marge_px: i32,
    pub opacite: Option<Piste>,
    pub echelle_x: Option<Piste>,
    pub echelle_y: Option<Piste>,
    /// Centre des mises à l'échelle (px, repère du contenu sans marge).
    pub centre: (f32, f32),
    pub decalage_y: Option<Piste>,
    pub flou: Option<Piste>,
    /// Rayon des coins arrondis du découpage (px).
    pub rayon_coins: f32,
}

/// Plan d'un effet à shader (v0.4.7) : la progression `p` (0 = fenêtre
/// intacte, 1 = disparue) est évaluée par le processeur à chaque image et
/// transmise au pixel shader.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanShader {
    pub effet: EffetAnimation,
    /// Durée totale (s).
    pub duree: f64,
    /// Marge (px) autour de la fenêtre de surimpression.
    pub marge_px: i32,
    /// Progression dans le temps (fermeture : 0 -> 1 ; ouverture : la même
    /// piste inversée dans le temps, 1 -> 0).
    pub progression: Piste,
    /// Rayon des coins arrondis (px).
    pub rayon_coins: f32,
}

/// Plan d'animation, selon le moteur de l'effet.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Composition(PlanAnimation),
    Shader(PlanShader),
}

impl Plan {
    /// Durée totale (s).
    pub fn duree(&self) -> f64 {
        match self {
            Plan::Composition(p) => p.duree,
            Plan::Shader(p) => p.duree,
        }
    }

    /// Marge (px) autour de la fenêtre de surimpression.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn marge_px(&self) -> i32 {
        match self {
            Plan::Composition(p) => p.marge_px,
            Plan::Shader(p) => p.marge_px,
        }
    }
}

/// Construit le plan de l'effet, quel que soit son moteur. `None` pour
/// `Aucun` ou pour une taille nulle.
pub fn construire_plan(effet: EffetAnimation, sens: Sens, taille: Taille) -> Option<Plan> {
    match effet.moteur() {
        Moteur::Composition => construire(effet, sens, taille).map(Plan::Composition),
        Moteur::Shader => construire_shader(effet, sens, taille).map(Plan::Shader),
    }
}

/// Plan d'un effet à shader : progression 0 -> 1 sur toute la durée
/// (courbe de l'effet) en fermeture, inversée dans le temps en ouverture.
fn construire_shader(effet: EffetAnimation, sens: Sens, taille: Taille) -> Option<PlanShader> {
    if taille.largeur == 0 || taille.hauteur == 0 {
        return None;
    }
    let parametres = effets_shader::parametres(effet)?;
    let t = effet.duree().as_secs_f64();
    if !(t.is_finite() && t > 0.0) {
        return None;
    }
    let fermeture = Piste::constante(0.0).puis(t, 1.0, parametres.courbe);
    let progression = match sens {
        Sens::Fermeture => fermeture,
        Sens::Ouverture => fermeture.inverser_temps(),
    };
    Some(PlanShader {
        effet,
        duree: t,
        marge_px: effets_shader::marge_px(parametres.marge, taille),
        progression,
        rayon_coins: RAYON_COINS,
    })
}

/// Construit le plan DirectComposition de l'effet pour le sens et la
/// taille donnés. `None` pour `Aucun`, pour une taille nulle ou pour un
/// effet à shader (voir [`construire_plan`]).
pub fn construire(effet: EffetAnimation, sens: Sens, taille: Taille) -> Option<PlanAnimation> {
    if effet == EffetAnimation::Aucun
        || effet.moteur() != Moteur::Composition
        || taille.largeur == 0
        || taille.hauteur == 0
    {
        return None;
    }
    let t = effet.duree().as_secs_f64();
    if t <= 0.0 {
        return None;
    }
    let l = taille.largeur as f32;
    let h = taille.hauteur as f32;
    let milieu = (l / 2.0, h / 2.0);

    let fondu = || Piste::constante(1.0).puis(t, 0.0, Courbe::Lineaire);

    // Chronologie de FERMETURE.
    let mut plan = PlanAnimation {
        duree: t,
        marge_px: 0,
        opacite: None,
        echelle_x: None,
        echelle_y: None,
        centre: milieu,
        decalage_y: None,
        flou: None,
        rayon_coins: RAYON_COINS,
    };
    match effet {
        EffetAnimation::Aucun
        | EffetAnimation::Pixellisation
        | EffetAnimation::BalayagePixels
        | EffetAnimation::Glitch
        | EffetAnimation::RouePixels
        | EffetAnimation::Fonte
        | EffetAnimation::Hexagones
        | EffetAnimation::TelevisionParasitee
        | EffetAnimation::Plane
        | EffetAnimation::Apparition
        | EffetAnimation::EnergieA
        | EffetAnimation::EnergieB
        | EffetAnimation::Feu
        | EffetAnimation::Incineration
        | EffetAnimation::Portail
        | EffetAnimation::VerreBrise
        | EffetAnimation::Desintegration
        | EffetAnimation::Matrix
        | EffetAnimation::FeuxFollets => return None,
        EffetAnimation::Fondu => {
            plan.opacite = Some(fondu());
        }
        EffetAnimation::Zoom => {
            let echelle = Piste::constante(1.0).puis(t, ECHELLE_ZOOM, Courbe::EntreeCubique);
            plan.echelle_x = Some(echelle.clone());
            plan.echelle_y = Some(echelle);
            plan.opacite = Some(fondu());
        }
        EffetAnimation::Glissement => {
            let distance = (h * GLISSEMENT_FRACTION).min(GLISSEMENT_MAX_PX);
            plan.decalage_y =
                Some(Piste::constante(0.0).puis(t, distance, Courbe::EntreeCubique));
            plan.opacite = Some(fondu());
            plan.marge_px = distance.ceil() as i32;
        }
        EffetAnimation::Retrecissement => {
            let echelle = Piste::constante(1.0).puis(t, ECHELLE_RETRECI, Courbe::EntreeCubique);
            plan.echelle_x = Some(echelle.clone());
            plan.echelle_y = Some(echelle);
            plan.centre = (l / 2.0, h);
            plan.opacite = Some(
                Piste::constante(1.0)
                    .puis(t * 0.6, 1.0, Courbe::Constante)
                    .puis(t * 0.4, 0.0, Courbe::Lineaire),
            );
        }
        EffetAnimation::Television => {
            // Écrasement vertical en ligne fine, puis horizontal en point.
            plan.echelle_y = Some(
                Piste::constante(1.0).puis(t * 0.6, ECHELLE_TV_Y, Courbe::EntreeCubique),
            );
            plan.echelle_x = Some(
                Piste::constante(1.0)
                    .puis(t * 0.6, 1.0, Courbe::Constante)
                    .puis(t * 0.4, 0.0, Courbe::EntreeCubique),
            );
            plan.opacite = Some(
                Piste::constante(1.0)
                    .puis(t * 0.8, 1.0, Courbe::Constante)
                    .puis(t * 0.2, 0.0, Courbe::Lineaire),
            );
        }
        EffetAnimation::Flou => {
            plan.flou = Some(Piste::constante(0.0).puis(t, FLOU_MAX, Courbe::SortieCubique));
            plan.opacite = Some(
                Piste::constante(1.0)
                    .puis(t * 0.3, 1.0, Courbe::Constante)
                    .puis(t * 0.7, 0.0, Courbe::Lineaire),
            );
        }
    }

    // Toutes les pistes durent exactement `t` (indispensable avant inversion).
    let ajuster = |p: Option<Piste>| {
        p.map(|p| {
            let p = p.completer(t);
            match sens {
                Sens::Fermeture => p,
                Sens::Ouverture => p.inverser_temps(),
            }
        })
    };
    plan.opacite = ajuster(plan.opacite);
    plan.echelle_x = ajuster(plan.echelle_x);
    plan.echelle_y = ajuster(plan.echelle_y);
    plan.decalage_y = ajuster(plan.decalage_y);
    plan.flou = ajuster(plan.flou);
    Some(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAILLE: Taille = Taille {
        largeur: 1200,
        hauteur: 800,
    };

    /// Effets du moteur Composition (hors `Aucun`).
    fn effets_animes() -> impl Iterator<Item = EffetAnimation> {
        EffetAnimation::TOUS
            .into_iter()
            .filter(|e| *e != EffetAnimation::Aucun && e.moteur() == Moteur::Composition)
    }

    fn effets_shader() -> impl Iterator<Item = EffetAnimation> {
        EffetAnimation::TOUS
            .into_iter()
            .filter(|e| e.moteur() == Moteur::Shader)
    }

    fn plan_shader(effet: EffetAnimation, sens: Sens) -> PlanShader {
        match construire_plan(effet, sens, TAILLE) {
            Some(Plan::Shader(p)) => p,
            autre => panic!("{effet:?} : plan shader attendu, obtenu {autre:?}"),
        }
    }

    #[test]
    fn construire_none_pour_shader() {
        for e in effets_shader() {
            for sens in [Sens::Ouverture, Sens::Fermeture] {
                assert!(construire(e, sens, TAILLE).is_none(), "{e:?}");
                assert!(matches!(construire_plan(e, sens, TAILLE), Some(Plan::Shader(_))));
            }
        }
        for e in effets_animes() {
            assert!(matches!(
                construire_plan(e, Sens::Fermeture, TAILLE),
                Some(Plan::Composition(_))
            ));
        }
        assert!(construire_plan(EffetAnimation::Aucun, Sens::Fermeture, TAILLE).is_none());
        let nulle = Taille {
            largeur: 10,
            hauteur: 0,
        };
        assert!(construire_plan(EffetAnimation::Glitch, Sens::Ouverture, nulle).is_none());
    }

    #[test]
    fn progression_fermeture_0_vers_1() {
        for e in effets_shader() {
            let p = plan_shader(e, Sens::Fermeture);
            assert!(p.progression.valeur_a(0.0).abs() < 1e-6, "{e:?}");
            assert!((p.progression.valeur_a(p.duree) - 1.0).abs() < 1e-6, "{e:?}");
            assert!((p.progression.valeur_a(p.duree * 10.0) - 1.0).abs() < 1e-6);
            // Monotone croissante.
            let mut avant = -1.0f32;
            for k in 0..=50 {
                let v = p.progression.valeur_a(p.duree * k as f64 / 50.0);
                assert!(v >= avant - 1e-6 && (0.0..=1.0).contains(&v), "{e:?}");
                avant = v;
            }
        }
    }

    #[test]
    fn progression_ouverture_1_vers_0() {
        for e in effets_shader() {
            let p = plan_shader(e, Sens::Ouverture);
            assert!((p.progression.valeur_a(0.0) - 1.0).abs() < 1e-6, "{e:?}");
            assert!(p.progression.valeur_a(p.duree).abs() < 1e-6, "{e:?}");
            assert!(p.progression.valeur_a(p.duree + 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn ouverture_est_fermeture_inversee() {
        for e in effets_shader() {
            let f = plan_shader(e, Sens::Fermeture);
            let o = plan_shader(e, Sens::Ouverture);
            assert!((f.duree - o.duree).abs() < 1e-12);
            for k in 0..20 {
                let t = f.duree * k as f64 / 19.0;
                let a = o.progression.valeur_a(t);
                let b = f.progression.valeur_a(f.duree - t);
                assert!((a - b).abs() < 1e-4, "{e:?} t={t} : {a} vs {b}");
            }
        }
    }

    #[test]
    fn plan_shader_duree_egale_catalogue() {
        for e in effets_shader() {
            for sens in [Sens::Ouverture, Sens::Fermeture] {
                let p = plan_shader(e, sens);
                assert!((p.duree - e.duree().as_secs_f64()).abs() < 1e-9);
                assert!((p.progression.duree_totale() - p.duree).abs() < 1e-9);
                let plan = Plan::Shader(p.clone());
                assert!((plan.duree() - p.duree).abs() < 1e-12);
                assert_eq!(plan.marge_px(), p.marge_px);
                assert!((p.rayon_coins - 8.0).abs() < f32::EPSILON);
                assert_eq!(p.effet, e);
            }
        }
    }

    #[test]
    fn marge_bornee() {
        for e in effets_shader() {
            for (l, h) in [(1u32, 1u32), (300, 100), (1200, 800), (16000, 9000)] {
                let t = Taille {
                    largeur: l,
                    hauteur: h,
                };
                let p = match construire_plan(e, Sens::Fermeture, t) {
                    Some(Plan::Shader(p)) => p,
                    _ => panic!("plan"),
                };
                let max = match effets_shader::parametres(e).expect("shader").marge {
                    effets_shader::Marge::Aucune => 0,
                    effets_shader::Marge::Fraction { max_px, .. } => max_px,
                };
                assert!((0..=max).contains(&p.marge_px), "{e:?} {l}x{h} : {}", p.marge_px);
            }
        }
        assert_eq!(plan_shader(EffetAnimation::Pixellisation, Sens::Fermeture).marge_px, 0);
        assert_eq!(plan_shader(EffetAnimation::Glitch, Sens::Fermeture).marge_px, 24);
    }

    fn fin(p: &Option<Piste>, neutre: f32) -> f32 {
        p.as_ref().map(|p| p.valeur_finale()).unwrap_or(neutre)
    }

    fn debut(p: &Option<Piste>, neutre: f32) -> f32 {
        p.as_ref().map(|p| p.valeur_initiale()).unwrap_or(neutre)
    }

    #[test]
    fn aucun_et_taille_nulle() {
        assert!(construire(EffetAnimation::Aucun, Sens::Fermeture, TAILLE).is_none());
        let nulle = Taille {
            largeur: 0,
            hauteur: 10,
        };
        assert!(construire(EffetAnimation::Zoom, Sens::Ouverture, nulle).is_none());
    }

    #[test]
    fn ouverture_finit_a_l_etat_normal() {
        for effet in effets_animes() {
            let p = construire(effet, Sens::Ouverture, TAILLE).expect("plan");
            assert!((fin(&p.opacite, 1.0) - 1.0).abs() < 1e-4, "{effet:?} opacité");
            assert!((fin(&p.echelle_x, 1.0) - 1.0).abs() < 1e-4, "{effet:?} échelle X");
            assert!((fin(&p.echelle_y, 1.0) - 1.0).abs() < 1e-4, "{effet:?} échelle Y");
            assert!(fin(&p.decalage_y, 0.0).abs() < 1e-4, "{effet:?} décalage");
            assert!(fin(&p.flou, 0.0).abs() < 1e-4, "{effet:?} flou");
        }
    }

    #[test]
    fn fermeture_finit_invisible() {
        for effet in effets_animes() {
            let p = construire(effet, Sens::Fermeture, TAILLE).expect("plan");
            let invisible = fin(&p.opacite, 1.0) < 1e-4
                || fin(&p.echelle_x, 1.0) < 1e-4
                || fin(&p.echelle_y, 1.0) < 1e-4;
            assert!(invisible, "{effet:?}");
            // Et commence à l'état normal.
            assert!((debut(&p.opacite, 1.0) - 1.0).abs() < 1e-4, "{effet:?}");
            assert!((debut(&p.echelle_x, 1.0) - 1.0).abs() < 1e-4, "{effet:?}");
        }
    }

    #[test]
    fn segments_sans_chevauchement_strict() {
        for effet in effets_animes() {
            for sens in [Sens::Ouverture, Sens::Fermeture] {
                let p = construire(effet, sens, TAILLE).expect("plan");
                for piste in [&p.opacite, &p.echelle_x, &p.echelle_y, &p.decalage_y, &p.flou]
                    .into_iter()
                    .flatten()
                {
                    for w in piste.segments().windows(2) {
                        // Comparaison stricte, sans tolérance (AddCubic).
                        assert!(
                            w[1].debut >= w[0].debut + w[0].duree,
                            "{effet:?} {sens:?} : chevauchement"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn duree_plan_egale_duree_effet() {
        for effet in effets_animes() {
            for sens in [Sens::Ouverture, Sens::Fermeture] {
                let p = construire(effet, sens, TAILLE).expect("plan");
                let attendu = effet.duree().as_secs_f64();
                assert!((p.duree - attendu).abs() < 1e-9);
                for piste in [&p.opacite, &p.echelle_x, &p.echelle_y, &p.decalage_y, &p.flou]
                    .into_iter()
                    .flatten()
                {
                    assert!((piste.duree_totale() - attendu).abs() < 1e-9, "{effet:?}");
                }
            }
        }
    }

    #[test]
    fn television_ecrase_y_avant_x() {
        let p = construire(EffetAnimation::Television, Sens::Fermeture, TAILLE).expect("plan");
        let ey = p.echelle_y.as_ref().expect("échelle Y");
        let ex = p.echelle_x.as_ref().expect("échelle X");
        let t = p.duree * 0.6;
        assert!(ey.valeur_a(t) < 0.05);
        assert!((ex.valeur_a(t) - 1.0).abs() < 1e-4);
        assert!(ex.valeur_a(p.duree) < 1e-4);
        // Ouverture : l'inverse (X s'étire d'abord, puis Y).
        let o = construire(EffetAnimation::Television, Sens::Ouverture, TAILLE).expect("plan");
        let t = o.duree * 0.4;
        assert!((o.echelle_x.as_ref().expect("X").valeur_a(t) - 1.0).abs() < 1e-4);
        assert!(o.echelle_y.as_ref().expect("Y").valeur_a(t) < 0.05);
    }

    #[test]
    fn glissement_plafonne_et_marge() {
        let p = construire(EffetAnimation::Glissement, Sens::Fermeture, TAILLE).expect("plan");
        assert!((fin(&p.decalage_y, 0.0) - 64.0).abs() < 1e-3); // 8 % de 800
        assert_eq!(p.marge_px, 64);
        let grande = Taille {
            largeur: 3000,
            hauteur: 2000,
        };
        let p = construire(EffetAnimation::Glissement, Sens::Fermeture, grande).expect("plan");
        assert!((fin(&p.decalage_y, 0.0) - 80.0).abs() < 1e-3);
    }

    #[test]
    fn retrecissement_centre_bas_milieu() {
        let p = construire(EffetAnimation::Retrecissement, Sens::Fermeture, TAILLE).expect("plan");
        assert_eq!(p.centre, (600.0, 800.0));
        assert!((p.rayon_coins - 8.0).abs() < f32::EPSILON);
    }
}
