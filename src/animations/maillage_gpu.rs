//! Surimpression de l'effet « Jelly » rendue par un VRAI maillage courbe
//! (v0.4.1) : l'image de la fenêtre est plaquée sur une grille de
//! triangles indexés dont les sommets suivent la simulation
//! ([`ressort`](super::ressort)), interpolée finement par
//! [`grille_rendu`](super::grille_rendu). Les triangles partagent leurs
//! sommets : l'image est interpolée continûment, sans couture.
//!
//! TOUT le rendu est fait par le GPU matériel :
//! - l'image WGC (texture D3D11) est copiée GPU → GPU
//!   (`CopySubresourceRegion`) dans une texture échantillonnable (SRV) ;
//!   aucune lecture CPU (pas de `Map` en lecture, pas de texture de
//!   transfert, pas de GDI) ;
//! - le CPU n'écrit que le tampon de sommets dynamique
//!   (`Map(WRITE_DISCARD)`, ~4000 sommets) à chaque pas ;
//! - un seul `DrawIndexed` + `Present` par pas, dans une swapchain de
//!   composition (flip-model, alpha prémultiplié) posée comme contenu d'un
//!   visuel DirectComposition de la fenêtre de surimpression habituelle
//!   (topmost, non activable, transparente aux clics) ;
//! - aucun repli logiciel : un adaptateur WARP / « Microsoft Basic Render
//!   Driver » est refusé, et il n'existe plus de rendu de repli (les tuiles
//!   DirectComposition de la v0.4.0, sources de coutures, ont été retirées).
//!   Seul un échec du pipeline (compilateur, shaders, états) ou de
//!   l'adaptateur est [`EchecMaillage::Systeme`] : l'appelant désactive le
//!   Jelly pour la session. Un périphérique perdu est
//!   [`EchecMaillage::PeripheriquePerdu`] (contexte GPU recréé, Jelly
//!   maintenu) ; un échec de création de ressource dépendant de la taille
//!   (texture, tampons, swapchain, fenêtre) ou de composition est
//!   [`EchecMaillage::Ponctuel`].
//!
//! Shaders HLSL compilés à l'exécution par `D3DCompile`, chargée
//! DYNAMIQUEMENT (`LoadLibraryExW` dans System32 + `GetProcAddress`) : pas
//! d'import statique de `d3dcompiler_47.dll` (le démarrage du programme ne
//! peut pas en dépendre). Le pipeline (shaders, états, fabrique DXGI) est
//! compilé UNE fois par périphérique et mis en cache ; un échec définitif
//! est mémorisé (pas de nouvel essai à chaque glisser) jusqu'à
//! [`oublier_echec_pipeline`] (réactivation du Jelly depuis le menu).

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use windows::core::{s, w, Interface, HRESULT, PCSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D::{
    ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_FEATURE_LEVEL_10_0,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11Multithread, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE,
    D3D11_BLEND_OP_ADD, D3D11_BOX, D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL,
    D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE, D3D11_FILL_SOLID,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC,
    D3D11_USAGE_IMMUTABLE, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice2, IDCompositionTarget};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16_UINT,
    DXGI_FORMAT_R32G32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIOutput, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::capture::DerniereTrame;
use super::deformation::RAYON_COINS;
use super::erreur::ErreurAnimation;
use super::gpu::ContexteGpu;
use super::grille_rendu::{GrilleRendu, SommetRendu};
use super::jelly::{hresult_peripherique_perdu, taille_max_texture, NatureEchec};
use super::ressort::{Grille, Vec2};
use super::surimpression;
use crate::logging;

/// Source HLSL (modèle de shader 4.0 : niveau de fonctionnalité 10_0 min).
const SOURCE_HLSL: &str = r#"
cbuffer Constantes : register(b0)
{
    float2 taille_cible;   // swapchain (pixels)
    float2 taille_image;   // window image (pixels)
    float opacite;         // original window opacity
    float rayon;           // rounded corner radius (image pixels)
    float2 inutilise;
};

struct EntreeVS { float2 pos : POSITION; float2 uv : TEXCOORD0; };
struct SortieVS { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; };

SortieVS vs_principal(EntreeVS e)
{
    SortieVS s;
    float2 ndc = e.pos / taille_cible * float2(2.0, -2.0) + float2(-1.0, 1.0);
    s.pos = float4(ndc, 0.0, 1.0);
    s.uv = e.uv;
    return s;
}

Texture2D image : register(t0);
SamplerState echantillonneur : register(s0);

float4 ps_principal(SortieVS e) : SV_TARGET
{
    // WGC texture: premultiplied alpha.
    float4 c = image.Sample(echantillonneur, e.uv);
    // Windows 11 rounded corners, computed in image space (they follow
    // the deformation), edge smoothed over 1 pixel.
    float2 p = e.uv * taille_image;
    float2 q = min(p, taille_image - p);
    float masque = 1.0;
    float2 d = float2(rayon, rayon) - q;
    if (rayon > 0.0 && d.x > 0.0 && d.y > 0.0)
    {
        masque = saturate(0.5 - (length(d) - rayon));
    }
    return c * (opacite * masque);
}
"#;

// La source HLSL est passée telle quelle à D3DCompile : ASCII pur exigé
// (commentaires compris), vérifié à la compilation.
const _: () = assert!(SOURCE_HLSL.is_ascii(), "source HLSL non ASCII");

/// Constantes des shaders (32 octets, multiple de 16).
#[repr(C)]
#[derive(Clone, Copy)]
struct Constantes {
    taille_cible: [f32; 2],
    taille_image: [f32; 2],
    opacite: f32,
    rayon: f32,
    inutilise: [f32; 2],
}

/// `D3DCompile` (d3dcompiler_47.dll).
type FnD3DCompile = unsafe extern "system" fn(
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

/// `D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3`.
const DRAPEAUX_COMPILATION: u32 = 0x0800 | 0x8000;
/// Pas consécutifs en échec tolérés avant d'abandonner.
const MAX_ECHECS_CONSECUTIFS: u32 = 3;
/// Période minimale du battement de journal du rendu.
const BATTEMENT_MS: u128 = 1000;
/// Identifiants PCI du « Microsoft Basic Render Driver » (WARP, rendu CPU).
const VENDEUR_MICROSOFT: u32 = 0x1414;
const PERIPHERIQUE_BASIC_RENDER: u32 = 0x8c;

/// Échec d'une étape nommée (création ou rendu du maillage).
#[derive(Debug)]
pub struct EchecEtape {
    pub etape: String,
    pub erreur: ErreurAnimation,
}

impl std::fmt::Display for EchecEtape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "étape « {} » : {}", self.etape, self.erreur)
    }
}

