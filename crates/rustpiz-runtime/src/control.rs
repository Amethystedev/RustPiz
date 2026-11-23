//! Cycle de vie d'une session : démarrage, tube de contrôle, arrêt.

use crate::bindings::{IVisualTreeService3, IVisualTreeServiceCallback2, IXamlDiagnostics};
use crate::hook::Hook;
use crate::{backend, log, tap, thumbnails};
use rustpiz_protocol::diagnostic::{self, AdviseStatus};
use rustpiz_protocol::{
    pipe_name, Request, Response, RuntimeConfig, StatusReport, ThumbnailStatus, PROTOCOL_VERSION,
};
use rustpiz_xaml::{Mode, RuleStat};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, RwLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, CO_E_NOTINITIALIZED, ERROR_ACCESS_DENIED, ERROR_MORE_DATA,
    ERROR_PIPE_BUSY, ERROR_SUCCESS, HANDLE, HLOCAL,
    HMODULE, HWND, LPARAM, LRESULT, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::Com::{
    CoGetApartmentType, CoInitializeEx, CoUninitialize, APTTYPE, APTTYPEQUALIFIER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GetModuleHandleW, GetProcAddress, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, REG_DWORD, REG_SAM_FLAGS, REG_VALUE_TYPE,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcessToken,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, EnumThreadWindows, RegisterWindowMessageW, SendMessageTimeoutW,
    SetWindowsHookExW, UnhookWindowsHookEx, CWPSTRUCT, HC_ACTION, SMTO_ABORTIFHUNG, SMTO_NORMAL,
    WH_CALLWNDPROC,
};
use windows_core::{w, Interface, PWSTR};

const JOB_REPORT: usize = 1;
const JOB_ROLLBACK: usize = 2;
const UI_TIMEOUT_MS: u32 = 5000;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CONFIG: RwLock<Option<Arc<RuntimeConfig>>> = RwLock::new(None);
static UI_THREADS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static THUMBNAILS: Mutex<ThumbnailStatus> = Mutex::new(ThumbnailStatus {
    requested: false,
    active: false,
    message: None,
    activity: None,
});
static JOB_RESULT: LazyLock<Mutex<StatusReport>> = LazyLock::new(Mutex::default);
/// Issue de `AdviseVisualTreeChange` pour la session courante (rapport d'état).
static ADVISE_STATUS: Mutex<Option<AdviseStatus>> = Mutex::new(None);
/// Interceptions du contournement de registre pendant la tentative d'Advise
/// en cours (preuve que XAML est allé jusqu'à la lecture de la valeur).
static REG_OPEN_HITS: AtomicU32 = AtomicU32::new(0);
static REG_QUERY_HITS: AtomicU32 = AtomicU32::new(0);
static UI_MESSAGE: LazyLock<u32> =
    LazyLock::new(|| unsafe { RegisterWindowMessageW(w!("RustPiz_RunOnUiThread")) });

struct Session {
    service: IVisualTreeService3,
    watcher: IVisualTreeServiceCallback2,
}

// Ces objets ne sont utilisés que pour Advise et Unadvise, comme dans le Styler.
unsafe impl Send for Session {}

struct SendComObject<T>(T);
unsafe impl<T> Send for SendComObject<T> {}

impl<T> SendComObject<T> {
    /// Passe par une méthode pour que la closure capture l'enveloppe entière.
    fn into_inner(self) -> T {
        self.0
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn config() -> Option<Arc<RuntimeConfig>> {
    CONFIG.read().ok().and_then(|guard| guard.clone())
}

/// Mémorise le thread XAML appelant, pour y exécuter le rollback.
pub fn record_ui_thread() {
    // Par session : `start` vide la liste, et une session suivante dans le même
    // explorer doit retrouver ce thread pour pouvoir y restaurer l'état.
    thread_local! {
        static RECORDED_FOR: Cell<u64> = const { Cell::new(0) };
    }
    let current = generation();
    if RECORDED_FOR.replace(current) != current {
        let thread = unsafe { GetCurrentThreadId() };
        UI_THREADS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread);
        // Une ligne par thread et par session : borné.
        log::write(format!(
            "session {current} : thread XAML {thread} enregistré (premier événement de l'arbre visuel reçu)"
        ));
    }
}

fn load_config() -> Result<RuntimeConfig, String> {
    let path = log::data_dir()
        .ok_or("LOCALAPPDATA absent")?
        .join("runtime")
        .join("active.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: RuntimeConfig = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if config.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "protocole {} attendu, {} reçu",
            PROTOCOL_VERSION, config.protocol
        ));
    }
    Ok(config)
}

/// Empêche le déchargement : XAML garde des pointeurs vers nos délégués.
fn pin_module() {
    let mut module = HMODULE::default();
    let address = crate::module_handle() as *const u16;
    let _ = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            PCWSTR(address),
            &mut module,
        )
    };
}

