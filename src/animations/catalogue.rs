//! Catalogue des effets d'animation d'ouverture / fermeture des fenêtres.
//!
//! Module pur (aucune dépendance Win32) : noms de configuration, libellés
//! de menu et durées. Le rendu GPU est réalisé ailleurs.
//!
//! v0.4.7 : deux moteurs de rendu ([`Moteur`]) — les effets historiques
//! animés par le compositeur DirectComposition, et les effets « à
//! shader » (pixel shader HLSL par effet, rendu Direct3D 11 dans une
//! swapchain de composition, voir `effet_gpu` / `effets_shader`).

use std::time::Duration;

/// Moteur de rendu d'un effet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Moteur {
    /// Propriétés de visuels DirectComposition animées par le compositeur
    /// (un seul `Commit`, aucun rendu par image).
    Composition,
    /// Pixel shader HLSL rendu image par image (triangle plein écran).
    Shader,
}

/// Effet d'animation applicable à l'ouverture / fermeture d'une fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffetAnimation {
    /// Aucune animation.
    Aucun,
    /// Fondu enchaîné (opacité de 0 à 100 %).
    Fondu,
    /// Zoom depuis (ou vers) le centre de la fenêtre.
    Zoom,
    /// Glissement depuis un bord de l'écran.
    Glissement,
    /// Rétrécissement vers un point.
    Retrecissement,
    /// Extinction de télévision (écrasement vertical puis horizontal).
    Television,
    /// Flou progressif (nécessite un périphérique Direct3D 11.3+).
    Flou,
    /// Pixellisation croissante, puis extinction des blocs (shader).
    Pixellisation,
    /// Front de balayage qui pixellise puis efface la fenêtre (shader).
    BalayagePixels,
    /// Tranches décalées, séparation RVB et blocs vides (shader).
    Glitch,
    /// Blocs éteints dans l'ordre de leur angle autour du centre (shader).
    RouePixels,
    /// Colonnes qui coulent vers le bas, façon « Doom » (shader).
    Fonte,
    /// Grille hexagonale dont les cellules se rétrécissent (shader).
    Hexagones,
    /// Extinction de télévision avec parasites et neige (shader).
    TelevisionParasitee,
    /// Bascule en arrière en perspective, montée et fondu (shader).
    Plane,
    /// Tourbillon, dissolution et blanc spectral (shader).
    Apparition,
    /// Étincelles dorées pendant l'effacement (shader).
    EnergieA,
    /// Étincelles et traînées bleues, dissolution vers le haut (shader).
    EnergieB,
    /// Front de flammes montant du bas (shader).
    Feu,
    /// Combustion depuis un point, braises (shader).
    Incineration,
    /// Aspiration en spirale, anneau vert (shader).
    Portail,
    /// Éclats de verre en 6 couches rigides (shader, approximation).
    VerreBrise,
    /// Envol en poussière (shader, approximation).
    Desintegration,
    /// Pluie de pseudo-glyphes verts (shader, approximation).
    Matrix,
    /// Dissolution en points lumineux flottants (shader, approximation).
    FeuxFollets,
}

impl EffetAnimation {
    /// Tous les effets, dans l'ordre du sous-menu.
    /// Les effets à shader sont placés à la fin (sous-menu dédié).
    pub const TOUS: [EffetAnimation; 25] = [
        EffetAnimation::Aucun,
        EffetAnimation::Fondu,
        EffetAnimation::Zoom,
        EffetAnimation::Glissement,
        EffetAnimation::Retrecissement,
        EffetAnimation::Television,
        EffetAnimation::Flou,
        EffetAnimation::Pixellisation,
        EffetAnimation::BalayagePixels,
        EffetAnimation::Glitch,
        EffetAnimation::RouePixels,
        EffetAnimation::Fonte,
        EffetAnimation::Hexagones,
        EffetAnimation::TelevisionParasitee,
        EffetAnimation::Plane,
        EffetAnimation::Apparition,
        EffetAnimation::EnergieA,
        EffetAnimation::EnergieB,
        EffetAnimation::Feu,
        EffetAnimation::Incineration,
        EffetAnimation::Portail,
        EffetAnimation::VerreBrise,
        EffetAnimation::Desintegration,
        EffetAnimation::Matrix,
        EffetAnimation::FeuxFollets,
    ];

