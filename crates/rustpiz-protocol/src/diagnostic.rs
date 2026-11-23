//! Diagnostic de l'abonnement à l'arbre visuel XAML et des modules
//! concurrents chargés dans explorer (ajout RustPiz, pur et testable hors de
//! Windows ; partagé par l'hôte et la DLL).
//!
//! Contexte (retour utilisateur v0.6.0, build 26300) : `InitializeXaml
//! DiagnosticsEx` réussit, la DLL est chargée, mais `AdviseVisualTreeChange`
//! renvoie `E_UNEXPECTED` (0x8000FFFF) à chaque session : aucun élément de
//! l'arbre n'est jamais reçu, le thème n'a aucun effet. Les diagnostics XAML
//! n'acceptent qu'un seul consommateur par processus : la cause la plus
//! probable est un autre abonné déjà présent dans cet explorer (RustHawk,
//! Windhawk, TranslucentTB…). Ce module fournit le calendrier des nouvelles
//! tentatives, l'interprétation des codes d'échec et le classement des
//! modules suspects.

use serde::{Deserialize, Serialize};

/// `E_UNEXPECTED` : renvoyé par `AdviseVisualTreeChange` sur le build 26300.
pub const E_UNEXPECTED: i32 = 0x8000_FFFF_u32 as i32;

/// Délais (ms) avant chaque tentative d'`AdviseVisualTreeChange` : la 1re
/// est immédiate (comme RustHawk), puis 200, 500 et 1000 ms.
pub const DELAIS_ADVISE_MS: [u64; 4] = [0, 200, 500, 1000];

/// Délai (ms) avant la tentative `numero` (1 = première), `None` au-delà de
/// la dernière.
pub fn delai_avant_tentative(numero: usize) -> Option<u64> {
    numero
        .checked_sub(1)
        .and_then(|indice| DELAIS_ADVISE_MS.get(indice).copied())
}

/// Faut-il faire une nouvelle tentative après l'échec de la tentative
/// `numero` ? Jamais si la session a changé ou a été arrêtée entre-temps.
pub fn reessayer_advise(numero: usize, session_inchangee: bool) -> bool {
    session_inchangee && delai_avant_tentative(numero + 1).is_some()
}

/// La dernière tentative est faite depuis un fil explicitement initialisé
/// en MTA (`CoInitializeEx(COINIT_MULTITHREADED)`) ; les précédentes depuis
/// un fil sans initialisation COM, exactement comme RustHawk.
pub fn tentative_en_mta_explicite(numero: usize) -> bool {
    numero == DELAIS_ADVISE_MS.len()
}

/// Issue de l'abonnement à l'arbre visuel, transmise à l'hôte dans le
/// rapport d'état (`StatusReport::advise`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdviseStatus {
    /// Toutes les tentatives sont terminées (réussite ou abandon).
    pub termine: bool,
    /// `AdviseVisualTreeChange` a réussi.
    pub reussi: bool,
    /// Nombre de tentatives faites.
    pub tentatives: u32,
    /// Dernier HRESULT (0 si réussi ou inconnu).
    pub hresult: i32,
    /// Description de la dernière tentative (journal).
    pub message: String,
}

/// Interprétation d'un HRESULT d'`AdviseVisualTreeChange` pour l'utilisateur.
///
/// Formulation volontairement prudente : aucune cause n'est prouvée par le
/// seul HRESULT (voir aussi [`mention_suspects_forts`]).
pub fn cause_probable_advise(hresult: i32) -> &'static str {
    match hresult as u32 {
        0x8000_FFFF => {
            "E_UNEXPECTED : peut venir d'un autre abonné aux diagnostics XAML déjà présent dans \
             cet explorer (RustHawk, Windhawk, TranslucentTB… : un seul consommateur par \
             processus) ou d'un état interne de XAML. Si un tel logiciel est actif, désactivez-le ; \
             redémarrez l'Explorateur, puis réactivez la barre dans RustPiz"
        }
        0x8000_4002 => "E_NOINTERFACE : interface de rappel refusée par XAML (DLL incompatible ?)",
        0x8007_0005 => "accès refusé par XAML",
        0x8001_0106 => "RPC_E_CHANGED_MODE : conflit d'apartement COM",
        0x8000_4005 => "E_FAIL : échec interne de XAML",
        _ => "cause inconnue",
    }
}

