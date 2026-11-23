//! Bilan d'une application de thème (pur, testable hors de Windows).
//!
//! Le runtime répond à `Status` par un [`StatusReport`] (compteurs par règle,
//! fils XAML, disjoncteur, miniatures). Ce module le transforme en lignes de
//! journal lisibles et en verdicts : thème réellement appliqué ou non,
//! session périmée (une ancienne session répond encore), règles en échec.
//!
//! Avant la v0.6.1, l'hôte comptait l'application comme réussie dès que le
//! runtime répondait sans disjoncteur, même si aucun élément n'avait été
//! stylé : ces cas sont désormais détectés et journalisés.

use rustpiz_protocol::diagnostic::cause_probable_advise;
use rustpiz_protocol::StatusReport;

/// Noms de règles cités au plus dans une ligne de journal.
pub const MAX_REGLES_CITEES: usize = 12;
/// Longueur maximale d'une cible (sélecteur) ou d'un message cité.
pub const MAX_CARACTERES: usize = 160;

/// Ce que l'hôte a demandé à la session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendu {
    /// Nom du thème compilé (`CompiledTheme::name`), vide si aucun thème.
    pub nom_theme: String,
    /// Nombre de règles compilées envoyées dans `active.json`.
    pub regles: usize,
    /// Miniatures demandées (hook résolu et envoyé).
    pub miniatures: bool,
}

/// Résultat de l'examen d'un rapport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bilan {
    /// Ligne de synthèse.
    pub resume: String,
    /// Détails (règles sans correspondance, règles en échec).
    pub details: Vec<String>,
    /// Problèmes à journaliser en avertissement.
    pub avertissements: Vec<String>,
    /// Un thème était demandé mais rien n'a été stylé.
    pub theme_sans_effet: bool,
    /// La session qui répond applique un autre thème que celui demandé.
    pub session_perimee: bool,
    /// Au moins une règle a échoué (valeur, propriété, écriture).
    pub regles_en_echec: bool,
    /// `AdviseVisualTreeChange` a définitivement échoué (thème demandé) :
    /// message destiné à l'utilisateur (HRESULT et cause probable).
    pub advise_echoue: Option<String>,
}

fn couper(texte: &str) -> String {
    if texte.chars().count() <= MAX_CARACTERES {
        texte.to_owned()
    } else {
        let mut court: String = texte.chars().take(MAX_CARACTERES).collect();
        court.push('…');
        court
    }
}

/// Liste bornée de noms : « a ; b ; … (et N autres) ».
fn liste_bornee<'a>(noms: impl Iterator<Item = &'a str>) -> String {
    let noms: Vec<&str> = noms.collect();
    let mut texte = noms
        .iter()
        .take(MAX_REGLES_CITEES)
        .map(|n| couper(n))
        .collect::<Vec<_>>()
        .join(" ; ");
    if noms.len() > MAX_REGLES_CITEES {
        texte.push_str(&format!(" ; … (et {} autre(s))", noms.len() - MAX_REGLES_CITEES));
    }
    texte
}