/// Échec de création ou de mise à jour du maillage.
#[derive(Debug)]
pub enum EchecMaillage {
    /// Propre à cette fenêtre / cette image (taille, format de la capture,
    /// création d'une ressource dépendant de la taille : texture, tampons,
    /// swapchain, fenêtre ; composition, rendu au pas...) : pas d'effet
    /// cette fois-ci.
    Ponctuel(EchecEtape),
    /// Périphérique graphique perdu : effet terminé, contexte GPU recréé,
    /// Jelly maintenu.
    PeripheriquePerdu(EchecEtape),
    /// Pipeline Direct3D (compilateur, shaders, états) ou adaptateur : le
    /// Jelly doit être désactivé pour la session (aucun rendu de repli).
    Systeme(EchecEtape),
}

/// Classe un échec : périphérique perdu si son HRESULT l'indique, sinon
/// `sinon` (Ponctuel ou Systeme).
fn classer(e: EchecEtape, sinon: NatureEchec) -> EchecMaillage {
    if est_peripherique_perdu(&e) {
        return EchecMaillage::PeripheriquePerdu(e);
    }
    match sinon {
        NatureEchec::Systeme => EchecMaillage::Systeme(e),
        NatureEchec::PeripheriquePerdu => EchecMaillage::PeripheriquePerdu(e),
        NatureEchec::Ponctuel => EchecMaillage::Ponctuel(e),
    }
}

fn est_peripherique_perdu(e: &EchecEtape) -> bool {
    matches!(&e.erreur, ErreurAnimation::Win32(w) if hresult_peripherique_perdu(w.code().0 as u32))
}

/// Par défaut (opérateur `?`) : ponctuel, sauf périphérique perdu.
impl From<EchecEtape> for EchecMaillage {
    fn from(e: EchecEtape) -> Self {
        classer(e, NatureEchec::Ponctuel)
    }
}

impl EchecMaillage {
    pub fn detail(&self) -> &EchecEtape {
        match self {
            EchecMaillage::Ponctuel(e)
            | EchecMaillage::PeripheriquePerdu(e)
            | EchecMaillage::Systeme(e) => e,
        }
    }

    pub fn nature(&self) -> NatureEchec {
        match self {
            EchecMaillage::Ponctuel(_) => NatureEchec::Ponctuel,
            EchecMaillage::PeripheriquePerdu(_) => NatureEchec::PeripheriquePerdu,
            EchecMaillage::Systeme(_) => NatureEchec::Systeme,
        }
    }

    pub fn en_erreur(self) -> ErreurAnimation {
        match self {
            EchecMaillage::Ponctuel(e)
            | EchecMaillage::PeripheriquePerdu(e)
            | EchecMaillage::Systeme(e) => e.erreur,
        }
    }
}

/// Section critique du périphérique Direct3D (`ID3D11Multithread`, la
/// protection multifil étant activée à la création du contexte GPU : le
/// fil de capture WGC partage le même `ID3D11Device`). Sans interface :
/// aucune protection (comportement antérieur).
struct VerrouD3d<'a>(Option<&'a ID3D11Multithread>);

impl<'a> VerrouD3d<'a> {
    unsafe fn prendre(mt: Option<&'a ID3D11Multithread>) -> Self {
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
unsafe fn multifil(contexte: &ID3D11DeviceContext, d3d: &ID3D11Device) -> Option<ID3D11Multithread> {
    contexte
        .cast::<ID3D11Multithread>()
        .or_else(|_| d3d.cast::<ID3D11Multithread>())
        .ok()
}

/// Délie NOS liaisons du contexte immédiat partagé (vue de rendu, image,
/// tampons, constantes) au lieu d'un `ClearState` global, pour que nos
/// ressources soient libérées sans perturber l'état des autres usages.
unsafe fn delier(contexte: &ID3D11DeviceContext) {
    contexte.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
    contexte.PSSetShaderResources(0, Some(&[None]));
    contexte.VSSetConstantBuffers(0, Some(&[None]));
    contexte.PSSetConstantBuffers(0, Some(&[None]));
    let aucun: Option<ID3D11Buffer> = None;
    let zero = 0u32;
    contexte.IASetVertexBuffers(0, 1, Some(&aucun), Some(&zero), Some(&zero));
    contexte.IASetIndexBuffer(None::<&ID3D11Buffer>, DXGI_FORMAT_R16_UINT, 0);
}

impl std::fmt::Display for EchecMaillage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.detail().fmt(f)
    }
}

/// Exécute `f` avec le contexte DPI « per-monitor v2 » sur le fil courant
/// (coordonnées physiques, quel que soit le mode DPI du processus).
unsafe fn en_pmv2<R>(f: impl FnOnce() -> R) -> R {
    let ancien = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let r = f();
    if !ancien.0.is_null() {
        SetThreadDpiAwarenessContext(ancien);
    }
    r
}

/// Objets du pipeline, indépendants de la fenêtre (cache par périphérique).
struct Pipeline {
    appareil: usize,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    disposition: ID3D11InputLayout,
    echantillonneur: ID3D11SamplerState,
    melange: ID3D11BlendState,
    rasterisation: ID3D11RasterizerState,
    fabrique: IDXGIFactory2,
}

enum EtatPipeline {
    Vide,
    Pret(Rc<Pipeline>),
    /// Échec définitif pour ce périphérique (compilateur absent, shader
    /// refusé, niveau de fonctionnalité insuffisant, adaptateur logiciel...).
    Impossible {
        appareil: usize,
        etape: String,
        motif: String,
    },
}