/// Démarrage en cours (exclusion mutuelle de [`start`] : deux `SetSite`
/// concurrents ne peuvent pas créer deux sessions).
static STARTING: AtomicBool = AtomicBool::new(false);
/// Un fil d'Advise existe encore (éventuellement bloqué dans
/// `AdviseVisualTreeChange`) : aucune nouvelle session n'en lance un autre.
static ADVISE_BUSY: AtomicBool = AtomicBool::new(false);
/// Session dont le fil d'Advise est en cours (journal du refus).
static ADVISE_BUSY_SESSION: AtomicU64 = AtomicU64::new(0);
/// Sérialise les boucles de tentatives d'Advise (hooks de registre partagés).
static ADVISE_LOCK: Mutex<()> = Mutex::new(());

/// Remet `STARTING` à faux à la sortie de [`start`], même par panique.
struct StartGuard;

impl StartGuard {
    fn acquire() -> Option<Self> {
        STARTING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(StartGuard)
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        STARTING.store(false, Ordering::Release);
    }
}

/// Jeton « fil d'Advise vivant » : libéré (Drop) quand le fil se termine,
/// y compris après une panique interceptée, ou si le fil n'a pas pu naître.
struct AdviseBusy;

impl AdviseBusy {
    fn acquire() -> Option<Self> {
        ADVISE_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(AdviseBusy)
    }
}

impl Drop for AdviseBusy {
    fn drop(&mut self) {
        ADVISE_BUSY.store(false, Ordering::Release);
    }
}

/// Démarre une session. Une erreur laisse la barre des tâches intacte.
pub fn start(diagnostics: IXamlDiagnostics) {
    pin_module();
    let config = match load_config() {
        Ok(config) => Arc::new(config),
        Err(message) => {
            log::write(format!(
                "configuration illisible, rien n'est appliqué : {message}"
            ));
            return;
        }
    };
    // Démarrage atomique : un seul `start` à la fois.
    let Some(_starting) = StartGuard::acquire() else {
        log::write(format!(
            "un démarrage de session est déjà en cours : nouvelle demande (thème {:?}) ignorée",
            config.theme.name
        ));
        return;
    };
    if is_active() {
        log::write(format!(
            "une session (n° {}, thème {:?}) est déjà active : nouvelle demande (thème {:?}) ignorée",
            generation(),
            crate::control::config().map(|c| c.theme.name.clone()).unwrap_or_default(),
            config.theme.name
        ));
        return;
    }
    // Jamais deux fils d'Advise : l'ancien peut être bloqué dans l'appel.
    let Some(advise_busy) = AdviseBusy::acquire() else {
        log::write(format!(
            "le fil d'Advise de la session {} est encore dans AdviseVisualTreeChange : nouvelle \
             session (thème {:?}) refusée ; si cela persiste, redémarrez l'Explorateur",
            ADVISE_BUSY_SESSION.load(Ordering::Acquire),
            config.theme.name
        ));
        return;
    };

    // Contexte de SetSite (journal de diagnostic ; Advise n'y est PAS appelé,
    // voir `advise_loop`).
    log::write(format!(
        "SetSite : thread {} ({}), {} fenêtre(s) sur ce thread, IXamlDiagnostics {:p}, DLL {} \
         chargée à {:#x}",
        unsafe { GetCurrentThreadId() },
        apartment_description(),
        thread_window_count(unsafe { GetCurrentThreadId() }),
        diagnostics.as_raw(),
        toolchain(),
        crate::module_handle()
    ));

    let watcher = tap::new_watcher(diagnostics.clone());
    let service = match diagnostics.cast::<IVisualTreeService3>() {
        Ok(service) => service,
        Err(error) => {
            log::write(format!("IVisualTreeService3 indisponible : {error}"));
            return;
        }
    };
    log::write(format!(
        "SetSite : IVisualTreeService3 obtenu ({:p}) ; observateur {:p} (IVisualTreeServiceCallback : {})",
        service.as_raw(),
        watcher.as_raw(),
        match tap::as_base(&watcher) {
            Ok(base) => format!("{:p}", base.as_raw()),
            Err(error) => format!("REFUSÉ {error}"),
        }
    ));

    GENERATION.fetch_add(1, Ordering::AcqRel);
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(config.clone());
    UI_THREADS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(Session {
        service: service.clone(),
        watcher: watcher.clone(),
    });
    *LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *ADVISE_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(AdviseStatus::default());
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // Impossible tant que `STARTING` est respecté ; refus prudent.
        log::write("session déjà marquée active pendant le démarrage : demande ignorée");
        return;
    }
    log::write(format!(
        "session {} démarrée : thème {:?}, {} règles, {} variable(s) de ressources, mode {:?}, miniatures {}",
        generation(),
        config.theme.name,
        config.theme.rules.len(),
        config.theme.resources.len(),
        config.mode,
        if config.thumbnails.is_some() { "demandées" } else { "non demandées" }
    ));

    let thumbnails_status = match (&config.thumbnails, config.mode) {
        (Some(request), Mode::Apply) => thumbnails::install(request),
        (Some(_), Mode::DryRun) => ThumbnailStatus {
            requested: true,
            active: false,
            message: Some("dry-run : hooks non installés".to_owned()),
            activity: None,
        },
        (None, _) => ThumbnailStatus::default(),
    };
    *THUMBNAILS.lock().unwrap_or_else(|e| e.into_inner()) = thumbnails_status;

    // Appeler Advise depuis le thread XAML peut bloquer : thread dédié.
    let advise = SendComObject((service, watcher));
    let session = generation();
    ADVISE_BUSY_SESSION.store(session, Ordering::Release);
    let spawned = std::thread::Builder::new()
        .name("rustpiz-advise".to_owned())
        .spawn(move || {
            // Ordre de libération (inverse de la déclaration) : interfaces COM,
            // puis CoUninitialize (`com`), puis le jeton « fil vivant ».
            let _busy = advise_busy;
            let mut com = ComApartment::default();
            let (service, watcher) = advise.into_inner();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                advise_loop(session, &service, &watcher, &mut com)
            }));
            drop(watcher);
            drop(service);
            drop(com);
            if outcome.is_err() {
                log::write(format!(
                    "session {session} : panique interceptée pendant AdviseVisualTreeChange — le \
                     thème ne sera PAS appliqué"
                ));
                set_advise_status(session, |status| {
                    status.termine = true;
                    status.message = "panique interceptée".to_owned();
                });
            }
        });
    if let Err(error) = spawned {
        // Le jeton a été libéré avec la closure.
        log::write(format!(
            "session {session} : fil d'Advise non créé ({error}) — le thème ne sera PAS appliqué"
        ));
        set_advise_status(session, |status| {
            status.termine = true;
            status.message = format!("fil d'Advise non créé : {error}");
        });
    }

    let pipe = std::thread::Builder::new()
        .name("rustpiz-pipe".to_owned())
        .spawn(|| {
            if catch_unwind(pipe_server).is_err() {
                log::write("panique interceptée dans le serveur de tube");
            }
        });
    if let Err(error) = pipe {
        log::write(format!("serveur de tube non créé : {error}"));
    }
}