/// Gravité d'un module suspect trouvé dans explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gravite {
    /// Consommateur connu des diagnostics XAML, reconnu au NOM de son module
    /// chargé dans explorer : incompatible avec RustPiz (bloquant).
    Conflit,
    /// Indice fort d'un autre abonné à l'arbre visuel (DLL runtime de
    /// RustHawk chargée) : non bloquant avant l'essai, cité si Advise échoue.
    SuspectFort,
    /// Logiciel qui modifie la barre ou explorer : peut gêner, journalisé.
    Suspect,
    /// Copie de la DLL RustPiz (cette version ou une autre) : informatif.
    RustPiz,
}

/// Module d'explorer reconnu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSuspect {
    pub gravite: Gravite,
    /// Logiciel reconnu.
    pub logiciel: &'static str,
    /// Nom du module tel que listé.
    pub module: String,
}

/// Motifs (en minuscules) : (fragment, logiciel, gravité si le fragment est
/// dans le NOM du fichier du module). Un fragment trouvé seulement dans le
/// chemin (dossier) ne donne jamais plus que [`Gravite::Suspect`] : un
/// dossier « Windhawk » ou « TranslucentTB » ne prouve pas que le module
/// est un consommateur des diagnostics XAML.
///
/// L'ordre compte : le motif le plus précis d'abord (`rusthawk_runtime`
/// avant `rusthawk`).
const MOTIFS: &[(&str, &str, Gravite)] = &[
    ("rusthawk_runtime", "RustHawk", Gravite::SuspectFort),
    ("rusthawk", "RustHawk", Gravite::Suspect),
    ("windhawk", "Windhawk", Gravite::Conflit),
    ("explorertap", "TranslucentTB (ExplorerTAP)", Gravite::Conflit),
    ("translucenttb", "TranslucentTB", Gravite::Conflit),
    ("explorerblurmica", "ExplorerBlurMica", Gravite::Conflit),
    ("startallback", "StartAllBack", Gravite::Suspect),
    ("explorerpatcher", "ExplorerPatcher", Gravite::Suspect),
    ("ep_taskbar", "ExplorerPatcher", Gravite::Suspect),
    ("roundedtb", "RoundedTB", Gravite::Suspect),
    ("micaforeveryone", "Mica For Everyone", Gravite::Suspect),
    ("taskbarx", "TaskbarX", Gravite::Suspect),
    ("seelen", "Seelen UI", Gravite::Suspect),
    ("startisback", "StartIsBack", Gravite::Suspect),
    ("taskbartweaker", "7+ Taskbar Tweaker", Gravite::Suspect),
    ("xamldiagnostics", "XAML Diagnostics (Visual Studio ?)", Gravite::Suspect),
];

/// Nom de fichier d'un module : `nom` s'il est renseigné, sinon la dernière
/// composante de `chemin` (séparateurs `\` et `/`).
fn nom_de_fichier<'a>(nom: &'a str, chemin: &'a str) -> &'a str {
    if !nom.is_empty() {
        return nom;
    }
    chemin.rsplit(['\\', '/']).next().unwrap_or(chemin)
}

/// Classe un module chargé dans explorer (`nom` : nom court, `chemin` :
/// chemin complet). `None` si le module n'est pas suspect.
pub fn classer_module(nom: &str, chemin: &str) -> Option<ModuleSuspect> {
    let fichier = nom_de_fichier(nom, chemin);
    let nom_min = fichier.to_lowercase();
    let chemin_min = chemin.to_lowercase();
    if nom_min.starts_with("rustpiz_runtime") {
        return Some(ModuleSuspect {
            gravite: Gravite::RustPiz,
            logiciel: "RustPiz",
            module: fichier.to_owned(),
        });
    }
    let par_nom = MOTIFS
        .iter()
        .find(|(motif, _, _)| nom_min.contains(motif))
        .map(|(_, logiciel, gravite)| (*logiciel, *gravite));
    let trouve = par_nom.or_else(|| {
        MOTIFS
            .iter()
            .find(|(motif, _, _)| chemin_min.contains(motif))
            .map(|(_, logiciel, _)| (*logiciel, Gravite::Suspect))
    });
    trouve.map(|(logiciel, gravite)| ModuleSuspect {
        gravite,
        logiciel,
        module: fichier.to_owned(),
    })
}

