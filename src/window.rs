//! Logique de transparence des fenêtres (API Win32 « layered windows »)
//! et application des effets DWM / profils automatiques.
//!
//! Principe de la transparence : ajout du style étendu `WS_EX_LAYERED`
//! (`SetWindowLongPtrW` avec `GWL_EXSTYLE`), puis opacité fixée avec
//! `SetLayeredWindowAttributes` et le drapeau `LWA_ALPHA`.
//!
//! L'état d'origine de chaque fenêtre modifiée (style étendu, backdrop
//! DWM, verre) est mémorisé afin de restaurer exactement l'apparence
//! initiale à tout moment.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows::Win32::Foundation::{CloseHandle, COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetLayeredWindowAttributes, SetWindowLongPtrW, WindowFromPoint, GA_ROOT, GWL_EXSTYLE,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, GWL_STYLE, WS_CAPTION, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::config::{Config, Raccourcis};
use crate::dwm::{self, EffetDwm};
use crate::explorer;
use crate::logging;
use crate::profils;

/// Opacité minimale autorisée (en %) pour éviter les fenêtres invisibles.
const OPACITE_MIN: u8 = 10;

/// Informations mémorisées sur une fenêtre modifiée par nos soins.
struct FenetreModifiee {
    /// Style étendu d'origine (avant notre modification).
    exstyle_origine: isize,
    /// Vrai si c'est nous qui avons ajouté `WS_EX_LAYERED`.
    layered_ajoute: bool,
    /// Opacité actuellement appliquée, en pourcentage.
    opacite_courante: u8,
    /// Alpha d'origine d'une fenêtre DÉJÀ layered avant notre première
    /// transparence (ex. laissée layered alpha 255 par une animation) :
    /// remis à la restauration, puisque le style n'est alors pas retiré.
    alpha_origine: Option<u8>,
    /// Valeur d'origine du backdrop DWM (avant Mica/Acrylique), si modifié.
    backdrop_origine: Option<i32>,
    /// Vrai si le « verre » complet (marges -1) a été appliqué.
    verre_applique: bool,
}

/// État global de l'application (un seul thread : la boucle de messages).
pub struct EtatApp {
    /// Interrupteur général : si faux, les raccourcis sont ignorés.
    pub actif: bool,
    /// Opacité globale par défaut (en %), modifiable via les paramètres.
    pub opacite: u8,
    /// Fenêtres actuellement modifiées : clé = valeur du HWND.
    fenetres: HashMap<isize, FenetreModifiee>,
    /// Fenêtres déjà examinées par le scanner de profils.
    traitees: HashSet<isize>,
    /// Configuration chargée depuis `config.toml`.
    pub config: Config,
    /// Notre propre fenêtre de message (à ne jamais modifier).
    pub fenetre_message: HWND,
}

thread_local! {
    /// État de l'application, accessible depuis la procédure de fenêtre.
    static ETAT: RefCell<Option<EtatApp>> = const { RefCell::new(None) };
}

/// Initialise l'état global à partir de la configuration.
pub fn initialiser_etat(config: Config, fenetre_message: HWND) {
    let actif = config.activer_au_demarrage;
    let opacite = config.opacite_defaut;
    ETAT.with(|cellule| {
        *cellule.borrow_mut() = Some(EtatApp {
            actif,
            opacite,
            fenetres: HashMap::new(),
            traitees: HashSet::new(),
            config,
            fenetre_message,
        });
    });
    logging::info(&format!(
        "État initialisé : actif={actif}, opacité={opacite} %, {} profil(s).",
        ETAT.with(|c| c.borrow().as_ref().map(|e| e.config.profils.len()).unwrap_or(0))
    ));
}

/// Exécute une fermeture mutable sur l'état global (sans paniquer).
fn avec_etat<F: FnOnce(&mut EtatApp)>(f: F) {
    ETAT.with(|cellule| {
        if let Some(etat) = cellule.borrow_mut().as_mut() {
            f(etat);
        }
    });
}

/// Convertit un pourcentage d'opacité (10-100) en alpha (0-255).
fn pourcentage_vers_alpha(pourcentage: u8) -> u8 {
    let pourcentage = pourcentage.clamp(OPACITE_MIN, 100);
    ((pourcentage as u16 * 255) / 100) as u8
}