/// Initialisation COM du fil d'Advise, annulée (CoUninitialize) au Drop,
/// y compris après une panique. Les interfaces doivent être libérées AVANT.
#[derive(Default)]
struct ComApartment {
    initialized: bool,
}

impl ComApartment {
    /// Entre en MTA (une seule fois) ; renvoie le résultat pour le journal.
    fn enter_mta(&mut self) -> Option<windows_core::HRESULT> {
        if self.initialized {
            return None;
        }
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_FALSE (déjà initialisé) compte aussi : il doit être équilibré.
        self.initialized = hr.is_ok();
        Some(hr)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

// ---- Abonnement à l'arbre visuel et diagnostic (RustPiz) ----
//
// Retour utilisateur v0.6.0 (build 26300) : Advise renvoyait E_UNEXPECTED à
// chaque session, sans autre information. Chaque tentative est désormais
// journalisée (thread, apartement, HRESULT, durée, interceptions du registre,
// rappels reçus) et refaite jusqu'à 4 fois (0, 200, 500, 1000 ms ; la
// dernière depuis un thread explicitement en MTA). Advise n'est jamais appelé
// sur le thread de SetSite : Windhawk et TranslucentTB documentent un
// interblocage (`Advising::RunOnUIThread`) qui figerait la barre des tâches.

/// Chaîne d'outils qui a produit cette DLL.
fn toolchain() -> &'static str {
    if cfg!(target_env = "msvc") {
        "MSVC"
    } else if cfg!(target_env = "gnu") {
        "GNU/MinGW"
    } else {
        "?"
    }
}

/// Apartement COM du thread courant, pour le journal.
fn apartment_description() -> String {
    let mut kind = APTTYPE::default();
    let mut qualifier = APTTYPEQUALIFIER::default();
    match unsafe { CoGetApartmentType(&mut kind, &mut qualifier) } {
        Ok(()) => format!(
            "apartement {} / {}",
            diagnostic::nom_apartement(kind.0),
            diagnostic::nom_qualificatif(qualifier.0)
        ),
        Err(error) if error.code() == CO_E_NOTINITIALIZED => {
            "COM non initialisé sur ce thread".to_owned()
        }
        Err(error) => format!("apartement inconnu ({error})"),
    }
}

unsafe extern "system" fn count_window(_window: HWND, out: LPARAM) -> windows_core::BOOL {
    unsafe { *(out.0 as *mut u32) += 1 };
    true.into()
}

/// Nombre de fenêtres de premier niveau d'un thread (un thread XAML en a).
fn thread_window_count(thread: u32) -> u32 {
    let mut count = 0u32;
    let _ = unsafe {
        EnumThreadWindows(thread, Some(count_window), LPARAM(&mut count as *mut u32 as isize))
    };
    count
}

/// Modules suspects chargés dans ce processus (autres consommateurs XAML,
/// autres copies de la DLL), pour le journal : (ligne, mention des suspects
/// forts éventuels).
fn suspect_modules() -> (String, Option<String>) {
    let snapshot = match unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, GetCurrentProcessId())
    } {
        Ok(snapshot) => snapshot,
        Err(error) => return (format!("liste des modules indisponible ({error})"), None),
    };
    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let wide = |buffer: &[u16]| {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    };
    let mut modules = Vec::new();
    let mut more = unsafe { Module32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        modules.push((wide(&entry.szModule), wide(&entry.szExePath)));
        more = unsafe { Module32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
    let suspects = diagnostic::modules_suspects(
        modules.iter().map(|(name, path)| (name.as_str(), path.as_str())),
    );
    (
        format!(
            "{} module(s) chargé(s), suspects : {}",
            modules.len(),
            diagnostic::decrire_suspects(&suspects)
        ),
        diagnostic::mention_suspects_forts(&suspects),
    )
}

/// La session `session` est-elle toujours la session active ?
fn session_current(session: u64) -> bool {
    is_active() && generation() == session
}

/// Met à jour l'issue d'Advise, seulement si `session` est toujours courante.
fn set_advise_status(session: u64, update: impl FnOnce(&mut AdviseStatus)) {
    if generation() != session {
        return;
    }
    let mut guard = ADVISE_STATUS.lock().unwrap_or_else(|e| e.into_inner());
    update(guard.get_or_insert_with(AdviseStatus::default));
}

/// Tentatives d'`AdviseVisualTreeChange` (thread dédié de la session).
fn advise_loop(
    session: u64,
    service: &IVisualTreeService3,
    watcher: &IVisualTreeServiceCallback2,
    com: &mut ComApartment,
) {
    let thread = unsafe { GetCurrentThreadId() };
    let total = diagnostic::DELAIS_ADVISE_MS.len();
    // Une seule boucle de tentatives à la fois dans le processus.
    let _serialized = ADVISE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Hooks de registre installés une fois pour toute la boucle, retirés au
    // Drop (fin de boucle, `break` ou panique).
    let _hooks = InstalledRegistryHooks::install(session);
    let mut attempt = 1usize;
    while let Some(delay) = diagnostic::delai_avant_tentative(attempt) {
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        if !session_current(session) {
            log::write(format!(
                "session {session} : tentatives d'Advise abandonnées (session arrêtée ou remplacée)"
            ));
            break;
        }
        if diagnostic::tentative_en_mta_explicite(attempt) {
            if let Some(hr) = com.enter_mta() {
                log::write(format!(
                    "session {session} : CoInitializeEx(MTA) avant la tentative {attempt} : {hr:?}"
                ));
            }
        }
        REG_OPEN_HITS.store(0, Ordering::Release);
        REG_QUERY_HITS.store(0, Ordering::Release);
        let callbacks_before = tap::callback_count();
        let started = std::time::Instant::now();
        let result = advise_with_diag_workaround(service, watcher);
        let elapsed = started.elapsed().as_millis();
        let callbacks = tap::callback_count().saturating_sub(callbacks_before);
        let hresult = match &result {
            Ok(()) => 0,
            Err(error) => error.code().0,
        };
        let outcome = match &result {
            Ok(()) => "S_OK".to_owned(),
            Err(error) => format!("{:#010X} ({})", error.code().0 as u32, error.message()),
        };
        let message = format!(
            "tentative {attempt}/{total} — thread {thread} ({}) : {outcome} en {elapsed} ms ; \
             registre intercepté : {} ouverture(s) XAML\\Debug, {} lecture(s) \
             DisableCompositionDiag ; rappels de l'arbre reçus pendant l'appel : {callbacks}",
            apartment_description(),
            REG_OPEN_HITS.load(Ordering::Acquire),
            REG_QUERY_HITS.load(Ordering::Acquire),
        );
        log::write(format!("session {session} : AdviseVisualTreeChange {message}"));
        let succeeded = result.is_ok();
        let retry = !succeeded && diagnostic::reessayer_advise(attempt, session_current(session));
        set_advise_status(session, |status| {
            status.tentatives = attempt as u32;
            status.hresult = hresult;
            status.reussi = succeeded;
            status.termine = !retry;
            status.message = message.clone();
        });
        if succeeded && !session_current(session) {
            // Arrêt survenu pendant l'appel : `stop` a déjà tenté Unadvise
            // (sans effet) et libéré sa référence. On se désabonne ici ; la
            // référence de ce thread est volontairement conservée (fuite
            // bornée) car XAML appelle l'observateur sans en détenir.
            let unadvise = tap::as_base(watcher)
                .and_then(|base| unsafe { service.UnadviseVisualTreeChange(&base) });
            log::write(format!(
                "session {session} : Advise réussi APRÈS l'arrêt de la session ; désabonnement : \
                 {unadvise:?}"
            ));
            std::mem::forget(watcher.clone());
            break;
        }
        if succeeded {
            log::write(format!(
                "session {session} : AdviseVisualTreeChange réussi à la tentative {attempt} \
                 (l'arbre existant est rejoué de façon asynchrone)"
            ));
            break;
        }
        if !retry {
            let (modules, strong) = suspect_modules();
            log::write(format!(
                "session {session} : AdviseVisualTreeChange a échoué après {attempt} tentative(s) \
                 ({:#010X}) — le thème ne sera PAS appliqué. Cause possible : {}{}",
                hresult as u32,
                diagnostic::cause_probable_advise(hresult),
                strong.map(|m| format!(" ; {m}")).unwrap_or_default()
            ));
            log::write(format!(
                "session {session} : modules d'explorer au moment de l'échec : {modules}"
            ));
            break;
        }
        attempt += 1;
    }
}

// ---- Contournement du bogue des diagnostics de composition ----
//
// Ces diagnostics corrompent le tas si plusieurs threads XAML ajoutent des
// visuels. Windows ne les crée pas si la valeur de registre HKLM
// `XAML\Debug\DisableCompositionDiag` vaut 1. Elle n'est lue qu'une fois, pendant
// Advise : ce seul thread reçoit donc une réponse simulée, sans aucune écriture
// dans le registre.

thread_local! {
    static REPORT_DIAG_DISABLED: Cell<bool> = const { Cell::new(false) };
}
static REG_OPEN_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static REG_QUERY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

type RegOpenFn =
    unsafe extern "system" fn(HKEY, PCWSTR, u32, REG_SAM_FLAGS, *mut HKEY) -> WIN32_ERROR;
type RegQueryFn = unsafe extern "system" fn(
    HKEY,
    PCWSTR,
    *const u32,
    *mut REG_VALUE_TYPE,
    *mut u8,
    *mut u32,
) -> WIN32_ERROR;

fn wide_equals(text: PCWSTR, expected: &str) -> bool {
    !text.is_null() && unsafe { text.to_string() }.is_ok_and(|s| s.eq_ignore_ascii_case(expected))
}

unsafe extern "system" fn reg_open_detour(
    key: HKEY,
    sub_key: PCWSTR,
    options: u32,
    access: REG_SAM_FLAGS,
    out: *mut HKEY,
) -> WIN32_ERROR {
    let original: RegOpenFn =
        unsafe { std::mem::transmute(REG_OPEN_ORIGINAL.load(Ordering::Acquire)) };
    let result = unsafe { original(key, sub_key, options, access, out) };
    if !REPORT_DIAG_DISABLED.get()
        || key != HKEY_LOCAL_MACHINE
        || !wide_equals(sub_key, r"Software\Microsoft\XAML\Debug")
    {
        return result;
    }
    REG_OPEN_HITS.fetch_add(1, Ordering::AcqRel);
    if result == ERROR_SUCCESS {
        return result;
    }
    // La clé n'existe pas d'habitude : on en ouvre une qui existe.
    unsafe {
        original(
            HKEY_LOCAL_MACHINE,
            w!(r"Software\Microsoft"),
            options,
            access,
            out,
        )
    }
}

unsafe extern "system" fn reg_query_detour(
    key: HKEY,
    name: PCWSTR,
    reserved: *const u32,
    kind: *mut REG_VALUE_TYPE,
    data: *mut u8,
    size: *mut u32,
) -> WIN32_ERROR {
    let original: RegQueryFn =
        unsafe { std::mem::transmute(REG_QUERY_ORIGINAL.load(Ordering::Acquire)) };
    if !REPORT_DIAG_DISABLED.get() || !wide_equals(name, "DisableCompositionDiag") {
        return unsafe { original(key, name, reserved, kind, data, size) };
    }
    REG_QUERY_HITS.fetch_add(1, Ordering::AcqRel);
    const DWORD_SIZE: u32 = 4;
    unsafe {
        if !kind.is_null() {
            *kind = REG_DWORD;
        }
        if !data.is_null() && (size.is_null() || *size < DWORD_SIZE) {
            if !size.is_null() {
                *size = DWORD_SIZE;
            }
            return ERROR_MORE_DATA;
        }
        if !data.is_null() {
            std::ptr::write_unaligned(data.cast::<u32>(), 1);
        }
        if !size.is_null() {
            *size = DWORD_SIZE;
        }
    }
    ERROR_SUCCESS
}

/// Hooks de registre préparés UNE SEULE fois par processus (la DLL est
/// épinglée : ces statiques vivent autant qu'explorer). Préparer à nouveau à
/// chaque tentative fuyait un bloc `VirtualAlloc` par essai et, si un retrait
/// avait échoué, décodait un prologue déjà patché (récursion du détour).
static REGISTRY_HOOKS: OnceLock<Result<Mutex<Vec<Hook>>, String>> = OnceLock::new();

fn prepare_registry_hooks() -> Result<Vec<Hook>, String> {
    // Les adresses d'origine ne sont écrites qu'une fois : jamais pendant
    // qu'un détour peut les lire.
    if REG_OPEN_ORIGINAL.load(Ordering::Acquire) != 0
        || REG_QUERY_ORIGINAL.load(Ordering::Acquire) != 0
    {
        return Err("hooks de registre déjà préparés dans ce processus".to_owned());
    }
    let module = unsafe { GetModuleHandleW(w!("kernelbase.dll")) }.map_err(|e| e.to_string())?;
    let resolve = |name: windows_core::PCSTR| {
        unsafe { GetProcAddress(module, name) }
            .map(|f| f as usize)
            .ok_or_else(|| "fonction de registre introuvable".to_owned())
    };
    let open = resolve(windows_core::s!("RegOpenKeyExW"))?;
    let query = resolve(windows_core::s!("RegQueryValueExW"))?;
    let open_hook = unsafe { Hook::prepare(open, reg_open_detour as *const () as usize) }?;
    let query_hook = unsafe { Hook::prepare(query, reg_query_detour as *const () as usize) }?;
    REG_OPEN_ORIGINAL.store(open_hook.trampoline(), Ordering::Release);
    REG_QUERY_ORIGINAL.store(query_hook.trampoline(), Ordering::Release);
    Ok(vec![open_hook, query_hook])
}

/// Hooks de registre du processus (préparés au premier appel).
fn registry_hooks() -> Result<&'static Mutex<Vec<Hook>>, &'static str> {
    REGISTRY_HOOKS
        .get_or_init(|| prepare_registry_hooks().map(Mutex::new))
        .as_ref()
        .map_err(String::as_str)
}

/// Hooks de registre installés pour la durée d'une boucle de tentatives ;
/// retirés au Drop (y compris pendant le déroulement d'une panique).
struct InstalledRegistryHooks {
    hooks: MutexGuard<'static, Vec<Hook>>,
    session: u64,
}

impl InstalledRegistryHooks {
    fn install(session: u64) -> Option<Self> {
        let hooks = match registry_hooks() {
            Ok(hooks) => hooks,
            Err(message) => {
                log::write(format!(
                    "session {session} : contournement des diagnostics indisponible : {message}"
                ));
                return None;
            }
        };
        let mut hooks = hooks.lock().unwrap_or_else(|e| e.into_inner());
        for hook in hooks.iter_mut() {
            if let Err(message) = hook.install() {
                log::write(format!("session {session} : hook de registre : {message}"));
            }
        }
        Some(Self { hooks, session })
    }
}

impl Drop for InstalledRegistryHooks {
    fn drop(&mut self) {
        let session = self.session;
        for hook in self.hooks.iter_mut() {
            if let Err(message) = hook.uninstall() {
                // Laissé en place : le détour transmet tout appel à la
                // fonction d'origine hors de la tentative d'Advise, et la
                // prochaine boucle réutilise ce même hook (jamais re-préparé).
                log::write(format!(
                    "session {session} : retrait du hook de registre : {message} (hook laissé \
                     en place, inoffensif hors d'Advise)"
                ));
            }
        }
    }
}

/// Un appel d'`AdviseVisualTreeChange` avec la réponse simulée du registre
/// pour CE thread (les hooks sont installés par [`InstalledRegistryHooks`]).
fn advise_with_diag_workaround(
    service: &IVisualTreeService3,
    watcher: &IVisualTreeServiceCallback2,
) -> windows_core::Result<()> {
    REPORT_DIAG_DISABLED.set(true);
    let result =
        tap::as_base(watcher).and_then(|base| unsafe { service.AdviseVisualTreeChange(&base) });
    REPORT_DIAG_DISABLED.set(false);
    result
}

// ---- Exécution sur les threads XAML ----

unsafe extern "system" fn ui_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let message = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if message.message == *UI_MESSAGE {
            let job = message.wParam.0;
            let _ = catch_unwind(|| run_job(job));
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn run_job(job: usize) {
    let report = match job {
        JOB_REPORT => backend::thread_report(),
        JOB_ROLLBACK => backend::thread_rollback(),
        _ => None,
    };
    if let Some(report) = report {
        JOB_RESULT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .merge(report);
    }
}

unsafe extern "system" fn first_window(window: HWND, out: LPARAM) -> windows_core::BOOL {
    unsafe { *(out.0 as *mut HWND) = window };
    false.into()
}

/// Lance `job` sur un thread XAML, sans jamais bloquer indéfiniment.
fn run_on_thread(thread: u32, job: usize) -> bool {
    let mut window = HWND::default();
    let _ = unsafe {
        EnumThreadWindows(
            thread,
            Some(first_window),
            LPARAM(&mut window as *mut HWND as isize),
        )
    };
    if window.is_invalid() {
        return false;
    }
    let Ok(hook) = (unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(ui_hook), None, thread) })
    else {
        return false;
    };
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            *UI_MESSAGE,
            WPARAM(job),
            LPARAM(0),
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            UI_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    let _ = unsafe { UnhookWindowsHookEx(hook) };
    sent.0 != 0
}

