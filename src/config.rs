//! Lecture et création du fichier de configuration TOML.
//!
//! Emplacement : `%APPDATA%\RustPiz\config.toml`.
//! Si le fichier n'existe pas, il est créé avec des valeurs par défaut
//! commentées en français. Si le fichier est invalide, la configuration
//! par défaut est utilisée et l'erreur est journalisée.

use serde::Deserialize;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::animations::EffetAnimation;
use crate::explorer;
use crate::logging;

/// Configuration complète de l'application.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Opacité appliquée par défaut, en pourcentage (10 à 100).
    pub opacite_defaut: u8,
    /// Si vrai, la transparence est active dès le démarrage.
    pub activer_au_demarrage: bool,
    /// Si vrai, les fenêtres de l'Explorateur (explorer.exe, classe
    /// CabinetWClass) sont automatiquement rendues transparentes avec
    /// l'opacité de base — au démarrage et à l'ouverture de nouvelles
    /// fenêtres. Les profils utilisateur restent prioritaires.
    pub transparence_explorateur: bool,
    /// Effet d'animation d'ouverture / fermeture des fenêtres
    /// (`aucun`, `fondu`, `zoom`, `glissement`, `retrecissement`,
    /// `television`, `flou` ; effets à shader depuis la v0.4.7 :
    /// `pixellisation`, `balayage_pixels`, `glitch`).
    pub effet_fenetre: String,
    /// Si vrai, l'ouverture des fenêtres est animée.
    pub effet_ouverture: bool,
    /// Si vrai, la fermeture des fenêtres est animée.
    pub effet_fermeture: bool,
    /// Si vrai, l'effet « Jelly » déforme les fenêtres pendant leur
    /// déplacement (glisser de la barre de titre).
    pub jelly_actif: bool,
    /// Puissance de l'effet Jelly, en % (0 = aucun mouvement, 100 = très
    /// forte déformation).
    pub jelly_puissance: u8,
    /// Clé cachée (absente du modèle par défaut, v0.4.7) : si vrai, la
    /// transition native d'apparition de DWM est désactivée pendant une
    /// ouverture animée (`DWMWA_TRANSITIONS_FORCEDISABLED`, dès la création
    /// de la fenêtre), puis rétablie.
    pub desactiver_transition_dwm: bool,
    /// Raccourcis clavier globaux.
    pub raccourcis: Raccourcis,
    /// Noms de classes de fenêtres à ne jamais rendre transparentes.
    pub exclusions_classes: Vec<String>,
    /// Noms de processus (ex. `jeu.exe`) à ne jamais modifier.
    pub exclusions_processus: Vec<String>,
    /// Profils appliqués automatiquement aux nouvelles fenêtres.
    pub profils: Vec<Profil>,
}

/// Profil de transparence appliqué automatiquement.
///
/// Tous les critères non vides doivent correspondre (ET logique) :
/// processus, classe de fenêtre et/ou fragment de titre.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Profil {
    /// Nom lisible du profil (pour le journal).
    pub nom: String,
    /// Nom du processus, ex. `explorer.exe` (vide = critère ignoré).
    pub processus: String,
    /// Classe de fenêtre, ex. `CabinetWClass` (vide = critère ignoré).
    pub classe: String,
    /// Fragment du titre, insensible à la casse (vide = critère ignoré).
    pub titre_contient: String,
    /// Opacité à appliquer en % (absent = opacité inchangée).
    pub opacite: Option<u8>,
    /// Effet DWM : `aucun`, `mica`, `micaalt`, `acrylique` ou `verre`.
    pub effet_dwm: String,
}

impl Default for Profil {
    fn default() -> Self {
        Self {
            nom: String::new(),
            processus: String::new(),
            classe: String::new(),
            titre_contient: String::new(),
            opacite: None,
            effet_dwm: "aucun".to_string(),
        }
    }
}

/// Raccourcis clavier globaux, au format texte (ex. `Ctrl+Alt+T`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Raccourcis {
    /// Active / désactive la transparence de la fenêtre sous le curseur.
    pub bascule: String,
    /// Augmente l'opacité de la fenêtre sous le curseur.
    pub augmenter: String,
    /// Diminue l'opacité de la fenêtre sous le curseur.
    pub diminuer: String,
}

