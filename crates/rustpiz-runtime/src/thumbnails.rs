//! Détours de `Taskbar.View.dll` pour la taille des miniatures.

use crate::bindings::FrameworkElement;
use crate::hook::Hook;
use crate::log;
use rustpiz_protocol::{ThumbnailHook, ThumbnailStatus};
use rustpiz_thumbnails::{Size, ThumbnailConfig};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};
use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_core::{IInspectable, IUnknown, Interface, HSTRING};

const THUMBNAIL_VIEW_CLASS: &str = "Taskbar.TaskItemThumbnailView";

type ScaledFn = unsafe extern "system" fn(*mut Size, Size, f32) -> *mut Size;
type ApplyTemplateFn = unsafe extern "system" fn(*mut c_void);

static SCALED_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static APPLY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static CONFIG: RwLock<Option<ThumbnailConfig>> = RwLock::new(None);
static HOOKS: Mutex<Vec<Hook>> = Mutex::new(Vec::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SCALED_CALLS: AtomicU32 = AtomicU32::new(0);
static TEMPLATE_CALLS: AtomicU32 = AtomicU32::new(0);
static VIEWS_UNCAPPED: AtomicU32 = AtomicU32::new(0);
/// Dernier appel, en bits de `f32` : entrée (l, h), échelle, sortie (l, h).
static LAST_CALL: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];

fn record_scaled_call(input: Size, scale: f32, output: Size) {
    SCALED_CALLS.fetch_add(1, Ordering::Relaxed);
    let values = [
        input.width,
        input.height,
        scale,
        output.width,
        output.height,
    ];
    for (slot, value) in LAST_CALL.iter().zip(values) {
        slot.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Résumé de l'activité des détours, `None` si les hooks sont inactifs.
pub fn activity() -> Option<String> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let last = |index: usize| f32::from_bits(LAST_CALL[index].load(Ordering::Relaxed));
    Some(format!(
        "GetScaledThumbnailSize : {} appel(s), dernier {}x{} échelle {} -> {}x{}. OnApplyTemplate : {} appel(s). Vues sans limite de largeur : {}.",
        SCALED_CALLS.load(Ordering::Relaxed),
        last(0),
        last(1),
        last(2),
        last(3),
        last(4),
        TEMPLATE_CALLS.load(Ordering::Relaxed),
        VIEWS_UNCAPPED.load(Ordering::Relaxed),
    ))
}

/// Retire le plafond des vues déjà créées avant l'installation des hooks.
///
/// Leur `OnApplyTemplate` a déjà été exécuté et ne le sera plus.
pub fn element_added(element: &FrameworkElement) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let is_view = element
        .cast::<IInspectable>()
        .and_then(|inspectable| inspectable.GetRuntimeClassName())
        .is_ok_and(|class| class == THUMBNAIL_VIEW_CLASS);
    if is_view && element.SetMaxWidth(f64::INFINITY).is_ok() {
        VIEWS_UNCAPPED.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "system" fn scaled_detour(out: *mut Size, size: Size, scale: f32) -> *mut Size {
    let original_address = SCALED_ORIGINAL.load(Ordering::Acquire);
    let original: ScaledFn = unsafe { std::mem::transmute(original_address) };
    let config = CONFIG.read().ok().and_then(|guard| guard.clone());
    let Some(config) = config else {
        return unsafe { original(out, size, scale) };
    };
    let computed = catch_unwind(AssertUnwindSafe(|| {
        config.compute(scale, |factor| {
            let mut buffer = Size {
                width: 0.0,
                height: 0.0,
            };
            unsafe { original(&mut buffer, size, factor) };
            buffer
        })
    }));
    match computed {
        Ok(result) => {
            record_scaled_call(size, scale, result);
            unsafe { *out = result };
            out
        }
        Err(_) => unsafe { original(out, size, scale) },
    }
}

unsafe extern "system" fn apply_template_detour(this: *mut c_void) {
    let original_address = APPLY_ORIGINAL.load(Ordering::Acquire);
    let original: ApplyTemplateFn = unsafe { std::mem::transmute(original_address) };
    unsafe { original(this) };
    TEMPLATE_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { remove_max_width(this) }));
}