fn run_on_ui_threads(job: usize) -> StatusReport {
    *JOB_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = StatusReport::default();
    let threads: Vec<u32> = UI_THREADS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .collect();
    let mut unresponsive = 0;
    for thread in threads {
        if !run_on_thread(thread, job) {
            unresponsive += 1;
            log::write(format!("thread XAML {thread} injoignable"));
        }
    }
    let mut report = std::mem::take(&mut *JOB_RESULT.lock().unwrap_or_else(|e| e.into_inner()));
    report.unresponsive_threads += unresponsive;
    report.thumbnails = THUMBNAILS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    report.thumbnails.activity = thumbnails::activity();
    if let Some(activity) = &report.thumbnails.activity {
        log::write(format!("miniatures : {activity}"));
    }
    report.session = generation();
    report.advise = ADVISE_STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(config) = config() {
        report.theme = config.theme.name.clone();
        report.dry_run = config.mode == Mode::DryRun;
        if report.rules.is_empty() {
            report.rules = config
                .theme
                .rules
                .iter()
                .map(|rule| RuleStat {
                    source_index: rule.source_index,
                    target: rule.selector.raw.clone(),
                    ..RuleStat::default()
                })
                .collect();
        }
    }
    report
}

/// Ligne de synthèse d'un rapport (sert aussi de signature anti-répétition).
fn report_summary(report: &StatusReport) -> String {
    let applied: u32 = report.rules.iter().map(|r| r.applied).sum();
    let failed: u32 = report.rules.iter().map(|r| r.failed).sum();
    let matched = report.rules.iter().filter(|r| r.matched > 0).count();
    format!(
        "session {} thème {:?} : threads XAML {} (injoignables {}), éléments stylés {}, \
         règles avec correspondance {}/{}, valeurs appliquées {applied}, échecs {failed}, \
         échecs d'écriture {}, disjoncteur {}, miniatures {}",
        report.session,
        report.theme,
        report.ui_threads,
        report.unresponsive_threads,
        report.tracked_elements,
        matched,
        report.rules.len(),
        report.write_failures,
        if report.tripped { "DÉCLENCHÉ" } else { "non" },
        match (report.thumbnails.requested, report.thumbnails.active) {
            (true, true) => "hooks actifs",
            (true, false) => "demandées mais INACTIVES",
            (false, _) => "non demandées",
        }
    )
}

