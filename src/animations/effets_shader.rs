//! Paramètres des effets « à shader » (module pur, sans Win32).
//!
//! Chaque effet à shader est un pixel shader HLSL (voir [`hlsl_effets`])
//! appliqué à l'image capturée de la fenêtre, dessiné par un triangle plein
//! écran dans une swapchain de composition (`effet_gpu`). Le shader ne
//! dépend QUE de la position (uv / pixels), de la progression `p` (0 =
//! fenêtre intacte, 1 = disparue), de la graine et des tailles : l'ouverture
//! est exactement la fermeture parcourue à rebours (progression inversée
//! dans le temps, comme [`plan`](super::plan)).
//!
//! Ce module fournit : la courbe de progression, la marge autour de la
//! fenêtre, la source HLSL complète de chaque effet, la graine
//! déterministe et la disposition mémoire du tampon de constantes.

use super::catalogue::{EffetAnimation, Moteur};
use super::courbes::Courbe;
use super::hlsl_effets::{
    CORPS_APPARITION, CORPS_BALAYAGE_PIXELS, CORPS_DESINTEGRATION, CORPS_ENERGIE_A,
    CORPS_ENERGIE_B, CORPS_FEU, CORPS_FEUX_FOLLETS, CORPS_FONTE, CORPS_GLITCH, CORPS_HEXAGONES,
    CORPS_INCINERATION, CORPS_MATRIX, CORPS_PIXELLISATION, CORPS_PLANE, CORPS_PORTAIL,
    CORPS_ROUE_PIXELS, CORPS_TELEVISION_PARASITEE, CORPS_VERRE_BRISE, EPILOGUE_HLSL,
    PRELUDE_HLSL,
};
use super::plan::Taille;

/// Marge (pixels) ajoutée autour de la fenêtre pour un effet qui déborde.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Marge {
    /// Aucun débordement.
    Aucune,
    /// `part` de min(largeur, hauteur), plafonnée à `max_px`.
    Fraction { part: f32, max_px: i32 },
}

/// Paramètres d'un effet à shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParametresShader {
    /// Courbe de la progression 0 -> 1 (fermeture).
    pub courbe: Courbe,
    pub marge: Marge,
    /// Nom de source passé à `D3DCompile` (messages du compilateur).
    pub nom_source: &'static str,
    /// Corps HLSL : définit `float4 effet(float2 uv, float2 px)`.
    pub corps: &'static str,
}

/// Paramètres de l'effet (`None` <=> effet du moteur Composition).
pub fn parametres(effet: EffetAnimation) -> Option<ParametresShader> {
    let p = match effet {
        EffetAnimation::Pixellisation => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "pixellisation.hlsl",
            corps: CORPS_PIXELLISATION,
        },
        EffetAnimation::BalayagePixels => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "balayage_pixels.hlsl",
            corps: CORPS_BALAYAGE_PIXELS,
        },
        EffetAnimation::Glitch => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.03,
                max_px: 24,
            },
            nom_source: "glitch.hlsl",
            corps: CORPS_GLITCH,
        },
        EffetAnimation::RouePixels => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "roue_pixels.hlsl",
            corps: CORPS_ROUE_PIXELS,
        },
        EffetAnimation::Fonte => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "fonte.hlsl",
            corps: CORPS_FONTE,
        },
        EffetAnimation::Hexagones => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "hexagones.hlsl",
            corps: CORPS_HEXAGONES,
        },
        EffetAnimation::TelevisionParasitee => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.03,
                max_px: 24,
            },
            nom_source: "television_parasitee.hlsl",
            corps: CORPS_TELEVISION_PARASITEE,
        },
        EffetAnimation::Plane => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "plane.hlsl",
            corps: CORPS_PLANE,
        },
        EffetAnimation::Apparition => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.05,
                max_px: 40,
            },
            nom_source: "apparition.hlsl",
            corps: CORPS_APPARITION,
        },
        EffetAnimation::EnergieA => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.04,
                max_px: 32,
            },
            nom_source: "energie_a.hlsl",
            corps: CORPS_ENERGIE_A,
        },
        EffetAnimation::EnergieB => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.04,
                max_px: 32,
            },
            nom_source: "energie_b.hlsl",
            corps: CORPS_ENERGIE_B,
        },
        EffetAnimation::Feu => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.15,
                max_px: 160,
            },
            nom_source: "feu.hlsl",
            corps: CORPS_FEU,
        },
        EffetAnimation::Incineration => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.10,
                max_px: 120,
            },
            nom_source: "incineration.hlsl",
            corps: CORPS_INCINERATION,
        },
        EffetAnimation::Portail => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.10,
                max_px: 120,
            },
            nom_source: "portail.hlsl",
            corps: CORPS_PORTAIL,
        },
        EffetAnimation::VerreBrise => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.25,
                max_px: 240,
            },
            nom_source: "verre_brise.hlsl",
            corps: CORPS_VERRE_BRISE,
        },
        EffetAnimation::Desintegration => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.20,
                max_px: 200,
            },
            nom_source: "desintegration.hlsl",
            corps: CORPS_DESINTEGRATION,
        },
        EffetAnimation::Matrix => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Aucune,
            nom_source: "matrix.hlsl",
            corps: CORPS_MATRIX,
        },
        EffetAnimation::FeuxFollets => ParametresShader {
            courbe: Courbe::Lineaire,
            marge: Marge::Fraction {
                part: 0.15,
                max_px: 160,
            },
            nom_source: "feux_follets.hlsl",
            corps: CORPS_FEUX_FOLLETS,
        },
        EffetAnimation::Aucun
        | EffetAnimation::Fondu
        | EffetAnimation::Zoom
        | EffetAnimation::Glissement
        | EffetAnimation::Retrecissement
        | EffetAnimation::Television
        | EffetAnimation::Flou => return None,
    };
    debug_assert_eq!(effet.moteur(), Moteur::Shader);
    Some(p)
}

