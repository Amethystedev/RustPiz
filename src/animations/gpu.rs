//! Contexte GPU partagé : Direct3D 11 (matériel uniquement),
//! DirectComposition et périphérique WinRT pour Windows.Graphics.Capture.
//!
//! Aucun repli logiciel (WARP) : si aucun GPU matériel n'est disponible,
//! les animations sont simplement déclarées indisponibles.

use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::{HMODULE, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice3, IDCompositionDesktopDevice, IDCompositionDevice3,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};

use std::sync::Arc;

use super::capture::Atelier;
use super::erreur::ErreurAnimation;
use crate::logging;

/// Ensemble des périphériques graphiques. Clonable à faible coût
/// (simples incréments de compteurs de références COM), ce qui permet de
/// l'extraire de l'état global avant tout appel Win32 (règle
/// anti-réentrance).
#[derive(Clone)]
pub struct ContexteGpu {
    pub d3d: ID3D11Device,
    pub contexte: ID3D11DeviceContext,
    /// Périphérique DirectComposition (visuels, animations, surfaces).
    pub dcomp: IDCompositionDesktopDevice,
    /// Interface étendue (effets : flou gaussien), absente avant Windows 10 1607.
    pub dcomp3: Option<IDCompositionDevice3>,
    /// Même périphérique Direct3D, vu par WinRT (capture WGC dans le fil
    /// principal : repli si le fil de capture est indisponible).
    pub winrt: IDirect3DDevice,
    /// Fil de création des sessions WGC (v0.3.2), lié à ce périphérique ;
    /// `None` = création dans le fil principal.
    pub atelier: Option<Arc<Atelier>>,
}

impl ContexteGpu {
    /// Crée le contexte complet sur le GPU matériel.
    pub fn creer() -> Result<Self, ErreurAnimation> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            let mut contexte: Option<ID3D11DeviceContext> = None;
            // BGRA obligatoire pour l'interopérabilité DirectComposition / WGC.
            let resultat = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                Some(&mut contexte),
            );
            if let Err(e) = resultat {
                logging::avertir(&format!("D3D11CreateDevice (matériel) a échoué : {e}"));
                return Err(ErreurAnimation::GpuMaterielAbsent);
            }
            let (Some(d3d), Some(contexte)) = (d3d, contexte) else {
                return Err(ErreurAnimation::GpuMaterielAbsent);
            };

            // Les rappels FrameArrived de WGC s'exécutent sur un autre fil :
            // on protège le contexte immédiat contre les accès concurrents.
            if let Ok(mt) = d3d.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }

            let dxgi: IDXGIDevice = d3d.cast()?;
            let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice3(&dxgi)?;
            let dcomp3 = dcomp.cast::<IDCompositionDevice3>().ok();
            if dcomp3.is_none() {
                logging::avertir("IDCompositionDevice3 indisponible : l'effet Flou sera ignoré.");
            }
            let winrt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

            logging::info("Contexte GPU des animations créé (Direct3D 11 matériel + DirectComposition).");
            let atelier = Atelier::lancer(&d3d).map(Arc::new);
            Ok(ContexteGpu {
                d3d,
                contexte,
                dcomp,
                dcomp3,
                winrt,
                atelier,
            })
        }
    }

    /// Vrai si le périphérique Direct3D a été perdu (pilote réinitialisé,
    /// GPU retiré...). Il faut alors recréer tout le contexte.
    pub fn est_perdu(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }
}

/// Initialise WinRT sur le fil courant (fil de la boucle de messages, STA).
/// `RPC_E_CHANGED_MODE` (COM déjà initialisé dans un autre mode) est toléré.
pub fn initialiser_winrt() {
    unsafe {
        match RoInitialize(RO_INIT_SINGLETHREADED) {
            Ok(()) => {}
            Err(e) if e.code() == RPC_E_CHANGED_MODE => {
                logging::info("WinRT déjà initialisé dans un autre mode (toléré).");
            }
            Err(e) => logging::avertir(&format!("RoInitialize a échoué : {e}")),
        }
    }
}
