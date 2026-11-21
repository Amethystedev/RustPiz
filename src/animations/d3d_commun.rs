//! Utilitaires Direct3D 11 des effets à shader (v0.4.7).
//!
//! Copie GÉNÉRALISÉE des utilitaires privés de `maillage_gpu` (le Jelly
//! n'est volontairement pas modifié) : section critique `ID3D11Multithread`,
//! contexte DPI « per-monitor v2 », chargement DYNAMIQUE de `D3DCompile`
//! (`d3dcompiler_47.dll` de System32, résolu une seule fois), compilation
//! d'une source quelconque, journal des étapes et refus de l'adaptateur
//! logiciel (WARP / Microsoft Basic Render Driver).

use std::ffi::{c_void, CString};
use std::sync::OnceLock;
use std::time::Instant;

use windows::core::{s, w, Interface, HRESULT, PCSTR};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Multithread};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use super::jelly::hresult_peripherique_perdu;
use crate::logging;

/// `D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3`.
const DRAPEAUX_COMPILATION: u32 = 0x0800 | 0x8000;
/// Identifiants PCI du « Microsoft Basic Render Driver » (WARP, rendu CPU).
const VENDEUR_MICROSOFT: u32 = 0x1414;
const PERIPHERIQUE_BASIC_RENDER: u32 = 0x8c;

/// Échec d'une étape nommée (journalisé à sa création).
#[derive(Debug, Clone)]
pub struct EchecD3d {
    pub etape: String,
    pub motif: String,
    /// Erreur Windows d'origine (absente pour un refus non Windows).
    pub erreur: Option<windows::core::Error>,
}

impl EchecD3d {
    /// Vrai si le HRESULT indique un périphérique perdu.
    pub fn peripherique_perdu(&self) -> bool {
        self.erreur
            .as_ref()
            .is_some_and(|e| hresult_peripherique_perdu(e.code().0 as u32))
    }
}

/// Erreur Windows d'une étape (journalisée avec le HRESULT).
pub fn echec(contexte: &str, etape: &str, e: windows::core::Error) -> EchecD3d {
    let motif = format!("HRESULT {:#010x} ({})", e.code().0 as u32, e.message());
    logging::avertir(&format!(
        "{contexte} : échec à l'étape « {etape} » — {motif}."
    ));
    EchecD3d {
        etape: etape.to_string(),
        motif,
        erreur: Some(e),
    }
}

/// Erreur non Windows d'une étape (journalisée).
pub fn refus(contexte: &str, etape: &str, motif: String) -> EchecD3d {
    logging::avertir(&format!(
        "{contexte} : échec à l'étape « {etape} » — {motif}."
    ));
    EchecD3d {
        etape: etape.to_string(),
        motif,
        erreur: None,
    }
}

/// Résultat d'une étape : journal « OK » (si `detaille`) ou erreur.
pub fn etape<T>(
    contexte: &str,
    detaille: bool,
    nom: &str,
    debut: Instant,
    r: windows::core::Result<T>,
) -> Result<T, EchecD3d> {
    match r {
        Ok(v) => {
            if detaille {
                logging::info(&format!(
                    "{contexte} : {nom} OK (t = {} µs).",
                    debut.elapsed().as_micros()
                ));
            }
            Ok(v)
        }
        Err(e) => Err(echec(contexte, nom, e)),
    }
}

/// Objet COM produit en paramètre de sortie : absent = erreur.
pub fn sortie<T>(contexte: &str, nom: &str, o: Option<T>) -> Result<T, EchecD3d> {
    o.ok_or_else(|| refus(contexte, nom, format!("{nom} n'a renvoyé aucun objet")))
}

/// Section critique du périphérique Direct3D (`ID3D11Multithread` : le fil
/// de capture WGC partage le même `ID3D11Device`). Sans interface : aucune
/// protection.
pub struct VerrouD3d<'a>(Option<&'a ID3D11Multithread>);