thread_local! {
    static PIPELINE: RefCell<EtatPipeline> = const { RefCell::new(EtatPipeline::Vide) };
}

/// Oublie un échec définitif mémorisé : le prochain glisser retentera la
/// construction du pipeline (réactivation du Jelly depuis le menu).
pub fn oublier_echec_pipeline() {
    PIPELINE.with(|p| {
        if let Ok(mut g) = p.try_borrow_mut() {
            if matches!(*g, EtatPipeline::Impossible { .. }) {
                *g = EtatPipeline::Vide;
            }
        }
    });
}

/// Erreur d'une étape nommée (journalisée avec le HRESULT).
fn echec(contexte: &str, etape: &str, e: windows::core::Error) -> EchecEtape {
    logging::avertir(&format!(
        "{contexte} : échec à l'étape « {etape} » — HRESULT {:#010x} ({}).",
        e.code().0 as u32,
        e.message()
    ));
    EchecEtape {
        etape: etape.to_string(),
        erreur: ErreurAnimation::Win32(e),
    }
}

/// Résultat d'une étape : journal « OK » (si `detaille`) ou erreur.
fn etape<T>(
    contexte: &str,
    detaille: bool,
    nom: &str,
    debut: Instant,
    r: windows::core::Result<T>,
) -> Result<T, EchecEtape> {
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
fn sortie<T>(contexte: &str, nom: &str, o: Option<T>) -> Result<T, EchecEtape> {
    o.ok_or_else(|| refus(contexte, nom, format!("{nom} n'a renvoyé aucun objet")))
}

/// Erreur non Windows d'une étape (journalisée).
fn refus(contexte: &str, etape: &str, motif: String) -> EchecEtape {
    logging::avertir(&format!(
        "{contexte} : échec à l'étape « {etape} » — {motif}."
    ));
    EchecEtape {
        etape: etape.to_string(),
        erreur: ErreurAnimation::Rendu(motif),
    }
}

/// Charge `D3DCompile` dynamiquement depuis System32.
unsafe fn charger_d3dcompile() -> Result<FnD3DCompile, String> {
    let module = LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
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
}

/// Compile un point d'entrée ; renvoie le bytecode.
unsafe fn compiler(
    d3dcompile: FnD3DCompile,
    entree: PCSTR,
    cible: PCSTR,
) -> Result<Vec<u8>, String> {
    let mut code: *mut c_void = std::ptr::null_mut();
    let mut erreurs: *mut c_void = std::ptr::null_mut();
    let hr = d3dcompile(
        SOURCE_HLSL.as_ptr() as *const c_void,
        SOURCE_HLSL.len(),
        s!("jelly.hlsl"),
        std::ptr::null(),
        std::ptr::null_mut(),
        entree,
        cible,
        DRAPEAUX_COMPILATION,
        0,
        &mut code,
        &mut erreurs,
    );
    // Prise de possession des blobs (libérés à la fin de la portée).
    let code = (!code.is_null()).then(|| ID3DBlob::from_raw(code));
    let erreurs = (!erreurs.is_null()).then(|| ID3DBlob::from_raw(erreurs));
    let texte_erreurs = erreurs
        .as_ref()
        .map(|b| {
            let octets =
                std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize());
            let t = String::from_utf8_lossy(octets);
            t.trim_end_matches('\0')
                .chars()
                .take(400)
                .collect::<String>()
        })
        .unwrap_or_default();
    if hr.is_err() {
        return Err(format!(
            "HRESULT {:#010x}{}",
            hr.0 as u32,
            if texte_erreurs.is_empty() {
                String::new()
            } else {
                format!(" : {texte_erreurs}")
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
    Ok(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, n).to_vec())
}

/// Construit le pipeline (une ligne de journal par étape). L'erreur
/// porte `true` si l'échec est définitif pour ce périphérique.
unsafe fn construire_pipeline(d3d: &ID3D11Device) -> Result<Pipeline, (EchecEtape, bool)> {
    const C: &str = "Maillage GPU Jelly (pipeline)";
    let debut = Instant::now();
    let definitif = |e: EchecEtape| (e, true);
    let passager = |e: EchecEtape| (e, false);

    let niveau = d3d.GetFeatureLevel();
    if niveau.0 < D3D_FEATURE_LEVEL_10_0.0 {
        return Err(definitif(refus(
            C,
            "niveau de fonctionnalité",
            format!("niveau de fonctionnalité Direct3D {:#x} < 10_0", niveau.0),
        )));
    }
    logging::info(&format!(
        "{C} : niveau de fonctionnalité Direct3D {:#x}.",
        niveau.0
    ));

    // Adaptateur : GPU matériel exigé (pas de WARP / Basic Render Driver).
    let dxgi: IDXGIDevice = etape(C, true, "IDXGIDevice", debut, d3d.cast()).map_err(passager)?;
    let adaptateur = etape(C, true, "GetAdapter", debut, dxgi.GetAdapter()).map_err(passager)?;
    let adaptateur1: IDXGIAdapter1 =
        etape(C, true, "IDXGIAdapter1", debut, adaptateur.cast()).map_err(passager)?;
    let desc = etape(C, true, "GetDesc1 (adaptateur)", debut, adaptateur1.GetDesc1())
        .map_err(passager)?;
    let nom: String = String::from_utf16_lossy(&desc.Description)
        .trim_end_matches('\0')
        .chars()
        .take(128)
        .collect();
    let logiciel = desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0
        || (desc.VendorId == VENDEUR_MICROSOFT && desc.DeviceId == PERIPHERIQUE_BASIC_RENDER);
    logging::info(&format!(
        "{C} : adaptateur « {nom} » (fournisseur {:#06x}, périphérique {:#06x}, mémoire vidéo {} Mo){}.",
        desc.VendorId,
        desc.DeviceId,
        desc.DedicatedVideoMemory / (1024 * 1024),
        if logiciel { ", LOGICIEL" } else { ", matériel" }
    ));
    if logiciel {
        return Err(definitif(refus(
            C,
            "adaptateur",
            format!(
                "adaptateur logiciel « {nom} » (WARP / Microsoft Basic Render Driver) : rendu CPU \
                 refusé, GPU matériel exigé"
            ),
        )));
    }
    let fabrique: IDXGIFactory2 = etape(
        C,
        true,
        "fabrique IDXGIFactory2",
        debut,
        adaptateur.GetParent(),
    )
    .map_err(passager)?;

    let d3dcompile =
        charger_d3dcompile().map_err(|m| definitif(refus(C, "chargement de D3DCompile", m)))?;
    logging::info(&format!(
        "{C} : D3DCompile chargée dynamiquement (t = {} µs).",
        debut.elapsed().as_micros()
    ));
    let code_vs = compiler(d3dcompile, s!("vs_principal"), s!("vs_4_0"))
        .map_err(|m| definitif(refus(C, "D3DCompile (VS)", m)))?;
    logging::info(&format!(
        "{C} : D3DCompile VS OK ({} octets, t = {} µs).",
        code_vs.len(),
        debut.elapsed().as_micros()
    ));
    let code_ps = compiler(d3dcompile, s!("ps_principal"), s!("ps_4_0"))
        .map_err(|m| definitif(refus(C, "D3DCompile (PS)", m)))?;
    logging::info(&format!(
        "{C} : D3DCompile PS OK ({} octets, t = {} µs).",
        code_ps.len(),
        debut.elapsed().as_micros()
    ));

    let mut vs = None;
    etape(
        C,
        true,
        "CreateVertexShader",
        debut,
        d3d.CreateVertexShader(&code_vs, None, Some(&mut vs)),
    )
    .map_err(definitif)?;
    let vs = sortie(C, "CreateVertexShader", vs).map_err(definitif)?;
    let mut ps = None;
    etape(
        C,
        true,
        "CreatePixelShader",
        debut,
        d3d.CreatePixelShader(&code_ps, None, Some(&mut ps)),
    )
    .map_err(definitif)?;
    let ps = sortie(C, "CreatePixelShader", ps).map_err(definitif)?;

    let elements = [
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 8,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];
    let mut disposition = None;
    etape(
        C,
        true,
        "CreateInputLayout",
        debut,
        d3d.CreateInputLayout(&elements, &code_vs, Some(&mut disposition)),
    )
    .map_err(definitif)?;
    let disposition = sortie(C, "CreateInputLayout", disposition).map_err(definitif)?;

    // Bilinéaire, bords bloqués (pas de répétition de l'image).
    let desc_ech = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: 0.0,
    };
    let mut echantillonneur = None;
    etape(
        C,
        true,
        "CreateSamplerState",
        debut,
        d3d.CreateSamplerState(&desc_ech, Some(&mut echantillonneur)),
    )
    .map_err(passager)?;
    let echantillonneur = sortie(C, "CreateSamplerState", echantillonneur).map_err(passager)?;

    // Mélange « alpha prémultiplié » (les replis éventuels du maillage se
    // superposent proprement au lieu de se battre).
    let cible_melange = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let desc_melange = D3D11_BLEND_DESC {
        AlphaToCoverageEnable: false.into(),
        IndependentBlendEnable: false.into(),
        RenderTarget: [cible_melange; 8],
    };
    let mut melange = None;
    etape(
        C,
        true,
        "CreateBlendState",
        debut,
        d3d.CreateBlendState(&desc_melange, Some(&mut melange)),
    )
    .map_err(passager)?;
    let melange = sortie(C, "CreateBlendState", melange).map_err(passager)?;

    // Aucun élagage : un triangle retourné reste visible.
    let desc_rast = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: false.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };
    let mut rasterisation = None;
    etape(
        C,
        true,
        "CreateRasterizerState",
        debut,
        d3d.CreateRasterizerState(&desc_rast, Some(&mut rasterisation)),
    )
    .map_err(passager)?;
    let rasterisation = sortie(C, "CreateRasterizerState", rasterisation).map_err(passager)?;

    logging::info(&format!(
        "{C} : prêt en {} ms (compilé une fois pour ce périphérique).",
        debut.elapsed().as_millis()
    ));
    Ok(Pipeline {
        appareil: d3d.as_raw() as usize,
        vs,
        ps,
        disposition,
        echantillonneur,
        melange,
        rasterisation,
        fabrique,
    })
}

/// Pipeline en cache pour `d3d` (construit au besoin). Renvoie aussi
/// `true` s'il vient d'être construit (journal détaillé de la première
/// surimpression).
unsafe fn pipeline(d3d: &ID3D11Device) -> Result<(Rc<Pipeline>, bool), EchecEtape> {
    let appareil = d3d.as_raw() as usize;
    let en_cache = PIPELINE.with(|p| match &*p.borrow() {
        EtatPipeline::Pret(pl) if pl.appareil == appareil => Some(Ok(pl.clone())),
        EtatPipeline::Impossible {
            appareil: a,
            etape,
            motif,
        } if *a == appareil => Some(Err(EchecEtape {
            etape: format!("{etape} (échec mémorisé)"),
            erreur: ErreurAnimation::Rendu(format!("pipeline indisponible : {motif}")),
        })),
        _ => None,
    });
    match en_cache {
        Some(Ok(pl)) => return Ok((pl, false)),
        Some(Err(e)) => return Err(e),
        None => {}
    }
    match construire_pipeline(d3d) {
        Ok(pl) => {
            let pl = Rc::new(pl);
            PIPELINE.with(|p| *p.borrow_mut() = EtatPipeline::Pret(pl.clone()));
            Ok((pl, true))
        }
        Err((e, definitif)) => {
            // Périphérique perdu : rien de définitif (contexte recréé).
            if definitif && !est_peripherique_perdu(&e) {
                logging::avertir(&format!(
                    "Maillage GPU Jelly : pipeline indisponible pour ce périphérique ({e}) — échec \
                     mémorisé, aucun rendu de repli."
                ));
                PIPELINE.with(|p| {
                    *p.borrow_mut() = EtatPipeline::Impossible {
                        appareil,
                        etape: e.etape.clone(),
                        motif: e.erreur.to_string(),
                    }
                });
            }
            Err(e)
        }
    }
}

/// Statistiques de rendu (battement ≤ 1 Hz).
struct StatsRendu {
    depuis: Instant,
    rendus: u32,
    identiques: u32,
    somme_us: u128,
    max_us: u128,
}

impl StatsRendu {
    fn nouvelles() -> Self {
        StatsRendu {
            depuis: Instant::now(),
            rendus: 0,
            identiques: 0,
            somme_us: 0,
            max_us: 0,
        }
    }
}

/// Surimpression Jelly rendue par maillage Direct3D 11. La fenêtre est
/// détruite à la libération (`Drop`).
pub struct MaillageGpu {
    id: u32,
    fenetre: HWND,
    marge: i32,
    /// Position courante de la fenêtre de surimpression (pixels physiques).
    origine: (i32, i32),
    contexte: ID3D11DeviceContext,
    /// Section critique du périphérique partagé (voir [`VerrouD3d`]).
    multithread: Option<ID3D11Multithread>,
    /// Périphérique DirectComposition (retrait du contenu au `Drop`).
    dcomp: IDCompositionDevice2,
    pipeline: Rc<Pipeline>,
    grille: GrilleRendu,
    taille_cible: (u32, u32),
    sommets_gpu: ID3D11Buffer,
    indices_gpu: ID3D11Buffer,
    constantes: ID3D11Buffer,
    vue_image: ID3D11ShaderResourceView,
    swapchain: IDXGISwapChain1,
    vue_cible: ID3D11RenderTargetView,
    /// Borne des déplacements interpolés (reste dans la marge).
    borne: f32,
    /// Derniers déplacements de nœuds rendus (rendu sauté s'ils sont
    /// identiques).
    derniers: Vec<Vec2>,
    tampon: Vec<Vec2>,
    sommets: Vec<SommetRendu>,
    echecs_consecutifs: u32,
    stats: StatsRendu,
    _cible: IDCompositionTarget,
    _visuel: windows::Win32::Graphics::DirectComposition::IDCompositionVisual2,
    _texture: ID3D11Texture2D,
}

impl MaillageGpu {
    /// Crée la surimpression au-dessus de la fenêtre (bornes de
    /// `trame.bornes`), copie l'image (GPU → GPU) dans une texture
    /// échantillonnable, construit le maillage au repos, le rend une
    /// première fois puis valide l'arbre DirectComposition et l'affiche.
    pub unsafe fn creer(
        gpu: &ContexteGpu,
        trame: &DerniereTrame,
        grille_sim: &Grille,
        marge: i32,
        opacite: f32,
        id: u32,
    ) -> Result<MaillageGpu, EchecMaillage> {
        let debut = Instant::now();
        let ctx = format!("Jelly #{id} — maillage GPU");
        let c = ctx.as_str();
        let ponctuel = |e: EchecEtape| classer(e, NatureEchec::Ponctuel);
        let (largeur, hauteur) = trame.taille;
        let marge = marge.max(0);
        let lc = largeur as i64 + 2 * marge as i64;
        let hc = hauteur as i64 + 2 * marge as i64;
        // Borne selon le niveau de fonctionnalité (8192 en 10_x, 16384 en 11_x).
        let max = taille_max_texture(gpu.d3d.GetFeatureLevel().0) as i64;
        if largeur == 0 || hauteur == 0 || lc > max || hc > max {
            return Err(ponctuel(refus(
                c,
                "taille",
                format!("taille invalide {largeur}x{hauteur} (marge {marge}, maximum {max} px)"),
            )));
        }
        let (lc, hc) = (lc as u32, hc as u32);
        let Some(grille) = GrilleRendu::nouvelle(grille_sim) else {
            return Err(ponctuel(refus(
                c,
                "grille de rendu",
                format!("grille de rendu impossible pour {grille_sim:?}"),
            )));
        };
        if grille.nb_noeuds() != grille_sim.noeuds() {
            return Err(ponctuel(refus(
                c,
                "grille de rendu",
                "grille de rendu incohérente".into(),
            )));
        }

        // Pipeline (compilé une fois) : tout échec est systémique (sauf
        // périphérique perdu).
        let (pipeline, detaille) =
            pipeline(&gpu.d3d).map_err(|e| classer(e, NatureEchec::Systeme))?;
        let mt = multifil(&gpu.contexte, &gpu.d3d);
        let d3d = &gpu.d3d;

        // --- Texture source : l'image WGC reste sur le GPU ; elle est
        // copiée GPU → GPU dans une texture à nous (D3D11_BIND_SHADER_RESOURCE),
        // à la taille exacte du contenu. Problèmes propres à cette image :
        // ponctuels.
        let surface_wgc =
            etape(c, detaille, "Surface WGC", debut, trame.trame.Surface()).map_err(ponctuel)?;
        let acces: IDirect3DDxgiInterfaceAccess = etape(
            c,
            detaille,
            "IDirect3DDxgiInterfaceAccess",
            debut,
            surface_wgc.cast(),
        )
        .map_err(ponctuel)?;
        let source: ID3D11Texture2D =
            etape(c, detaille, "texture WGC", debut, acces.GetInterface()).map_err(ponctuel)?;
        let mut desc_src = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut desc_src);
        if desc_src.Width < largeur || desc_src.Height < hauteur {
            return Err(ponctuel(refus(
                c,
                "texture WGC",
                format!(
                    "texture WGC {}x{} plus petite que l'image {largeur}x{hauteur}",
                    desc_src.Width, desc_src.Height
                ),
            )));
        }
        if desc_src.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(ponctuel(refus(
                c,
                "texture WGC",
                format!("format de texture WGC inattendu {:?}", desc_src.Format),
            )));
        }
        let desc_tex = D3D11_TEXTURE2D_DESC {
            Width: largeur,
            Height: hauteur,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        etape(
            c,
            detaille,
            "CreateTexture2D (image)",
            debut,
            d3d.CreateTexture2D(&desc_tex, None, Some(&mut texture)),
        )?;
        let texture = sortie(c, "CreateTexture2D (image)", texture)?;
        let boite = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: largeur,
            bottom: hauteur,
            back: 1,
        };
        {
            let _verrou = VerrouD3d::prendre(mt.as_ref());
            gpu.contexte
                .CopySubresourceRegion(&texture, 0, 0, 0, 0, &source, 0, Some(&boite));
            // La session WGC est fermée juste après : copie soumise tout de suite.
            gpu.contexte.Flush();
        }
        let mut vue_image = None;
        etape(
            c,
            detaille,
            "CreateShaderResourceView",
            debut,
            d3d.CreateShaderResourceView(&texture, None, Some(&mut vue_image)),
        )?;
        let vue_image = sortie(c, "CreateShaderResourceView", vue_image)?;

        // --- Tampons : sommets (dynamique), index (statique), constantes.
        let octets_sommets = grille.nb_sommets() * std::mem::size_of::<SommetRendu>();
        let desc_vb = D3D11_BUFFER_DESC {
            ByteWidth: octets_sommets as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut sommets_gpu = None;
        etape(
            c,
            detaille,
            "CreateBuffer (sommets)",
            debut,
            d3d.CreateBuffer(&desc_vb, None, Some(&mut sommets_gpu)),
        )?;
        let sommets_gpu = sortie(c, "CreateBuffer (sommets)", sommets_gpu)?;

        let indices = grille.indices();
        let desc_ib = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of_val(indices) as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_INDEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let init_ib = D3D11_SUBRESOURCE_DATA {
            pSysMem: indices.as_ptr() as *const c_void,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut indices_gpu = None;
        etape(
            c,
            detaille,
            "CreateBuffer (index)",
            debut,
            d3d.CreateBuffer(&desc_ib, Some(&init_ib), Some(&mut indices_gpu)),
        )?;
        let indices_gpu = sortie(c, "CreateBuffer (index)", indices_gpu)?;

        let opacite = if opacite.is_finite() {
            opacite.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let valeurs = Constantes {
            taille_cible: [lc as f32, hc as f32],
            taille_image: [largeur as f32, hauteur as f32],
            opacite,
            rayon: RAYON_COINS,
            inutilise: [0.0; 2],
        };
        let desc_cb = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<Constantes>() as u32,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let init_cb = D3D11_SUBRESOURCE_DATA {
            pSysMem: &valeurs as *const Constantes as *const c_void,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut constantes = None;
        etape(
            c,
            detaille,
            "CreateBuffer (constantes)",
            debut,
            d3d.CreateBuffer(&desc_cb, Some(&init_cb), Some(&mut constantes)),
        )?;
        let constantes = sortie(c, "CreateBuffer (constantes)", constantes)?;

        // --- Swapchain de composition (flip-model, alpha prémultiplié).
        let desc_sc = DXGI_SWAP_CHAIN_DESC1 {
            Width: lc,
            Height: hc,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swapchain = etape(
            c,
            detaille,
            "CreateSwapChainForComposition",
            debut,
            pipeline
                .fabrique
                .CreateSwapChainForComposition(d3d, &desc_sc, None::<&IDXGIOutput>),
        )?;
        let tampon_sc: ID3D11Texture2D =
            etape(c, detaille, "GetBuffer(0)", debut, swapchain.GetBuffer(0))?;
        let mut vue_cible = None;
        etape(
            c,
            detaille,
            "CreateRenderTargetView",
            debut,
            d3d.CreateRenderTargetView(&tampon_sc, None, Some(&mut vue_cible)),
        )?;
        let vue_cible = sortie(c, "CreateRenderTargetView", vue_cible)?;
        drop(tampon_sc);

        // --- Fenêtre de surimpression (topmost, non activable, transparente aux clics).
        let b = trame.bornes;
        let x = b.left - marge;
        let y = b.top - marge;
        let instance = etape(
            c,
            detaille,
            "GetModuleHandleW",
            debut,
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None),
        )?;
        let fenetre = etape(
            c,
            detaille,
            "CreateWindowExW (surimpression)",
            debut,
            en_pmv2(|| {
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP
                        | WS_EX_TOPMOST
                        | WS_EX_TOOLWINDOW
                        | WS_EX_NOACTIVATE
                        | WS_EX_TRANSPARENT,
                    surimpression::CLASSE,
                    w!(""),
                    WS_POPUP,
                    x,
                    y,
                    lc as i32,
                    hc as i32,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
            }),
        )?;

        let borne = (marge as f32 - 1.0).max(0.0);
        let mut m = MaillageGpuPartiel {
            fenetre,
            contexte: gpu.contexte.clone(),
            multithread: mt.clone(),
        };
        // À partir d'ici, toute erreur détruit la fenêtre (Drop du partiel).
        let premier = Instant::now();
        let mut tampon = Vec::with_capacity(grille.nb_sommets());
        let mut sommets = Vec::with_capacity(grille.nb_sommets());
        grille.remplir(
            &[],
            Vec2::new(marge as f32, marge as f32),
            borne,
            &mut tampon,
            &mut sommets,
        );
        let rendu = Rendu {
            contexte: &gpu.contexte,
            multithread: mt.as_ref(),
            pipeline: &pipeline,
            sommets_gpu: &sommets_gpu,
            indices_gpu: &indices_gpu,
            constantes: &constantes,
            vue_image: &vue_image,
            vue_cible: &vue_cible,
            swapchain: &swapchain,
            taille_cible: (lc, hc),
            nb_indices: indices.len() as u32,
        };
        etape(
            c,
            true,
            "premier rendu + Present",
            debut,
            rendu.dessiner(&sommets),
        )?;
        let duree_premier = premier.elapsed();

        let dev: IDCompositionDevice2 =
            etape(c, detaille, "IDCompositionDevice2", debut, gpu.dcomp.cast())?;
        let cible = etape(
            c,
            detaille,
            "CreateTargetForHwnd",
            debut,
            gpu.dcomp.CreateTargetForHwnd(fenetre, true),
        )?;
        let visuel = etape(c, detaille, "CreateVisual", debut, dev.CreateVisual())?;
        // La swapchain couvre toute la fenêtre (marge comprise) : la marge
        // est intégrée aux positions des sommets, le visuel reste en (0, 0).
        etape(
            c,
            true,
            "SetContent(swapchain)",
            debut,
            visuel.SetContent(&swapchain),
        )?;
        etape(c, detaille, "SetRoot", debut, cible.SetRoot(&visuel))?;
        etape(c, detaille, "Commit", debut, dev.Commit())?;

        let _ = ShowWindow(fenetre, SW_SHOWNA);
        en_pmv2(|| {
            let _ = SetWindowPos(
                fenetre,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        });
        // Création réussie : la fenêtre passe sous la garde de MaillageGpu.
        m.fenetre = HWND::default();
        drop(m);

        logging::info(&format!(
            "{c} : créé en {} ms (premier rendu {} µs) — {} sommets ({}x{}), {} triangles \
             (grille {}x{} cases, sous-division {}), swapchain {lc}x{hc} flip-séquentielle 2 tampons \
             (alpha prémultiplié), opacité {opacite:.2}{}.",
            debut.elapsed().as_millis(),
            duree_premier.as_micros(),
            grille.nb_sommets(),
            grille.colonnes(),
            grille.lignes(),
            grille.nb_triangles(),
            grille_sim.tuiles_x,
            grille_sim.tuiles_y,
            grille.subdivision(),
            if detaille { ", pipeline compilé pour l'occasion" } else { "" }
        ));

        Ok(MaillageGpu {
            id,
            fenetre,
            marge,
            origine: (x, y),
            contexte: gpu.contexte.clone(),
            multithread: mt,
            dcomp: dev,
            pipeline,
            derniers: vec![Vec2::ZERO; grille.nb_noeuds()],
            grille,
            taille_cible: (lc, hc),
            sommets_gpu,
            indices_gpu,
            constantes,
            vue_image,
            swapchain,
            vue_cible,
            borne,
            tampon,
            sommets,
            echecs_consecutifs: 0,
            stats: StatsRendu::nouvelles(),
            _cible: cible,
            _visuel: visuel,
            _texture: texture,
        })
    }

    /// Position de la fenêtre de surimpression (journal).
    pub fn position(&self) -> (i32, i32) {
        self.origine
    }

    /// Met à jour la position (si la fenêtre a bougé) et redessine le
    /// maillage déformé (déplacements des nœuds de la simulation). Le rendu
    /// est sauté si les déplacements n'ont pas changé. Un échec isolé est
    /// toléré (réessayé au pas suivant) ; erreur seulement après
    /// [`MAX_ECHECS_CONSECUTIFS`] pas consécutifs en échec, ou immédiatement
    /// si le périphérique est perdu ([`EchecMaillage::PeripheriquePerdu`]).
    /// Les autres échecs répétés (rendu Map / Present, déplacement de la
    /// fenêtre de surimpression) sont ponctuels : seul cet effet s'arrête.
    pub unsafe fn mettre_a_jour(
        &mut self,
        bornes: &RECT,
        deplacements: &[Vec2],
    ) -> Result<(), EchecMaillage> {
        const ETAPE_RENDU: &str = "rendu (Map / DrawIndexed / Present)";
        const ETAPE_POSITION: &str = "SetWindowPos (surimpression)";
        let debut = Instant::now();
        let mut erreur: Option<(&'static str, windows::core::Error)> = None;
        let origine = (bornes.left - self.marge, bornes.top - self.marge);
        if origine != self.origine {
            let fenetre = self.fenetre;
            let r = en_pmv2(|| {
                SetWindowPos(
                    fenetre,
                    None,
                    origine.0,
                    origine.1,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            });
            match r {
                Ok(()) => self.origine = origine,
                Err(e) => erreur = Some((ETAPE_POSITION, e)),
            }
        }

        let identiques = deplacements.len() == self.derniers.len()
            && deplacements
                .iter()
                .zip(self.derniers.iter())
                .all(|(a, b)| a == b);
        if identiques {
            self.stats.identiques += 1;
        } else {
            self.grille.remplir(
                deplacements,
                Vec2::new(self.marge as f32, self.marge as f32),
                self.borne,
                &mut self.tampon,
                &mut self.sommets,
            );
            let rendu = Rendu {
                contexte: &self.contexte,
                multithread: self.multithread.as_ref(),
                pipeline: &self.pipeline,
                sommets_gpu: &self.sommets_gpu,
                indices_gpu: &self.indices_gpu,
                constantes: &self.constantes,
                vue_image: &self.vue_image,
                vue_cible: &self.vue_cible,
                swapchain: &self.swapchain,
                taille_cible: self.taille_cible,
                nb_indices: self.grille.indices().len() as u32,
            };
            match rendu.dessiner(&self.sommets) {
                Ok(()) => {
                    self.derniers.clear();
                    self.derniers.extend_from_slice(deplacements);
                    let us = debut.elapsed().as_micros();
                    self.stats.rendus += 1;
                    self.stats.somme_us += us;
                    self.stats.max_us = self.stats.max_us.max(us);
                }
                Err(e) => {
                    // Périphérique perdu : inutile d'insister.
                    if hresult_peripherique_perdu(e.code().0 as u32) {
                        logging::avertir(&format!(
                            "Jelly #{} — maillage GPU : périphérique perdu au rendu (HRESULT \
                             {:#010x}).",
                            self.id,
                            e.code().0 as u32
                        ));
                        return Err(EchecMaillage::PeripheriquePerdu(EchecEtape {
                            etape: "rendu (périphérique perdu)".into(),
                            erreur: e.into(),
                        }));
                    }
                    // Le rendu prime sur un échec de positionnement.
                    erreur = Some((ETAPE_RENDU, e));
                }
            }
        }

        if self.stats.depuis.elapsed().as_millis() >= BATTEMENT_MS {
            let s = &self.stats;
            logging::info(&format!(
                "Jelly #{} — maillage GPU : {} rendus en {} ms (temps CPU moyen {:.2} ms, max \
                 {:.2} ms), {} pas sans changement.",
                self.id,
                s.rendus,
                s.depuis.elapsed().as_millis(),
                s.somme_us as f64 / 1000.0 / s.rendus.max(1) as f64,
                s.max_us as f64 / 1000.0,
                s.identiques
            ));
            self.stats = StatsRendu::nouvelles();
        }

        match erreur {
            Some((nom, e)) => {
                self.echecs_consecutifs += 1;
                logging::avertir(&format!(
                    "Jelly #{} — maillage GPU : échec au pas, étape « {nom} » (HRESULT {:#010x} : \
                     {}), échec consécutif {}/{MAX_ECHECS_CONSECUTIFS}.",
                    self.id,
                    e.code().0 as u32,
                    e.message(),
                    self.echecs_consecutifs
                ));
                if self.echecs_consecutifs >= MAX_ECHECS_CONSECUTIFS {
                    let detail = EchecEtape {
                        etape: nom.to_string(),
                        erreur: e.into(),
                    };
                    return Err(classer(detail, NatureEchec::Ponctuel));
                }
            }
            None => self.echecs_consecutifs = 0,
        }
        Ok(())
    }
}

impl Drop for MaillageGpu {
    fn drop(&mut self) {
        unsafe {
            // Retire la swapchain du visuel et valide AVANT de détruire la
            // fenêtre (erreurs ignorées : périphérique éventuellement perdu).
            let _ = self._visuel.SetContent(None::<&windows::core::IUnknown>);
            let _ = self.dcomp.Commit();
            {
                // Détache nos ressources du contexte immédiat partagé (vue
                // de rendu, image, tampons) pour qu'elles soient libérées
                // tout de suite, sans toucher au reste de son état.
                let _verrou = VerrouD3d::prendre(self.multithread.as_ref());
                delier(&self.contexte);
                self.contexte.Flush();
            }
            // Erreur ignorée : la fenêtre a pu être détruite avec le processus.
            let _ = DestroyWindow(self.fenetre);
        }
    }
}

/// Garde de la fenêtre pendant la création (détruite si la création
/// échoue après `CreateWindowExW`).
struct MaillageGpuPartiel {
    fenetre: HWND,
    contexte: ID3D11DeviceContext,
    multithread: Option<ID3D11Multithread>,
}

impl Drop for MaillageGpuPartiel {
    fn drop(&mut self) {
        unsafe {
            if !self.fenetre.is_invalid() {
                {
                    let _verrou = VerrouD3d::prendre(self.multithread.as_ref());
                    delier(&self.contexte);
                }
                let _ = DestroyWindow(self.fenetre);
            }
        }
    }
}

/// Références nécessaires à un rendu.
struct Rendu<'a> {
    contexte: &'a ID3D11DeviceContext,
    multithread: Option<&'a ID3D11Multithread>,
    pipeline: &'a Pipeline,
    sommets_gpu: &'a ID3D11Buffer,
    indices_gpu: &'a ID3D11Buffer,
    constantes: &'a ID3D11Buffer,
    vue_image: &'a ID3D11ShaderResourceView,
    vue_cible: &'a ID3D11RenderTargetView,
    swapchain: &'a IDXGISwapChain1,
    taille_cible: (u32, u32),
    nb_indices: u32,
}

impl Rendu<'_> {
    /// Envoie les sommets, efface (transparent), dessine et présente.
    /// Tout l'état du pipeline est reposé à chaque fois : le contexte
    /// immédiat est partagé avec le reste des animations.
    /// Encadré par la section critique du périphérique (`ID3D11Multithread`).
    unsafe fn dessiner(&self, sommets: &[SommetRendu]) -> windows::core::Result<()> {
        let _verrou = VerrouD3d::prendre(self.multithread);
        let ctx = self.contexte;
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(
            self.sommets_gpu,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            Some(&mut map),
        )?;
        if map.pData.is_null() {
            ctx.Unmap(self.sommets_gpu, 0);
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_POINTER,
                "Map du tampon de sommets sans pointeur",
            ));
        }
        std::ptr::copy_nonoverlapping(
            sommets.as_ptr() as *const u8,
            map.pData as *mut u8,
            std::mem::size_of_val(sommets),
        );
        ctx.Unmap(self.sommets_gpu, 0);

        let vue_cible = Some(self.vue_cible.clone());
        ctx.OMSetRenderTargets(
            Some(std::slice::from_ref(&vue_cible)),
            None::<&ID3D11DepthStencilView>,
        );
        ctx.ClearRenderTargetView(self.vue_cible, &[0.0, 0.0, 0.0, 0.0]);
        ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.taille_cible.0 as f32,
            Height: self.taille_cible.1 as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]));
        ctx.RSSetState(&self.pipeline.rasterisation);
        ctx.OMSetBlendState(&self.pipeline.melange, None, 0xffff_ffff);

        let pas = std::mem::size_of::<SommetRendu>() as u32;
        let decalage = 0u32;
        let tampon = Some(self.sommets_gpu.clone());
        ctx.IASetInputLayout(&self.pipeline.disposition);
        ctx.IASetVertexBuffers(0, 1, Some(&tampon), Some(&pas), Some(&decalage));
        ctx.IASetIndexBuffer(self.indices_gpu, DXGI_FORMAT_R16_UINT, 0);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let constantes = [Some(self.constantes.clone())];
        ctx.VSSetShader(&self.pipeline.vs, None);
        ctx.VSSetConstantBuffers(0, Some(&constantes));
        ctx.PSSetShader(&self.pipeline.ps, None);
        ctx.PSSetConstantBuffers(0, Some(&constantes));
        ctx.PSSetShaderResources(0, Some(&[Some(self.vue_image.clone())]));
        ctx.PSSetSamplers(0, Some(&[Some(self.pipeline.echantillonneur.clone())]));

        ctx.DrawIndexed(self.nb_indices, 0, 0);
        // Intervalle 0 : jamais d'attente de la synchronisation verticale
        // sur le fil de l'interface. Le pas est déjà cadencé par l'horloge du
        // compositeur (fil de battement) ; `Present(1, 0)` pourrait bloquer
        // le fil principal (file de présentation pleine, jusqu'à 3 images
        // d'attente) sans gain : en flip-model, DWM affiche la trame la plus
        // récente à la composition suivante.
        self.swapchain.Present(0, DXGI_PRESENT(0)).ok()
    }
}