/// Le logiciel `logiciel` (préfixe du nom affiché) a-t-il un module en
/// [`Gravite::Conflit`] dans la liste ?
pub fn conflit_charge(suspects: &[ModuleSuspect], logiciel: &str) -> bool {
    suspects
        .iter()
        .any(|s| s.gravite == Gravite::Conflit && s.logiciel.starts_with(logiciel))
}

/// Phrase à ajouter au message d'échec d'Advise si des suspects forts sont
/// chargés dans explorer (`None` sinon).
pub fn mention_suspects_forts(suspects: &[ModuleSuspect]) -> Option<String> {
    let forts: Vec<&str> = suspects
        .iter()
        .filter(|s| s.gravite == Gravite::SuspectFort)
        .take(5)
        .map(|s| s.module.as_str())
        .collect();
    if forts.is_empty() {
        return None;
    }
    Some(format!(
        "suspect FORT : {} chargée(s) dans cet explorer (DLL runtime de RustHawk : même inactive, \
         elle a pu rester abonnée à l'arbre visuel) — redémarrez l'Explorateur sans RustHawk",
        forts.join(", ")
    ))
}

/// Classe une liste de modules `(nom, chemin)` ; ordre conservé.
pub fn modules_suspects<'a>(
    modules: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<ModuleSuspect> {
    modules
        .into_iter()
        .filter_map(|(nom, chemin)| classer_module(nom, chemin))
        .collect()
}

/// Ligne de journal décrivant des modules suspects (bornée à 20 entrées).
pub fn decrire_suspects(suspects: &[ModuleSuspect]) -> String {
    if suspects.is_empty() {
        return "aucun".to_owned();
    }
    const MAX: usize = 20;
    let mut texte = suspects
        .iter()
        .take(MAX)
        .map(|s| {
            let gravite = match s.gravite {
                Gravite::Conflit => "CONFLIT",
                Gravite::SuspectFort => "SUSPECT FORT",
                Gravite::Suspect => "suspect",
                Gravite::RustPiz => "RustPiz",
            };
            format!("{} [{} : {gravite}]", s.module, s.logiciel)
        })
        .collect::<Vec<_>>()
        .join(", ");
    if suspects.len() > MAX {
        texte.push_str(&format!(", … (et {} autre(s))", suspects.len() - MAX));
    }
    texte
}

/// Nom d'un type d'apartement COM (`APTTYPE`) pour le journal.
pub fn nom_apartement(apttype: i32) -> &'static str {
    match apttype {
        -1 => "courant",
        0 => "STA",
        1 => "MTA",
        2 => "NA (neutre)",
        3 => "STA principal",
        _ => "inconnu",
    }
}

