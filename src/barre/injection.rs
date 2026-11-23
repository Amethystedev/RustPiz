//! Opérations Windows : déploiement de la DLL, chargement dans explorer par
//! les diagnostics XAML, détection des conflits, résolution des symboles de
//! miniatures. Porté de RustHawk (`windows_ops.rs`), adapté à RustPiz.
//!
//! Rien n'est jamais écrit sur `explorer.exe` ni `Taskbar.View.dll` : ces
//! fichiers sont seulement lus. La DLL est extraite dans
//! `%LOCALAPPDATA%\RustPiz\runtime\`, jamais dans le dossier système.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use rustpiz_protocol::{ThumbnailHook, TAP_CLSID};
use rustpiz_thumbnails::ThumbnailConfig;

use windows::core::{s, w, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NOT_FOUND, ERROR_SEM_TIMEOUT, ERROR_SUCCESS,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::Networking::WinInet::DeleteUrlCacheEntryW;
use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
    MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};

use super::orchestration::arret_demande;
use crate::logging;

/// Premier build utilisant les fonctions détournées pour les miniatures.
pub const BUILD_MIN_MINIATURES: u32 = 26100;
/// Premier build Windows 11 pris en charge pour les thèmes de barre.
pub const BUILD_MIN_BARRE: u32 = 22621;

const SERVEUR_SYMBOLES: &str = "https://msdl.microsoft.com/download/symbols";
/// Durée maximale d'un téléchargement de PDB (plusieurs dizaines de Mio).
const DELAI_TELECHARGEMENT: std::time::Duration = std::time::Duration::from_secs(600);
const MODULES_MINIATURES: [&str; 2] = ["taskbar.view.dll", "explorerextensions.dll"];

type Ixde = unsafe extern "system" fn(PCWSTR, u32, PCWSTR, PCWSTR, GUID, PCWSTR) -> HRESULT;

/// Dossier de données de RustPiz (`%LOCALAPPDATA%\RustPiz`).
pub fn dossier_donnees() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("RustPiz"))
        .ok_or_else(|| "LOCALAPPDATA n'est pas défini".to_owned())
}

fn registre_dword(cle: PCWSTR, valeur: PCWSTR) -> Option<u32> {
    let mut donnee = 0u32;
    let mut taille = 4u32;
    let statut = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            cle,
            valeur,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut donnee as *mut u32).cast::<c_void>()),
            Some(&mut taille),
        )
    };
    (statut == ERROR_SUCCESS).then_some(donnee)
}

/// Renvoie `(build, révision)` depuis le registre.
pub fn build_windows() -> Result<(u32, u32), String> {
    const CLE: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let mut tampon = [0u16; 32];
    let mut taille = std::mem::size_of_val(&tampon) as u32;
    let statut = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CLE,
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(tampon.as_mut_ptr().cast::<c_void>()),
            Some(&mut taille),
        )
    };
    if statut != ERROR_SUCCESS {
        return Err(format!("lecture du build impossible : {statut:?}"));
    }
    let fin = tampon.iter().position(|&c| c == 0).unwrap_or(tampon.len());
    let build = String::from_utf16_lossy(&tampon[..fin])
        .parse()
        .map_err(|_| "build illisible".to_owned())?;
    Ok((build, registre_dword(CLE, w!("UBR")).unwrap_or(0)))
}

/// Smart App Control actif ? (peut bloquer la DLL non signée.)
pub fn smart_app_control_actif() -> bool {
    registre_dword(
        w!(r"SYSTEM\CurrentControlSet\Control\CI\Policy"),
        w!("VerifiedAndReputablePolicyState"),
    ) == Some(1)
}

/// Pid de l'explorer qui détient la barre des tâches.
///
/// Le propriétaire de `Shell_TrayWnd` doit être `explorer.exe` (nom du
/// fichier image, `QueryFullProcessImageNameW`) : une fenêtre de même classe
/// créée par un autre processus (shell de remplacement, logiciel tiers) ne
/// doit jamais recevoir la DLL. Refus journalisé une fois par pid.
pub fn explorer_pid() -> Option<u32> {
    let fenetre = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok()?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(fenetre, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    match image_processus(pid) {
        Ok(image) if super::decision::est_image_explorer(&image) => Some(pid),
        resultat => {
            static DERNIER_REFUS: std::sync::atomic::AtomicU32 =
                std::sync::atomic::AtomicU32::new(0);
            if DERNIER_REFUS.swap(pid, std::sync::atomic::Ordering::Relaxed) != pid {
                logging::avertir(&format!(
                    "Barre : Shell_TrayWnd appartient au pid {pid} qui n'est pas explorer.exe ({}) : \
                     intégration ignorée.",
                    match resultat {
                        Ok(image) => image,
                        Err(e) => format!("image illisible : {e}"),
                    }
                ));
            }
            None
        }
    }
}

/// Chemin complet de l'image d'un processus.
fn image_processus(pid: u32) -> Result<String, String> {
    unsafe {
        let processus = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| e.to_string())?;
        let mut tampon = [0u16; 1024];
        let mut taille = tampon.len() as u32;
        let issue = QueryFullProcessImageNameW(
            processus,
            PROCESS_NAME_WIN32,
            PWSTR(tampon.as_mut_ptr()),
            &mut taille,
        );
        let _ = CloseHandle(processus);
        issue.map_err(|e| e.to_string())?;
        let taille = (taille as usize).min(tampon.len());
        Ok(String::from_utf16_lossy(&tampon[..taille]))
    }
}