impl Default for Raccourcis {
    fn default() -> Self {
        Self {
            bascule: "Ctrl+Alt+T".to_string(),
            augmenter: "Ctrl+Alt+Haut".to_string(),
            diminuer: "Ctrl+Alt+Bas".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            opacite_defaut: 80,
            activer_au_demarrage: true,
            transparence_explorateur: true,
            effet_fenetre: "zoom".to_string(),
            effet_ouverture: false,
            effet_fermeture: false,
            jelly_actif: false,
            jelly_puissance: 50,
            desactiver_transition_dwm: true,
            raccourcis: Raccourcis::default(),
            exclusions_classes: explorer::classes_exclues_defaut(),
            exclusions_processus: explorer::processus_exclus_defaut(),
            profils: Vec::new(),
        }
    }
}

impl Config {
    /// Corrige les valeurs incohérentes (opacité hors limites, etc.).
    fn valider(&mut self) {
        if !(10..=100).contains(&self.opacite_defaut) {
            logging::avertir(&format!(
                "opacite_defaut={} hors limites, valeur 80 utilisée.",
                self.opacite_defaut
            ));
            self.opacite_defaut = 80;
        }
        if EffetAnimation::depuis_nom(&self.effet_fenetre).is_none() {
            logging::avertir(&format!(
                "effet_fenetre=\"{}\" inconnu, valeur \"aucun\" utilisée.",
                self.effet_fenetre
            ));
            self.effet_fenetre = "aucun".to_string();
        }
        if self.jelly_puissance > 100 {
            logging::avertir(&format!(
                "jelly_puissance={} hors limites, valeur 100 utilisée.",
                self.jelly_puissance
            ));
            self.jelly_puissance = 100;
        }
    }
}

/// Contenu du `config.toml` écrit à la première exécution.
const TOML_DEFAUT: &str = r#"# ==========================================================================
# RustPiz — configuration
# Emplacement : %APPDATA%\RustPiz\config.toml
# Après modification, utilisez « Recharger la configuration » dans le menu
# de l'icône de la barre des tâches (ou redémarrez l'application).
# ==========================================================================

# Opacité appliquée par défaut, en pourcentage (10 = très transparent,
# 100 = opaque).
opacite_defaut = 80

# Si vrai, la transparence est active dès le démarrage de l'application.
activer_au_demarrage = true

# Si vrai, les fenêtres de l'Explorateur Windows (explorer.exe) sont
# automatiquement rendues transparentes avec l'opacité de base :
# - au démarrage de RustPiz (fenêtres déjà ouvertes) ;
# - à l'ouverture de toute nouvelle fenêtre de l'Explorateur.
# Les profils [[profils]] définis plus bas restent prioritaires.
transparence_explorateur = true

# Animation d'ouverture / fermeture des fenêtres (rendue par le GPU).
# Effets : "aucun", "fondu", "zoom", "glissement", "retrecissement", "television", "flou",
# et effets à shader (GPU) : "pixellisation", "balayage_pixels", "glitch".
effet_fenetre = "zoom"
effet_ouverture = false
effet_fermeture = false

# Effet « Jelly » : la fenêtre se déforme comme une gelée pendant son
# déplacement (glisser de la barre de titre), puis rebondit en s'amortissant.
# Puissance en % : 0 (aucun mouvement), 25, 50, 75 ou 100 (très forte déformation).
jelly_actif = false
jelly_puissance = 50

[raccourcis]
# Format : Modificateurs+Touche. Modificateurs : Ctrl, Alt, Shift, Win.
# Touches : A-Z, 0-9, F1-F24, Haut, Bas, Gauche, Droite, Espace, Echap,
# Suppr, Insertion, Debut, Fin, PageHaut, PageBas, Tab.
# Les raccourcis agissent sur la fenêtre située sous le curseur de la souris.

# Active / désactive la transparence de la fenêtre sous le curseur.
bascule = "Ctrl+Alt+T"
# Augmente l'opacité de la fenêtre sous le curseur (+10 %).
augmenter = "Ctrl+Alt+Haut"
# Diminue l'opacité de la fenêtre sous le curseur (-10 %).
diminuer = "Ctrl+Alt+Bas"

# Noms de classes de fenêtres à ne jamais rendre transparentes
# (barre des tâches, menu Démarrer, bureau, écran de verrouillage...).
exclusions_classes = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "TopLevelWindowForOverflowXamlIsland",
    "DV2ControlHost",
    "NotifyIconOverflowWindow",
    "Progman",
    "WorkerW",
    "LockScreenFrame",
]