/// Marge en pixels pour une fenêtre de taille donnée (>= 0).
pub fn marge_px(marge: Marge, taille: Taille) -> i32 {
    match marge {
        Marge::Aucune => 0,
        Marge::Fraction { part, max_px } => {
            let cote = taille.largeur.min(taille.hauteur) as f32;
            let v = (cote * part.max(0.0)).ceil();
            if v.is_finite() {
                (v as i32).clamp(0, max_px.max(0))
            } else {
                0
            }
        }
    }
}

/// Source HLSL complète du pixel shader de l'effet (prélude + corps +
/// épilogue ; point d'entrée `ps_effet`). `None` pour un effet Composition.
pub fn source_ps(effet: EffetAnimation) -> Option<String> {
    let p = parametres(effet)?;
    let mut s = String::with_capacity(PRELUDE_HLSL.len() + p.corps.len() + EPILOGUE_HLSL.len());
    s.push_str(PRELUDE_HLSL);
    s.push_str(p.corps);
    s.push_str(EPILOGUE_HLSL);
    Some(s)
}

/// Graine déterministe dans [0, 1) dérivée de l'identifiant d'animation et
/// de la fenêtre (hachage entier type « splitmix64 »).
pub fn graine(id: u32, cible: isize) -> f32 {
    let mut x = (id as u64) ^ (cible as u64).rotate_left(32) ^ 0x9e37_79b9_7f4a_7c15;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    // 24 bits de poids fort : exactement représentables en f32, < 1.
    ((x >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Tampon de constantes du pixel shader (`cbuffer Constantes : register(b0)`
/// de [`PRELUDE_HLSL`], même ordre ; 48 octets, multiple de 16).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConstantesEffet {
    /// Taille de la swapchain (pixels, marge comprise).
    pub taille_cible: [f32; 2],
    /// Taille de l'image de la fenêtre (pixels).
    pub taille_image: [f32; 2],
    /// Marge (pixels) en X et en Y.
    pub marge: [f32; 2],
    /// Progression : 0 = intacte, 1 = disparue.
    pub progression: f32,
    /// Graine dans [0, 1).
    pub graine: f32,
    /// Rayon des coins arrondis (pixels).
    pub rayon: f32,
    /// 0 = fermeture, 1 = ouverture (information : les shaders n'en
    /// dépendent pas, l'inversion temporelle reste exacte).
    pub sens: f32,
    /// Opacité globale appliquée au résultat (1.0).
    pub opacite: f32,
    pub reserve: f32,
}

const _: () = assert!(std::mem::size_of::<ConstantesEffet>() == 48);
const _: () = assert!(std::mem::size_of::<ConstantesEffet>().is_multiple_of(16));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animations::hlsl_effets::VS_HLSL;

    fn effets_shader() -> impl Iterator<Item = EffetAnimation> {
        EffetAnimation::TOUS
            .into_iter()
            .filter(|e| e.moteur() == Moteur::Shader)
    }

    #[test]
    fn moteur_coherent_avec_parametres() {
        for e in EffetAnimation::TOUS {
            assert_eq!(
                parametres(e).is_some(),
                e.moteur() == Moteur::Shader,
                "{e:?}"
            );
            assert_eq!(
                source_ps(e).is_some(),
                e.moteur() == Moteur::Shader,
                "{e:?}"
            );
        }
    }

    #[test]
    fn constantes_48_octets() {
        assert_eq!(std::mem::size_of::<ConstantesEffet>(), 48);
        assert_eq!(std::mem::align_of::<ConstantesEffet>(), 4);
    }

    #[test]
    fn graine_dans_0_1_et_deterministe() {
        let mut distinctes = std::collections::HashSet::new();
        for id in 0..200u32 {
            for cible in [0isize, 1, 0x1234, -1, isize::MAX, 0x7ff0_0042] {
                let g = graine(id, cible);
                assert!((0.0..1.0).contains(&g), "{g}");
                assert_eq!(g, graine(id, cible));
                distinctes.insert(g.to_bits());
            }
        }
        // Le hachage disperse bien les valeurs.
        assert!(distinctes.len() > 1000);
    }

    #[test]
    fn marges() {
        let t = Taille {
            largeur: 1200,
            hauteur: 800,
        };
        assert_eq!(marge_px(Marge::Aucune, t), 0);
        let g = parametres(EffetAnimation::Glitch).expect("glitch").marge;
        assert_eq!(marge_px(g, t), 24); // 3 % de 800 = 24
        let petite = Taille {
            largeur: 300,
            hauteur: 100,
        };
        assert_eq!(marge_px(g, petite), 3);
        let geante = Taille {
            largeur: 8000,
            hauteur: 8000,
        };
        assert_eq!(marge_px(g, geante), 24);
        // Lot 3 : marges en fraction, plafonnées.
        let feu = parametres(EffetAnimation::Feu).expect("feu").marge;
        // 15 % de 800 = 120 (121 : arrondi supérieur de 120,00001 en f32).
        assert!((120..=121).contains(&marge_px(feu, t)));
        assert_eq!(marge_px(feu, geante), 160);
        let verre = parametres(EffetAnimation::VerreBrise).expect("verre").marge;
        assert_eq!(marge_px(verre, t), 200);
        assert_eq!(marge_px(verre, geante), 240);
        for e in [
            EffetAnimation::RouePixels,
            EffetAnimation::Fonte,
            EffetAnimation::Hexagones,
            EffetAnimation::Plane,
            EffetAnimation::Matrix,
        ] {
            assert_eq!(parametres(e).expect("shader").marge, Marge::Aucune, "{e:?}");
        }
    }

    #[test]
    fn marges_raisonnables() {
        for e in effets_shader() {
            match parametres(e).expect("shader").marge {
                Marge::Aucune => {}
                Marge::Fraction { part, max_px } => {
                    assert!(part > 0.0 && part <= 0.25, "{e:?}");
                    assert!((1..=240).contains(&max_px), "{e:?}");
                }
            }
        }
    }

    #[test]
    fn source_ps_ascii_et_point_entree() {
        for e in effets_shader() {
            let s = source_ps(e).expect("source");
            assert!(s.is_ascii(), "{e:?}");
            assert!(s.contains("float4 effet("), "{e:?}");
            assert!(s.contains("ps_effet"), "{e:?}");
            assert!(s.contains("cbuffer Constantes : register(b0)"), "{e:?}");
            // Un seul corps d'effet par source.
            assert_eq!(s.matches("float4 effet(").count(), 1, "{e:?}");
            assert!(!s.contains('\0'));
        }
        assert!(VS_HLSL.is_ascii());
        assert!(VS_HLSL.contains("SV_VertexID"));
    }

    fn equilibre(s: &str, ouvrant: char, fermant: char) -> bool {
        let mut n: i64 = 0;
        for c in s.chars() {
            if c == ouvrant {
                n += 1;
            } else if c == fermant {
                n -= 1;
                if n < 0 {
                    return false;
                }
            }
        }
        n == 0
    }

    #[test]
    fn hlsl_parentheses_accolades_equilibrees() {
        let mut sources: Vec<String> = effets_shader().filter_map(source_ps).collect();
        sources.push(VS_HLSL.to_string());
        for s in &sources {
            assert!(equilibre(s, '(', ')'), "parenthèses : {s}");
            assert!(equilibre(s, '{', '}'), "accolades : {s}");
            assert!(equilibre(s, '[', ']'), "crochets : {s}");
        }
    }

    #[test]
    fn hlsl_sans_sample_non_level() {
        for e in effets_shader() {
            let s = source_ps(e).expect("source");
            for (i, _) in s.match_indices(".Sample") {
                assert!(
                    s[i..].starts_with(".SampleLevel("),
                    "{e:?} : .Sample sans Level"
                );
            }
            assert!(!s.contains("ddx") && !s.contains("ddy"), "{e:?} : gradient");
            assert!(!s.contains("tex2D"), "{e:?}");
        }
    }

    /// Découpe une source HLSL en jetons (identifiants, nombres, symboles),
    /// commentaires `//` retirés.
    fn jetons(source: &str) -> Vec<String> {
        let mut sortie = Vec::new();
        for ligne in source.lines() {
            let ligne = ligne.split("//").next().unwrap_or("");
            let car: Vec<char> = ligne.chars().collect();
            let mut i = 0;
            while i < car.len() {
                let c = car[i];
                if c.is_ascii_alphabetic() || c == '_' {
                    let debut = i;
                    while i < car.len() && (car[i].is_ascii_alphanumeric() || car[i] == '_') {
                        i += 1;
                    }
                    sortie.push(car[debut..i].iter().collect());
                } else if c.is_ascii_digit() {
                    let debut = i;
                    while i < car.len() && (car[i].is_ascii_digit() || car[i] == '.') {
                        i += 1;
                    }
                    sortie.push(car[debut..i].iter().collect());
                } else if c.is_whitespace() {
                    i += 1;
                } else if i + 1 < car.len()
                    && matches!((c, car[i + 1]), ('<', '=') | ('+', '+') | ('-', '-'))
                {
                    sortie.push(car[i..i + 2].iter().collect());
                    i += 2;
                } else {
                    sortie.push(c.to_string());
                    i += 1;
                }
            }
        }
        sortie
    }

    const TYPES_HLSL: [&str; 7] = ["float", "float2", "float3", "float4", "int", "uint", "bool"];

    /// Noms déclarés (variables, paramètres, fonctions) : identifiant qui
    /// suit un nom de type (les transtypages `(float)x` et les
    /// constructeurs `float2(` ne sont pas des déclarations).
    fn declarations(source: &str) -> Vec<String> {
        let j = jetons(source);
        let mut noms = Vec::new();
        for w in j.windows(2) {
            let suivant = &w[1];
            if TYPES_HLSL.contains(&w[0].as_str())
                && suivant
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                noms.push(suivant.clone());
            }
        }
        noms
    }

    /// Mots réservés HLSL et intrinsèques qu'un corps ne doit pas redéclarer.
    const RESERVES_HLSL: &[&str] = &[
        "point", "line", "triangle", "lineadj", "triangleadj", "sample", "linear", "centroid",
        "precise", "shared", "vector", "matrix", "texture", "sampler", "pass", "technique",
        "compile", "string", "half", "double", "in", "out", "inout", "uniform", "static", "const",
        "register", "packoffset", "snorm", "unorm", "row_major", "column_major",
        "nointerpolation", "noperspective", "groupshared", "export", "extern", "volatile",
        "inline", "asm", "decl", "dword", "discard", "true", "false", "do", "switch", "case",
        "default", "return", "break", "continue", "if", "else", "while", "for", "struct",
        "typedef", "abs", "step", "length", "distance", "lerp", "frac", "floor", "ceil", "sin",
        "cos", "tan", "exp", "exp2", "log", "log2", "pow", "dot", "sqrt", "rsqrt", "saturate",
        "smoothstep", "min", "max", "clamp", "noise", "fmod", "sign", "atan", "atan2",
        "normalize", "mul", "cross", "reflect", "round", "trunc", "rcp", "any", "all",
    ];

    #[test]
    fn hlsl_garde_des_corps() {
        let signature = "float4 effet(float2 uv, float2 px)";
        // Noms globaux du prélude : champs du cbuffer et fonctions (leurs
        // variables locales peuvent être réutilisées par les corps).
        let debut_cb = PRELUDE_HLSL.find('{').expect("cbuffer");
        let fin_cb = PRELUDE_HLSL.find("};").expect("fin du cbuffer");
        let jp = jetons(PRELUDE_HLSL);
        let fonctions_prelude = jp
            .windows(3)
            .filter(|w| TYPES_HLSL.contains(&w[0].as_str()) && w[2] == "(")
            .map(|w| w[1].clone());
        let prelude: Vec<String> = declarations(&PRELUDE_HLSL[debut_cb..fin_cb])
            .into_iter()
            .chain(fonctions_prelude)
            .chain(["image", "ech", "Constantes", "ps_effet"].map(String::from))
            .collect();
        assert!(!prelude.iter().any(|n| n == "p" || n == "q"), "{prelude:?}");
        // Le prélude déclare bien ses utilitaires et ses constantes.
        for attendu in ["lire", "h21", "bruit", "fbm", "remap", "masque_coins", "taille_image"] {
            assert!(prelude.iter().any(|n| n == attendu), "{attendu}");
        }
        for e in effets_shader() {
            let corps = parametres(e).expect("shader").corps;
            assert_eq!(corps.matches(signature).count(), 1, "{e:?} : signature");
            assert!(!corps.contains(".Sample("), "{e:?} : .Sample(");
            assert!(!corps.contains("tex2D") && !corps.contains("ddx") && !corps.contains("ddy"));
            for nom in declarations(corps) {
                assert!(!prelude.contains(&nom), "{e:?} : `{nom}` redéclaré (prélude)");
                assert!(!RESERVES_HLSL.contains(&nom.as_str()), "{e:?} : `{nom}` réservé");
            }
            // Fonctions du corps : déclarées une seule fois.
            let j = jetons(corps);
            let mut fonctions = Vec::new();
            for w in j.windows(3) {
                if TYPES_HLSL.contains(&w[0].as_str()) && w[2] == "(" && w[1] != "(" {
                    let n = &w[1];
                    if n.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        fonctions.push(n.clone());
                    }
                }
            }
            for (i, f) in fonctions.iter().enumerate() {
                assert!(!fonctions[i + 1..].contains(f), "{e:?} : fonction {f} dupliquée");
            }
            // Boucles : `[unroll] for (int v = N; v < M (ou <=); v++)`.
            for (i, t) in j.iter().enumerate() {
                if t != "for" {
                    continue;
                }
                assert!(
                    i >= 3 && j[i - 3] == "[" && j[i - 2] == "unroll" && j[i - 1] == "]",
                    "{e:?} : for sans [unroll]"
                );
                let entier = |s: &str| s.parse::<i32>().is_ok();
                let v = &j[i + 3];
                let mut k = i + 5; // après « ( int v = »
                if j[k] == "-" {
                    k += 1;
                }
                assert!(
                    j[i + 1] == "(" && j[i + 2] == "int" && j[i + 4] == "=" && entier(&j[k]),
                    "{e:?} : initialisation de boucle"
                );
                k += 1;
                assert!(j[k] == ";" && &j[k + 1] == v, "{e:?} : condition de boucle");
                assert!(j[k + 2] == "<" || j[k + 2] == "<=", "{e:?} : comparaison");
                k += 3;
                if j[k] == "-" {
                    k += 1;
                }
                assert!(entier(&j[k]), "{e:?} : borne non littérale");
                assert!(
                    j[k + 1] == ";" && &j[k + 2] == v && j[k + 3] == "++" && j[k + 4] == ")",
                    "{e:?} : incrément de boucle"
                );
            }
        }
    }

    #[test]
    fn hlsl_garde_detecte_les_fautes() {
        // Le découpage reconnaît déclarations et boucles.
        let d = declarations("float a = 1.0; float2 b = float2(a, (float)k); int c;");
        assert_eq!(d, vec!["a", "b", "c"]);
        let j = jetons("[unroll] for (int y = -1; y <= 1; y++)");
        assert_eq!(&j[..7], &["[", "unroll", "]", "for", "(", "int", "y"]);
        assert!(j.contains(&"<=".to_string()) && j.contains(&"++".to_string()));
    }

    #[test]
    fn noms_sources_uniques() {
        let noms: Vec<&str> = effets_shader()
            .filter_map(parametres)
            .map(|p| p.nom_source)
            .collect();
        for (i, a) in noms.iter().enumerate() {
            assert!(a.is_ascii() && a.ends_with(".hlsl"));
            for b in &noms[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn cbuffer_dans_l_ordre_de_la_structure() {
        // Ordre des champs dans le prélude HLSL = ordre de ConstantesEffet.
        let ordre = [
            "float2 taille_cible;",
            "float2 taille_image;",
            "float2 marge;",
            "float progression;",
            "float graine;",
            "float rayon;",
            "float sens;",
            "float opacite;",
            "float reserve;",
        ];
        let mut depuis = 0;
        for champ in ordre {
            let pos = PRELUDE_HLSL[depuis..]
                .find(champ)
                .unwrap_or_else(|| panic!("{champ} absent ou hors d'ordre"));
            depuis += pos + champ.len();
        }
    }
}