/// Dernière synthèse journalisée pour `Status` (une session vérifiée
/// périodiquement ne fait pas grossir `runtime.log` à chaque requête).
static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);

/// Journalise un rapport d'état seulement s'il a changé depuis le dernier.
fn log_status_report(report: &StatusReport) {
    let summary = report_summary(report);
    let mut signature = summary.clone();
    for rule in &report.rules {
        if let Some(error) = &rule.last_error {
            signature.push_str(error);
        }
    }
    {
        let mut last = LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() == Some(signature.as_str()) {
            return;
        }
        *last = Some(signature);
    }
    log::write(format!("rapport d'état : {summary}"));
    log_report(report);
}

fn log_report(report: &StatusReport) {
    for rule in report.unmatched() {
        log::write(format!("aucun élément pour : {}", rule.target));
    }
    for rule in &report.rules {
        if let Some(error) = &rule.last_error {
            log::write(format!(
                "{} : {} échec(s), dernier : {error}",
                rule.target, rule.failed
            ));
        }
    }
    if report.tripped {
        log::write("disjoncteur déclenché : état d'origine restauré");
    }
}

/// Arrête tout, dans l'ordre inverse du démarrage.
fn stop() -> StatusReport {
    ACTIVE.store(false, Ordering::Release);
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(session) = &session {
        if let Ok(base) = tap::as_base(&session.watcher) {
            if let Err(error) = unsafe { session.service.UnadviseVisualTreeChange(&base) } {
                log::write(format!("UnadviseVisualTreeChange : {error}"));
            }
        }
    }
    let report = run_on_ui_threads(JOB_ROLLBACK);
    // XAML appelle le watcher sans détenir de référence. Une fois que les threads
    // XAML ont traité un message, plus aucun appel n'est en cours : on peut le
    // libérer, et il faut le faire avant toute nouvelle injection.
    drop(session);
    thumbnails::uninstall();
    log::write(format!("arrêt, bilan avant restauration : {}", report_summary(&report)));
    log_report(&report);
    log::write("session arrêtée");
    report
}

