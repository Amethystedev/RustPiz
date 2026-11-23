//! DLL chargée dans explorer.exe par les diagnostics XAML.
//!
//! Elle n'exécute rien dans `DllMain`. Tout démarre quand XAML appelle
//! `IObjectWithSite::SetSite`, et aucune panique ne franchit une frontière FFI.

#![cfg(windows)]

mod backend;
#[allow(clippy::all, warnings)]
mod bindings;
mod control;
mod hook;
mod log;
mod tap;
mod thumbnails;

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{HINSTANCE, S_FALSE};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_core::{BOOL, GUID, HRESULT};

static MODULE: AtomicUsize = AtomicUsize::new(0);

/// Adresse de chargement de cette DLL.
pub(crate) fn module_handle() -> usize {
    MODULE.load(Ordering::Relaxed)
}

/// # Safety
/// Appelée uniquement par le chargeur Windows.
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as usize, Ordering::Relaxed);
    }
    true.into()
}

/// # Safety
/// Appelée par COM avec des pointeurs valides.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    class_id: *const GUID,
    interface_id: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe { tap::get_class_object(class_id, interface_id, out) }
}

/// La DLL reste chargée jusqu'à la fin d'explorer : XAML peut encore la rappeler.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