# Noms de processus à ne jamais modifier (minuscules, avec l'extension).
# Ajoutez ici vos jeux si le mode plein écran fenêtré pose problème.
exclusions_processus = [
    "lockapp.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "searchapp.exe",
    "shellexperiencehost.exe",
    "shellinfrastructurehost.exe",
    "sihost.exe",
    "textinputhost.exe",
]

# ==========================================================================
# Profils automatiques (appliqués aux nouvelles fenêtres, toutes les 2 s).
# Critères (ET logique) : processus, classe, titre_contient.
# Effets DWM : "aucun", "mica", "micaalt", "acrylique", "verre".
# Utilisez le menu « Identifier la fenêtre sous le curseur » pour connaître
# le processus, la classe et le titre d'une fenêtre.
#
# Exemple : Explorateur Windows 11 avec matière Mica (opacité intacte) :
#
# [[profils]]
# nom = "Explorateur Mica"
# processus = "explorer.exe"
# classe = "CabinetWClass"
# effet_dwm = "mica"
#
# Exemple : Bloc-notes translucide à 85 % :
#
# [[profils]]
# nom = "Bloc-notes translucide"
# processus = "notepad.exe"
# opacite = 85
# effet_dwm = "aucun"
# ==========================================================================
"#;

/// Nom du dossier de configuration actuel.
const DOSSIER: &str = "RustPiz";
/// Ancien nom du dossier (avant le renommage en RustPiz), migré automatiquement.
const ANCIEN_DOSSIER: &str = "TransparentWindows";

/// Dossier de configuration : `%APPDATA%\RustPiz`
/// (ou le dossier de l'exécutable si `%APPDATA%` est indisponible).
/// Au premier appel, un ancien dossier `%APPDATA%\TransparentWindows` est migré.
pub fn chemin_dossier() -> PathBuf {
    match env::var("APPDATA") {
        Ok(appdata) if !appdata.is_empty() => {
            let base = PathBuf::from(appdata);
            let nouveau = base.join(DOSSIER);
            static MIGRATION: std::sync::Once = std::sync::Once::new();
            MIGRATION.call_once(|| migrer_ancien_dossier(&base.join(ANCIEN_DOSSIER), &nouveau));
            nouveau
        }
        _ => env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join(DOSSIER)))
            .unwrap_or_else(|| PathBuf::from(".")),
    }
}

/// Reprend les fichiers de l'ancien dossier (`config.toml`, `journal.log`) dans le
/// nouveau, sans jamais écraser un fichier déjà présent. Silencieux : le journal
/// n'est pas encore disponible ici. L'ancien dossier est conservé si la copie
/// partielle échoue.
fn migrer_ancien_dossier(ancien: &std::path::Path, nouveau: &std::path::Path) {
    if !ancien.is_dir() || ancien == nouveau {
        return;
    }
    // Cas simple : le nouveau dossier n'existe pas -> on déplace le dossier entier.
    if !nouveau.exists() && fs::rename(ancien, nouveau).is_ok() {
        return;
    }
    // Sinon (ou si le déplacement échoue) : copie fichier par fichier.
    if fs::create_dir_all(nouveau).is_err() {
        return;
    }
    for nom in ["config.toml", "journal.log"] {
        let (src, dst) = (ancien.join(nom), nouveau.join(nom));
        if src.is_file() && !dst.exists() {
            let _ = fs::copy(&src, &dst);
        }
    }
}

/// Chemin complet du fichier `config.toml`.
pub fn chemin_config() -> PathBuf {
    chemin_dossier().join("config.toml")
}

/// Chemin complet du fichier journal.
pub fn chemin_journal() -> PathBuf {
    chemin_dossier().join("journal.log")
}