/// Examine un rapport d'état au regard de ce qui a été demandé.
pub fn evaluer(rapport: &StatusReport, attendu: &Attendu) -> Bilan {
    let mut bilan = Bilan::default();
    let appliquees: u32 = rapport.rules.iter().map(|r| r.applied).sum();
    let echecs: u32 = rapport.rules.iter().map(|r| r.failed).sum();
    let avec = rapport.rules.iter().filter(|r| r.matched > 0).count();
    let sans: Vec<&str> = rapport
        .unmatched()
        .map(|r| r.target.as_str())
        .collect();
    let theme_demande = attendu.regles > 0;

    let nom_rapporte = if rapport.theme.is_empty() {
        "(non indiqué par la DLL)".to_owned()
    } else {
        format!("« {} »", rapport.theme)
    };
    let miniatures = match (rapport.thumbnails.requested, rapport.thumbnails.active) {
        (true, true) => format!(
            "hooks actifs{}",
            rapport
                .thumbnails
                .activity
                .as_deref()
                .map(|a| format!(" ({})", couper(a)))
                .unwrap_or_default()
        ),
        (true, false) => format!(
            "INACTIVES ({})",
            couper(rapport.thumbnails.message.as_deref().unwrap_or("raison inconnue"))
        ),
        (false, _) => "non demandées".to_owned(),
    };
    let advise = match &rapport.advise {
        None => "inconnu (DLL antérieure)".to_owned(),
        Some(a) if a.reussi => format!("réussi (tentative {})", a.tentatives),
        Some(a) if !a.termine => format!("en cours ({} tentative(s) faite(s))", a.tentatives),
        Some(a) => format!(
            "ÉCHEC {:#010X} après {} tentative(s)",
            a.hresult as u32, a.tentatives
        ),
    };
    bilan.resume = format!(
        "thème {nom_rapporte} (session n° {}) : abonnement à l'arbre visuel {advise} ; {} règle(s) compilée(s) envoyée(s), {} dans le \
         rapport, {avec} avec correspondance, {} sans ; {} élément(s) stylé(s) ; {appliquees} \
         valeur(s) appliquée(s), {echecs} échec(s) de règle, {} échec(s) d'écriture ; fils XAML \
         {} ({} injoignable(s)) ; disjoncteur {} ; miniatures {miniatures}",
        rapport.session,
        attendu.regles,
        rapport.rules.len(),
        sans.len(),
        rapport.tracked_elements,
        rapport.write_failures,
        rapport.ui_threads,
        rapport.unresponsive_threads,
        if rapport.tripped { "DÉCLENCHÉ" } else { "non" },
    );

    if !sans.is_empty() && theme_demande {
        bilan.details.push(format!(
            "règle(s) sans élément correspondant (normal pour un élément pas encore affiché, \
             sinon sélecteur obsolète) : {}",
            liste_bornee(sans.iter().copied())
        ));
    }
    let en_echec: Vec<String> = rapport
        .rules
        .iter()
        .filter(|r| r.failed > 0)
        .map(|r| {
            format!(
                "« {} » : {} échec(s), dernier : {}",
                couper(&r.target),
                r.failed,
                couper(r.last_error.as_deref().unwrap_or("(inconnu)"))
            )
        })
        .collect();
    if !en_echec.is_empty() {
        bilan.regles_en_echec = true;
        let mut texte = en_echec
            .iter()
            .take(MAX_REGLES_CITEES)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        if en_echec.len() > MAX_REGLES_CITEES {
            texte.push_str(&format!(" | … (et {} autre(s))", en_echec.len() - MAX_REGLES_CITEES));
        }
        bilan
            .avertissements
            .push(format!("{} règle(s) en échec : {texte}", en_echec.len()));
    }

    if !rapport.theme.is_empty() && rapport.theme != attendu.nom_theme {
        bilan.session_perimee = true;
        bilan.avertissements.push(format!(
            "la session qui répond applique le thème « {} » au lieu de « {} » : une ancienne \
             session n'a pas été arrêtée (voir runtime.log : « déjà active »)",
            rapport.theme,
            if attendu.nom_theme.is_empty() { "(aucun)" } else { &attendu.nom_theme }
        ));
    } else if theme_demande && rapport.rules.len() != attendu.regles {
        bilan.avertissements.push(format!(
            "le rapport compte {} règle(s) alors que {} ont été envoyées (active.json relu \
             d'une autre version ?)",
            rapport.rules.len(),
            attendu.regles
        ));
    }

    let echec_advise = rapport
        .advise
        .as_ref()
        .filter(|a| a.termine && !a.reussi);
    if let (true, Some(a)) = (theme_demande, echec_advise) {
        bilan.theme_sans_effet = true;
        let message = format!(
            "AdviseVisualTreeChange a échoué ({:#010X}, {} tentative(s)) : {}",
            a.hresult as u32,
            a.tentatives,
            cause_probable_advise(a.hresult)
        );
        bilan
            .avertissements
            .push(format!("{message} — thème NON appliqué (détail : {})", couper(&a.message)));
        bilan.advise_echoue = Some(message);
    } else if theme_demande {
        if rapport.ui_threads == 0 {
            bilan.theme_sans_effet = true;
            bilan.avertissements.push(
                "aucun fil XAML n'a rejoint la session : l'arbre visuel n'a pas (encore) été \
                 rejoué (AdviseVisualTreeChange en échec ou lent, voir runtime.log) — thème NON \
                 appliqué"
                    .to_owned(),
            );
        } else if rapport.tracked_elements == 0 || avec == 0 {
            bilan.theme_sans_effet = true;
            bilan.avertissements.push(
                "0 élément stylé : aucune règle ne correspond à la barre des tâches de ce build \
                 (sélecteurs obsolètes ?) — thème SANS EFFET"
                    .to_owned(),
            );
        } else if appliquees == 0 {
            bilan.theme_sans_effet = true;
            bilan.avertissements.push(
                "des éléments correspondent mais AUCUNE valeur n'a été appliquée (échecs XAML, \
                 voir les règles en échec et runtime.log) — thème SANS EFFET"
                    .to_owned(),
            );
        }
    }
    if rapport.tripped {
        bilan.avertissements.push(
            "disjoncteur déclenché : trop d'échecs d'écriture, l'état d'origine a été restauré"
                .to_owned(),
        );
    }
    if rapport.unresponsive_threads > 0 {
        bilan.avertissements.push(format!(
            "{} fil(s) XAML n'ont pas répondu au rapport",
            rapport.unresponsive_threads
        ));
    }
    if attendu.miniatures && !rapport.thumbnails.active {
        bilan.avertissements.push(format!(
            "miniatures demandées mais hooks inactifs : {}",
            couper(rapport.thumbnails.message.as_deref().unwrap_or("raison inconnue"))
        ));
    }
    bilan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustpiz_protocol::ThumbnailStatus;
    use rustpiz_xaml::RuleStat;

    fn regle(cible: &str, matched: u32, applied: u32, failed: u32) -> RuleStat {
        RuleStat {
            source_index: 0,
            target: cible.to_owned(),
            matched,
            applied,
            failed,
            last_error: (failed > 0).then(|| "écriture de Fill : 80004005 Erreur".to_owned()),
        }
    }

    fn attendu(regles: usize) -> Attendu {
        Attendu {
            nom_theme: "Surface".to_owned(),
            regles,
            miniatures: false,
        }
    }

    fn rapport(regles: Vec<RuleStat>, elements: u32, fils: u32) -> StatusReport {
        StatusReport {
            ui_threads: fils,
            tracked_elements: elements,
            rules: regles,
            theme: "Surface".to_owned(),
            session: 2,
            ..StatusReport::default()
        }
    }

    #[test]
    fn theme_applique_sans_avertissement() {
        let r = rapport(vec![regle("A", 2, 6, 0), regle("B", 0, 0, 0)], 30, 1);
        let b = evaluer(&r, &attendu(2));
        assert!(!b.theme_sans_effet && !b.session_perimee && !b.regles_en_echec);
        assert!(b.avertissements.is_empty(), "{:?}", b.avertissements);
        assert!(b.resume.contains("1 avec correspondance"));
        assert!(b.resume.contains("30 élément(s) stylé(s)"));
        assert!(b.resume.contains("6 valeur(s) appliquée(s)"));
        assert_eq!(b.details.len(), 1);
        assert!(b.details[0].contains('B'));
    }

    #[test]
    fn aucun_fil_xaml_signale() {
        // Cas d'une DLL dont l'arbre n'a jamais été rejoué : les règles sont
        // recopiées de la configuration, toutes à 0.
        let r = rapport(vec![regle("A", 0, 0, 0)], 0, 0);
        let b = evaluer(&r, &attendu(1));
        assert!(b.theme_sans_effet);
        assert!(b.avertissements.iter().any(|a| a.contains("aucun fil XAML")));
    }

    #[test]
    fn zero_element_style_signale() {
        let r = rapport(vec![regle("A", 0, 0, 0), regle("B", 0, 0, 0)], 0, 1);
        let b = evaluer(&r, &attendu(2));
        assert!(b.theme_sans_effet);
        assert!(b.avertissements.iter().any(|a| a.contains("0 élément stylé")));
    }

    #[test]
    fn valeurs_toutes_en_echec_signalees() {
        let r = rapport(vec![regle("A", 3, 0, 3)], 3, 1);
        let b = evaluer(&r, &attendu(1));
        assert!(b.theme_sans_effet && b.regles_en_echec);
        assert!(b.avertissements.iter().any(|a| a.contains("80004005")));
        assert!(b.avertissements.iter().any(|a| a.contains("AUCUNE valeur")));
    }

    #[test]
    fn session_perimee_detectee() {
        let mut r = rapport(vec![regle("A", 1, 1, 0)], 1, 1);
        r.theme = "Plasma".to_owned();
        let b = evaluer(&r, &attendu(1));
        assert!(b.session_perimee);
        // Ancienne DLL (champ absent) : pas de faux positif.
        r.theme.clear();
        let b = evaluer(&r, &attendu(1));
        assert!(!b.session_perimee);
        assert!(b.resume.contains("non indiqué"));
    }

    #[test]
    fn miniatures_seules_sans_theme() {
        let mut r = rapport(vec![], 0, 1);
        r.theme.clear();
        r.thumbnails = ThumbnailStatus {
            requested: true,
            active: true,
            message: None,
            activity: Some("12 appels".to_owned()),
        };
        let a = Attendu {
            nom_theme: String::new(),
            regles: 0,
            miniatures: true,
        };
        let b = evaluer(&r, &a);
        assert!(!b.theme_sans_effet && !b.session_perimee);
        assert!(b.avertissements.is_empty(), "{:?}", b.avertissements);
        assert!(b.resume.contains("hooks actifs (12 appels)"));
        r.thumbnails.active = false;
        r.thumbnails.message = Some("signature PE différente".to_owned());
        let b = evaluer(&r, &a);
        assert!(b.avertissements.iter().any(|a| a.contains("signature PE")));
    }

    #[test]
    fn listes_bornees() {
        let regles: Vec<RuleStat> = (0..30).map(|i| regle(&format!("R{i}"), 0, 0, 1)).collect();
        let mut r = rapport(regles, 0, 1);
        r.rules[0].matched = 1;
        r.rules[0].applied = 1;
        r.tracked_elements = 1;
        let b = evaluer(&r, &attendu(30));
        assert!(b.details[0].contains("et 17 autre(s)"), "{}", b.details[0]);
        assert!(b.avertissements[0].contains("et 18 autre(s)"), "{}", b.avertissements[0]);
        let long = "x".repeat(500);
        assert!(couper(&long).chars().count() == MAX_CARACTERES + 1);
    }

    #[test]
    fn echec_d_advise_explique() {
        use rustpiz_protocol::diagnostic::{AdviseStatus, E_UNEXPECTED};
        let mut r = rapport(vec![regle("A", 0, 0, 0)], 0, 0);
        r.advise = Some(AdviseStatus {
            termine: true,
            reussi: false,
            tentatives: 4,
            hresult: E_UNEXPECTED,
            message: "tentative 4/4".to_owned(),
        });
        let b = evaluer(&r, &attendu(1));
        assert!(b.theme_sans_effet);
        let m = b.advise_echoue.as_deref().unwrap();
        assert!(m.contains("0x8000FFFF") && m.contains("un seul consommateur"), "{m}");
        assert!(b.resume.contains("ÉCHEC 0x8000FFFF après 4"), "{}", b.resume);
        // Le message générique « aucun fil XAML » est remplacé par la cause.
        assert!(!b.avertissements.iter().any(|a| a.contains("aucun fil XAML")));
        // Tentatives encore en cours : pas de verdict d'échec.
        r.advise.as_mut().unwrap().termine = false;
        let b = evaluer(&r, &attendu(1));
        assert!(b.advise_echoue.is_none());
        assert!(b.resume.contains("en cours"));
        // Miniatures seules (aucun thème) : l'échec n'est pas signalé.
        r.advise.as_mut().unwrap().termine = true;
        let a = Attendu {
            nom_theme: String::new(),
            regles: 0,
            miniatures: false,
        };
        r.theme.clear();
        assert!(evaluer(&r, &a).advise_echoue.is_none());
        // Réussite.
        r.advise = Some(AdviseStatus {
            termine: true,
            reussi: true,
            tentatives: 2,
            ..AdviseStatus::default()
        });
        assert!(evaluer(&r, &a).resume.contains("réussi (tentative 2)"));
    }

    #[test]
    fn disjoncteur_et_nombre_de_regles() {
        let mut r = rapport(vec![regle("A", 1, 1, 0)], 1, 1);
        r.tripped = true;
        let b = evaluer(&r, &attendu(4));
        assert!(b.avertissements.iter().any(|a| a.contains("disjoncteur")));
        assert!(b.avertissements.iter().any(|a| a.contains("4 ont été envoyées")));
        assert!(b.resume.contains("DÉCLENCHÉ"));
    }
}
