//! Objets COM vus par les diagnostics XAML.

use crate::bindings::{
    Add, FrameworkElement, IObjectWithSite, IObjectWithSite_Impl, IVisualTreeServiceCallback,
    IVisualTreeServiceCallback2, IVisualTreeServiceCallback2_Impl, IVisualTreeServiceCallback_Impl,
    IXamlDiagnostics, ParentChildRelation, Remove, VisualElement, VisualElementState,
    VisualMutationType,
};
use crate::{backend, control, log};
use rustpiz_protocol::TAP_CLSID;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use windows::core::{implement, Ref as ComRef};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_NOINTERFACE, E_POINTER};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::{IInspectable, IUnknown, Interface, Ref, BOOL, GUID, HRESULT, PCWSTR};

/// Exécute `body` en bloquant toute panique à la frontière COM.
fn guarded(
    name: &str,
    body: impl FnOnce() -> windows_core::Result<()>,
) -> windows_core::Result<()> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(_) => {
            log::write(format!("panique interceptée dans {name}"));
            Err(E_FAIL.into())
        }
    }
}

/// Rappels `OnVisualTreeChange` reçus depuis le chargement (toutes sessions),
/// pour le diagnostic d'Advise (RustPiz).
static CALLBACKS: AtomicU64 = AtomicU64::new(0);

/// Nombre de rappels de l'arbre visuel reçus depuis le chargement.
pub fn callback_count() -> u64 {
    CALLBACKS.load(Ordering::Acquire)
}

/// Reçoit les changements de l'arbre visuel.
#[implement(IVisualTreeServiceCallback2)]
pub struct Watcher {
    pub diagnostics: IXamlDiagnostics,
}

impl IVisualTreeServiceCallback_Impl for Watcher_Impl {
    fn OnVisualTreeChange(
        &self,
        _relation: &ParentChildRelation,
        element: &VisualElement,
        mutation: VisualMutationType,
    ) -> windows_core::Result<()> {
        CALLBACKS.fetch_add(1, Ordering::AcqRel);
        let handle = element.Handle;
        let result = guarded("OnVisualTreeChange", || {
            if !control::is_active() {
                return Ok(());
            }
            control::record_ui_thread();
            if mutation == Add {
                let object: IInspectable =
                    unsafe { self.diagnostics.GetIInspectableFromHandle(handle) }?;
                if let Ok(framework) = object.cast::<FrameworkElement>() {
                    crate::thumbnails::element_added(&framework);
                    backend::element_added(handle, framework);
                }
            } else if mutation == Remove {
                backend::element_removed(handle);
            }
            Ok(())
        });
        if let Err(error) = result {
            log::write(format!("OnVisualTreeChange : {error}"));
        }
        // Une erreur renvoyée à XAML interromprait les notifications suivantes.
        Ok(())
    }
}

impl IVisualTreeServiceCallback2_Impl for Watcher_Impl {
    fn OnElementStateChanged(
        &self,
        _element: u64,
        _state: VisualElementState,
        _context: &PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
}

/// Point d'entrée : XAML lui transmet son service de diagnostic.
#[implement(IObjectWithSite)]
struct Tap {
    site: Mutex<Option<IUnknown>>,
}

impl IObjectWithSite_Impl for Tap_Impl {
    fn SetSite(&self, site: Ref<IUnknown>) -> windows_core::Result<()> {
        let site = site.as_ref().cloned();
        guarded("SetSite", || {
            *self.site.lock().unwrap_or_else(|e| e.into_inner()) = site.clone();
            match site {
                Some(site) => match site.cast::<IXamlDiagnostics>() {
                    Ok(diagnostics) => control::start(diagnostics),
                    Err(error) => {
                        log::write(format!("SetSite : IXamlDiagnostics indisponible : {error}"));
                        return Err(error);
                    }
                },
                None => log::write("SetSite(nul) : XAML libère le point d'entrée (TAP)"),
            }
            Ok(())
        })
    }

    fn GetSite(&self, interface: *const GUID, out: *mut *mut c_void) -> windows_core::Result<()> {
        if out.is_null() || interface.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *out = std::ptr::null_mut() };
        let guard = self.site.lock().unwrap_or_else(|e| e.into_inner());
        let site = guard.as_ref().ok_or(windows_core::Error::from(E_FAIL))?;
        unsafe { site.query(interface, out) }.ok()
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: ComRef<IUnknown>,
        interface: *const GUID,
        out: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        if outer.is_some() {
            return Err(E_NOINTERFACE.into());
        }
        let tap: IObjectWithSite = Tap {
            site: Mutex::new(None),
        }
        .into();
        unsafe { tap.query(interface, out) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

/// Implémente `DllGetClassObject`.
///
/// # Safety
/// Les pointeurs doivent être valides, comme le garantit COM.
pub unsafe fn get_class_object(
    class_id: *const GUID,
    interface_id: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if class_id.is_null() || interface_id.is_null() || out.is_null() {
            return E_POINTER;
        }
        unsafe { *out = std::ptr::null_mut() };
        if unsafe { *class_id } != GUID::from_u128(TAP_CLSID) {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        unsafe { factory.query(interface_id, out) }
    }));
    result.unwrap_or(E_FAIL)
}

/// Crée l'objet transmis à `AdviseVisualTreeChange`.
pub fn new_watcher(diagnostics: IXamlDiagnostics) -> IVisualTreeServiceCallback2 {
    Watcher { diagnostics }.into()
}

/// Vue parente de l'interface, pour `Advise` et `Unadvise`.
pub fn as_base(
    watcher: &IVisualTreeServiceCallback2,
) -> windows_core::Result<IVisualTreeServiceCallback> {
    watcher.cast()
}