fn texte_large(tampon: &[u16]) -> String {
    let fin = tampon.iter().position(|&c| c == 0).unwrap_or(tampon.len());
    String::from_utf16_lossy(&tampon[..fin])
}

fn modules_de(pid: u32) -> Result<Vec<(String, PathBuf)>, String> {
    let instantane =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) }
            .map_err(|e| format!("liste des modules d'explorer : {e}"))?;
    let mut entree = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let mut modules = Vec::new();
    let mut encore = unsafe { Module32FirstW(instantane, &mut entree) }.is_ok();
    while encore {
        modules.push((
            texte_large(&entree.szModule),
            PathBuf::from(texte_large(&entree.szExePath)),
        ));
        encore = unsafe { Module32NextW(instantane, &mut entree) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(instantane);
    }
    Ok(modules)
}

fn processus_lance(nom: &str) -> bool {
    let Ok(instantane) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entree = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut trouve = false;
    let mut encore = unsafe { Process32FirstW(instantane, &mut entree) }.is_ok();
    while encore && !trouve {
        trouve = texte_large(&entree.szExeFile).eq_ignore_ascii_case(nom);
        encore = unsafe { Process32NextW(instantane, &mut entree) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(instantane);
    }
    trouve
}

/// Le tube nommé `nom` existe-t-il ? (`WaitNamedPipeW`, 1 ms : aucune
/// connexion n'est ouverte, le serveur n'est pas dérangé.)
fn tube_existe(nom: &str) -> bool {
    let large = HSTRING::from(nom);
    if unsafe { WaitNamedPipeW(PCWSTR(large.as_ptr()), 1) }.as_bool() {
        return true;
    }
    // Instance occupée : le tube existe quand même.
    let erreur = unsafe { GetLastError() };
    erreur == ERROR_SEM_TIMEOUT
}

/// Examen d'explorer avant une injection.
pub struct Examen {
    /// Consommateur des diagnostics XAML incompatible (bloquant avant une
    /// nouvelle injection), s'il y en a.
    pub conflit: Option<String>,
    /// Suspect fort (DLL runtime de RustHawk chargée) : non bloquant, cité
    /// dans le message si `AdviseVisualTreeChange` échoue.
    pub suspect_fort: Option<String>,
    /// Ligne de journal : modules suspects, tube RustHawk, processus.
    pub journal: String,
}

/// Un autre consommateur des diagnostics XAML est-il présent ? Un seul est
/// autorisé par processus.
///
/// Bloquants : le processus Windhawk (son moteur s'injecte dans explorer) ou
/// son module, une session RustHawk active (son tube de contrôle), et les
/// modules TranslucentTB (ExplorerTAP) / ExplorerBlurMica seulement s'ils
/// sont CHARGÉS dans explorer (reconnus au nom de fichier du module, pas au
/// chemin : leur processus seul ne suffit pas). La DLL runtime de RustHawk
/// chargée sans session est un suspect fort, non bloquant.
///
/// Journalise aussi les modules suspects chargés dans explorer (autres TAP,
/// logiciels de personnalisation, copies de la DLL RustPiz).
pub fn examiner_explorer(pid: u32) -> Examen {
    use rustpiz_protocol::diagnostic::{
        conflit_charge, decrire_suspects, mention_suspects_forts, modules_suspects, Gravite,
    };

    let modules = modules_de(pid);
    let suspects = match &modules {
        Ok(modules) => {
            let chemins: Vec<(String, String)> = modules
                .iter()
                .map(|(nom, chemin)| (nom.clone(), chemin.to_string_lossy().into_owned()))
                .collect();
            modules_suspects(chemins.iter().map(|(n, c)| (n.as_str(), c.as_str())))
        }
        Err(_) => Vec::new(),
    };
    let notre_dll =
        super::decision::nom_dll(super::decision::empreinte(super::DLL_RUNTIME)).to_lowercase();
    let notre_dll_chargee = suspects
        .iter()
        .any(|s| s.gravite == Gravite::RustPiz && s.module.to_lowercase() == notre_dll);
    let autres_rustpiz = suspects
        .iter()
        .filter(|s| s.gravite == Gravite::RustPiz && s.module.to_lowercase() != notre_dll)
        .count();
    let session_rusthawk = tube_existe(&format!(r"\\.\pipe\rusthawk.{pid}"));
    let rusthawk_lance = processus_lance("RustHawk.exe") || processus_lance("rusthawk-cli.exe");
    let windhawk_lance = processus_lance("windhawk.exe");
    let windhawk = windhawk_lance || conflit_charge(&suspects, "Windhawk");
    let translucenttb = conflit_charge(&suspects, "TranslucentTB");
    let blurmica = conflit_charge(&suspects, "ExplorerBlurMica");
    let suspect_fort = mention_suspects_forts(&suspects);

    let journal = format!(
        "explorer (pid {pid}) : {} ; modules suspects : {} ; cette DLL RustPiz ({notre_dll}) \
         {} ; {autres_rustpiz} autre(s) copie(s) RustPiz chargée(s) (inertes) ; tube RustHawk \
         {} ; processus RustHawk {} ; processus Windhawk {}",
        match &modules {
            Ok(m) => format!("{} module(s)", m.len()),
            Err(e) => format!("modules illisibles ({e})"),
        },
        decrire_suspects(&suspects),
        if notre_dll_chargee {
            "déjà chargée (session précédente : réutilisée)"
        } else {
            "pas encore chargée"
        },
        if session_rusthawk {
            "PRÉSENT (session RustHawk active)"
        } else {
            "absent"
        },
        if rusthawk_lance { "lancé" } else { "absent" },
        if windhawk_lance { "lancé" } else { "absent" },
    );

    let conflit = if windhawk {
        Some("Windhawk".to_owned())
    } else if translucenttb {
        Some("TranslucentTB (module chargé dans explorer)".to_owned())
    } else if blurmica {
        Some("ExplorerBlurMica (module chargé dans explorer)".to_owned())
    } else if session_rusthawk {
        Some("RustHawk (session active dans cet Explorateur : désactivez-la)".to_owned())
    } else {
        None
    };
    Examen {
        conflit,
        suspect_fort,
        journal,
    }
}

fn module_miniatures(pid: u32) -> Result<(String, PathBuf), String> {
    let modules = modules_de(pid)?;
    MODULES_MINIATURES
        .iter()
        .find_map(|voulu| {
            modules
                .iter()
                .find(|(nom, _)| nom.eq_ignore_ascii_case(voulu))
                .cloned()
        })
        .ok_or_else(|| {
            "ni Taskbar.View.dll ni ExplorerExtensions.dll ne sont chargés dans explorer".to_owned()
        })
}

/// Le module XAML de la barre (Taskbar.View.dll ou ExplorerExtensions.dll)
/// est-il chargé dans explorer ? Sert d'indice « barre prête » (comme
/// `wait_for_taskbar` de RustHawk).
pub fn barre_chargee(pid: u32) -> bool {
    module_miniatures(pid).is_ok()
}

/// Retire l'URL du cache WinINet (best effort) : un téléchargement
/// précédent tronqué ne doit pas être resservi par `URLDownloadToFileW`.
fn oublier_cache(url: &HSTRING) {
    unsafe {
        let _ = DeleteUrlCacheEntryW(PCWSTR(url.as_ptr()));
    }
}

/// Télécharge `url` vers `partiel` dans un fil éphémère : l'attente est
/// abandonnée si l'arrêt est demandé (le fil, bloqué dans
/// `URLDownloadToFileW`, se termine seul ; son fichier partiel, propre au
/// processus, n'est jamais adopté).
fn telecharger(url: &str, partiel: &Path) -> Result<(), String> {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    let (url_large, fichier_large) = (HSTRING::from(url), HSTRING::from(partiel));
    oublier_cache(&url_large);
    let (envoi, reception) = channel();
    std::thread::Builder::new()
        .name("rustpiz-pdb".to_owned())
        .spawn(move || {
            let issue = unsafe {
                URLDownloadToFileW(
                    None,
                    PCWSTR(url_large.as_ptr()),
                    PCWSTR(fichier_large.as_ptr()),
                    0,
                    None,
                )
            }
            .map_err(|e| format!("téléchargement du PDB : {e}"));
            // Le fichier est copié hors du cache : l'entrée n'a plus d'utilité.
            oublier_cache(&url_large);
            let _ = envoi.send(issue);
        })
        .map_err(|e| format!("fil de téléchargement non créé : {e}"))?;
    let debut = std::time::Instant::now();
    loop {
        match reception.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(issue) => return issue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("téléchargement du PDB interrompu".to_owned())
            }
            Err(RecvTimeoutError::Timeout) => {
                if arret_demande() {
                    return Err("téléchargement du PDB abandonné : arrêt demandé".to_owned());
                }
                if debut.elapsed() >= DELAI_TELECHARGEMENT {
                    return Err("téléchargement du PDB trop long (abandonné)".to_owned());
                }
            }
        }
    }
}