/// Charge la configuration. Crée le fichier par défaut si absent ;
/// retombe sur les valeurs par défaut en cas d'erreur de lecture/parsing.
pub fn charger() -> Config {
    let chemin = chemin_config();

    if !chemin.exists() {
        if let Some(dossier) = chemin.parent() {
            let _ = fs::create_dir_all(dossier);
        }
        match fs::write(&chemin, TOML_DEFAUT) {
            Ok(()) => logging::info(&format!(
                "Fichier de configuration créé : {}",
                chemin.display()
            )),
            Err(e) => logging::erreur(&format!(
                "Impossible de créer {} : {}",
                chemin.display(),
                e
            )),
        }
    }

    let mut config = match fs::read_to_string(&chemin) {
        Ok(contenu) => match toml::from_str::<Config>(&contenu) {
            Ok(config) => config,
            Err(e) => {
                logging::erreur(&format!(
                    "Configuration invalide dans {} : {}. Valeurs par défaut utilisées.",
                    chemin.display(),
                    e
                ));
                Config::default()
            }
        },
        Err(e) => {
            logging::erreur(&format!(
                "Impossible de lire {} : {}. Valeurs par défaut utilisées.",
                chemin.display(),
                e
            ));
            Config::default()
        }
    };

    config.valider();
    config
}

/// Met à jour les valeurs modifiables depuis la fenêtre de paramètres,
/// en préservant le reste du fichier (commentaires, listes, profils).
///
/// La stratégie est volontairement simple : on réécrit uniquement les
/// lignes des cinq clés concernées, toutes les autres sont inchangées.
pub fn mettre_a_jour_valeurs(
    opacite: u8,
    activer_au_demarrage: bool,
    transparence_explorateur: bool,
    bascule: &str,
    augmenter: &str,
    diminuer: &str,
) -> Result<(), String> {
    let chemin = chemin_config();
    let contenu = fs::read_to_string(&chemin)
        .map_err(|e| format!("Impossible de lire {} : {e}", chemin.display()))?;

    let remplacements = [
        ("opacite_defaut", format!("opacite_defaut = {opacite}")),
        (
            "activer_au_demarrage",
            format!("activer_au_demarrage = {activer_au_demarrage}"),
        ),
        (
            "transparence_explorateur",
            format!("transparence_explorateur = {transparence_explorateur}"),
        ),
        ("bascule", format!("bascule = \"{bascule}\"")),
        ("augmenter", format!("augmenter = \"{augmenter}\"")),
        ("diminuer", format!("diminuer = \"{diminuer}\"")),
    ];

    let mut lignes: Vec<String> = contenu.lines().map(String::from).collect();
    for (cle, nouvelle) in &remplacements {
        let mut remplace = false;
        for ligne in lignes.iter_mut() {
            let sans_espaces = ligne.trim_start();
            // Correspondance exacte de la clé suivie de « = ».
            if let Some(reste) = sans_espaces.strip_prefix(cle) {
                if reste.trim_start().starts_with('=') {
                    let indentation = &ligne[..ligne.len() - sans_espaces.len()];
                    *ligne = format!("{indentation}{nouvelle}");
                    remplace = true;
                    break;
                }
            }
        }
        if !remplace {
            lignes.push(nouvelle.clone());
        }
    }

    fs::write(&chemin, lignes.join("\r\n") + "\r\n")
        .map_err(|e| format!("Impossible d'écrire {} : {e}", chemin.display()))?;
    logging::info("Paramètres enregistrés dans config.toml.");
    Ok(())
}

/// Met à jour (ou insère) des clés situées à la racine du TOML (avant le
/// premier en-tête de table). Fonction pure.
///
/// Chaque élément de `cles` est `(nom, valeur déjà formatée)`, par exemple
/// `("effet_fenetre", "\"zoom\"")`. Une clé présente est remplacée sur
/// place ; une clé absente est insérée après la dernière ligne
/// clé/valeur de la zone racine (sinon avant le premier en-tête, sinon en
/// fin de fichier). Les clés qui n'existent que dans une table sont
/// ignorées. Le style de fin de ligne (CRLF ou LF) est conservé.
fn maj_cles_racine(contenu: &str, cles: &[(&str, String)]) -> String {
    // Un BOM UTF-8 en tête fausserait la reconnaissance de la 1re clé.
    let contenu = contenu.strip_prefix('\u{feff}').unwrap_or(contenu);
    let fin = if contenu.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lignes: Vec<String> = contenu.lines().map(String::from).collect();

    for (cle, valeur) in cles {
        let nouvelle = format!("{cle} = {valeur}");
        let structure = analyser(&lignes);
        let debut_instruction = |i: usize| structure[i].is_some();
        let limite = lignes
            .iter()
            .enumerate()
            .position(|(i, l)| debut_instruction(i) && l.trim_start().starts_with('['))
            .unwrap_or(lignes.len());

        // Ligne de la clé dans la zone racine (correspondance exacte).
        let existante = (0..limite).find(|&i| debut_instruction(i) && est_cle(&lignes[i], cle));

        if let Some(i) = existante {
            let indentation_len = lignes[i].len() - lignes[i].trim_start().len();
            let indentation = lignes[i][..indentation_len].to_string();
            // Une valeur sur plusieurs lignes est remplacée en entier.
            let derniere_ligne = structure[i].unwrap_or(i);
            lignes.splice(i..=derniere_ligne, [format!("{indentation}{nouvelle}")]);
        } else {
            let derniere = (0..limite).rev().find(|&i| {
                let t = lignes[i].trim_start();
                debut_instruction(i) && !t.is_empty() && !t.starts_with('#') && t.contains('=')
            });
            // Insertion après la fin de l'instruction (tableau multi-lignes inclus).
            let position = match derniere {
                Some(i) => structure[i].unwrap_or(i) + 1,
                None => limite,
            };
            lignes.insert(position, nouvelle);
        }
    }

    let mut resultat = lignes.join(fin);
    resultat.push_str(fin);
    resultat
}

