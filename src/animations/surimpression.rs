//! Fenêtre de surimpression DirectComposition : affiche l'image capturée
//! d'une fenêtre et l'anime entièrement sur le GPU.
//!
//! Principe : une fenêtre `WS_EX_NOREDIRECTIONBITMAP` (pas de surface GDI)
//! reçoit un arbre de visuels DirectComposition. Les propriétés animées
//! (opacité, échelle, décalage, flou) sont liées à des
//! `IDCompositionAnimation` (courbes cubiques) ; après UN `Commit()`, c'est
//! le compositeur DWM qui joue l'animation image par image. Aucun calcul
//! ni rendu CPU pendant l'animation.
//!
//! Arbre :
//! ```text
//! racine (décalage = marge, effet = groupe d'effets : opacité animée)
//!   └─ contenu (transformation d'échelle animée + décalage Y animé,
//!               découpage arrondi, [flou gaussien animé], surface = image)
//! ```
//!
//! v0.4.7 : une animation peut aussi porter un effet « à shader »
//! ([`Contenu::Shader`], rendu image par image par
//! [`effet_gpu`](super::effet_gpu) dans une swapchain de composition) ; le
//! chemin DirectComposition ci-dessus est inchangé.

use windows::core::{w, IUnknown, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D1_BORDER_MODE_HARD;
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_BOX, D3D11_TEXTURE2D_DESC};
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionAnimation, IDCompositionDevice2, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SW_SHOWNA, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use std::time::Instant;

use super::capture::{texte_bornes, DerniereTrame};
use super::courbes::{coefficients, Piste};
use super::effet_gpu::{EchecEffet, EtatRendu, RenduEffet};
use super::erreur::ErreurAnimation;
use super::gpu::ContexteGpu;
use super::masquage;
use super::plan::{PlanAnimation, PlanShader, Sens};
use crate::logging;

/// Nom de la classe des fenêtres de surimpression.
pub(super) const CLASSE: windows::core::PCWSTR = w!("TransparentWindows_Surimpression");

/// Animation en cours d'affichage.
pub struct AnimationActive {
    pub id: u32,
    pub sens: Sens,
    /// Fenêtre de surimpression.
    pub fenetre: HWND,
    /// Fenêtre animée (valeur brute du HWND).
    pub cible: isize,
    /// Vrai si la vraie fenêtre est masquée par nous (transparence
    /// temporaire, ouverture) et doit être restaurée à la fin.
    pub masquee_par_nous: bool,
    /// Instant du `Commit` (durée réelle journalisée à la fin).
    pub debut: Instant,
    /// Durée prévue de l'effet (s).
    pub duree: f64,
    /// Objets du rendu (selon le moteur de l'effet).
    contenu: Contenu,
}

/// Contenu d'une surimpression, selon le moteur de l'effet.
enum Contenu {
    /// Arbre DirectComposition animé par le compositeur.
    Composition {
        _cible_dcomp: IDCompositionTarget,
        /// Visuels, surface, effets et animations à garder en vie.
        _objets: Vec<IUnknown>,
    },
    /// Effet à shader rendu image par image (détruit sa fenêtre au `Drop`).
    Shader(Box<RenduEffet>),
}

impl AnimationActive {
    /// Vrai si l'effet est rendu par un shader (battement nécessaire).
    pub fn est_shader(&self) -> bool {
        matches!(self.contenu, Contenu::Shader(_))
    }

    /// Vrai s'il reste des images à rendre (shader non terminé).
    pub fn besoin_rendu(&self) -> bool {
        matches!(&self.contenu, Contenu::Shader(r) if !r.est_fini())
    }

    /// Rend l'image courante d'un effet à shader (Composition : rien à
    /// faire, `DejaFini`).
    pub unsafe fn rendre(&mut self) -> Result<EtatRendu, EchecEffet> {
        match &mut self.contenu {
            Contenu::Composition { .. } => Ok(EtatRendu::DejaFini),
            Contenu::Shader(r) => r.dessiner_maintenant(),
        }
    }
}

