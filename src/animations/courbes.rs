//! Courbes d'animation (module pur, sans Win32).
//!
//! Une [`Piste`] décrit l'évolution d'une valeur scalaire (opacité, échelle,
//! décalage, flou...) au cours du temps, sous forme d'une suite de
//! [`Segment`]s contigus. Chaque segment est un polynôme cubique en temps
//! local, exactement ce qu'attend `IDCompositionAnimation::AddCubic` :
//! le compositeur DWM évalue lui-même la courbe à chaque image, aucun
//! calcul n'est fait par le processeur pendant l'animation.
//!
//! Les temps sont exprimés en **secondes** (unité de DirectComposition).

/// Forme d'un segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Courbe {
    /// Valeur constante. Convention : la valeur saute à `vers` dès le début
    /// du segment puis y reste (`de == vers` après construction).
    Constante,
    /// Interpolation linéaire.
    Lineaire,
    /// Accélération (« ease-in ») : f(u) = u³.
    EntreeCubique,
    /// Décélération (« ease-out ») : f(u) = 1 − (1 − u)³.
    SortieCubique,
}

impl Courbe {
    /// Courbe obtenue quand on parcourt le segment à rebours.
    fn inverse(self) -> Courbe {
        match self {
            Courbe::EntreeCubique => Courbe::SortieCubique,
            Courbe::SortieCubique => Courbe::EntreeCubique,
            autre => autre,
        }
    }

    /// Progression normalisée (0 → 1) pour un avancement `u` dans [0, 1].
    // Évaluation CPU : sert aux tests (le GPU évalue lui-même les courbes).
    #[cfg_attr(not(test), allow(dead_code))]
    fn progression(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Courbe::Constante => 1.0,
            Courbe::Lineaire => u,
            Courbe::EntreeCubique => u * u * u,
            Courbe::SortieCubique => {
                let v = 1.0 - u;
                1.0 - v * v * v
            }
        }
    }
}

/// Segment cubique d'une piste.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Instant de début (s), relatif au début de l'animation.
    pub debut: f64,
    /// Durée (s), strictement positive.
    pub duree: f64,
    /// Valeur au début du segment.
    pub de: f32,
    /// Valeur à la fin du segment.
    pub vers: f32,
    /// Forme de l'interpolation.
    pub courbe: Courbe,
}

impl Segment {
    /// Valeur du segment au temps local `t` (s), borné à [0, durée].
    // Évaluation CPU : sert aux tests (le GPU évalue lui-même les courbes).
    #[cfg_attr(not(test), allow(dead_code))]
    fn valeur_locale(&self, t: f64) -> f32 {
        let u = if self.duree > 0.0 { t / self.duree } else { 1.0 };
        let p = self.courbe.progression(u);
        (self.de as f64 + (self.vers as f64 - self.de as f64) * p) as f32
    }
}

/// Coefficients `[c0, c1, c2, c3]` du polynôme v(t) = c0 + c1·t + c2·t² + c3·t³
/// (t = temps local en secondes), tels qu'attendus par `AddCubic`.
pub fn coefficients(s: &Segment) -> [f32; 4] {
    let de = s.de as f64;
    let delta = s.vers as f64 - de;
    let t = if s.duree > 0.0 { s.duree } else { 1.0 };
    let c = match s.courbe {
        Courbe::Constante => [de, 0.0, 0.0, 0.0],
        Courbe::Lineaire => [de, delta / t, 0.0, 0.0],
        Courbe::EntreeCubique => [de, 0.0, 0.0, delta / (t * t * t)],
        Courbe::SortieCubique => [
            de,
            3.0 * delta / t,
            -3.0 * delta / (t * t),
            delta / (t * t * t),
        ],
    };
    [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]
}

/// Évolution d'une valeur dans le temps : segments triés, contigus,
/// de durées strictement positives.
#[derive(Debug, Clone, PartialEq)]
pub struct Piste {
    /// Valeur avant le premier segment (et valeur d'une piste vide).
    initiale: f32,
    segments: Vec<Segment>,
}