// ---- Tube de contrôle ----

/// Autorise uniquement l'utilisateur courant.
fn user_only_descriptor() -> Result<PSECURITY_DESCRIPTOR, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| e.to_string())?;
        let mut length = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        let filled = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            length,
            &mut length,
        );
        let _ = CloseHandle(token);
        filled.map_err(|e| e.to_string())?;

        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid_text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_text).map_err(|e| e.to_string())?;
        let sid = sid_text.to_string().map_err(|e| e.to_string())?;
        let _ = LocalFree(Some(HLOCAL(sid_text.0.cast::<c_void>())));

        let sddl = windows_core::HSTRING::from(format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|e| e.to_string())?;
        Ok(descriptor)
    }
}

/// Essais de création du tube : l'instance du serveur de la session
/// précédente peut être en train de se fermer (arrêt puis réinjection
/// immédiate) ; `FILE_FLAG_FIRST_PIPE_INSTANCE` la refuserait.
const PIPE_CREATE_ATTEMPTS: u32 = 20;
const PIPE_CREATE_RETRY_MS: u64 = 100;

/// Crée l'instance unique du tube. `FILE_FLAG_FIRST_PIPE_INSTANCE` : si un
/// autre processus a déjà créé un tube de ce nom (squat), la création échoue
/// au lieu d'ajouter une instance à un tube qui ne nous appartient pas.
fn create_pipe_instance(name: &windows_core::HSTRING, attributes: &SECURITY_ATTRIBUTES) -> Option<HANDLE> {
    for attempt in 1..=PIPE_CREATE_ATTEMPTS {
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                1 << 20,
                1 << 16,
                0,
                Some(attributes),
            )
        };
        if !handle.is_invalid() {
            return Some(handle);
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_ACCESS_DENIED && error != ERROR_PIPE_BUSY {
            log::write(format!("CreateNamedPipeW a échoué ({error:?})"));
            return None;
        }
        if attempt == PIPE_CREATE_ATTEMPTS {
            log::write(format!(
                "CreateNamedPipeW refusé ({error:?}) après {attempt} essais : une autre instance \
                 du tube existe (serveur précédent ou autre processus) — tube de contrôle NON créé"
            ));
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(PIPE_CREATE_RETRY_MS));
    }
    None
}