/// Retourne le HWND racine de la fenêtre située sous le curseur de la souris.
pub fn fenetre_sous_curseur() -> Option<HWND> {
    unsafe {
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_err() {
            return None;
        }
        let hwnd = WindowFromPoint(point);
        if hwnd.0.is_null() {
            return None;
        }
        // On remonte à la fenêtre racine pour traiter la fenêtre entière
        // et non un contrôle enfant.
        let racine = GetAncestor(hwnd, GA_ROOT);
        if racine.0.is_null() {
            None
        } else {
            Some(racine)
        }
    }
}

/// Nom de classe Win32 de la fenêtre (ex. `CabinetWClass`).
unsafe fn nom_classe(hwnd: HWND) -> String {
    let mut tampon = [0u16; 256];
    let longueur = GetClassNameW(hwnd, &mut tampon);
    if longueur <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&tampon[..longueur as usize])
}

/// Titre de la fenêtre (barre de titre).
unsafe fn titre_fenetre(hwnd: HWND) -> String {
    let mut tampon = [0u16; 512];
    let longueur = GetWindowTextW(hwnd, &mut tampon);
    if longueur <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&tampon[..longueur as usize])
}

/// Nom du processus propriétaire de la fenêtre (ex. `notepad.exe`),
/// en minuscules, ou `None` si le processus n'est pas accessible.
unsafe fn nom_processus(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return None;
    }
    let processus = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut tampon = [0u16; 260];
    let mut longueur = tampon.len() as u32;
    let resultat = QueryFullProcessImageNameW(
        processus,
        PROCESS_NAME_WIN32,
        windows::core::PWSTR(tampon.as_mut_ptr()),
        &mut longueur,
    );
    let _ = CloseHandle(processus);
    resultat.ok()?;
    let chemin = String::from_utf16_lossy(&tampon[..longueur as usize]);
    chemin.rsplit('\\').next().map(|nom| nom.to_lowercase())
}

/// Informations d'identification d'une fenêtre : (processus, classe, titre).
pub fn infos_fenetre(hwnd: HWND) -> (String, String, String) {
    unsafe {
        (
            nom_processus(hwnd).unwrap_or_else(|| "?".to_string()),
            nom_classe(hwnd),
            titre_fenetre(hwnd),
        )
    }
}

/// Vrai si la fenêtre occupe exactement la totalité de son écran
/// (cas typique des jeux et lecteurs vidéo plein écran).
unsafe fn est_plein_ecran(hwnd: HWND) -> bool {
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return false;
    }
    let moniteur: HMONITOR = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut infos = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(moniteur, &mut infos).as_bool() {
        return false;
    }
    let ecran = infos.rcMonitor;
    rect.left == ecran.left
        && rect.top == ecran.top
        && rect.right == ecran.right
        && rect.bottom == ecran.bottom
}

/// Indique si la fenêtre doit être ignorée (shell, plein écran, invisible...).
unsafe fn est_exclue(hwnd: HWND, config: &Config, fenetre_message: HWND) -> bool {
    if hwnd == fenetre_message {
        return true;
    }
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return true;
    }
    let classe = nom_classe(hwnd);
    if explorer::classe_est_exclue(&classe, &config.exclusions_classes) {
        logging::info(&format!("Fenêtre ignorée (classe exclue : {classe})."));
        return true;
    }
    if let Some(processus) = nom_processus(hwnd) {
        if explorer::processus_est_exclu(&processus, &config.exclusions_processus) {
            logging::info(&format!("Fenêtre ignorée (processus exclu : {processus})."));
            return true;
        }
    }
    if est_plein_ecran(hwnd) {
        logging::info("Fenêtre ignorée (plein écran).");
        return true;
    }
    false
}