impl Piste {
    /// Piste vide de valeur constante `v`.
    pub fn constante(v: f32) -> Piste {
        Piste {
            initiale: v,
            segments: Vec::new(),
        }
    }

    /// Ajoute un segment de `duree` secondes allant de la valeur courante
    /// jusqu'à `vers`. Une durée nulle, négative ou non finie est ignorée.
    pub fn puis(mut self, duree: f64, vers: f32, courbe: Courbe) -> Piste {
        if !(duree.is_finite() && duree > 0.0) {
            return self;
        }
        let debut = self.duree_totale();
        let de = if courbe == Courbe::Constante {
            vers
        } else {
            self.valeur_finale()
        };
        self.segments.push(Segment {
            debut,
            duree,
            de,
            vers,
            courbe,
        });
        self
    }

    /// Prolonge la piste par un palier jusqu'à atteindre `total` secondes
    /// (utile pour que toutes les pistes d'un plan aient la même durée
    /// avant l'inversion temporelle).
    pub fn completer(self, total: f64) -> Piste {
        let manque = total - self.duree_totale();
        if manque > 1e-9 {
            let v = self.valeur_finale();
            self.puis(manque, v, Courbe::Constante)
        } else {
            self
        }
    }

    /// Durée totale (s).
    pub fn duree_totale(&self) -> f64 {
        self.segments
            .last()
            .map(|s| s.debut + s.duree)
            .unwrap_or(0.0)
    }