impl<'a> VerrouD3d<'a> {
    pub unsafe fn prendre(mt: Option<&'a ID3D11Multithread>) -> Self {
        if let Some(m) = mt {
            m.Enter();
        }
        VerrouD3d(mt)
    }
}

impl Drop for VerrouD3d<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.0 {
            unsafe { m.Leave() };
        }
    }
}

/// `ID3D11Multithread` du contexte immédiat (ou, à défaut, du périphérique).
pub unsafe fn multifil(
    contexte: &ID3D11DeviceContext,
    d3d: &ID3D11Device,
) -> Option<ID3D11Multithread> {
    contexte
        .cast::<ID3D11Multithread>()
        .or_else(|_| d3d.cast::<ID3D11Multithread>())
        .ok()
}

/// Exécute `f` avec le contexte DPI « per-monitor v2 » sur le fil courant.
pub unsafe fn en_pmv2<R>(f: impl FnOnce() -> R) -> R {
    let ancien = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let r = f();
    if !ancien.0.is_null() {
        SetThreadDpiAwarenessContext(ancien);
    }
    r
}

/// `D3DCompile` (d3dcompiler_47.dll).
pub type FnD3DCompile = unsafe extern "system" fn(
    psrcdata: *const c_void,
    srcdatasize: usize,
    psourcename: PCSTR,
    pdefines: *const c_void,
    pinclude: *mut c_void,
    pentrypoint: PCSTR,
    ptarget: PCSTR,
    flags1: u32,
    flags2: u32,
    ppcode: *mut *mut c_void,
    pperrormsgs: *mut *mut c_void,
) -> HRESULT;

/// Résolution unique de `D3DCompile` (le résultat, succès ou échec, est
/// conservé pour la durée du processus).
static D3DCOMPILE: OnceLock<Result<FnD3DCompile, String>> = OnceLock::new();

/// `D3DCompile`, chargée dynamiquement depuis System32 (une fois).
pub fn charger_d3dcompile() -> Result<FnD3DCompile, String> {
    D3DCOMPILE
        .get_or_init(|| unsafe {
            let module =
                LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                    .map_err(|e| {
                        format!(
                            "chargement de d3dcompiler_47.dll impossible — HRESULT {:#010x} ({})",
                            e.code().0 as u32,
                            e.message()
                        )
                    })?;
            // Module jamais libéré : il reste chargé pour la durée du processus.
            match GetProcAddress(module, s!("D3DCompile")) {
                // SAFETY : signature documentée de D3DCompile (d3dcompiler.h).
                Some(f) => Ok(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    FnD3DCompile,
                >(f)),
                None => Err("D3DCompile introuvable dans d3dcompiler_47.dll".into()),
            }
        })
        .clone()
}

/// Bytecode compilé et éventuels avertissements du compilateur.
pub struct Compilation {
    pub code: Vec<u8>,
    pub avertissements: String,
}

/// Texte d'un blob de messages, tronqué à `max` caractères.
unsafe fn texte_blob(b: &ID3DBlob, max: usize) -> String {
    let p = b.GetBufferPointer() as *const u8;
    let n = b.GetBufferSize();
    if p.is_null() || n == 0 {
        return String::new();
    }
    let octets = std::slice::from_raw_parts(p, n);
    String::from_utf8_lossy(octets)
        .trim_end_matches('\0')
        .trim_end()
        .chars()
        .take(max)
        .collect()
}