fn pipe_server() {
    let descriptor = match user_only_descriptor() {
        Ok(descriptor) => descriptor,
        Err(message) => {
            log::write(format!("tube non créé, droits non déterminés : {message}"));
            return;
        }
    };
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = windows_core::HSTRING::from(pipe_name(unsafe { GetCurrentProcessId() }));

    loop {
        let Some(handle) = create_pipe_instance(&name, &attributes) else {
            return;
        };
        // Déjà connecté (ERROR_PIPE_CONNECTED) compte aussi comme un succès.
        let _ = unsafe { ConnectNamedPipe(handle, None) };

        let stop_requested = serve_client(handle);
        unsafe {
            let _ = DisconnectNamedPipe(handle);
            let _ = CloseHandle(handle);
        }
        if stop_requested {
            return;
        }
    }
}

/// Traite une requête. Renvoie vrai après un arrêt.
fn serve_client(handle: HANDLE) -> bool {
    // ManuallyDrop : le handle est fermé par l'appelant.
    let mut file = std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(handle.0) });
    let mut line = String::new();
    {
        let limited = (&*file).take(1 << 16);
        if BufReader::new(limited).read_line(&mut line).is_err() {
            return false;
        }
    }

    let request: Result<Request, _> = serde_json::from_str(line.trim());
    let mut stopping = false;
    let response = match request {
        Err(error) => Response::Error(format!("requête invalide : {error}")),
        Ok(Request::Hello) => Response::Hello {
            protocol: PROTOCOL_VERSION,
            pid: unsafe { GetCurrentProcessId() },
        },
        Ok(Request::Status) => {
            let report = run_on_ui_threads(JOB_REPORT);
            log_status_report(&report);
            Response::Status(report)
        }
        Ok(Request::SetThumbnails(request)) => {
            if config().is_some_and(|config| config.mode == Mode::Apply) {
                *THUMBNAILS.lock().unwrap_or_else(|e| e.into_inner()) =
                    thumbnails::configure(request.as_ref());
                Response::Status(run_on_ui_threads(JOB_REPORT))
            } else {
                Response::Error("aucune session d'application active".to_owned())
            }
        }
        Ok(Request::Disable) => {
            stopping = true;
            Response::Disabled(stop())
        }
    };

    if let Ok(mut text) = serde_json::to_string(&response) {
        text.push('\n');
        let _ = file.write_all(text.as_bytes());
        // `File::flush` ne fait rien sous Windows. `sync_all` appelle
        // FlushFileBuffers, qui attend que le client ait lu : sinon
        // DisconnectNamedPipe détruit la réponse (erreur 233 côté client).
        let _ = file.sync_all();
    }
    stopping
}