/// Variante silencieuse de `est_exclue` pour le module d'animations :
/// mêmes critères (notre fenêtre de message, invisible/réduite, classe ou
/// processus exclu, plein écran), sans aucune ligne de journal. Renvoie la
/// raison de l'exclusion (`None` = fenêtre non exclue) pour que le module
/// d'animations puisse la journaliser lui-même.
///
/// L'état global n'est lu que par `try_borrow` (jamais `avec_etat`) et
/// relâché AVANT les appels Win32 ; si l'état est déjà emprunté, la
/// fenêtre est considérée exclue (choix prudent : on n'anime pas).
pub fn raison_exclusion_silencieuse(hwnd: HWND) -> Option<&'static str> {
    let lecture = ETAT.with(|cellule| {
        cellule.try_borrow().ok().and_then(|etat| {
            etat.as_ref().map(|e| {
                (
                    e.fenetre_message,
                    e.config.exclusions_classes.clone(),
                    e.config.exclusions_processus.clone(),
                )
            })
        })
    });
    let Some((fenetre_message, classes, processus_exclus)) = lecture else {
        return Some("état de l'application indisponible");
    };
    unsafe {
        if hwnd == fenetre_message {
            return Some("fenêtre de message de l'application");
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return Some("fenêtre invisible");
        }
        if IsIconic(hwnd).as_bool() {
            return Some("fenêtre réduite");
        }
        if explorer::classe_est_exclue(&nom_classe(hwnd), &classes) {
            return Some("classe exclue (exclusions_classes)");
        }
        if let Some(processus) = nom_processus(hwnd) {
            if explorer::processus_est_exclu(&processus, &processus_exclus) {
                return Some("processus exclu (exclusions_processus)");
            }
        }
        if est_plein_ecran(hwnd) {
            return Some("plein écran");
        }
        None
    }
}

/// Crée une entrée « fenêtre modifiée » si absente (sans appliquer d'opacité).
fn obtenir_ou_creer_entree<'a>(
    etat: &'a mut EtatApp,
    hwnd: HWND,
) -> &'a mut FenetreModifiee {
    let cle = hwnd.0 as isize;
    etat.fenetres.entry(cle).or_insert_with(|| {
        let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        FenetreModifiee {
            exstyle_origine: exstyle,
            layered_ajoute: false,
            opacite_courante: 100,
            alpha_origine: None,
            backdrop_origine: None,
            verre_applique: false,
        }
    })
}

/// Applique une opacité donnée à une fenêtre, avec mémorisation de
/// l'état d'origine. Ne panique jamais : journalise et ignore en cas
/// d'échec (fenêtre protégée, détruite entre-temps, etc.).
fn appliquer_interne(etat: &mut EtatApp, hwnd: HWND, opacite: u8) {
    // Fenêtre masquée (alpha 0) par une animation d'ouverture : on n'y
    // touche pas, sinon l'état temporaire serait pris pour l'état d'origine.
    if crate::animations::fenetre_masquee_par_animation(hwnd) {
        return;
    }
    if unsafe { est_exclue(hwnd, &etat.config, etat.fenetre_message) } {
        return;
    }
    let cle = hwnd.0 as isize;
    let opacite = opacite.clamp(OPACITE_MIN, 100);

    if etat.fenetres.contains_key(&cle) && etat.fenetres[&cle].layered_ajoute {
        // Fenêtre déjà transparente : on met simplement à jour l'opacité.
        let alpha = pourcentage_vers_alpha(opacite);
        unsafe {
            match SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) {
                Ok(()) => {
                    if let Some(modifiee) = etat.fenetres.get_mut(&cle) {
                        modifiee.opacite_courante = opacite;
                    }
                }
                Err(e) => logging::erreur(&format!(
                    "SetLayeredWindowAttributes a échoué sur {hwnd:?} : {e}"
                )),
            }
        }
        return;
    }

    unsafe {
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let deja_layered = exstyle & (WS_EX_LAYERED.0 as isize) != 0;
        if !deja_layered {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle | (WS_EX_LAYERED.0 as isize));
        }
        let alpha = pourcentage_vers_alpha(opacite);
        // Première transparence sur une fenêtre déjà layered : on relève son
        // alpha d'origine AVANT de le modifier.
        let premiere = etat
            .fenetres
            .get(&cle)
            .map_or(true, |e| e.opacite_courante == 100 && !e.layered_ajoute);
        let alpha_lu = if deja_layered && premiere {
            let (mut a, mut f) = (255u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0));
            match GetLayeredWindowAttributes(hwnd, None, Some(&mut a), Some(&mut f)) {
                Ok(()) if f.0 & LWA_ALPHA.0 != 0 => Some(a),
                _ => None,
            }
        } else {
            None
        };
        match SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) {
            Ok(()) => {
                let entree = obtenir_ou_creer_entree(etat, hwnd);
                if alpha_lu.is_some() {
                    entree.alpha_origine = alpha_lu;
                }
                entree.exstyle_origine = exstyle;
                entree.layered_ajoute = !deja_layered || entree.layered_ajoute;
                entree.opacite_courante = opacite;
                logging::info(&format!(
                    "Transparence appliquée ({opacite} %) à la fenêtre {hwnd:?} \
                     (classe : {}).",
                    nom_classe(hwnd)
                ));
            }
            Err(e) => {
                // Échec : on annule notre modification de style.
                if !deja_layered {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle);
                }
                logging::erreur(&format!(
                    "Impossible de rendre la fenêtre {hwnd:?} transparente : {e}"
                ));
            }
        }
    }
}