/// Sans cela, Windows plafonne la largeur de la vue de miniature.
unsafe fn remove_max_width(this: *mut c_void) {
    let raw = unsafe { *this.cast::<*mut c_void>().add(1) };
    let Some(unknown) = (unsafe { IUnknown::from_raw_borrowed(&raw) }) else {
        return;
    };
    if let Ok(element) = unknown.cast::<FrameworkElement>() {
        if element.SetMaxWidth(f64::INFINITY).is_ok() {
            VIEWS_UNCAPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Lit `(TimeDateStamp, SizeOfImage)` depuis un en-tête PE en mémoire.
unsafe fn pe_signature(base: usize) -> (u32, u32) {
    unsafe {
        let nt = base + std::ptr::read_unaligned((base + 0x3C) as *const u32) as usize;
        (
            std::ptr::read_unaligned((nt + 8) as *const u32),
            std::ptr::read_unaligned((nt + 80) as *const u32),
        )
    }
}

/// Installe les hooks si le module chargé est celui analysé par l'hôte.
pub fn install(request: &ThumbnailHook) -> ThumbnailStatus {
    let mut status = ThumbnailStatus {
        requested: true,
        ..ThumbnailStatus::default()
    };
    match try_install(request) {
        Ok(()) => status.active = true,
        Err(message) => {
            log::write(format!("miniatures : {message}"));
            status.message = Some(message);
        }
    }
    status
}

/// Active, met à jour ou retire les hooks pendant une session.
pub fn configure(request: Option<&ThumbnailHook>) -> ThumbnailStatus {
    match request {
        None => {
            uninstall();
            ThumbnailStatus::default()
        }
        Some(request) if ACTIVE.load(Ordering::Acquire) => {
            *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(request.config.clone());
            ThumbnailStatus {
                requested: true,
                active: true,
                ..ThumbnailStatus::default()
            }
        }
        Some(request) => install(request),
    }
}

fn try_install(request: &ThumbnailHook) -> Result<(), String> {
    let name = HSTRING::from(request.module_name.as_str());
    let module = unsafe { GetModuleHandleW(PCWSTR(name.as_ptr())) }
        .map_err(|_| format!("{} n'est pas chargé dans explorer", request.module_name))?;
    let base = module.0 as usize;
    let (timestamp, size) = unsafe { pe_signature(base) };
    if (timestamp, size) != (request.timestamp, request.size_of_image) {
        return Err(format!(
            "{} a changé depuis l'analyse des symboles : hooks annulés",
            request.module_name
        ));
    }

    let mut hooks = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
    if !hooks.is_empty() {
        return Err("hooks déjà installés".to_owned());
    }
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(request.config.clone());

    // Tout est préparé avant la première écriture : un échec ne laisse rien.
    let mut scaled = unsafe {
        Hook::prepare(
            base + request.scaled_size_rva as usize,
            scaled_detour as *const () as usize,
        )
    }?;
    let mut template = unsafe {
        Hook::prepare(
            base + request.on_apply_template_rva as usize,
            apply_template_detour as *const () as usize,
        )
    }?;
    SCALED_ORIGINAL.store(scaled.trampoline(), Ordering::Release);
    APPLY_ORIGINAL.store(template.trampoline(), Ordering::Release);

    scaled.install()?;
    if let Err(message) = template.install() {
        let _ = scaled.uninstall();
        return Err(message);
    }
    hooks.push(scaled);
    hooks.push(template);
    for counter in [&SCALED_CALLS, &TEMPLATE_CALLS, &VIEWS_UNCAPPED] {
        counter.store(0, Ordering::Relaxed);
    }
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}

/// Retire les hooks. Un échec est journalisé.
pub fn uninstall() {
    ACTIVE.store(false, Ordering::Release);
    let mut hooks = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
    for hook in hooks.iter_mut() {
        if let Err(message) = hook.uninstall() {
            log::write(format!("miniatures : retrait impossible : {message}"));
        }
    }
    hooks.clear();
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = None;
}