/// Télécharge le PDB si besoin, puis trouve les fonctions à détourner.
///
/// Le PDB est téléchargé vers un fichier `.part` propre au processus,
/// vérifié (`symboles::verifier_pdb` : taille, format, GUID) puis seulement
/// renommé ; sinon il est supprimé. Un PDB déjà présent mais invalide (copie
/// d'une version antérieure) est supprimé puis retéléchargé.
pub fn resoudre_hook_miniatures(pid: u32, config: ThumbnailConfig) -> Result<ThumbnailHook, String> {
    let (nom_module, chemin_module) = module_miniatures(pid)?;
    let image = std::fs::read(&chemin_module).map_err(|e| format!("{}: {e}", chemin_module.display()))?;
    let info = super::pe::analyser(&image)
        .ok_or_else(|| format!("{} : PE ou info de débogage illisible", chemin_module.display()))?;
    if !super::pe::nom_pdb_valide(&info.nom_pdb) {
        return Err(format!(
            "{} : nom de PDB refusé ({:?})",
            chemin_module.display(),
            info.nom_pdb
        ));
    }
    // GUID seul (32 chiffres), sans l'âge, pour la vérification du PDB.
    let guid = info.id_pdb.get(..32).unwrap_or(&info.id_pdb).to_owned();

    let dossier = dossier_donnees()?
        .join("symbols")
        .join(&info.nom_pdb)
        .join(&info.id_pdb);
    let chemin_pdb = dossier.join(&info.nom_pdb);
    if chemin_pdb.exists() {
        if let Err(raison) = super::symboles::verifier_pdb(&chemin_pdb, &guid) {
            logging::avertir(&format!(
                "Barre : PDB en cache invalide ({raison}), suppression et nouveau téléchargement."
            ));
            let _ = std::fs::remove_file(&chemin_pdb);
        }
    }
    if !chemin_pdb.exists() {
        if arret_demande() {
            return Err("arrêt demandé".to_owned());
        }
        std::fs::create_dir_all(&dossier).map_err(|e| e.to_string())?;
        let url = format!("{SERVEUR_SYMBOLES}/{0}/{1}/{0}", info.nom_pdb, info.id_pdb);
        logging::info(&format!("Barre : téléchargement du PDB public ({url})."));
        // Fichiers partiels d'essais précédents (best effort ; un fichier
        // encore ouvert par un téléchargement abandonné reste en place).
        if let Ok(entrees) = std::fs::read_dir(&dossier) {
            for entree in entrees.flatten() {
                let nom = entree.file_name();
                let nom = nom.to_string_lossy();
                if nom.starts_with("download") && nom.ends_with(".part") {
                    let _ = std::fs::remove_file(entree.path());
                }
            }
        }
        let partiel = dossier.join(format!("download-{}.part", std::process::id()));
        let verification = telecharger(&url, &partiel)
            .and_then(|()| super::symboles::verifier_pdb(&partiel, &guid));
        if let Err(raison) = verification {
            let _ = std::fs::remove_file(&partiel);
            return Err(raison);
        }
        if let Err(e) = std::fs::rename(&partiel, &chemin_pdb) {
            let _ = std::fs::remove_file(&partiel);
            return Err(format!("{}: {e}", chemin_pdb.display()));
        }
    }

    let rvas = super::symboles::localiser(&chemin_pdb)?;
    Ok(ThumbnailHook {
        config,
        module_name: nom_module,
        timestamp: info.horodatage,
        size_of_image: info.taille_image,
        scaled_size_rva: rvas.taille_echelle,
        on_apply_template_rva: rvas.on_apply_template,
    })
}