/// Enregistre la classe des fenêtres de surimpression (une fois).
pub unsafe fn enregistrer_classe() -> Result<(), ErreurAnimation> {
    let instance = GetModuleHandleW(None)?;
    let classe = WNDCLASSW {
        lpfnWndProc: Some(procedure),
        hInstance: instance.into(),
        lpszClassName: CLASSE,
        ..Default::default()
    };
    if RegisterClassW(&classe) == 0 {
        return Err(windows::core::Error::from_thread().into());
    }
    Ok(())
}

/// Procédure des fenêtres de surimpression : totalement transparente aux
/// clics, jamais activée. Ne touche JAMAIS l'état des animations (elle est
/// appelée de façon synchrone pendant `CreateWindowExW` / `DestroyWindow`).
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Convertit une piste en animation DirectComposition :
/// un `AddCubic` par segment, puis `End` (valeur finale conservée).
unsafe fn creer_animation(
    dev: &IDCompositionDevice2,
    piste: &Piste,
) -> Result<IDCompositionAnimation, ErreurAnimation> {
    let anim = dev.CreateAnimation()?;
    for s in piste.segments() {
        let c = coefficients(s);
        anim.AddCubic(s.debut, c[0], c[1], c[2], c[3])?;
    }
    anim.End(piste.duree_totale(), piste.valeur_finale())?;
    Ok(anim)
}

/// Lance une animation : crée la surimpression au-dessus de la fenêtre,
/// y copie l'image (GPU → GPU), construit l'arbre et le valide.
pub unsafe fn lancer(
    gpu: &ContexteGpu,
    id: u32,
    sens: Sens,
    cible: isize,
    trame: &DerniereTrame,
    plan: &PlanAnimation,
) -> Result<AnimationActive, ErreurAnimation> {
    let (largeur, hauteur) = trame.taille;
    if largeur == 0 || hauteur == 0 || largeur > 16384 || hauteur > 16384 {
        return Err(ErreurAnimation::TailleInvalide);
    }

    // --- Texture source de l'image WGC (reste sur le GPU).
    let surface_wgc = trame.trame.Surface()?;
    let acces: IDirect3DDxgiInterfaceAccess = surface_wgc.cast()?;
    let source: ID3D11Texture2D = acces.GetInterface()?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    source.GetDesc(&mut desc);
    // Le pool peut être plus grand que le contenu (redimensionnement récent).
    let largeur = largeur.min(desc.Width);
    let hauteur = hauteur.min(desc.Height);
    if largeur == 0 || hauteur == 0 {
        return Err(ErreurAnimation::TailleInvalide);
    }

    let dev: IDCompositionDevice2 = gpu.dcomp.cast()?;

    // --- Surface DirectComposition remplie par copie GPU.
    let surface = dev.CreateSurface(
        largeur,
        hauteur,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_ALPHA_MODE_PREMULTIPLIED,
    )?;
    let mut decalage = POINT::default();
    let destination: ID3D11Texture2D = surface.BeginDraw(None, &mut decalage)?;
    let boite = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: largeur,
        bottom: hauteur,
        back: 1,
    };
    gpu.contexte.CopySubresourceRegion(
        &destination,
        0,
        decalage.x.max(0) as u32,
        decalage.y.max(0) as u32,
        0,
        &source,
        0,
        Some(&boite),
    );
    drop(destination);
    surface.EndDraw()?;
    // Soumet la copie au GPU tout de suite : l'image WGC peut être rendue
    // au pool (et la session fermée) juste après le retour de `lancer`.
    gpu.contexte.Flush();

    // --- Fenêtre de surimpression, à la position de la vraie fenêtre.
    let marge = plan.marge_px.max(0);
    let b = trame.bornes;
    let x = b.left - marge;
    let y = b.top - marge;
    let l = largeur as i32 + 2 * marge;
    let h = hauteur as i32 + 2 * marge;
    // Coordonnées physiques : la fenêtre est créée « per-monitor v2 » quel
    // que soit le mode DPI du processus (SetProcessDPIAware = système).
    let instance = GetModuleHandleW(None)?;
    let ancien_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let fenetre = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        CLASSE,
        w!(""),
        WS_POPUP,
        x,
        y,
        l,
        h,
        None,
        None,
        Some(instance.into()),
        None,
    );
    if !ancien_dpi.0.is_null() {
        SetThreadDpiAwarenessContext(ancien_dpi);
    }
    let fenetre = fenetre?;

    // À partir d'ici, toute erreur doit détruire la fenêtre créée.
    match construire_arbre(gpu, &dev, fenetre, &surface, largeur, hauteur, marge, plan) {
        Ok((cible_dcomp, objets)) => {
            logging::info(&format!(
                "Surimpression #{id} ({sens:?}) lancée pour {cible:#x} : image {largeur}x{hauteur}, \
                 fenêtre ({x}, {y}) {l}x{h}, bornes cible {}, durée {:.0} ms.",
                texte_bornes(&b),
                plan.duree * 1000.0
            ));
            let _ = ShowWindow(fenetre, SW_SHOWNA);
            let ancien_dpi =
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = SetWindowPos(
                fenetre,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            if !ancien_dpi.0.is_null() {
                SetThreadDpiAwarenessContext(ancien_dpi);
            }
            Ok(AnimationActive {
                id,
                sens,
                fenetre,
                cible,
                masquee_par_nous: false,
                debut: Instant::now(),
                duree: plan.duree,
                contenu: Contenu::Composition {
                    _cible_dcomp: cible_dcomp,
                    _objets: objets,
                },
            })
        }
        Err(e) => {
            let _ = DestroyWindow(fenetre);
            Err(e)
        }
    }
}