/// Applique un effet DWM (Mica, Acrylique, Verre...) à une fenêtre.
fn appliquer_effet_interne(etat: &mut EtatApp, hwnd: HWND, effet: EffetDwm) {
    if crate::animations::fenetre_masquee_par_animation(hwnd) {
        return;
    }
    if unsafe { est_exclue(hwnd, &etat.config, etat.fenetre_message) } {
        return;
    }
    let cle = hwnd.0 as isize;
    // On mémorise la valeur d'origine du backdrop une seule fois.
    let origine = etat
        .fenetres
        .get(&cle)
        .and_then(|f| f.backdrop_origine)
        .or_else(|| dwm::lire_backdrop(hwnd));

    // Si l'on quitte le mode « Verre », on retire d'abord les marges.
    let avait_verre = etat
        .fenetres
        .get(&cle)
        .map(|f| f.verre_applique)
        .unwrap_or(false);
    if avait_verre && effet != EffetDwm::Verre {
        let _ = dwm::definir_verre(hwnd, false);
        if let Some(f) = etat.fenetres.get_mut(&cle) {
            f.verre_applique = false;
        }
    }

    match dwm::appliquer(hwnd, effet) {
        Ok(_) => {
            let entree = obtenir_ou_creer_entree(etat, hwnd);
            entree.backdrop_origine = origine;
            if effet == EffetDwm::Verre {
                entree.verre_applique = true;
            }
            logging::info(&format!(
                "Effet DWM « {} » appliqué à la fenêtre {hwnd:?}.",
                effet.nom()
            ));
        }
        Err(e) => logging::erreur(&format!(
            "Impossible d'appliquer l'effet DWM « {} » à {hwnd:?} : {e}",
            effet.nom()
        )),
    }
}

/// Retire l'effet DWM appliqué (restaure le backdrop d'origine et le verre).
fn retirer_effet_interne(etat: &mut EtatApp, hwnd: HWND) {
    let cle = hwnd.0 as isize;
    if let Some(modifiee) = etat.fenetres.get_mut(&cle) {
        if modifiee.backdrop_origine.is_some() || modifiee.verre_applique {
            dwm::restaurer(hwnd, modifiee.backdrop_origine, modifiee.verre_applique);
            modifiee.backdrop_origine = None;
            modifiee.verre_applique = false;
            logging::info(&format!("Effet DWM retiré de la fenêtre {hwnd:?}."));
        }
    }
}

/// Restaure l'apparence d'origine d'une fenêtre modifiée.
unsafe fn restaurer_interne(hwnd: HWND, modifiee: &FenetreModifiee) {
    if !IsWindow(Some(hwnd)).as_bool() {
        return; // la fenêtre a été détruite entre-temps
    }
    // Restaure l'effet DWM d'abord...
    dwm::restaurer(hwnd, modifiee.backdrop_origine, modifiee.verre_applique);
    // ...puis l'opacité à 100 %...
    if modifiee.layered_ajoute {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        // ...et enfin on retire WS_EX_LAYERED si c'est nous qui l'avions ajouté.
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, modifiee.exstyle_origine);
    } else if let (Some(a), true) = (modifiee.alpha_origine, modifiee.opacite_courante != 100) {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA);
    }
}

