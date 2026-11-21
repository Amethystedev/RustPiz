//! Constantes géométriques de la surimpression de l'effet « Jelly »
//! (module pur, sans `windows`).
//!
//! v0.4.1 : le rendu par tuiles affines DirectComposition (v0.4.0) a été
//! retiré (coutures visibles entre les tuiles) ; la déformation est
//! désormais rendue par le maillage Direct3D de
//! [`maillage_gpu`](super::maillage_gpu) ([`grille_rendu`](super::grille_rendu)).
//! Restent ici le rayon des coins et la marge de la surimpression.

use super::ressort::ParametresRessort;

/// Rayon des coins arrondis de Windows 11 (pixels).
pub const RAYON_COINS: f32 = 8.0;

/// Marge de la surimpression autour de l'image (pixels) : couvre le
/// déplacement maximal des sommets.
pub fn marge_px(params: &ParametresRessort) -> i32 {
    (params.deplacement_max_px.max(0.0) + 4.0).ceil() as i32
}

#[cfg(test)]
mod tests {
    use super::super::ressort::{parametres, Puissance};
    use super::*;

    #[test]
    fn marge_couvre_le_deplacement_max() {
        for v in [25u8, 50, 75, 100] {
            for (l, h) in [(200u32, 150u32), (800, 600), (16384, 16384)] {
                let p = parametres(Puissance::new(v), l, h).expect("paramètres");
                assert!(marge_px(&p) as f32 >= p.deplacement_max_px);
            }
        }
    }
}