/// Déploie la DLL embarquée : une version déjà chargée par explorer reste
/// verrouillée, donc on écrit une copie nommée par empreinte FNV (atomique).
/// Une copie existante n'est réutilisée que si sa taille ET son contenu sont
/// identiques à l'image embarquée (sinon elle est réécrite). Les anciennes
/// copies `rustpiz_runtime-*.dll` non verrouillées sont supprimées (best
/// effort ; celles chargées dans un explorer restent jusqu'à son redémarrage).
pub fn deployer_dll() -> Result<PathBuf, String> {
    let octets = super::DLL_RUNTIME;
    let dossier = dossier_donnees()?.join("runtime");
    std::fs::create_dir_all(&dossier).map_err(|e| e.to_string())?;
    let nom = super::decision::nom_dll(super::decision::empreinte(octets));
    let cible = dossier.join(&nom);
    let identique = std::fs::metadata(&cible).is_ok_and(|m| m.len() == octets.len() as u64)
        && std::fs::read(&cible).is_ok_and(|existant| existant == octets);
    if !identique {
        if cible.exists() {
            logging::avertir(&format!(
                "Barre : {} différente de l'image embarquée, réécriture.",
                cible.display()
            ));
        }
        let temporaire = cible.with_extension("dll.part");
        std::fs::write(&temporaire, octets).map_err(|e| format!("{}: {e}", temporaire.display()))?;
        if let Err(e) = std::fs::rename(&temporaire, &cible) {
            let _ = std::fs::remove_file(&temporaire);
            return Err(format!("{}: {e}", cible.display()));
        }
    }
    if let Ok(entrees) = std::fs::read_dir(&dossier) {
        for entree in entrees.flatten() {
            let nom_entree = entree.file_name();
            let Some(nom_entree) = nom_entree.to_str() else { continue };
            if super::decision::est_ancienne_dll(nom_entree, &nom)
                && std::fs::remove_file(entree.path()).is_ok()
            {
                logging::info(&format!("Barre : ancienne copie {nom_entree} supprimée."));
            }
        }
    }
    Ok(cible)
}