/// Raccourci « bascule » : rend transparente la fenêtre sous le curseur,
/// ou la restaure si elle l'est déjà.
pub fn basculer_fenetre_sous_curseur() {
    avec_etat(|etat| {
        if !etat.actif {
            return;
        }
        let Some(hwnd) = fenetre_sous_curseur() else {
            return;
        };
        let cle = hwnd.0 as isize;
        let deja_transparente = etat
            .fenetres
            .get(&cle)
            .map(|f| f.layered_ajoute)
            .unwrap_or(false);
        if deja_transparente {
            if let Some(modifiee) = etat.fenetres.remove(&cle) {
                unsafe { restaurer_interne(hwnd, &modifiee) };
            }
            logging::info(&format!("Fenêtre {hwnd:?} restaurée (bascule)."));
        } else {
            let opacite = etat.opacite;
            appliquer_interne(etat, hwnd, opacite);
        }
    });
}

/// Raccourcis « + / - » : ajuste l'opacité de la fenêtre sous le curseur
/// par pas de 10 %. À 100 %, la fenêtre est complètement restaurée.
pub fn ajuster_opacite_sous_curseur(delta: i16) {
    avec_etat(|etat| {
        if !etat.actif {
            return;
        }
        let Some(hwnd) = fenetre_sous_curseur() else {
            return;
        };
        let cle = hwnd.0 as isize;
        let opacite_actuelle = etat
            .fenetres
            .get(&cle)
            .filter(|f| f.layered_ajoute)
            .map(|f| f.opacite_courante);
        if let Some(actuelle) = opacite_actuelle {
            let nouvelle =
                (actuelle as i16 + delta).clamp(OPACITE_MIN as i16, 100) as u8;
            if nouvelle >= 100 {
                // Retour à l'opacité normale : restauration complète.
                if let Some(modifiee) = etat.fenetres.remove(&cle) {
                    unsafe { restaurer_interne(hwnd, &modifiee) };
                }
                logging::info(&format!("Fenêtre {hwnd:?} restaurée (opacité 100 %)."));
            } else {
                appliquer_interne(etat, hwnd, nouvelle);
            }
        } else if delta < 0 {
            // Fenêtre pas encore transparente : « - » applique l'opacité par défaut.
            let opacite = etat.opacite;
            appliquer_interne(etat, hwnd, opacite);
        }
    });
}

/// Applique un effet DWM à la fenêtre sous le curseur
/// (paramètres, liste « Effet DWM », action différée de 3 secondes).
pub fn appliquer_effet_sous_curseur(effet: EffetDwm) {
    avec_etat(|etat| {
        if !etat.actif {
            return;
        }
        let Some(hwnd) = fenetre_sous_curseur() else {
            return;
        };
        if effet == EffetDwm::Aucun {
            retirer_effet_interne(etat, hwnd);
        } else {
            appliquer_effet_interne(etat, hwnd, effet);
        }
    });
}

/// Applique le premier profil correspondant à une fenêtre pas encore
/// traitée par le scanner. Retourne vrai si un profil a été appliqué.
/// La fenêtre est marquée « traitée » qu'une règle s'applique ou non
/// (sauf si une animation la masque : réessai au prochain scan).
pub fn appliquer_profil_automatique(hwnd: HWND) -> bool {
    let mut applique = false;
    avec_etat(|etat| {
        let cle = hwnd.0 as isize;
        if etat.traitees.contains(&cle) {
            return;
        }
        // Masquée par une animation : NON marquée « traitée », le scanner
        // réessaiera au prochain passage (500 ms).
        if crate::animations::fenetre_masquee_par_animation(hwnd) {
            return;
        }
        etat.traitees.insert(cle);
        applique = appliquer_profil_interne(etat, hwnd) == IssueProfil::Appliquee;
    });
    applique
}