/// Pour chaque ligne : `Some(dernière ligne de l'instruction)` si la ligne
/// COMMENCE une instruction (profondeur de crochets nulle, hors chaîne
/// multi-lignes), `None` si c'est une ligne de continuation (tableau ou
/// table en ligne répartis sur plusieurs lignes).
fn analyser(lignes: &[String]) -> Vec<Option<usize>> {
    let mut resultat: Vec<Option<usize>> = vec![None; lignes.len()];
    let mut profondeur: i32 = 0;
    let mut multi: Option<&str> = None;
    let mut debut_courant = 0;
    for (idx, ligne) in lignes.iter().enumerate() {
        if profondeur == 0 && multi.is_none() {
            debut_courant = idx;
            resultat[idx] = Some(idx);
        }
        let c: Vec<char> = ligne.chars().collect();
        let commence = |pos: usize, d: &str| {
            let d: Vec<char> = d.chars().collect();
            c.len() >= pos + d.len() && c[pos..pos + d.len()] == d[..]
        };
        let mut pos = 0;
        while pos < c.len() {
            if let Some(d) = multi {
                if commence(pos, d) {
                    multi = None;
                    pos += 3;
                } else {
                    pos += 1;
                }
                continue;
            }
            match c[pos] {
                '#' => break,
                q @ ('"' | '\'') => {
                    let triple = if q == '"' { "\"\"\"" } else { "\'\'\'" };
                    if commence(pos, triple) {
                        multi = Some(triple);
                        pos += 3;
                    } else {
                        pos += 1;
                        while pos < c.len() && c[pos] != q {
                            if q == '"' && c[pos] == '\\' {
                                pos += 1;
                            }
                            pos += 1;
                        }
                        pos += 1;
                    }
                }
                '[' | '{' => {
                    profondeur += 1;
                    pos += 1;
                }
                ']' | '}' => {
                    profondeur = (profondeur - 1).max(0);
                    pos += 1;
                }
                _ => pos += 1,
            }
        }
        resultat[debut_courant] = Some(idx);
    }
    resultat
}

/// Vrai si la ligne définit la clé `cle` (nue ou entre guillemets), hors commentaire.
fn est_cle(ligne: &str, cle: &str) -> bool {
    let t = ligne.trim_start();
    if t.starts_with('#') {
        return false;
    }
    let reste = if let Some(q) = t.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let interieur = &t[1..];
        match interieur.find(q) {
            Some(fin) if &interieur[..fin] == cle => &interieur[fin + 1..],
            _ => return false,
        }
    } else {
        match t.strip_prefix(cle) {
            Some(r) => r,
            None => return false,
        }
    };
    reste.trim_start().starts_with('=')
}