    /// Valeur au début de la piste.
    // Évaluation CPU : sert aux tests (le GPU évalue lui-même les courbes).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn valeur_initiale(&self) -> f32 {
        self.segments.first().map(|s| s.de).unwrap_or(self.initiale)
    }

    /// Valeur à la fin de la piste (conservée ensuite indéfiniment).
    pub fn valeur_finale(&self) -> f32 {
        self.segments.last().map(|s| s.vers).unwrap_or(self.initiale)
    }

    /// Valeur au temps `t` (s) : valeur initiale avant 0, finale après la fin.
    // Évaluation CPU : sert aux tests (le GPU évalue lui-même les courbes).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn valeur_a(&self, t: f64) -> f32 {
        if t <= 0.0 {
            return self.valeur_initiale();
        }
        for s in &self.segments {
            if t < s.debut + s.duree {
                return s.valeur_locale(t - s.debut);
            }
        }
        self.valeur_finale()
    }

    /// Inversion temporelle : v'(t) = v(T − t), T = durée totale.
    /// Les accélérations deviennent des décélérations et réciproquement.
    pub fn inverser_temps(&self) -> Piste {
        // Débuts construits de façon cumulative (comme `puis`) : aucune
        // soustraction flottante, donc jamais de chevauchement entre segments.
        let mut debut = 0.0_f64;
        let mut segments = Vec::with_capacity(self.segments.len());
        for s in self.segments.iter().rev() {
            segments.push(Segment {
                debut,
                duree: s.duree,
                de: s.vers,
                vers: s.de,
                courbe: s.courbe.inverse(),
            });
            debut += s.duree;
        }
        Piste {
            initiale: self.valeur_finale(),
            segments,
        }
    }

    /// Segments de la piste (triés par instant de début).
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn proche(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    /// Évalue le polynôme AddCubic d'un segment au temps local `t`.
    fn polynome(s: &Segment, t: f64) -> f32 {
        let c = coefficients(s);
        let t = t as f32;
        c[0] + c[1] * t + c[2] * t * t + c[3] * t * t * t
    }

    #[test]
    fn coefficients_correspondent_aux_courbes() {
        for courbe in [
            Courbe::Lineaire,
            Courbe::EntreeCubique,
            Courbe::SortieCubique,
        ] {
            let s = Segment {
                debut: 0.0,
                duree: 0.25,
                de: 1.0,
                vers: 0.2,
                courbe,
            };
            for i in 0..=10 {
                let t = 0.025 * i as f64;
                assert!(
                    proche(polynome(&s, t), s.valeur_locale(t)),
                    "{courbe:?} à t={t}"
                );
            }
            assert!(proche(polynome(&s, 0.0), 1.0));
            assert!(proche(polynome(&s, 0.25), 0.2));
        }
        let c = Segment {
            debut: 0.0,
            duree: 0.1,
            de: 0.5,
            vers: 0.5,
            courbe: Courbe::Constante,
        };
        assert_eq!(coefficients(&c), [0.5, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn coefficients_formules_du_plan() {
        let s = Segment {
            debut: 0.0,
            duree: 2.0,
            de: 1.0,
            vers: 3.0,
            courbe: Courbe::SortieCubique,
        };
        // Δ = 2, T = 2 : [1, 3Δ/T, −3Δ/T², Δ/T³] = [1, 3, −1.5, 0.25].
        assert_eq!(coefficients(&s), [1.0, 3.0, -1.5, 0.25]);
        let e = Segment {
            courbe: Courbe::EntreeCubique,
            ..s
        };
        assert_eq!(coefficients(&e), [1.0, 0.0, 0.0, 0.25]);
        let l = Segment {
            courbe: Courbe::Lineaire,
            ..s
        };
        assert_eq!(coefficients(&l), [1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn piste_contigue_et_continue() {
        let p = Piste::constante(1.0)
            .puis(0.1, 0.5, Courbe::EntreeCubique)
            .puis(0.2, 0.0, Courbe::SortieCubique)
            .puis(0.0, 9.0, Courbe::Lineaire); // ignoré (durée nulle)
        assert_eq!(p.segments().len(), 2);
        assert!((p.duree_totale() - 0.3).abs() < 1e-9);
        let segs = p.segments();
        assert!((segs[1].debut - (segs[0].debut + segs[0].duree)).abs() < 1e-12);
        assert!(proche(segs[0].vers, segs[1].de));
        // Continuité numérique autour de la jonction.
        assert!(proche(p.valeur_a(0.1 - 1e-7), p.valeur_a(0.1 + 1e-7)));
        assert!(proche(p.valeur_a(-1.0), 1.0));
        assert!(proche(p.valeur_a(10.0), 0.0));
    }

    #[test]
    fn inversion_temporelle() {
        let p = Piste::constante(1.0)
            .puis(0.15, 0.4, Courbe::EntreeCubique)
            .puis(0.05, 0.4, Courbe::Constante)
            .puis(0.1, 0.0, Courbe::Lineaire);
        let q = p.inverser_temps();
        let total = p.duree_totale();
        assert!((q.duree_totale() - total).abs() < 1e-9);
        for i in 0..=30 {
            let t = total * i as f64 / 30.0;
            assert!(
                proche(q.valeur_a(t), p.valeur_a(total - t)),
                "t={t} : {} != {}",
                q.valeur_a(t),
                p.valeur_a(total - t)
            );
        }
        assert_eq!(q.segments()[2].courbe, Courbe::SortieCubique);
        assert!(proche(q.valeur_initiale(), 0.0));
        assert!(proche(q.valeur_finale(), 1.0));
        // Double inversion = identité (aux arrondis près).
        let r = q.inverser_temps();
        assert_eq!(r.segments().len(), p.segments().len());
        for (a, b) in r.segments().iter().zip(p.segments()) {
            assert!((a.debut - b.debut).abs() < 1e-9);
            assert_eq!(a.courbe, b.courbe);
            assert!(proche(a.de, b.de) && proche(a.vers, b.vers));
        }
    }

    #[test]
    fn completer_prolonge_par_un_palier() {
        let p = Piste::constante(0.0).puis(0.1, 1.0, Courbe::Lineaire).completer(0.3);
        assert!((p.duree_totale() - 0.3).abs() < 1e-9);
        assert!(proche(p.valeur_a(0.2), 1.0));
        let vide = Piste::constante(2.0);
        assert!(proche(vide.valeur_a(0.5), 2.0));
        assert!(proche(vide.inverser_temps().valeur_finale(), 2.0));
    }
}