/// Variante événementielle de [`appliquer_profil_automatique`] (appelée
/// dès l'affichage d'une fenêtre, voir `reactivite.rs`) : la fenêtre n'est
/// marquée « traitée » QUE si une règle lui a été appliquée. Sinon (aucun
/// profil ne correspond, titre pas encore connu, transparence inactive,
/// fenêtre invisible ou réduite…), elle reste à évaluer par le scan de
/// secours, dont le comportement est inchangé. Exception : une fenêtre
/// exclue (classe, processus, plein écran) est marquée comme le ferait le
/// scan. Retourne vrai si une règle a été appliquée.
pub fn appliquer_profil_evenementiel(hwnd: HWND) -> bool {
    let mut applique = false;
    avec_etat(|etat| {
        let cle = hwnd.0 as isize;
        if etat.traitees.contains(&cle) {
            return;
        }
        if crate::animations::fenetre_masquee_par_animation(hwnd) {
            return;
        }
        // Invisible ou réduite au moment de l'événement : rien de décidé,
        // le scan (ou un prochain SHOW) réévaluera.
        if unsafe { !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() } {
            return;
        }
        // Fenêtres éphémères (infobulles, menus, popups sans barre de
        // titre, fenêtres outils / non activables) : non traitées ici, le
        // scan garde son comportement.
        if unsafe { est_ephemere(hwnd) } {
            return;
        }
        // Le titre est souvent vide au SHOW : si un profil dépend du titre,
        // le résultat pourrait changer une fois le titre connu -> on laisse
        // le scan trancher tant que le titre est vide.
        if etat
            .config
            .profils
            .iter()
            .any(|p| !p.titre_contient.is_empty())
            && unsafe { titre_fenetre(hwnd) }.is_empty()
        {
            return;
        }
        match appliquer_profil_interne(etat, hwnd) {
            IssueProfil::Appliquee => {
                applique = true;
                etat.traitees.insert(cle);
            }
            // Exclusion (classe, processus, plein écran) : décision
            // définitive, identique à celle du scan ; marquée pour ne pas
            // réévaluer (ni rejournaliser « Fenêtre ignorée ») à chaque SHOW
            // d'une fenêtre du shell qui s'affiche et se masque souvent.
            // Sauf le plein écran seul : état transitoire au SHOW, non
            // marqué (le scan réévaluera).
            IssueProfil::Exclue => {
                let exclusion_definitive = unsafe {
                    explorer::classe_est_exclue(&nom_classe(hwnd), &etat.config.exclusions_classes)
                        || nom_processus(hwnd).is_some_and(|p| {
                            explorer::processus_est_exclu(&p, &etat.config.exclusions_processus)
                        })
                        || !est_plein_ecran(hwnd)
                };
                if exclusion_definitive {
                    etat.traitees.insert(cle);
                }
            }
            IssueProfil::Rien => {}
        }
    });
    applique
}

/// Fenêtre éphémère ou auxiliaire à ne pas traiter par le chemin
/// événementiel : `WS_EX_TOOLWINDOW`, `WS_EX_NOACTIVATE`, classes de menus
/// (`#32768`) et d'infobulles (`tooltips_class32`), popups sans `WS_CAPTION`.
unsafe fn est_ephemere(hwnd: HWND) -> bool {
    let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if exstyle & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
        return true;
    }
    let classe = nom_classe(hwnd);
    if classe == "#32768" || classe.eq_ignore_ascii_case("tooltips_class32") {
        return true;
    }
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    style & WS_POPUP.0 != 0 && style & WS_CAPTION.0 != WS_CAPTION.0
}

/// Cœur commun aux deux chemins (scan et événement) : cherche la première
/// règle (profil utilisateur, puis règle intégrée de l'Explorateur) et
/// l'applique. Ne touche pas à `traitees`.
fn appliquer_profil_interne(etat: &mut EtatApp, hwnd: HWND) -> IssueProfil {
    let mut applique = false;
    {
        if !etat.actif {
            return IssueProfil::Rien;
        }
        if unsafe { est_exclue(hwnd, &etat.config, etat.fenetre_message) } {
            return IssueProfil::Exclue;
        }
        let (processus, classe, titre) = infos_fenetre(hwnd);
        let profil_trouve = etat
            .config
            .profils
            .iter()
            .find(|p| profils::profil_correspond(p, &processus, &classe, &titre))
            .cloned();
        if let Some(profil) = profil_trouve {
            logging::info(&format!(
                "Profil « {} » appliqué à {hwnd:?} ({processus} / {classe}).",
                profil.nom
            ));
            if let Some(opacite) = profil.opacite {
                if opacite < 100 {
                    appliquer_interne(etat, hwnd, opacite);
                }
            }
            if let Some(effet) = EffetDwm::depuis_nom(&profil.effet_dwm) {
                if effet != EffetDwm::Aucun {
                    appliquer_effet_interne(etat, hwnd, effet);
                }
            }
            applique = true;
        } else if etat.config.transparence_explorateur
            && processus.eq_ignore_ascii_case("explorer.exe")
            && (classe.eq_ignore_ascii_case("CabinetWClass")
                || classe.eq_ignore_ascii_case("ExploreWClass"))
        {
            // Règle intégrée : les fenêtres de l'Explorateur sont rendues
            // transparentes avec l'opacité de base (priorité moindre que
            // les profils utilisateur).
            let opacite = etat.opacite;
            if opacite < 100 {
                logging::info(&format!(
                    "Explorateur détecté ({classe}) : transparence automatique \
                     appliquée ({opacite} %) à {hwnd:?}."
                ));
                appliquer_interne(etat, hwnd, opacite);
                applique = true;
            }
        }
    }
    if applique {
        IssueProfil::Appliquee
    } else {
        IssueProfil::Rien
    }
}

