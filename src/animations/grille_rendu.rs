//! Grille de rendu du maillage « Jelly » (module pur, sans `windows`).
//!
//! La simulation ([`ressort::Grille`](super::ressort::Grille)) ne compte que
//! (tx+1)(ty+1) nœuds (3 à 10 cases par axe). Pour que la courbure soit
//! lisse à l'écran, chaque case est sous-divisée en `s × s` sous-quads ; le
//! déplacement de chaque sommet de rendu est interpolé à partir des nœuds
//! par une spline de Catmull-Rom bicubique (produit tensoriel), qui :
//! - passe exactement par les nœuds (sommets de nœud = déplacement simulé) ;
//! - est continue et à dérivée continue d'une case à l'autre (pas de
//!   cassure de pente aux coutures, contrairement aux tuiles affines) ;
//! - reproduit exactement un champ affine (extrapolation linéaire aux bords).
//!
//! Les coordonnées de texture sont fixes ; les triangles indexés partagent
//! leurs sommets, donc l'image est interpolée continûment : aucune couture.

use super::ressort::{Grille, Vec2};

/// Nombre maximal de sommets de rendu (coût GPU/CPU négligeable).
pub const SOMMETS_MAX: usize = 4900;
/// Sous-division maximale d'une case de la simulation (par axe).
pub const SUBDIVISION_MAX: u32 = 8;
/// Sous-division minimale (au moins 2 pour courber chaque case).
pub const SUBDIVISION_MIN: u32 = 2;
/// Taille visée d'un sous-quad (pixels).
pub const PAS_CIBLE_PX: f32 = 16.0;

/// Sommet transmis au GPU : position (pixels de la cible) et coordonnées
/// de texture. Disposition C (16 octets), conforme à la disposition
/// d'entrée `POSITION` (R32G32_FLOAT) + `TEXCOORD0` (R32G32_FLOAT).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SommetRendu {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

/// Poids d'interpolation le long d'un axe : 4 couples (index de nœud,
/// poids), extrapolation linéaire déjà repliée sur les nœuds existants.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Poids([(usize, f32); 4]);

/// Poids de Catmull-Rom pour le paramètre `t` ∈ [0, 1] entre les nœuds
/// `k` et `k+1` (nœuds `k-1 .. k+2`).
fn poids_catmull_rom(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t + 2.0 * t2 - t3),
        0.5 * (2.0 - 5.0 * t2 + 3.0 * t3),
        0.5 * (t + 4.0 * t2 - 3.0 * t3),
        0.5 * (-t2 + t3),
    ]
}

/// Poids du sous-sommet `j` (0 ..= cases × sub) sur un axe de `noeuds`
/// nœuds (≥ 2). Un nœud fictif hors grille est extrapolé linéairement :
/// p(-1) = 2·p(0) − p(1) ; p(n) = 2·p(n−1) − p(n−2).
fn poids_axe(j: u32, sub: u32, noeuds: u32) -> Poids {
    let cases = noeuds - 1;
    let k = (j / sub).min(cases - 1);
    let t = (j - k * sub) as f32 / sub as f32;
    let w = poids_catmull_rom(t);
    let n = noeuds as i64;
    let mut sortie = [(0usize, 0.0f32); 4];
    let mut libre = 0;
    let mut ajouter = |index: i64, poids: f32| {
        // Fusion avec un couple existant de même index.
        for couple in sortie.iter_mut().take(libre) {
            if couple.0 as i64 == index {
                couple.1 += poids;
                return;
            }
        }
        if libre < 4 {
            sortie[libre] = (index as usize, poids);
            libre += 1;
        }
    };
    for (a, &wa) in w.iter().enumerate() {
        let index = k as i64 - 1 + a as i64;
        if index < 0 {
            ajouter(0, 2.0 * wa);
            ajouter(1, -wa);
        } else if index >= n {
            ajouter(n - 1, 2.0 * wa);
            ajouter(n - 2, -wa);
        } else {
            ajouter(index, wa);
        }
    }
    Poids(sortie)
}

