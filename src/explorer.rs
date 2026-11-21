//! Exclusions liées au shell Windows (barre des tâches, menu Démarrer, etc.).
//!
//! Ces listes évitent de rendre transparentes des fenêtres système sensibles,
//! ce qui rendrait le système difficilement utilisable. En v0.1, il s'agit
//! uniquement de listes de noms de classes de fenêtres et de noms de
//! processus : aucune injection dans `explorer.exe` n'est effectuée.

/// Noms de classes de fenêtres exclues par défaut.
///
/// - `Shell_TrayWnd` / `Shell_SecondaryTrayWnd` : barre des tâches ;
/// - `Windows.UI.Core.CoreWindow` : menu Démarrer, centre de notifications,
///   zone de recherche (hôtes XAML/WinUI) ;
/// - `XamlExplorerHostIslandWindow`, `Windows.UI.Composition.DesktopWindowContentBridge`,
///   `TopLevelWindowForOverflowXamlIsland` : hôtes d'îlots XAML du shell ;
/// - `DV2ControlHost` : menus contextuels du shell ;
/// - `NotifyIconOverflowWindow` : débordement de la zone de notification ;
/// - `Progman` / `WorkerW` : bureau ;
/// - `LockScreenFrame` : écran de verrouillage.
pub fn classes_exclues_defaut() -> Vec<String> {
    [
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
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Noms de processus exclus par défaut (toujours en minuscules).
///
/// - `lockapp.exe` : écran de verrouillage ;
/// - `startmenuexperiencehost.exe`, `searchhost.exe`, `searchapp.exe` :
///   menu Démarrer et recherche Windows 11 ;
/// - `shellexperiencehost.exe` : centre de notifications, horloge ;
/// - `shellinfrastructurehost.exe`, `sihost.exe` : infrastructure du shell ;
/// - `textinputhost.exe` : panneau de saisie (émojis, clavier tactile).
///
/// Remarque : la plupart des jeux tournent en plein écran et sont donc déjà
/// exclus par la détection de fenêtres plein écran (voir `window.rs`).
pub fn processus_exclus_defaut() -> Vec<String> {
    [
        "lockapp.exe",
        "startmenuexperiencehost.exe",
        "searchhost.exe",
        "searchapp.exe",
        "shellexperiencehost.exe",
        "shellinfrastructurehost.exe",
        "sihost.exe",
        "textinputhost.exe",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Indique si un nom de classe figure dans la liste d'exclusions
/// (comparaison insensible à la casse).
pub fn classe_est_exclue(classe: &str, exclusions: &[String]) -> bool {
    exclusions
        .iter()
        .any(|exclue| exclue.eq_ignore_ascii_case(classe))
}

/// Indique si un nom de processus (exe) figure dans la liste d'exclusions
/// (comparaison insensible à la casse).
pub fn processus_est_exclu(processus: &str, exclusions: &[String]) -> bool {
    let processus = processus.to_lowercase();
    exclusions
        .iter()
        .any(|exclue| exclue.to_lowercase() == processus)
}