/// Issue de [`appliquer_profil_interne`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueProfil {
    /// Un profil ou la règle de l'Explorateur a été appliqué.
    Appliquee,
    /// Fenêtre exclue (`est_exclue` : notre fenêtre, invisible/réduite,
    /// classe ou processus exclu, plein écran).
    Exclue,
    /// Transparence inactive, ou aucune règle ne correspond.
    Rien,
}

/// Supprime de l'ensemble des fenêtres traitées les HWND détruits,
/// afin de pouvoir re-traiter une fenêtre réutilisant le même handle.
pub fn nettoyer_fenetres_traitees() {
    avec_etat(|etat| {
        etat.traitees
            .retain(|cle| unsafe { IsWindow(Some(HWND(*cle as *mut core::ffi::c_void))).as_bool() });
    });
}

/// Interrupteur général (paramètres + double-clic sur l'icône).
/// La désactivation restaure immédiatement toutes les fenêtres.
pub fn basculer_activation() {
    avec_etat(|etat| {
        etat.actif = !etat.actif;
        logging::info(&format!(
            "Transparence globale : {}.",
            if etat.actif { "activée" } else { "désactivée" }
        ));
        if !etat.actif {
            restaurer_toutes_interne(etat);
        } else {
            // Réactivation : on réévalue les règles automatiques
            // (profils + Explorateur) au prochain scan.
            etat.traitees.clear();
        }
    });
}

/// Indique si la transparence est actuellement activée.
pub fn est_actif() -> bool {
    let mut actif = false;
    ETAT.with(|cellule| {
        if let Some(etat) = cellule.borrow().as_ref() {
            actif = etat.actif;
        }
    });
    actif
}

/// Raccourcis de la configuration en mémoire (ceux enregistrés au
/// démarrage ou au dernier rechargement) : rétablis après une capture
/// abandonnée dans la fenêtre de paramètres.
pub fn raccourcis_actifs() -> Raccourcis {
    ETAT.with(|cellule| match cellule.try_borrow() {
        Ok(etat) => etat.as_ref().map(|e| e.config.raccourcis.clone()).unwrap_or_default(),
        Err(_) => Raccourcis::default(),
    })
}

/// Remplace la configuration en mémoire (après « Recharger la configuration »).
pub fn remplacer_config(config: Config) {
    avec_etat(|etat| {
        let opacite = config.opacite_defaut;
        etat.opacite = opacite;
        etat.traitees.clear(); // re-évalue les profils au prochain scan
        etat.config = config;
    });
    logging::info("Configuration rechargée.");
}

fn restaurer_toutes_interne(etat: &mut EtatApp) {
    let fenetres: Vec<(isize, FenetreModifiee)> = etat.fenetres.drain().collect();
    let nombre = fenetres.len();
    for (cle, modifiee) in fenetres {
        unsafe {
            restaurer_interne(HWND(cle as *mut core::ffi::c_void), &modifiee)
        };
    }
    if nombre > 0 {
        logging::info(&format!("{nombre} fenêtre(s) restaurée(s)."));
    }
}

/// Remet l'état d'origine de toutes les fenêtres (arrêt, fin de session).
pub fn restaurer_toutes() {
    avec_etat(restaurer_toutes_interne);
}