/// Grille de rendu dérivée de la grille de simulation.
#[derive(Debug, Clone)]
pub struct GrilleRendu {
    noeuds_x: u32,
    noeuds_y: u32,
    sub: u32,
    colonnes: u32,
    lignes: u32,
    poids_x: Vec<Poids>,
    poids_y: Vec<Poids>,
    repos: Vec<Vec2>,
    uv: Vec<[f32; 2]>,
    indices: Vec<u16>,
}

/// Sous-division retenue pour une grille : sous-quads d'environ
/// [`PAS_CIBLE_PX`], bornée à [`SUBDIVISION_MIN`]..[`SUBDIVISION_MAX`],
/// puis réduite jusqu'à tenir dans [`SOMMETS_MAX`] sommets.
pub fn subdivision(grille: &Grille) -> u32 {
    let tx = grille.tuiles_x.max(1);
    let ty = grille.tuiles_y.max(1);
    let case = (grille.largeur as f32 / tx as f32).max(grille.hauteur as f32 / ty as f32);
    let mut sub = if case.is_finite() {
        ((case / PAS_CIBLE_PX).ceil() as u32).clamp(SUBDIVISION_MIN, SUBDIVISION_MAX)
    } else {
        SUBDIVISION_MIN
    };
    while sub > 1 && ((tx * sub + 1) as usize) * ((ty * sub + 1) as usize) > SOMMETS_MAX {
        sub -= 1;
    }
    sub
}