    /// Convertit un nom de la configuration en effet (insensible à la
    /// casse et aux espaces autour du nom).
    pub fn depuis_nom(nom: &str) -> Option<EffetAnimation> {
        match nom.trim().to_lowercase().as_str() {
            "aucun" => Some(EffetAnimation::Aucun),
            "fondu" => Some(EffetAnimation::Fondu),
            "zoom" => Some(EffetAnimation::Zoom),
            "glissement" => Some(EffetAnimation::Glissement),
            "retrecissement" | "rétrécissement" => Some(EffetAnimation::Retrecissement),
            "television" | "télévision" | "tv" => Some(EffetAnimation::Television),
            "flou" => Some(EffetAnimation::Flou),
            "pixellisation" | "pixelisation" | "pixelate" => Some(EffetAnimation::Pixellisation),
            "balayage_pixels" | "balayage de pixels" | "pixel_wipe" => {
                Some(EffetAnimation::BalayagePixels)
            }
            "glitch" => Some(EffetAnimation::Glitch),
            "roue_pixels" | "roue de pixels" | "pixel_wheel" => Some(EffetAnimation::RouePixels),
            "fonte" | "doom" => Some(EffetAnimation::Fonte),
            "hexagones" | "hexagone" | "hexagon" => Some(EffetAnimation::Hexagones),
            "television_parasitee" | "télévision parasitée" | "tv_glitch" => {
                Some(EffetAnimation::TelevisionParasitee)
            }
            "plane" | "plané" | "glide" => Some(EffetAnimation::Plane),
            "apparition" => Some(EffetAnimation::Apparition),
            "energie_a" | "énergie a" | "energize_a" => Some(EffetAnimation::EnergieA),
            "energie_b" | "énergie b" | "energize_b" => Some(EffetAnimation::EnergieB),
            "feu" | "fire" => Some(EffetAnimation::Feu),
            "incineration" | "incinération" | "incinerate" => Some(EffetAnimation::Incineration),
            "portail" | "portal" => Some(EffetAnimation::Portail),
            "verre_brise" | "verre brisé" | "broken_glass" => Some(EffetAnimation::VerreBrise),
            "desintegration" | "désintégration" | "snap" => Some(EffetAnimation::Desintegration),
            "matrix" => Some(EffetAnimation::Matrix),
            "feux_follets" | "feux follets" | "wisps" => Some(EffetAnimation::FeuxFollets),
            _ => None,
        }
    }

