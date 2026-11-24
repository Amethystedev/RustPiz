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
use crate::themes_barre;

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
    /// Barre des tâches personnalisée (v0.6.0) : si vrai, le thème
    /// `barre_theme` est appliqué à l'Explorateur via la DLL d'intégration.
    pub barre_active: bool,
    /// Thème de la barre des tâches (identifiant de `themes_barre::THEMES`).
    pub barre_theme: String,
    /// Miniatures personnalisées (v0.6.0) : si vrai, la taille des miniatures
    /// de la barre est portée à `miniatures_taille` % (build 26100+).
    pub miniatures_actives: bool,
    /// Taille des miniatures, en % (100 à 300).
    pub miniatures_taille: u32,
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
            barre_active: false,
            barre_theme: themes_barre::THEME_DEFAUT.to_string(),
            miniatures_actives: false,
            miniatures_taille: MINIATURES_TAILLE_DEFAUT,
            desactiver_transition_dwm: true,
            raccourcis: Raccourcis::default(),
            exclusions_classes: explorer::classes_exclues_defaut(),
            exclusions_processus: explorer::processus_exclus_defaut(),
            profils: Vec::new(),
        }
    }
}

/// Taille des miniatures par défaut, en %.
pub const MINIATURES_TAILLE_DEFAUT: u32 = 150;
/// Taille minimale des miniatures, en %.
pub const MINIATURES_TAILLE_MIN: u32 = 100;
/// Taille maximale des miniatures, en %.
pub const MINIATURES_TAILLE_MAX: u32 = 300;

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
        if !themes_barre::existe(&self.barre_theme) {
            logging::avertir(&format!(
                "barre_theme=\"{}\" inconnu, valeur \"{}\" utilisée.",
                self.barre_theme,
                themes_barre::THEME_DEFAUT
            ));
            self.barre_theme = themes_barre::THEME_DEFAUT.to_string();
        }
        let taille = self
            .miniatures_taille
            .clamp(MINIATURES_TAILLE_MIN, MINIATURES_TAILLE_MAX);
        if taille != self.miniatures_taille {
            logging::avertir(&format!(
                "miniatures_taille={} hors limites ({MINIATURES_TAILLE_MIN}-{MINIATURES_TAILLE_MAX}), valeur {taille} utilisée.",
                self.miniatures_taille
            ));
            self.miniatures_taille = taille;
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
# Puissance en % : de 0 (aucun mouvement) à 100 (très forte déformation).
jelly_actif = false
jelly_puissance = 50

# Barre des tâches personnalisée : si true, le thème ci-dessous est appliqué
# à l'Explorateur (Windows 11) via la DLL d'intégration injectée dans explorer.
# Thème : identifiant parmi ceux proposés dans la fenêtre de paramètres
# (onglet « Param Barre des Taches »), par exemple "surface", "liquidglass",
# "windows7", "winxp". Un identifiant inconnu est remplacé par "surface".
barre_active = false
barre_theme = "surface"

# Miniatures personnalisées : si true, la taille des miniatures de la barre
# est modifiée (Windows 11 24H2 / build 26100 ou plus récent).
# Taille en % : de 100 à 300.
miniatures_actives = false
miniatures_taille = 150

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
# Profils automatiques (appliqués aux nouvelles fenêtres dès leur affichage).
# Critères (ET logique) : processus, classe, titre_contient.
# Effets DWM : "aucun", "mica", "micaalt", "acrylique", "verre".
# Le nom du processus d'une fenêtre est visible dans le Gestionnaire des
# tâches (onglet Détails) ; sa classe, avec un outil comme Spy++.
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

/// Vrai si la ligne est l'en-tête de la table simple `[table]` (pas un
/// tableau de tables `[[...]]`), commentaire de fin de ligne toléré.
fn est_entete(ligne: &str, table: &str) -> bool {
    let t = ligne.trim_start();
    if t.starts_with("[[") {
        return false;
    }
    let Some(reste) = t.strip_prefix('[') else {
        return false;
    };
    let Some(fin) = reste.find(']') else {
        return false;
    };
    let apres = reste[fin + 1..].trim_start();
    reste[..fin].trim() == table && (apres.is_empty() || apres.starts_with('#'))
}

/// Met à jour (ou insère) des clés de la table `[table]`. Fonction pure.
///
/// Une clé présente dans la table est remplacée sur place (indentation
/// conservée, valeur multi-lignes remplacée en entier) ; une clé absente est
/// insérée après la dernière ligne clé/valeur de la table (sinon juste après
/// l'en-tête). Table absente : créée en fin de fichier. Cas particulier : si
/// la table est écrite EN LIGNE à la racine (`table = { ... }`), elle est
/// remplacée en entier par une table en ligne contenant `cles` (toutes les
/// clés de la table doivent alors être fournies, ce qui est le cas des
/// raccourcis). Le style de fin de ligne est conservé, un BOM est retiré.
fn maj_cles_table(contenu: &str, table: &str, cles: &[(&str, String)]) -> String {
    let contenu = contenu.strip_prefix('\u{feff}').unwrap_or(contenu);
    let fin = if contenu.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lignes: Vec<String> = contenu.lines().map(String::from).collect();

    // Table en ligne à la racine.
    {
        let structure = analyser(&lignes);
        let limite = (0..lignes.len())
            .find(|&i| structure[i].is_some() && lignes[i].trim_start().starts_with('['))
            .unwrap_or(lignes.len());
        if (0..limite).any(|i| structure[i].is_some() && est_cle(&lignes[i], table)) {
            let interieur: Vec<String> = cles.iter().map(|(c, v)| format!("{c} = {v}")).collect();
            return maj_cles_racine(contenu, &[(table, format!("{{ {} }}", interieur.join(", ")))]);
        }
    }

    for (cle, valeur) in cles {
        let nouvelle = format!("{cle} = {valeur}");
        let structure = analyser(&lignes);
        let debut_instruction = |i: usize| structure[i].is_some();
        let entete = (0..lignes.len()).find(|&i| debut_instruction(i) && est_entete(&lignes[i], table));
        let Some(h) = entete else {
            // Table absente : créée en fin de fichier (séparée par une ligne vide).
            if lignes.last().is_some_and(|l| !l.trim().is_empty()) {
                lignes.push(String::new());
            }
            lignes.push(format!("[{table}]"));
            lignes.push(nouvelle);
            continue;
        };
        let limite = (h + 1..lignes.len())
            .find(|&i| debut_instruction(i) && lignes[i].trim_start().starts_with('['))
            .unwrap_or(lignes.len());
        let existante = (h + 1..limite).find(|&i| debut_instruction(i) && est_cle(&lignes[i], cle));
        if let Some(i) = existante {
            let indentation_len = lignes[i].len() - lignes[i].trim_start().len();
            let indentation = lignes[i][..indentation_len].to_string();
            let derniere_ligne = structure[i].unwrap_or(i);
            lignes.splice(i..=derniere_ligne, [format!("{indentation}{nouvelle}")]);
        } else {
            let derniere = (h + 1..limite).rev().find(|&i| {
                let t = lignes[i].trim_start();
                debut_instruction(i) && !t.is_empty() && !t.starts_with('#') && t.contains('=')
            });
            let position = match derniere {
                Some(i) => structure[i].unwrap_or(i) + 1,
                None => h + 1,
            };
            lignes.insert(position, nouvelle);
        }
    }

    let mut resultat = lignes.join(fin);
    resultat.push_str(fin);
    resultat
}

/// Chaîne TOML de base (entre guillemets, `\` et `"` échappés, caractères
/// de contrôle en `\uXXXX`).
fn chaine_toml(texte: &str) -> String {
    let mut resultat = String::with_capacity(texte.len() + 2);
    resultat.push('"');
    for c in texte.chars() {
        match c {
            '\\' => resultat.push_str("\\\\"),
            '"' => resultat.push_str("\\\""),
            c if c.is_control() => resultat.push_str(&format!("\\u{:04X}", c as u32)),
            c => resultat.push(c),
        }
    }
    resultat.push('"');
    resultat
}

/// Valeurs modifiables depuis la fenêtre de paramètres (v0.5.0 ; barre des
/// tâches et miniatures depuis la v0.6.0).
#[derive(Debug, Clone)]
pub struct Parametres {
    pub opacite_defaut: u8,
    pub activer_au_demarrage: bool,
    pub transparence_explorateur: bool,
    /// Nom de configuration de l'effet (`EffetAnimation::nom_config`).
    pub effet_fenetre: String,
    pub effet_ouverture: bool,
    pub effet_fermeture: bool,
    pub jelly_actif: bool,
    pub jelly_puissance: u8,
    pub barre_active: bool,
    /// Identifiant de thème ; inconnu : `themes_barre::THEME_DEFAUT` écrit.
    pub barre_theme: String,
    pub miniatures_actives: bool,
    /// Taille en % ; bornée à 100-300 à l'écriture.
    pub miniatures_taille: u32,
    pub raccourcis: Raccourcis,
}

impl Parametres {
    /// Configuration en mémoire obtenue en appliquant ces paramètres (mêmes
    /// bornes qu'à l'écriture de `config.toml`) sur `base` : sert au test
    /// temporaire (« Tester »), sans aucune écriture. Fonction pure.
    pub fn appliquer_a(&self, mut base: Config) -> Config {
        base.opacite_defaut = self.opacite_defaut.clamp(10, 100);
        base.activer_au_demarrage = self.activer_au_demarrage;
        base.transparence_explorateur = self.transparence_explorateur;
        base.effet_fenetre = self.effet_fenetre.clone();
        base.effet_ouverture = self.effet_ouverture;
        base.effet_fermeture = self.effet_fermeture;
        base.jelly_actif = self.jelly_actif;
        base.jelly_puissance = self.jelly_puissance.min(100);
        base.barre_active = self.barre_active;
        base.barre_theme = themes_barre::normaliser(&self.barre_theme).to_string();
        base.miniatures_actives = self.miniatures_actives;
        base.miniatures_taille = self
            .miniatures_taille
            .clamp(MINIATURES_TAILLE_MIN, MINIATURES_TAILLE_MAX);
        base.raccourcis = self.raccourcis.clone();
        base
    }
}

/// Applique les paramètres au contenu du fichier (fonction pure) : clés
/// racine via [`maj_cles_racine`], raccourcis dans la table `[raccourcis]`
/// via [`maj_cles_table`] ; le reste du fichier (commentaires, listes,
/// profils) est préservé.
fn maj_parametres(contenu: &str, p: &Parametres) -> String {
    let racine = [
        ("opacite_defaut", p.opacite_defaut.clamp(10, 100).to_string()),
        ("activer_au_demarrage", p.activer_au_demarrage.to_string()),
        ("transparence_explorateur", p.transparence_explorateur.to_string()),
        ("effet_fenetre", chaine_toml(&p.effet_fenetre)),
        ("effet_ouverture", p.effet_ouverture.to_string()),
        ("effet_fermeture", p.effet_fermeture.to_string()),
        ("jelly_actif", p.jelly_actif.to_string()),
        ("jelly_puissance", p.jelly_puissance.min(100).to_string()),
        ("barre_active", p.barre_active.to_string()),
        ("barre_theme", chaine_toml(themes_barre::normaliser(&p.barre_theme))),
        ("miniatures_actives", p.miniatures_actives.to_string()),
        (
            "miniatures_taille",
            p.miniatures_taille
                .clamp(MINIATURES_TAILLE_MIN, MINIATURES_TAILLE_MAX)
                .to_string(),
        ),
    ];
    let contenu = maj_cles_racine(contenu, &racine);
    let raccourcis = [
        ("bascule", chaine_toml(&p.raccourcis.bascule)),
        ("augmenter", chaine_toml(&p.raccourcis.augmenter)),
        ("diminuer", chaine_toml(&p.raccourcis.diminuer)),
    ];
    maj_cles_table(&contenu, "raccourcis", &raccourcis)
}

/// Écrit `contenu` dans `chemin` de façon atomique (fichier temporaire
/// synchronisé sur disque, puis renommage).
fn ecrire_atomique(chemin: &std::path::Path, contenu: &str) -> Result<(), String> {
    let temporaire = chemin.with_file_name("config.toml.tmp");
    let ecrire = || -> std::io::Result<()> {
        let mut f = fs::File::create(&temporaire)?;
        f.write_all(contenu.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = ecrire() {
        let _ = fs::remove_file(&temporaire);
        return Err(format!("Impossible d'écrire {} : {e}", temporaire.display()));
    }
    fs::rename(&temporaire, chemin).map_err(|e| {
        let _ = fs::remove_file(&temporaire);
        format!("Impossible de remplacer {} : {e}", chemin.display())
    })
}

/// Enregistre les paramètres de la fenêtre de paramètres dans
/// `config.toml` en UNE lecture et UNE écriture atomique. Le résultat est
/// relu (`toml`) avant écriture : un fichier qui deviendrait invalide n'est
/// jamais écrit (erreur renvoyée). Fichier absent : modèle par défaut.
pub fn enregistrer_parametres(p: &Parametres) -> Result<(), String> {
    let chemin = chemin_config();
    let contenu = if chemin.exists() {
        fs::read_to_string(&chemin)
            .map_err(|e| format!("Impossible de lire {} : {e}", chemin.display()))?
    } else {
        TOML_DEFAUT.to_string()
    };
    let nouveau = maj_parametres(&contenu, p);
    if let Err(e) = toml::from_str::<Config>(&nouveau) {
        return Err(if toml::from_str::<Config>(&contenu).is_err() {
            format!(
                "{} est invalide : corrigez-le à la main avant d'enregistrer ({e})",
                chemin.display()
            )
        } else {
            format!("Enregistrement refusé : le fichier modifié serait invalide ({e})")
        });
    }
    if let Some(dossier) = chemin.parent() {
        let _ = fs::create_dir_all(dossier);
    }
    ecrire_atomique(&chemin, &nouveau)?;
    logging::info("Paramètres enregistrés dans config.toml (fenêtre de paramètres).");
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

    fn parametres_exemple() -> Parametres {
        Parametres {
            opacite_defaut: 65,
            activer_au_demarrage: false,
            transparence_explorateur: false,
            effet_fenetre: "balayage_pixels".to_string(),
            effet_ouverture: true,
            effet_fermeture: true,
            jelly_actif: true,
            jelly_puissance: 37,
            barre_active: true,
            barre_theme: "liquidglass-legacy-alternate".to_string(),
            miniatures_actives: true,
            miniatures_taille: 225,
            raccourcis: Raccourcis {
                bascule: "Ctrl+Shift+F9".to_string(),
                augmenter: "Alt+PageHaut".to_string(),
                diminuer: "Win+Alt+Bas".to_string(),
            },
        }
    }

    fn verifier_exemple(c: &Config) {
        assert_eq!(c.opacite_defaut, 65);
        assert!(!c.activer_au_demarrage && !c.transparence_explorateur);
        assert_eq!(c.effet_fenetre, "balayage_pixels");
        assert!(c.effet_ouverture && c.effet_fermeture && c.jelly_actif);
        assert_eq!(c.jelly_puissance, 37);
        assert!(c.barre_active && c.miniatures_actives);
        assert_eq!(c.barre_theme, "liquidglass-legacy-alternate");
        assert_eq!(c.miniatures_taille, 225);
        assert_eq!(c.raccourcis.bascule, "Ctrl+Shift+F9");
        assert_eq!(c.raccourcis.augmenter, "Alt+PageHaut");
        assert_eq!(c.raccourcis.diminuer, "Win+Alt+Bas");
    }

    #[test]
    fn parametres_sur_modele_par_defaut() {
        let r = maj_parametres(TOML_DEFAUT, &parametres_exemple());
        let c: Config = toml::from_str(&r).expect("parse");
        verifier_exemple(&c);
        // Remplacements sur place : aucune clé dupliquée, commentaires gardés.
        assert_eq!(r.matches("opacite_defaut = ").count(), 1);
        assert_eq!(r.matches("bascule = ").count(), 1);
        assert_eq!(r.matches("jelly_puissance = ").count(), 1);
        assert_eq!(r.matches("barre_theme = ").count(), 1);
        assert_eq!(r.matches("miniatures_taille = ").count(), 1);
        assert!(r.contains("# Active / désactive la transparence de la fenêtre sous le curseur."));
        assert!(r.contains("exclusions_processus = ["));
        // Idempotent.
        assert_eq!(maj_parametres(&r, &parametres_exemple()), r);
    }

    #[test]
    fn parametres_crlf_et_ancienne_config() {
        let src = "opacite_defaut = 80\r\n\r\n[raccourcis]\r\nbascule = \"Ctrl+Alt+T\"\r\n\r\n[[profils]]\r\nnom = \"p\"\r\nopacite = 50\r\n";
        let r = maj_parametres(src, &parametres_exemple());
        assert!(!r.replace("\r\n", "").contains('\n'));
        let c: Config = toml::from_str(&r).expect("parse");
        verifier_exemple(&c);
        assert_eq!(c.profils.len(), 1);
        assert_eq!(c.profils[0].opacite, Some(50));
        // Les raccourcis absents sont insérés DANS la table, avant [[profils]].
        let table = r.find("[raccourcis]").expect("table");
        let profils = r.find("[[profils]]").expect("profils");
        let diminuer = r.find("diminuer = ").expect("clé");
        assert!(table < diminuer && diminuer < profils);
    }

    #[test]
    fn parametres_sans_table_raccourcis() {
        let src = "opacite_defaut = 80\n";
        let r = maj_parametres(src, &parametres_exemple());
        let c: Config = toml::from_str(&r).expect("parse");
        verifier_exemple(&c);
        assert!(r.contains("\n\n[raccourcis]\nbascule = \"Ctrl+Shift+F9\"\naugmenter"));
    }

    #[test]
    fn parametres_table_raccourcis_en_ligne() {
        let src = "opacite_defaut = 80\nraccourcis = { bascule = \"Ctrl+Alt+T\" }\n[[profils]]\nnom = \"p\"\n";
        let r = maj_parametres(src, &parametres_exemple());
        let c: Config = toml::from_str(&r).expect("parse");
        verifier_exemple(&c);
        assert!(!r.contains("[raccourcis]"));
    }

    #[test]
    fn cle_raccourci_hors_table_ignoree() {
        // Une clé `bascule` dans une autre table n'est pas touchée.
        let src = "[[profils]]\nbascule = \"x\"\n[raccourcis]\nbascule = \"Ctrl+Alt+T\"\n";
        let r = maj_cles_table(src, "raccourcis", &[("bascule", "\"F5\"".to_string())]);
        assert_eq!(r, "[[profils]]\nbascule = \"x\"\n[raccourcis]\nbascule = \"F5\"\n");
    }

    #[test]
    fn entete_de_table() {
        assert!(est_entete("[raccourcis]", "raccourcis"));
        assert!(est_entete("  [ raccourcis ]  # commentaire", "raccourcis"));
        assert!(!est_entete("[[raccourcis]]", "raccourcis"));
        assert!(!est_entete("[raccourcis2]", "raccourcis"));
        assert!(!est_entete("# [raccourcis]", "raccourcis"));
    }

    #[test]
    fn chaine_toml_echappee() {
        assert_eq!(chaine_toml("Ctrl+Alt+T"), "\"Ctrl+Alt+T\"");
        assert_eq!(chaine_toml("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000A\"");
        let c: Config = toml::from_str(&format!("effet_fenetre = {}\n", chaine_toml("a\"b\\c\n"))).expect("parse");
        assert_eq!(c.effet_fenetre, "a\"b\\c\n");
    }

    #[test]
    fn effet_invalide_remplace_par_aucun() {
        let mut c = Config { effet_fenetre: "n'importe quoi".to_string(), ..Config::default() };
        c.valider();
        assert_eq!(c.effet_fenetre, "aucun");
    }

    #[test]
    fn toml_defaut_contient_barre_et_miniatures() {
        let c: Config = toml::from_str(TOML_DEFAUT).expect("TOML_DEFAUT doit se parser");
        assert!(!c.barre_active && !c.miniatures_actives);
        assert_eq!(c.barre_theme, "surface");
        assert_eq!(c.miniatures_taille, 150);
        // Clés à la racine, avant la première table.
        let table = TOML_DEFAUT.find("[raccourcis]").expect("table présente");
        for cle in [
            "barre_active = false",
            "barre_theme = \"surface\"",
            "miniatures_actives = false",
            "miniatures_taille = 150",
        ] {
            let pos = TOML_DEFAUT.find(cle).expect("clé présente");
            assert!(pos < table, "{cle} doit précéder [raccourcis]");
        }
    }

    #[test]
    fn ancienne_config_sans_barre_ni_miniatures_prend_defauts() {
        let mut c: Config =
            toml::from_str("opacite_defaut = 70\njelly_actif = true\n").expect("parse");
        c.valider();
        assert!(!c.barre_active && !c.miniatures_actives);
        assert_eq!(c.barre_theme, "surface");
        assert_eq!(c.miniatures_taille, 150);
        assert!(c.jelly_actif);
        assert_eq!(c.opacite_defaut, 70);
    }

    #[test]
    fn miniatures_taille_bornee() {
        for (lu, attendu) in [(0, 100), (99, 100), (100, 100), (150, 150), (300, 300), (301, 300), (5000, 300)] {
            let mut c: Config =
                toml::from_str(&format!("miniatures_taille = {lu}\n")).expect("parse");
            c.valider();
            assert_eq!(c.miniatures_taille, attendu, "valeur lue {lu}");
        }
        // Bornée aussi à l'écriture.
        let mut p = parametres_exemple();
        p.miniatures_taille = 999;
        let r = maj_parametres(TOML_DEFAUT, &p);
        assert!(r.contains("miniatures_taille = 300\n"));
        p.miniatures_taille = 3;
        let r = maj_parametres(TOML_DEFAUT, &p);
        assert!(r.contains("miniatures_taille = 100\n"));
    }

    #[test]
    fn miniatures_taille_negative_refusee_par_serde() {
        // u32 : une valeur négative rend le fichier invalide (défauts utilisés
        // par `charger`), elle n'est jamais acceptée silencieusement.
        assert!(toml::from_str::<Config>("miniatures_taille = -5\n").is_err());
    }

    #[test]
    fn theme_inconnu_remplace_par_surface() {
        let mut c: Config = toml::from_str("barre_theme = \"n'existe pas\"\n").expect("parse");
        c.valider();
        assert_eq!(c.barre_theme, "surface");
        let mut c: Config = toml::from_str("barre_theme = \"Surface\"\n").expect("parse");
        c.valider();
        assert_eq!(c.barre_theme, "surface");
        let mut c: Config = toml::from_str("barre_theme = \"winxp-zune\"\n").expect("parse");
        c.valider();
        assert_eq!(c.barre_theme, "winxp-zune");
        // À l'écriture : un thème inconnu est écrit "surface".
        let mut p = parametres_exemple();
        p.barre_theme = "inconnu".to_string();
        let r = maj_parametres(TOML_DEFAUT, &p);
        let c: Config = toml::from_str(&r).expect("parse");
        assert_eq!(c.barre_theme, "surface");
    }

    #[test]
    fn aller_retour_barre_et_miniatures_ancienne_config() {
        // Ancienne config v0.5.0 (sans les nouvelles clés) : insertion avant
        // la table, relecture identique, puis remplacement sur place.
        let src = "opacite_defaut = 80\njelly_puissance = 50\n\n[raccourcis]\nbascule = \"Ctrl+Alt+T\"\n";
        let r = maj_parametres(src, &parametres_exemple());
        let c: Config = toml::from_str(&r).expect("parse");
        verifier_exemple(&c);
        let table = r.find("[raccourcis]").expect("table");
        assert!(r.find("barre_theme = ").expect("clé") < table);
        assert!(r.find("miniatures_taille = ").expect("clé") < table);
        let mut p = parametres_exemple();
        p.barre_active = false;
        p.barre_theme = "windows7".to_string();
        p.miniatures_actives = false;
        p.miniatures_taille = 100;
        let r2 = maj_parametres(&r, &p);
        let c: Config = toml::from_str(&r2).expect("parse");
        assert!(!c.barre_active && !c.miniatures_actives);
        assert_eq!(c.barre_theme, "windows7");
        assert_eq!(c.miniatures_taille, 100);
        assert_eq!(r2.matches("barre_theme = ").count(), 1);
        assert_eq!(maj_parametres(&r2, &p), r2);
    }
}