/// Lance un effet à shader : crée la surimpression (swapchain de
/// composition), copie l'image (GPU -> GPU) et présente la première image.
pub unsafe fn lancer_shader(
    gpu: &ContexteGpu,
    id: u32,
    sens: Sens,
    cible: isize,
    trame: &DerniereTrame,
    plan: &PlanShader,
    graine: f32,
) -> Result<AnimationActive, EchecEffet> {
    let rendu = RenduEffet::creer(gpu, id, sens, trame, plan, graine)?;
    logging::info(&format!(
        "Surimpression #{id} ({sens:?}) lancée pour {cible:#x} : effet à shader « {} », durée \
         {:.0} ms.",
        plan.effet.nom(),
        plan.duree * 1000.0
    ));
    Ok(AnimationActive {
        id,
        sens,
        fenetre: rendu.fenetre(),
        cible,
        masquee_par_nous: false,
        debut: Instant::now(),
        duree: plan.duree,
        contenu: Contenu::Shader(Box::new(rendu)),
    })
}

/// Construit l'arbre de visuels, lie les animations et valide (`Commit`).
#[allow(clippy::too_many_arguments)]
unsafe fn construire_arbre(
    gpu: &ContexteGpu,
    dev: &IDCompositionDevice2,
    fenetre: HWND,
    surface: &windows::Win32::Graphics::DirectComposition::IDCompositionSurface,
    largeur: u32,
    hauteur: u32,
    marge: i32,
    plan: &PlanAnimation,
) -> Result<(IDCompositionTarget, Vec<IUnknown>), ErreurAnimation> {
    let mut objets: Vec<IUnknown> = Vec::new();
    let cible_dcomp = gpu.dcomp.CreateTargetForHwnd(fenetre, true)?;

    let racine = dev.CreateVisual()?;
    racine.SetOffsetX2(marge as f32)?;
    racine.SetOffsetY2(marge as f32)?;

    // Opacité : portée par un groupe d'effets sur la racine.
    if let Some(piste) = &plan.opacite {
        let groupe = dev.CreateEffectGroup()?;
        let anim = creer_animation(dev, piste)?;
        groupe.SetOpacity(&anim)?;
        racine.SetEffect(&groupe)?;
        objets.push(groupe.cast()?);
        objets.push(anim.cast()?);
    }

    let contenu = dev.CreateVisual()?;
    contenu.SetContent(surface)?;

    // Coins arrondis de Windows 11.
    let decoupe = dev.CreateRectangleClip()?;
    decoupe.SetLeft2(0.0)?;
    decoupe.SetTop2(0.0)?;
    decoupe.SetRight2(largeur as f32)?;
    decoupe.SetBottom2(hauteur as f32)?;
    let r = plan.rayon_coins.max(0.0);
    decoupe.SetTopLeftRadiusX2(r)?;
    decoupe.SetTopLeftRadiusY2(r)?;
    decoupe.SetTopRightRadiusX2(r)?;
    decoupe.SetTopRightRadiusY2(r)?;
    decoupe.SetBottomLeftRadiusX2(r)?;
    decoupe.SetBottomLeftRadiusY2(r)?;
    decoupe.SetBottomRightRadiusX2(r)?;
    decoupe.SetBottomRightRadiusY2(r)?;
    contenu.SetClip(&decoupe)?;
    objets.push(decoupe.cast()?);

    // Échelle autour du centre du plan.
    if plan.echelle_x.is_some() || plan.echelle_y.is_some() {
        let echelle = dev.CreateScaleTransform()?;
        echelle.SetCenterX2(plan.centre.0)?;
        echelle.SetCenterY2(plan.centre.1)?;
        if let Some(piste) = &plan.echelle_x {
            let anim = creer_animation(dev, piste)?;
            echelle.SetScaleX(&anim)?;
            objets.push(anim.cast()?);
        }
        if let Some(piste) = &plan.echelle_y {
            let anim = creer_animation(dev, piste)?;
            echelle.SetScaleY(&anim)?;
            objets.push(anim.cast()?);
        }
        contenu.SetTransform(&echelle)?;
        objets.push(echelle.cast()?);
    }

    // Décalage vertical (glissement).
    if let Some(piste) = &plan.decalage_y {
        let anim = creer_animation(dev, piste)?;
        contenu.SetOffsetY(&anim)?;
        objets.push(anim.cast()?);
    }

    // Flou gaussien (IDCompositionDevice3 requis ; sinon ignoré, il reste
    // le fondu de l'effet).
    if let (Some(piste), Some(dcomp3)) = (&plan.flou, &gpu.dcomp3) {
        let flou = dcomp3.CreateGaussianBlurEffect()?;
        let anim = creer_animation(dev, piste)?;
        flou.SetStandardDeviation(&anim)?;
        flou.SetBorderMode(D2D1_BORDER_MODE_HARD)?;
        contenu.SetEffect(&flou)?;
        objets.push(flou.cast()?);
        objets.push(anim.cast()?);
    }

    racine.AddVisual(&contenu, false, None::<&IDCompositionVisual>)?;
    cible_dcomp.SetRoot(&racine)?;
    objets.push(contenu.cast()?);
    objets.push(racine.cast()?);
    objets.push(surface.cast()?);

    // Un seul Commit : les animations démarrent à cet instant, côté DWM.
    dev.Commit()?;
    Ok((cible_dcomp, objets))
}