    /// Nom utilisé dans `config.toml` (ASCII, sans accent).
    pub fn nom_config(self) -> &'static str {
        match self {
            EffetAnimation::Aucun => "aucun",
            EffetAnimation::Fondu => "fondu",
            EffetAnimation::Zoom => "zoom",
            EffetAnimation::Glissement => "glissement",
            EffetAnimation::Retrecissement => "retrecissement",
            EffetAnimation::Television => "television",
            EffetAnimation::Flou => "flou",
            EffetAnimation::Pixellisation => "pixellisation",
            EffetAnimation::BalayagePixels => "balayage_pixels",
            EffetAnimation::Glitch => "glitch",
            EffetAnimation::RouePixels => "roue_pixels",
            EffetAnimation::Fonte => "fonte",
            EffetAnimation::Hexagones => "hexagones",
            EffetAnimation::TelevisionParasitee => "television_parasitee",
            EffetAnimation::Plane => "plane",
            EffetAnimation::Apparition => "apparition",
            EffetAnimation::EnergieA => "energie_a",
            EffetAnimation::EnergieB => "energie_b",
            EffetAnimation::Feu => "feu",
            EffetAnimation::Incineration => "incineration",
            EffetAnimation::Portail => "portail",
            EffetAnimation::VerreBrise => "verre_brise",
            EffetAnimation::Desintegration => "desintegration",
            EffetAnimation::Matrix => "matrix",
            EffetAnimation::FeuxFollets => "feux_follets",
        }
    }

    /// Libellé affiché dans le menu.
    pub fn nom(self) -> &'static str {
        match self {
            EffetAnimation::Aucun => "Aucun",
            EffetAnimation::Fondu => "Fondu",
            EffetAnimation::Zoom => "Zoom",
            EffetAnimation::Glissement => "Glissement",
            EffetAnimation::Retrecissement => "Rétrécissement",
            EffetAnimation::Television => "Télévision (extinction)",
            EffetAnimation::Flou => "Flou",
            EffetAnimation::Pixellisation => "Pixellisation",
            EffetAnimation::BalayagePixels => "Balayage de pixels",
            EffetAnimation::Glitch => "Glitch",
            EffetAnimation::RouePixels => "Roue de pixels",
            EffetAnimation::Fonte => "Fonte",
            EffetAnimation::Hexagones => "Hexagones",
            EffetAnimation::TelevisionParasitee => "Télévision parasitée",
            EffetAnimation::Plane => "Plané",
            EffetAnimation::Apparition => "Apparition",
            EffetAnimation::EnergieA => "Énergie A",
            EffetAnimation::EnergieB => "Énergie B",
            EffetAnimation::Feu => "Feu",
            EffetAnimation::Incineration => "Incinération",
            EffetAnimation::Portail => "Portail",
            EffetAnimation::VerreBrise => "Verre brisé",
            EffetAnimation::Desintegration => "Désintégration",
            EffetAnimation::Matrix => "Matrix",
            EffetAnimation::FeuxFollets => "Feux follets",
        }
    }

    /// Durée de l'animation (nulle pour `Aucun`).
    pub fn duree(self) -> Duration {
        let ms = match self {
            EffetAnimation::Aucun => 0,
            EffetAnimation::Fondu => 200,
            EffetAnimation::Zoom => 220,
            EffetAnimation::Glissement => 250,
            EffetAnimation::Retrecissement => 250,
            EffetAnimation::Television => 350,
            EffetAnimation::Flou => 300,
            EffetAnimation::Pixellisation => 450,
            EffetAnimation::BalayagePixels => 450,
            EffetAnimation::Glitch => 400,
            EffetAnimation::RouePixels => 500,
            EffetAnimation::Fonte => 700,
            EffetAnimation::Hexagones => 600,
            EffetAnimation::TelevisionParasitee => 450,
            EffetAnimation::Plane => 450,
            EffetAnimation::Apparition => 550,
            EffetAnimation::EnergieA => 700,
            EffetAnimation::EnergieB => 700,
            EffetAnimation::Feu => 900,
            EffetAnimation::Incineration => 900,
            EffetAnimation::Portail => 650,
            EffetAnimation::VerreBrise => 800,
            EffetAnimation::Desintegration => 900,
            EffetAnimation::Matrix => 900,
            EffetAnimation::FeuxFollets => 900,
        };
        Duration::from_millis(ms)
    }

    /// Vrai si l'effet exige un périphérique Direct3D 11.3+ (`ID3D11Device3`).
    pub fn exige_device3(self) -> bool {
        matches!(self, EffetAnimation::Flou)
    }

    /// Moteur de rendu de l'effet.
    pub fn moteur(self) -> Moteur {
        match self {
            EffetAnimation::Aucun
            | EffetAnimation::Fondu
            | EffetAnimation::Zoom
            | EffetAnimation::Glissement
            | EffetAnimation::Retrecissement
            | EffetAnimation::Television
            | EffetAnimation::Flou => Moteur::Composition,
            EffetAnimation::Pixellisation
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
            | EffetAnimation::FeuxFollets => Moteur::Shader,
        }
    }

    /// Vrai si l'effet n'est qu'une approximation de l'effet qu'il imite :
    /// Télévision (écrasement par les seules échelles du compositeur, sans
    /// balayage ni bruit) et, parmi les shaders, Verre brisé (éclats groupés
    /// en 6 couches rigides), Désintégration (poussière en 8 couches, source
    /// retrouvée par point fixe), Matrix (pseudo-glyphes aléatoires) et Feux
    /// follets (points en 4 couches translatées). Le menu ajoute
    /// « (approx.) » au libellé des effets à shader approximatifs.
    pub fn est_approximation(self) -> bool {
        matches!(
            self,
            EffetAnimation::Television
                | EffetAnimation::VerreBrise
                | EffetAnimation::Desintegration
                | EffetAnimation::Matrix
                | EffetAnimation::FeuxFollets
        )
    }

    /// Libellé du menu : [`nom`](Self::nom), suivi de « (approx.) » pour un
    /// effet à shader approximatif.
    pub fn libelle_menu(self) -> String {
        if self.moteur() == Moteur::Shader && self.est_approximation() {
            format!("{} (approx.)", self.nom())
        } else {
            self.nom().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_et_accents() {
        assert_eq!(EffetAnimation::depuis_nom(" Rétrécissement "), Some(EffetAnimation::Retrecissement));
        assert_eq!(EffetAnimation::depuis_nom("RETRECISSEMENT"), Some(EffetAnimation::Retrecissement));
        assert_eq!(EffetAnimation::depuis_nom("Télévision"), Some(EffetAnimation::Television));
        assert_eq!(EffetAnimation::depuis_nom("tv"), Some(EffetAnimation::Television));
        assert_eq!(EffetAnimation::depuis_nom("ZOOM"), Some(EffetAnimation::Zoom));
    }

    #[test]
    fn nom_inconnu_rejete() {
        assert_eq!(EffetAnimation::depuis_nom("inconnu"), None);
        assert_eq!(EffetAnimation::depuis_nom(""), None);
    }

    #[test]
    fn aller_retour_nom_config() {
        for effet in EffetAnimation::TOUS {
            assert_eq!(EffetAnimation::depuis_nom(effet.nom_config()), Some(effet));
            assert!(effet.nom_config().is_ascii());
        }
    }

    #[test]
    fn durees() {
        for effet in EffetAnimation::TOUS {
            if effet == EffetAnimation::Aucun {
                assert!(effet.duree().is_zero());
            } else {
                assert!(effet.duree() > Duration::ZERO);
            }
        }
        assert!(EffetAnimation::Flou.exige_device3());
        assert!(!EffetAnimation::Zoom.exige_device3());
    }

    #[test]
    fn alias_shader() {
        assert_eq!(EffetAnimation::depuis_nom("pixelate"), Some(EffetAnimation::Pixellisation));
        assert_eq!(EffetAnimation::depuis_nom("Pixellisation"), Some(EffetAnimation::Pixellisation));
        assert_eq!(EffetAnimation::depuis_nom("PIXEL_WIPE"), Some(EffetAnimation::BalayagePixels));
        assert_eq!(
            EffetAnimation::depuis_nom(" balayage_pixels "),
            Some(EffetAnimation::BalayagePixels)
        );
        assert_eq!(EffetAnimation::depuis_nom("Glitch"), Some(EffetAnimation::Glitch));
        // Anciens noms inchangés.
        for (nom, e) in [
            ("aucun", EffetAnimation::Aucun),
            ("fondu", EffetAnimation::Fondu),
            ("zoom", EffetAnimation::Zoom),
            ("glissement", EffetAnimation::Glissement),
            ("retrecissement", EffetAnimation::Retrecissement),
            ("television", EffetAnimation::Television),
            ("flou", EffetAnimation::Flou),
        ] {
            assert_eq!(EffetAnimation::depuis_nom(nom), Some(e));
            assert_eq!(e.nom_config(), nom);
        }
    }

    #[test]
    fn noms_uniques() {
        for (i, a) in EffetAnimation::TOUS.iter().enumerate() {
            for b in &EffetAnimation::TOUS[i + 1..] {
                assert_ne!(a, b);
                assert_ne!(a.nom_config(), b.nom_config());
                assert_ne!(a.nom(), b.nom());
            }
        }
    }

    #[test]
    fn durees_shader_bornees() {
        for effet in EffetAnimation::TOUS {
            if effet.moteur() == Moteur::Shader {
                let ms = effet.duree().as_millis();
                assert!((200..=1000).contains(&ms), "{effet:?} : {ms} ms");
            }
        }
    }

    #[test]
    fn tous_moins_de_100() {
        assert!(EffetAnimation::TOUS.len() < 100);
        assert_eq!(EffetAnimation::TOUS[0], EffetAnimation::Aucun);
    }

    #[test]
    fn approximations_marquees() {
        let approx = [
            EffetAnimation::VerreBrise,
            EffetAnimation::Desintegration,
            EffetAnimation::Matrix,
            EffetAnimation::FeuxFollets,
        ];
        for effet in EffetAnimation::TOUS {
            if effet.moteur() == Moteur::Shader {
                assert_eq!(effet.est_approximation(), approx.contains(&effet), "{effet:?}");
                if effet.est_approximation() {
                    assert_eq!(effet.libelle_menu(), format!("{} (approx.)", effet.nom()));
                } else {
                    assert_eq!(effet.libelle_menu(), effet.nom());
                }
            }
        }
        assert!(EffetAnimation::Television.est_approximation());
        // Libellé historique inchangé pour la Télévision (moteur Composition).
        assert_eq!(EffetAnimation::Television.libelle_menu(), "Télévision (extinction)");
        assert!(!EffetAnimation::Aucun.est_approximation());
        assert!(!EffetAnimation::Fondu.est_approximation());
        assert!(!EffetAnimation::Pixellisation.est_approximation());
        assert!(!EffetAnimation::Feu.est_approximation());
    }

    #[test]
    fn alias_lot_3() {
        for (nom, e) in [
            ("roue_pixels", EffetAnimation::RouePixels),
            ("pixel_wheel", EffetAnimation::RouePixels),
            ("Roue de pixels", EffetAnimation::RouePixels),
            ("fonte", EffetAnimation::Fonte),
            ("DOOM", EffetAnimation::Fonte),
            ("hexagones", EffetAnimation::Hexagones),
            ("hexagon", EffetAnimation::Hexagones),
            ("television_parasitee", EffetAnimation::TelevisionParasitee),
            ("tv_glitch", EffetAnimation::TelevisionParasitee),
            ("Télévision parasitée", EffetAnimation::TelevisionParasitee),
            ("plane", EffetAnimation::Plane),
            ("glide", EffetAnimation::Plane),
            ("Plané", EffetAnimation::Plane),
            ("apparition", EffetAnimation::Apparition),
            ("energie_a", EffetAnimation::EnergieA),
            ("energize_a", EffetAnimation::EnergieA),
            ("Énergie A", EffetAnimation::EnergieA),
            ("energie_b", EffetAnimation::EnergieB),
            ("ENERGIZE_B", EffetAnimation::EnergieB),
            ("feu", EffetAnimation::Feu),
            ("fire", EffetAnimation::Feu),
            ("incineration", EffetAnimation::Incineration),
            ("incinerate", EffetAnimation::Incineration),
            ("Incinération", EffetAnimation::Incineration),
            ("portail", EffetAnimation::Portail),
            ("portal", EffetAnimation::Portail),
            ("verre_brise", EffetAnimation::VerreBrise),
            ("broken_glass", EffetAnimation::VerreBrise),
            ("Verre brisé", EffetAnimation::VerreBrise),
            ("desintegration", EffetAnimation::Desintegration),
            ("snap", EffetAnimation::Desintegration),
            ("Désintégration", EffetAnimation::Desintegration),
            ("matrix", EffetAnimation::Matrix),
            (" Matrix ", EffetAnimation::Matrix),
            ("feux_follets", EffetAnimation::FeuxFollets),
            ("wisps", EffetAnimation::FeuxFollets),
            ("Feux follets", EffetAnimation::FeuxFollets),
        ] {
            assert_eq!(EffetAnimation::depuis_nom(nom), Some(e), "{nom}");
        }
    }

    #[test]
    fn noms_et_durees_lot_3() {
        for (e, config, menu, ms) in [
            (EffetAnimation::RouePixels, "roue_pixels", "Roue de pixels", 500),
            (EffetAnimation::Fonte, "fonte", "Fonte", 700),
            (EffetAnimation::Hexagones, "hexagones", "Hexagones", 600),
            (
                EffetAnimation::TelevisionParasitee,
                "television_parasitee",
                "Télévision parasitée",
                450,
            ),
            (EffetAnimation::Plane, "plane", "Plané", 450),
            (EffetAnimation::Apparition, "apparition", "Apparition", 550),
            (EffetAnimation::EnergieA, "energie_a", "Énergie A", 700),
            (EffetAnimation::EnergieB, "energie_b", "Énergie B", 700),
            (EffetAnimation::Feu, "feu", "Feu", 900),
            (EffetAnimation::Incineration, "incineration", "Incinération", 900),
            (EffetAnimation::Portail, "portail", "Portail", 650),
            (EffetAnimation::VerreBrise, "verre_brise", "Verre brisé", 800),
            (EffetAnimation::Desintegration, "desintegration", "Désintégration", 900),
            (EffetAnimation::Matrix, "matrix", "Matrix", 900),
            (EffetAnimation::FeuxFollets, "feux_follets", "Feux follets", 900),
        ] {
            assert_eq!(e.nom_config(), config);
            assert_eq!(e.nom(), menu);
            assert_eq!(e.duree(), Duration::from_millis(ms));
            assert_eq!(e.moteur(), Moteur::Shader);
            assert!(!e.exige_device3());
        }
        // Effets à shader regroupés à la fin de TOUS (sous-menu dédié).
        let premier = EffetAnimation::TOUS
            .iter()
            .position(|e| e.moteur() == Moteur::Shader)
            .expect("shader");
        assert!(EffetAnimation::TOUS[premier..]
            .iter()
            .all(|e| e.moteur() == Moteur::Shader));
        assert_eq!(EffetAnimation::TOUS.len(), 25);
    }

    #[test]
    fn moteurs() {
        assert_eq!(EffetAnimation::Pixellisation.moteur(), Moteur::Shader);
        assert_eq!(EffetAnimation::BalayagePixels.moteur(), Moteur::Shader);
        assert_eq!(EffetAnimation::Glitch.moteur(), Moteur::Shader);
        for e in EffetAnimation::TOUS.into_iter().take(7) {
            assert_eq!(e.moteur(), Moteur::Composition, "{e:?}");
        }
    }
}