/// Enregistre des clés racine dans `config.toml` (voir [`maj_cles_racine`]),
/// avec une écriture atomique (fichier temporaire puis renommage).
pub fn enregistrer_cles_racine(cles: &[(&str, String)]) -> Result<(), String> {
    let chemin = chemin_config();
    let contenu = fs::read_to_string(&chemin)
        .map_err(|e| format!("Impossible de lire {} : {e}", chemin.display()))?;
    let nouveau = maj_cles_racine(&contenu, cles);

    let temporaire = chemin.with_file_name("config.toml.tmp");
    let ecrire = || -> std::io::Result<()> {
        let mut f = fs::File::create(&temporaire)?;
        f.write_all(nouveau.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = ecrire() {
        let _ = fs::remove_file(&temporaire);
        return Err(format!("Impossible d'écrire {} : {e}", temporaire.display()));
    }
    fs::rename(&temporaire, &chemin).map_err(|e| {
        let _ = fs::remove_file(&temporaire);
        format!("Impossible de remplacer {} : {e}", chemin.display())
    })?;
    logging::info("Réglages d'animation enregistrés dans config.toml.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z() -> Vec<(&'static str, String)> {
        vec![("effet_fenetre", "\"zoom\"".to_string())]
    }

    #[test]
    fn remplace_cle_existante() {
        let src = "a = 1\neffet_fenetre = \"fondu\"\nb = 2\n";
        assert_eq!(maj_cles_racine(src, &z()), "a = 1\neffet_fenetre = \"zoom\"\nb = 2\n");
    }

    #[test]
    fn insere_apres_derniere_cle_racine() {
        let src = "# c\na = 1\nb = 2\n\n[raccourcis]\nx = 1\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "# c\na = 1\nb = 2\neffet_fenetre = \"zoom\"\n\n[raccourcis]\nx = 1\n"
        );
    }

    #[test]
    fn ignore_cle_sous_table() {
        let src = "a = 1\n[[profils]]\neffet_fenetre = \"fondu\"\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "a = 1\neffet_fenetre = \"zoom\"\n[[profils]]\neffet_fenetre = \"fondu\"\n"
        );
    }

    #[test]
    fn sans_table_va_en_fin() {
        assert_eq!(maj_cles_racine("a = 1", &z()), "a = 1\neffet_fenetre = \"zoom\"\n");
    }

    #[test]
    fn preserve_crlf_et_lf() {
        let crlf = maj_cles_racine("a = 1\r\n[t]\r\n", &z());
        assert_eq!(crlf, "a = 1\r\neffet_fenetre = \"zoom\"\r\n[t]\r\n");
        let lf = maj_cles_racine("a = 1\n[t]\n", &z());
        assert_eq!(lf, "a = 1\neffet_fenetre = \"zoom\"\n[t]\n");
    }

    #[test]
    fn ignore_commentaire() {
        let src = "a = 1\n# effet_fenetre = \"fondu\"\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "a = 1\neffet_fenetre = \"zoom\"\n# effet_fenetre = \"fondu\"\n"
        );
    }

    #[test]
    fn correspondance_exacte() {
        let src = "effet_fenetre2 = 5\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "effet_fenetre2 = 5\neffet_fenetre = \"zoom\"\n"
        );
    }

    #[test]
    fn tableau_multiligne_non_coupe() {
        let src = "a = 1\nexclusions_processus = [\n    \"x.exe\",\n    [1],\n]\n\n[raccourcis]\nx = 1\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "a = 1\nexclusions_processus = [\n    \"x.exe\",\n    [1],\n]\neffet_fenetre = \"zoom\"\n\n[raccourcis]\nx = 1\n"
        );
    }

    #[test]
    fn tableau_multiligne_en_fin_de_zone_racine_sans_table() {
        let src = "liste = [\n  1,\n  2\n]";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "liste = [\n  1,\n  2\n]\neffet_fenetre = \"zoom\"\n"
        );
    }

    #[test]
    fn remplace_cle_tableau_multiligne() {
        let src = "effet_fenetre = [\n  \"a\",\n]\nb = 2\n";
        assert_eq!(maj_cles_racine(src, &z()), "effet_fenetre = \"zoom\"\nb = 2\n");
    }

    #[test]
    fn cle_entre_guillemets() {
        let src = "a = 1\n\"effet_fenetre\" = \"fondu\"\n";
        assert_eq!(maj_cles_racine(src, &z()), "a = 1\neffet_fenetre = \"zoom\"\n");
        let src = "'effet_fenetre' = \"fondu\"\n";
        assert_eq!(maj_cles_racine(src, &z()), "effet_fenetre = \"zoom\"\n");
    }

    #[test]
    fn bom_utf8_retire() {
        let src = "\u{feff}effet_fenetre = \"fondu\"\nb = 2\n";
        assert_eq!(maj_cles_racine(src, &z()), "effet_fenetre = \"zoom\"\nb = 2\n");
    }

    #[test]
    fn crochet_dans_chaine_ou_commentaire_ignore() {
        let src = "a = \"[x\" # [\nb = 2\n[t]\n";
        assert_eq!(
            maj_cles_racine(src, &z()),
            "a = \"[x\" # [\nb = 2\neffet_fenetre = \"zoom\"\n[t]\n"
        );
    }

    #[test]
    fn idempotent() {
        let src = "a = 1\n[t]\nb = 2\n";
        let une = maj_cles_racine(src, &z());
        assert_eq!(maj_cles_racine(&une, &z()), une);
    }

    #[test]
    fn toml_defaut_se_parse() {
        let c: Config = toml::from_str(TOML_DEFAUT).expect("TOML_DEFAUT doit se parser");
        assert_eq!(c.effet_fenetre, "zoom");
        assert!(!c.effet_ouverture && !c.effet_fermeture);
        assert_eq!(c.raccourcis.bascule, "Ctrl+Alt+T");
    }

    #[test]
    fn ancienne_config_sans_cles_prend_defauts() {
        let c: Config = toml::from_str("opacite_defaut = 70\n").expect("parse");
        assert_eq!(c.opacite_defaut, 70);
        assert_eq!(c.effet_fenetre, "zoom");
        assert!(!c.effet_ouverture && !c.effet_fermeture);
    }

    #[test]
    fn desactiver_transition_dwm_cachee_vraie_par_defaut() {
        let c: Config = toml::from_str(TOML_DEFAUT).expect("TOML_DEFAUT doit se parser");
        assert!(c.desactiver_transition_dwm);
        assert!(!TOML_DEFAUT.contains("desactiver_transition_dwm"));
        let c: Config = toml::from_str("desactiver_transition_dwm = false\n").expect("parse");
        assert!(!c.desactiver_transition_dwm);
    }

    #[test]
    fn toml_defaut_contient_jelly() {
        let c: Config = toml::from_str(TOML_DEFAUT).expect("TOML_DEFAUT doit se parser");
        assert!(!c.jelly_actif);
        assert_eq!(c.jelly_puissance, 50);
        // Les clés sont à la racine, avant la première table.
        let jelly = TOML_DEFAUT.find("jelly_puissance = 50").expect("clé présente");
        let table = TOML_DEFAUT.find("[raccourcis]").expect("table présente");
        assert!(jelly < table);
    }

    #[test]
    fn ancienne_config_sans_jelly_prend_defauts() {
        let c: Config = toml::from_str("opacite_defaut = 70\neffet_ouverture = true\n").expect("parse");
        assert!(!c.jelly_actif);
        assert_eq!(c.jelly_puissance, 50);
        assert!(c.effet_ouverture);
    }

    #[test]
    fn jelly_puissance_hors_limites_bornee() {
        let mut c: Config = toml::from_str("jelly_actif = true\njelly_puissance = 250\n").expect("parse");
        assert!(c.jelly_actif);
        c.valider();
        assert_eq!(c.jelly_puissance, 100);
        let mut c = Config { jelly_puissance: 75, ..Config::default() };
        c.valider();
        assert_eq!(c.jelly_puissance, 75);
    }

    #[test]
    fn cles_jelly_inserees_avant_table() {
        let src = "opacite_defaut = 80\neffet_fermeture = false\n\n[raccourcis]\nbascule = \"Ctrl+Alt+T\"\n";
        let cles = [
            ("jelly_actif", "true".to_string()),
            ("jelly_puissance", "75".to_string()),
        ];
        let r = maj_cles_racine(src, &cles);
        assert_eq!(
            r,
            "opacite_defaut = 80\neffet_fermeture = false\njelly_actif = true\njelly_puissance = 75\n\n[raccourcis]\nbascule = \"Ctrl+Alt+T\"\n"
        );
        let c: Config = toml::from_str(&r).expect("parse");
        assert!(c.jelly_actif);
        assert_eq!(c.jelly_puissance, 75);
        // Remplacement sur place, idempotent.
        let r2 = maj_cles_racine(&r, &[("jelly_puissance", "25".to_string())]);
        assert!(r2.contains("jelly_puissance = 25\n\n[raccourcis]"));
        assert!(!r2.contains("jelly_puissance = 75"));
    }

    #[test]
    fn effet_invalide_remplace_par_aucun() {
        let mut c = Config { effet_fenetre: "n'importe quoi".to_string(), ..Config::default() };
        c.valider();
        assert_eq!(c.effet_fenetre, "aucun");
    }
}
