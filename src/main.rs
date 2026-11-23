//! RustPiz — point d'entrée.
//!
//! Application Windows 11 native, sans fenêtre visible : une icône dans la
//! zone de notification permet de rendre les fenêtres transparentes via
//! l'API Win32 « layered windows » (`WS_EX_LAYERED` +
//! `SetLayeredWindowAttributes`) et d'appliquer des effets DWM modernes
//! (Mica, Acrylique, Verre).
//!
//! Démarrage : journalisation, configuration, vérification de
//! compatibilité Windows 11, instance unique, fenêtre de message cachée,
//! icône tray (+ notification de démarrage), raccourcis globaux, minuteur
//! de scan des profils, puis boucle de messages.

#![windows_subsystem = "windows"]

mod animations;
mod barre;
mod compat;
mod config;
mod dwm;
mod explorer;
mod hotkey;
mod logging;
mod parametres;
mod profils;
mod themes_barre;
mod touches;
mod tray;
mod window;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MessageBoxW, SetProcessDPIAware, TranslateMessage,
    MB_ICONERROR, MB_OK, MSG,
};

/// Version de l'application (affichée dans le journal et les notifications).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Garantit qu'un seul exemplaire de l'application tourne à la fois.
unsafe fn instance_unique() -> bool {
    match CreateMutexW(None, true, w!("Global\\TransparentWindows_InstanceUnique")) {
        Ok(_) => {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                logging::avertir("Une instance est déjà en cours d'exécution : arrêt.");
                false
            } else {
                true
            }
        }
        Err(e) => {
            // En cas d'échec du mutex, on continue quand même.
            logging::avertir(&format!("CreateMutexW a échoué : {e}. Démarrage quand même."));
            true
        }
    }
}

/// Affiche une erreur critique dans une boîte de dialogue
/// (l'application n'a pas de console, c'est le seul canal visible).
unsafe fn erreur_critique(message: &str) {
    logging::erreur(message);
    let message_large: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    MessageBoxW(
        None,
        windows::core::PCWSTR(message_large.as_ptr()),
        w!("RustPiz — Erreur"),
        MB_OK | MB_ICONERROR,
    );
}

fn main() {
    // Le dossier de configuration doit exister avant la journalisation.
    let dossier = config::chemin_dossier();
    let _ = std::fs::create_dir_all(&dossier);
    logging::initialiser(&config::chemin_journal());

    // Les paniques sont consignées dans le journal plutôt que perdues
    // (l'application n'a pas de console).
    let fil_principal = std::thread::current().id();
    std::panic::set_hook(Box::new(move |infos| {
        logging::erreur(&format!("PANIQUE : {infos}"));
        // Aucune fenêtre ne doit rester masquée (transparence temporaire
        // d'une animation d'ouverture) par nous.
        animations::urgence_restaurer();
        // Barre des tâches : seulement pour une panique du fil principal
        // (fin du processus). Une panique d'un autre fil (fil dédié de la
        // barre, rattrapée par `catch_unwind`, fils d'échange…) ne doit pas
        // arrêter l'intégration. Sans verrou bloquant, 500 ms au plus.
        if std::thread::current().id() == fil_principal {
            barre::arreter(barre::ModeArret::Panique);
        }
    }));

    logging::info(&format!("=== Démarrage de RustPiz v{VERSION} ==="));

    // v0.6.0 : contrôle de la DLL d'intégration de la barre des tâches
    // embarquée (injectée plus tard, seulement si une fonction est active).
    match barre::verifier_dll_embarquee() {
        Ok(taille) => logging::info(&format!(
            "DLL d'intégration de la barre des tâches embarquée : {taille} octets (PE x64 valide, \
             construite pour {}).",
            barre::CHAINE_DLL
        )),
        Err(raison) => logging::avertir(&format!(
            "DLL d'intégration de la barre des tâches embarquée invalide : {raison}."
        )),
    }

    let configuration = config::charger();

    unsafe {
        // Coordonnées en pixels réels, correctes sur écrans haute densité.
        let _ = SetProcessDPIAware();

        if !instance_unique() {
            return;
        }

        // Vérification de compatibilité (Windows 11 = build ≥ 22000).
        compat::verifier();

        // Fenêtre de message cachée : reçoit les rappels du tray, les
        // raccourcis globaux et les minuteurs.
        let hwnd = match tray::creer_fenetre_message() {
            Ok(hwnd) => hwnd,
            Err(e) => {
                erreur_critique(&format!(
                    "Impossible de créer la fenêtre de message : {e}"
                ));
                return;
            }
        };

        window::initialiser_etat(configuration.clone(), hwnd);
        animations::initialiser(&configuration);
        tray::ajouter_icone(hwnd);
        hotkey::enregistrer(hwnd, &configuration.raccourcis);
        tray::demarrer_minuteurs(hwnd);

        // Scan immédiat au démarrage : applique les profils et la
        // transparence automatique de l'Explorateur aux fenêtres déjà
        // ouvertes (sans attendre le premier minuteur de 2 secondes).
        profils::scanner();

        // Intégration de la barre des tâches (v0.6.0) : injection dans
        // explorer déléguée à un fil dédié ; aucun effet (ni fil) si la
        // barre et les miniatures sont désactivées.
        barre::appliquer(&configuration);

        // Notification de démarrage : confirme visuellement que
        // l'application tourne (elle n'a ni console ni fenêtre).
        tray::afficher_notification(
            hwnd,
            "RustPiz",
            &format!(
                "v{VERSION} en cours d'exécution.\n\
                 Clic droit sur l'icône puis « Paramètres » pour tout régler, \
                 Ctrl+Alt+T sur une fenêtre pour la rendre transparente."
            ),
        );

        logging::info("Application initialisée, entrée dans la boucle de messages.");

        // Boucle de messages Win32 classique.
        let mut message = MSG::default();
        loop {
            let resultat = GetMessageW(&mut message, None, 0, 0);
            if resultat.0 <= 0 {
                break; // 0 = WM_QUIT reçu, -1 = erreur
            }
            // Navigation clavier (Tab, Entrée, Echap) de la fenêtre de
            // paramètres seulement : message déjà distribué si traité.
            if parametres::traiter_message_dialogue(&message) {
                continue;
            }
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }

        // Nettoyage : on restaure l'état d'origine de toutes les fenêtres
        // avant de quitter, pour ne laisser aucune fenêtre modifiée.
        logging::info("Arrêt demandé, nettoyage...");
        // Ordre : transparence d'abord (immédiat), puis la barre (bornée).
        animations::arreter();
        window::restaurer_toutes();
        barre::arreter(barre::ModeArret::Quitter);
        hotkey::liberer(hwnd);
        tray::retirer_icone(hwnd);
    }

    logging::info("=== Arrêt propre de RustPiz ===");
}