/// Compile `source` (point d'entrée `entree`, profil `cible`). L'erreur
/// contient le HRESULT et le message du compilateur (tronqué à `max_msg`).
pub unsafe fn compiler(
    source: &str,
    nom_source: &str,
    entree: &str,
    cible: &str,
    max_msg: usize,
) -> Result<Compilation, String> {
    let d3dcompile = charger_d3dcompile()?;
    let nom = CString::new(nom_source).map_err(|_| "nom de source invalide".to_string())?;
    let entree_c = CString::new(entree).map_err(|_| "point d'entrée invalide".to_string())?;
    let cible_c = CString::new(cible).map_err(|_| "profil invalide".to_string())?;
    let mut code: *mut c_void = std::ptr::null_mut();
    let mut erreurs: *mut c_void = std::ptr::null_mut();
    let hr = d3dcompile(
        source.as_ptr() as *const c_void,
        source.len(),
        PCSTR(nom.as_ptr() as *const u8),
        std::ptr::null(),
        std::ptr::null_mut(),
        PCSTR(entree_c.as_ptr() as *const u8),
        PCSTR(cible_c.as_ptr() as *const u8),
        DRAPEAUX_COMPILATION,
        0,
        &mut code,
        &mut erreurs,
    );
    // Prise de possession des blobs (libérés à la fin de la portée).
    let code = (!code.is_null()).then(|| ID3DBlob::from_raw(code));
    let erreurs = (!erreurs.is_null()).then(|| ID3DBlob::from_raw(erreurs));
    let messages = erreurs
        .as_ref()
        .map(|b| texte_blob(b, max_msg))
        .unwrap_or_default();
    if hr.is_err() {
        return Err(format!(
            "HRESULT {:#010x}{}",
            hr.0 as u32,
            if messages.is_empty() {
                String::new()
            } else {
                format!(" : {messages}")
            }
        ));
    }
    let Some(code) = code else {
        return Err("aucun bytecode produit".into());
    };
    let n = code.GetBufferSize();
    if n == 0 || code.GetBufferPointer().is_null() {
        return Err("bytecode vide".into());
    }
    Ok(Compilation {
        code: std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, n).to_vec(),
        avertissements: messages,
    })
}

/// Vérifie que l'adaptateur du périphérique est un GPU matériel (WARP /
/// Basic Render Driver refusés) et renvoie sa fabrique DXGI. L'erreur porte
/// `true` si l'échec est définitif pour ce périphérique (adaptateur
/// logiciel).
pub unsafe fn verifier_adaptateur(
    contexte: &str,
    d3d: &ID3D11Device,
    debut: Instant,
) -> Result<IDXGIFactory2, (EchecD3d, bool)> {
    let passager = |e: EchecD3d| (e, false);
    let dxgi: IDXGIDevice =
        etape(contexte, true, "IDXGIDevice", debut, d3d.cast()).map_err(passager)?;
    let adaptateur =
        etape(contexte, true, "GetAdapter", debut, dxgi.GetAdapter()).map_err(passager)?;
    let adaptateur1: IDXGIAdapter1 =
        etape(contexte, true, "IDXGIAdapter1", debut, adaptateur.cast()).map_err(passager)?;
    let desc = etape(
        contexte,
        true,
        "GetDesc1 (adaptateur)",
        debut,
        adaptateur1.GetDesc1(),
    )
    .map_err(passager)?;
    let nom: String = String::from_utf16_lossy(&desc.Description)
        .trim_end_matches('\0')
        .chars()
        .take(128)
        .collect();
    let logiciel = desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0
        || (desc.VendorId == VENDEUR_MICROSOFT && desc.DeviceId == PERIPHERIQUE_BASIC_RENDER);
    logging::info(&format!(
        "{contexte} : adaptateur « {nom} » (fournisseur {:#06x}, périphérique {:#06x}){}.",
        desc.VendorId,
        desc.DeviceId,
        if logiciel {
            ", LOGICIEL"
        } else {
            ", matériel"
        }
    ));
    if logiciel {
        return Err((
            refus(
                contexte,
                "adaptateur",
                format!(
                    "adaptateur logiciel « {nom} » (WARP / Microsoft Basic Render Driver) : \
                     rendu CPU refusé, GPU matériel exigé"
                ),
            ),
            true,
        ));
    }
    etape(
        contexte,
        true,
        "fabrique IDXGIFactory2",
        debut,
        adaptateur.GetParent(),
    )
    .map_err(passager)
}