/// Nom d'un qualificatif d'apartement (`APTTYPEQUALIFIER`) pour le journal.
pub fn nom_qualificatif(qualificatif: i32) -> &'static str {
    match qualificatif {
        0 => "aucun",
        1 => "MTA implicite",
        2 => "NA sur MTA",
        3 => "NA sur STA",
        4 => "NA sur MTA implicite",
        5 => "NA sur STA principal",
        6 => "STA d'application",
        7 => "STA réservé",
        _ => "inconnu",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendrier_des_tentatives() {
        assert_eq!(delai_avant_tentative(0), None);
        assert_eq!(delai_avant_tentative(1), Some(0));
        assert_eq!(delai_avant_tentative(2), Some(200));
        assert_eq!(delai_avant_tentative(3), Some(500));
        assert_eq!(delai_avant_tentative(4), Some(1000));
        assert_eq!(delai_avant_tentative(5), None);
        assert!(reessayer_advise(1, true));
        assert!(reessayer_advise(3, true));
        assert!(!reessayer_advise(4, true));
        assert!(!reessayer_advise(1, false));
        assert!(!tentative_en_mta_explicite(1));
        assert!(tentative_en_mta_explicite(4));
        // Attente totale bornée (< 2 s : le rapport est lu 2 s après l'injection).
        assert!(DELAIS_ADVISE_MS.iter().sum::<u64>() < 2000);
    }

    #[test]
    fn causes_connues() {
        assert!(cause_probable_advise(E_UNEXPECTED).contains("un seul"));
        assert!(cause_probable_advise(E_UNEXPECTED).contains("peut venir"));
        assert!(cause_probable_advise(0x8000_4002_u32 as i32).contains("E_NOINTERFACE"));
        assert_eq!(cause_probable_advise(0x1234), "cause inconnue");
    }

    #[test]
    fn classement_des_modules() {
        let c = classer_module(
            "rusthawk_runtime-00ff.dll",
            r"C:\Users\u\AppData\Local\RustHawk\runtime\rusthawk_runtime-00ff.dll",
        )
        .unwrap();
        assert_eq!(c.gravite, Gravite::SuspectFort);
        assert_eq!(c.logiciel, "RustHawk");
        assert!(mention_suspects_forts(&[c.clone()])
            .unwrap()
            .contains("rusthawk_runtime-00ff.dll"));
        // Autre module de RustHawk (ni la DLL runtime) : simple suspect.
        let h = classer_module("rusthawk_ui.dll", "x").unwrap();
        assert_eq!(h.gravite, Gravite::Suspect);
        assert!(mention_suspects_forts(&[h]).is_none());
        let t = classer_module("ExplorerTAP.dll", r"C:\Program Files\WindowsApps\x\ExplorerTAP.dll")
            .unwrap();
        assert_eq!(t.gravite, Gravite::Conflit);
        // Le chemin seul ne fait qu'un suspect (le NOM fait le conflit).
        let w = classer_module("mod.dll", r"C:\ProgramData\Windhawk\Engine\mod.dll").unwrap();
        assert_eq!(w.logiciel, "Windhawk");
        assert_eq!(w.gravite, Gravite::Suspect);
        let w = classer_module("windhawk.dll", r"C:\Program Files\Windhawk\windhawk.dll").unwrap();
        assert_eq!(w.gravite, Gravite::Conflit);
        assert!(conflit_charge(&[w], "Windhawk"));
        let b = classer_module("ExplorerBlurMica.dll", "x").unwrap();
        assert_eq!(b.gravite, Gravite::Conflit);
        // Dossier TranslucentTB, module sans rapport : pas de conflit.
        let t2 = classer_module("helper.dll", r"C:\Apps\TranslucentTB\helper.dll").unwrap();
        assert_eq!(t2.gravite, Gravite::Suspect);
        assert!(!conflit_charge(&[t2], "TranslucentTB"));
        // Nom absent : nom de fichier tiré du chemin.
        let n = classer_module("", r"C:\x\ExplorerTAP.dll").unwrap();
        assert_eq!((n.gravite, n.module.as_str()), (Gravite::Conflit, "ExplorerTAP.dll"));
        let r = classer_module("rustpiz_runtime-0123456789abcdef.dll", "x").unwrap();
        assert_eq!(r.gravite, Gravite::RustPiz);
        assert_eq!(
            classer_module("StartAllBackX64.dll", "x").map(|s| s.gravite),
            Some(Gravite::Suspect)
        );
        assert!(classer_module("Taskbar.View.dll", r"C:\Windows\SystemApps\x\Taskbar.View.dll")
            .is_none());
        assert!(classer_module("ntdll.dll", r"C:\Windows\SYSTEM32\ntdll.dll").is_none());
    }

    #[test]
    fn description_bornee() {
        assert_eq!(decrire_suspects(&[]), "aucun");
        let liste: Vec<(String, String)> = (0..25)
            .map(|i| (format!("windhawk{i}.dll"), String::new()))
            .collect();
        let suspects =
            modules_suspects(liste.iter().map(|(n, c)| (n.as_str(), c.as_str())));
        assert_eq!(suspects.len(), 25);
        let texte = decrire_suspects(&suspects);
        assert!(texte.contains("CONFLIT"));
        assert!(texte.contains("et 5 autre(s)"));
    }

    #[test]
    fn noms_d_apartement() {
        assert_eq!(nom_apartement(1), "MTA");
        assert_eq!(nom_apartement(0), "STA");
        assert_eq!(nom_qualificatif(1), "MTA implicite");
        assert_eq!(nom_apartement(42), "inconnu");
    }

    #[test]
    fn statut_serde() {
        let s = AdviseStatus {
            termine: true,
            reussi: false,
            tentatives: 4,
            hresult: E_UNEXPECTED,
            message: "x".to_owned(),
        };
        let texte = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<AdviseStatus>(&texte).unwrap(), s);
    }
}