/// Écrit `active.json` dans le dossier runtime (écriture atomique).
pub fn ecrire_active_json(contenu: &str) -> Result<(), String> {
    let dossier = dossier_donnees()?.join("runtime");
    std::fs::create_dir_all(&dossier).map_err(|e| e.to_string())?;
    let cible = dossier.join("active.json");
    let temporaire = cible.with_extension("json.part");
    std::fs::write(&temporaire, contenu).map_err(|e| format!("{}: {e}", temporaire.display()))?;
    std::fs::rename(&temporaire, &cible).map_err(|e| format!("{}: {e}", cible.display()))
}

/// Échec de l'injection : message et HRESULT éventuel (pour reconnaître un
/// blocage par Smart App Control / WDAC).
pub struct ErreurInjection {
    pub message: String,
    pub code: Option<i32>,
}

impl ErreurInjection {
    fn texte(message: impl Into<String>) -> Self {
        ErreurInjection {
            message: message.into(),
            code: None,
        }
    }
}

/// Demande à XAML de charger la DLL dans explorer. Refusé si l'arrêt de
/// RustPiz est demandé (vérifié avant et pendant l'essai des connexions).
pub fn injecter(pid: u32, dll: &Path) -> Result<(), ErreurInjection> {
    if arret_demande() {
        return Err(ErreurInjection::texte("arrêt demandé : injection refusée"));
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
        let module = LoadLibraryExW(w!("Windows.UI.Xaml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
            .map_err(|e| ErreurInjection::texte(format!("Windows.UI.Xaml.dll : {e}")))?;
        let fonction = GetProcAddress(module, s!("InitializeXamlDiagnosticsEx"))
            .ok_or_else(|| ErreurInjection::texte("InitializeXamlDiagnosticsEx introuvable"))?;
        let initialiser: Ixde = std::mem::transmute(fonction);
        let chemin = HSTRING::from(dll);

        // Le numéro de connexion libre n'est pas connu : on les essaie.
        for essai in 1..=10_000 {
            if arret_demande() {
                return Err(ErreurInjection::texte("arrêt demandé : injection refusée"));
            }
            let nom = HSTRING::from(format!("VisualDiagConnection{essai}"));
            let resultat = initialiser(
                PCWSTR(nom.as_ptr()),
                pid,
                w!(""),
                PCWSTR(chemin.as_ptr()),
                GUID::from_u128(TAP_CLSID),
                PCWSTR::null(),
            );
            if resultat != ERROR_NOT_FOUND.to_hresult() {
                return resultat.ok().map_err(|e| ErreurInjection {
                    message: format!("InitializeXamlDiagnosticsEx : {e}"),
                    code: Some(resultat.0),
                });
            }
        }
    }
    Err(ErreurInjection::texte("aucune connexion de diagnostic XAML disponible"))
}