impl GrilleRendu {
    /// Construit la grille de rendu. `None` si la grille de simulation est
    /// dégénérée (aucune case, taille nulle) ou trop grande pour des index
    /// 16 bits.
    pub fn nouvelle(grille: &Grille) -> Option<GrilleRendu> {
        if grille.tuiles_x == 0
            || grille.tuiles_y == 0
            || grille.largeur == 0
            || grille.hauteur == 0
        {
            return None;
        }
        let sub = subdivision(grille);
        let colonnes = grille.tuiles_x.checked_mul(sub)?.checked_add(1)?;
        let lignes = grille.tuiles_y.checked_mul(sub)?.checked_add(1)?;
        let n = (colonnes as usize).checked_mul(lignes as usize)?;
        if n > u16::MAX as usize + 1 {
            return None;
        }
        let noeuds_x = grille.tuiles_x + 1;
        let noeuds_y = grille.tuiles_y + 1;
        let poids_x: Vec<Poids> = (0..colonnes).map(|j| poids_axe(j, sub, noeuds_x)).collect();
        let poids_y: Vec<Poids> = (0..lignes).map(|i| poids_axe(i, sub, noeuds_y)).collect();

        let dx = (colonnes - 1) as f32;
        let dy = (lignes - 1) as f32;
        let mut repos = Vec::with_capacity(n);
        let mut uv = Vec::with_capacity(n);
        for i in 0..lignes {
            for j in 0..colonnes {
                let u = j as f32 / dx;
                let v = i as f32 / dy;
                // Même formule que `Grille::position_repos` aux sommets de
                // nœud (j = ix·sub) : positions identiques.
                repos.push(Vec2::new(
                    grille.largeur as f32 * j as f32 / dx,
                    grille.hauteur as f32 * i as f32 / dy,
                ));
                uv.push([u, v]);
            }
        }

        let mut indices = Vec::with_capacity(((colonnes - 1) * (lignes - 1) * 6) as usize);
        for i in 0..lignes - 1 {
            for j in 0..colonnes - 1 {
                let a = (i * colonnes + j) as u16;
                let b = a + 1;
                let c = ((i + 1) * colonnes + j) as u16;
                let d = c + 1;
                // Sens horaire à l'écran (y vers le bas) : face avant par
                // défaut de Direct3D.
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }

        Some(GrilleRendu {
            noeuds_x,
            noeuds_y,
            sub,
            colonnes,
            lignes,
            poids_x,
            poids_y,
            repos,
            uv,
            indices,
        })
    }

    /// Sous-division par case (par axe).
    pub fn subdivision(&self) -> u32 {
        self.sub
    }

    /// Sommets par ligne.
    pub fn colonnes(&self) -> u32 {
        self.colonnes
    }

    /// Lignes de sommets.
    pub fn lignes(&self) -> u32 {
        self.lignes
    }

    pub fn nb_sommets(&self) -> usize {
        self.repos.len()
    }

    pub fn nb_triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Index des triangles (3 par triangle, sens horaire à l'écran).
    pub fn indices(&self) -> &[u16] {
        &self.indices
    }

    /// Coordonnées de texture des sommets (fixes).
    #[cfg(test)]
    pub fn uv(&self) -> &[[f32; 2]] {
        &self.uv
    }

    /// Positions de repos des sommets (coordonnées de l'image).
    #[cfg(test)]
    pub fn repos(&self) -> &[Vec2] {
        &self.repos
    }

    /// Index du sommet de rendu situé sur le nœud (ix, iy) de la
    /// simulation.
    #[cfg(test)]
    pub fn sommet_du_noeud(&self, ix: u32, iy: u32) -> usize {
        (iy * self.sub * self.colonnes + ix * self.sub) as usize
    }

    /// Déplacements interpolés de tous les sommets de rendu. `noeuds` =
    /// déplacements des nœuds de la simulation (absent ou non fini = nul) ;
    /// chaque résultat est borné en norme à `borne` (si `borne` > 0).
    pub fn deplacements(&self, noeuds: &[Vec2], borne: f32, sortie: &mut Vec<Vec2>) {
        sortie.clear();
        sortie.reserve(self.repos.len());
        let nx = self.noeuds_x as usize;
        let lire = |i: usize| {
            let d = noeuds.get(i).copied().unwrap_or(Vec2::ZERO);
            if d.est_fini() {
                d
            } else {
                Vec2::ZERO
            }
        };
        // Interpolation séparable : d'abord selon y (une valeur par
        // colonne de nœuds), puis selon x.
        let mut ligne_noeuds = vec![Vec2::ZERO; nx];
        for py in &self.poids_y {
            for (ix, valeur) in ligne_noeuds.iter_mut().enumerate() {
                let mut s = Vec2::ZERO;
                for &(iy, w) in py.0.iter() {
                    if w != 0.0 {
                        s += lire(iy * nx + ix) * w;
                    }
                }
                *valeur = s;
            }
            for px in &self.poids_x {
                let mut s = Vec2::ZERO;
                for &(ix, w) in px.0.iter() {
                    if w != 0.0 {
                        if let Some(v) = ligne_noeuds.get(ix) {
                            s += *v * w;
                        }
                    }
                }
                sortie.push(borner(s, borne));
            }
        }
    }

    /// Remplit `sortie` avec les sommets déformés : position = repos +
    /// déplacement interpolé + `decalage` (marge de la cible), UV fixes.
    /// `tampon` sert aux déplacements intermédiaires (réutilisé d'un pas à
    /// l'autre).
    pub fn remplir(
        &self,
        noeuds: &[Vec2],
        decalage: Vec2,
        borne: f32,
        tampon: &mut Vec<Vec2>,
        sortie: &mut Vec<SommetRendu>,
    ) {
        self.deplacements(noeuds, borne, tampon);
        sortie.clear();
        sortie.extend(
            self.repos
                .iter()
                .zip(self.uv.iter())
                .zip(tampon.iter())
                .map(|((r, uv), d)| SommetRendu {
                    x: r.x + d.x + decalage.x,
                    y: r.y + d.y + decalage.y,
                    u: uv[0],
                    v: uv[1],
                }),
        );
    }

    /// Nombre de nœuds de simulation attendus.
    pub fn nb_noeuds(&self) -> usize {
        (self.noeuds_x * self.noeuds_y) as usize
    }
}

/// Borne la norme de `d` à `borne` (> 0) ; non fini -> nul.
fn borner(d: Vec2, borne: f32) -> Vec2 {
    if !d.est_fini() {
        return Vec2::ZERO;
    }
    if borne > 0.0 {
        let n = d.norme();
        if n > borne {
            return d * (borne / n);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grille(tx: u32, ty: u32, l: u32, h: u32) -> Grille {
        Grille {
            tuiles_x: tx,
            tuiles_y: ty,
            largeur: l,
            hauteur: h,
        }
    }

    fn grilles_types() -> Vec<Grille> {
        vec![
            grille(3, 3, 300, 200),
            grille(8, 5, 1000, 700),
            grille(10, 10, 1920, 1080),
            grille(10, 3, 3840, 300),
            grille(1, 1, 50, 50),
            Grille::pour(1000, 700).unwrap(),
            Grille::pour(120, 90).unwrap(),
        ]
    }

    /// Champ de déplacement pseudo-aléatoire déterministe.
    fn champ(n: usize, graine: u32) -> Vec<Vec2> {
        let mut x = graine.wrapping_mul(2654435761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let a = (x % 2001) as f32 / 100.0 - 10.0;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let b = (x % 2001) as f32 / 100.0 - 10.0;
                Vec2::new(a, b)
            })
            .collect()
    }

    #[test]
    fn nombres_de_sommets_et_d_indices() {
        for g in grilles_types() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            let s = r.subdivision();
            assert!((1..=SUBDIVISION_MAX).contains(&s));
            let c = (g.tuiles_x * s + 1) as usize;
            let l = (g.tuiles_y * s + 1) as usize;
            assert_eq!(r.colonnes() as usize, c);
            assert_eq!(r.lignes() as usize, l);
            assert_eq!(r.nb_sommets(), c * l);
            assert!(r.nb_sommets() <= SOMMETS_MAX, "{g:?} : {}", r.nb_sommets());
            assert_eq!(r.indices().len(), (c - 1) * (l - 1) * 6);
            assert_eq!(r.nb_triangles(), (c - 1) * (l - 1) * 2);
            assert_eq!(r.uv().len(), r.nb_sommets());
            assert_eq!(r.nb_noeuds(), g.noeuds());
        }
    }

    #[test]
    fn subdivision_au_moins_deux_pour_les_grilles_reelles() {
        // Grille maximale 10×10 : ≤ 4900 sommets avec une sous-division ≥ 2.
        let g = grille(10, 10, 3840, 2160);
        let s = subdivision(&g);
        assert!(s >= SUBDIVISION_MIN);
        assert!(((10 * s + 1) * (10 * s + 1)) as usize <= SOMMETS_MAX);
        // Petite fenêtre : jamais moins que le minimum.
        assert_eq!(subdivision(&grille(3, 3, 30, 30)), SUBDIVISION_MIN);
    }

    #[test]
    fn uv_dans_l_intervalle_et_coins_couverts() {
        for g in grilles_types() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            for uv in r.uv() {
                assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]));
            }
            let c = r.colonnes() as usize;
            let n = r.nb_sommets();
            assert_eq!(r.uv()[0], [0.0, 0.0]);
            assert_eq!(r.uv()[c - 1], [1.0, 0.0]);
            assert_eq!(r.uv()[n - c], [0.0, 1.0]);
            assert_eq!(r.uv()[n - 1], [1.0, 1.0]);
            // Positions de repos aux coins = coins de l'image.
            assert_eq!(r.repos()[0], Vec2::new(0.0, 0.0));
            assert_eq!(
                r.repos()[n - 1],
                Vec2::new(g.largeur as f32, g.hauteur as f32)
            );
        }
    }

    #[test]
    fn indices_valides_sans_degenerescence_et_orientes() {
        for g in grilles_types() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            let n = r.nb_sommets();
            for t in r.indices().chunks(3) {
                assert!(t.iter().all(|&i| (i as usize) < n));
                assert!(t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
                let p: Vec<Vec2> = t.iter().map(|&i| r.repos()[i as usize]).collect();
                let aire =
                    (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x);
                // Repère écran (y vers le bas) : aire > 0 = sens horaire.
                assert!(
                    aire > 1e-3,
                    "{g:?} : triangle dégénéré ou retourné ({aire})"
                );
            }
            // Couverture : la somme des aires = aire de l'image.
            let total: f32 = r
                .indices()
                .chunks(3)
                .map(|t| {
                    let p: Vec<Vec2> = t.iter().map(|&i| r.repos()[i as usize]).collect();
                    0.5 * ((p[1].x - p[0].x) * (p[2].y - p[0].y)
                        - (p[1].y - p[0].y) * (p[2].x - p[0].x))
                })
                .sum();
            let attendu = g.largeur as f32 * g.hauteur as f32;
            assert!((total - attendu).abs() / attendu < 1e-4);
        }
    }

    #[test]
    fn positions_de_repos_identiques_aux_noeuds() {
        for g in grilles_types() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            for iy in 0..=g.tuiles_y {
                for ix in 0..=g.tuiles_x {
                    let a = r.repos()[r.sommet_du_noeud(ix, iy)];
                    let b = g.position_repos(g.index(ix, iy));
                    assert!((a - b).norme() < 1e-3, "{g:?} ({ix},{iy}) : {a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn interpolation_egale_aux_noeuds() {
        for (k, g) in grilles_types().into_iter().enumerate() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            let noeuds = champ(g.noeuds(), k as u32 + 7);
            let mut d = Vec::new();
            r.deplacements(&noeuds, 0.0, &mut d);
            assert_eq!(d.len(), r.nb_sommets());
            for iy in 0..=g.tuiles_y {
                for ix in 0..=g.tuiles_x {
                    let a = d[r.sommet_du_noeud(ix, iy)];
                    let b = noeuds[g.index(ix, iy)];
                    assert!((a - b).norme() < 1e-4, "{g:?} ({ix},{iy}) : {a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn champ_affine_reproduit_exactement() {
        let g = grille(5, 4, 640, 480);
        let r = GrilleRendu::nouvelle(&g).unwrap();
        let f = |p: Vec2| {
            Vec2::new(
                3.0 + 0.02 * p.x - 0.01 * p.y,
                -2.0 + 0.015 * p.x + 0.03 * p.y,
            )
        };
        let noeuds: Vec<Vec2> = (0..g.noeuds()).map(|i| f(g.position_repos(i))).collect();
        let mut d = Vec::new();
        r.deplacements(&noeuds, 0.0, &mut d);
        for (p, v) in r.repos().iter().zip(d.iter()) {
            assert!((f(*p) - *v).norme() < 1e-3, "{p:?} : {v:?} vs {:?}", f(*p));
        }
    }

    #[test]
    fn continuite_entre_sommets_voisins() {
        // Écart entre sommets voisins borné : pas de saut aux coutures des
        // cases (le pas maximal de la spline sur un sous-quad est borné par
        // les différences entre nœuds voisins).
        for (k, g) in grilles_types().into_iter().enumerate() {
            let r = GrilleRendu::nouvelle(&g).unwrap();
            let noeuds = champ(g.noeuds(), k as u32 + 101);
            let ecart_noeuds = noeuds.iter().map(|d| d.norme()).fold(0.0f32, f32::max) * 2.0;
            let mut d = Vec::new();
            r.deplacements(&noeuds, 0.0, &mut d);
            let c = r.colonnes() as usize;
            let l = r.lignes() as usize;
            let s = r.subdivision() as f32;
            // Borne : |Δ| ≤ 1,25 × 3 × écart maximal entre nœuds / s (marge
            // large ; un saut de couture donnerait ~écart complet).
            let borne = 1.25 * 3.0 * ecart_noeuds / s + 1e-3;
            for i in 0..l {
                for j in 0..c {
                    let a = d[i * c + j];
                    if j + 1 < c {
                        assert!((d[i * c + j + 1] - a).norme() <= borne);
                    }
                    if i + 1 < l {
                        assert!((d[(i + 1) * c + j] - a).norme() <= borne);
                    }
                }
            }
        }
    }

    #[test]
    fn pente_continue_aux_coutures() {
        // Champ quadratique en x : la différence seconde de part et d'autre
        // d'une couture de case reste du même ordre (pas de cassure).
        let g = grille(4, 1, 400, 100);
        let r = GrilleRendu::nouvelle(&g).unwrap();
        let noeuds: Vec<Vec2> = (0..g.noeuds())
            .map(|i| {
                let p = g.position_repos(i);
                Vec2::new(0.0, (p.x / 100.0) * (p.x / 100.0))
            })
            .collect();
        let mut d = Vec::new();
        r.deplacements(&noeuds, 0.0, &mut d);
        let s = r.subdivision() as usize;
        for case in 1..4 {
            let j = case * s; // sommet de couture (ligne 0)
            let gauche = d[j].y - d[j - 1].y;
            let droite = d[j + 1].y - d[j].y;
            let pas = d[j + 1].y - d[j - 1].y;
            assert!((droite - gauche).abs() <= 0.5 * pas.abs() + 1e-3);
        }
    }

    #[test]
    fn deplacements_nuls_non_finis_et_bornes() {
        let g = grille(3, 3, 300, 300);
        let r = GrilleRendu::nouvelle(&g).unwrap();
        let mut d = Vec::new();
        // Liste vide : tout nul.
        r.deplacements(&[], 0.0, &mut d);
        assert!(d.iter().all(|v| *v == Vec2::ZERO));
        // NaN : traité comme nul, résultat fini.
        let mut noeuds = vec![Vec2::ZERO; g.noeuds()];
        noeuds[5] = Vec2::new(f32::NAN, 1.0);
        r.deplacements(&noeuds, 0.0, &mut d);
        assert!(d.iter().all(|v| v.est_fini()));
        // Borne respectée.
        let noeuds = vec![Vec2::new(100.0, -80.0); g.noeuds()];
        r.deplacements(&noeuds, 20.0, &mut d);
        assert!(d.iter().all(|v| v.norme() <= 20.0 + 1e-3));
    }

    #[test]
    fn remplir_applique_le_decalage_et_les_uv() {
        let g = grille(3, 2, 300, 200);
        let r = GrilleRendu::nouvelle(&g).unwrap();
        let mut tampon = Vec::new();
        let mut sommets = Vec::new();
        let noeuds = vec![Vec2::new(1.0, 2.0); g.noeuds()];
        r.remplir(
            &noeuds,
            Vec2::new(10.0, 20.0),
            0.0,
            &mut tampon,
            &mut sommets,
        );
        assert_eq!(sommets.len(), r.nb_sommets());
        let n = sommets.len();
        assert_eq!(
            sommets[0],
            SommetRendu {
                x: 11.0,
                y: 22.0,
                u: 0.0,
                v: 0.0
            }
        );
        assert!((sommets[n - 1].x - 311.0).abs() < 1e-3);
        assert!((sommets[n - 1].y - 222.0).abs() < 1e-3);
        assert_eq!(std::mem::size_of::<SommetRendu>(), 16);
    }

    #[test]
    fn poids_somme_unite() {
        for noeuds in 2..12u32 {
            for sub in 1..=SUBDIVISION_MAX {
                for j in 0..=(noeuds - 1) * sub {
                    let p = poids_axe(j, sub, noeuds);
                    let s: f32 = p.0.iter().map(|c| c.1).sum();
                    assert!((s - 1.0).abs() < 1e-5);
                    assert!(p.0.iter().all(|c| (c.0 as u32) < noeuds));
                }
            }
        }
    }
}