/// Termine une animation : restaure la vraie fenêtre si elle était
/// masquée par nous (transparence temporaire ; `masquage::restaurer`
/// attend la composition DWM suivante pour éviter un « trou » visuel),
/// puis détruit la surimpression. À appeler HORS de tout emprunt de l'état.
/// Fin d'ouverture (v0.4.6, comme en v0.4.3) : restauration immédiate
/// (alpha 255, `WS_EX_LAYERED` conservé, invalidation légère) puis levée
/// instantanée de la surimpression — plus de maintien.
pub unsafe fn terminer(anim: AnimationActive) {
    logging::info(&format!(
        "Surimpression #{} ({:?}) terminée pour {:#x} après {} ms.",
        anim.id,
        anim.sens,
        anim.cible,
        anim.debut.elapsed().as_millis()
    ));
    if anim.masquee_par_nous {
        let _ = masquage::restaurer(anim.cible);
    }
    match anim.contenu {
        Contenu::Composition { .. } => {
            let _ = DestroyWindow(anim.fenetre);
            drop(anim.contenu);
        }
        Contenu::Shader(rendu) => {
            logging::info(&format!(
                "Surimpression #{} : {} image(s) rendue(s) par le shader.",
                anim.id,
                rendu.rendus()
            ));
            // Retire la swapchain, délie les ressources et détruit la fenêtre.
            drop(rendu);
        }
    }
}
