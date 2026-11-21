//! Rendu des effets « à shader » (v0.4.7) : l'image capturée de la fenêtre
//! est traitée par le pixel shader de l'effet, dessiné par un triangle
//! plein écran dans une swapchain de composition (flip-séquentielle, alpha
//! prémultiplié) liée à un visuel DirectComposition de la fenêtre de
//! surimpression habituelle (topmost, non activable, transparente aux
//! clics). Modèle : `maillage_gpu` (Jelly), dont les utilitaires sont
//! dupliqués dans [`d3d_commun`](super::d3d_commun).
//!
//! TOUT le rendu est fait par le GPU matériel : copie GPU -> GPU de
//! l'image WGC dans une texture échantillonnable ; le CPU n'écrit que le
//! tampon de constantes (48 octets, `Map(WRITE_DISCARD)`) à chaque image ;
//! un `Draw(3, 0)` + `Present(0, 0)` par image, cadencé par le fil de
//! battement (`metronome`) du module d'animations.
//!
//! Cache par périphérique (remis à zéro si l'`ID3D11Device` change) :
//! - la BASE (niveau de fonctionnalité >= 10_0, adaptateur matériel,
//!   nuanceur de sommets, échantillonneur, rastériseur, fabrique DXGI),
//!   construite une fois ;
//! - un pixel shader PAR EFFET, compilé à la demande : un shader fautif
//!   ne rend indisponible que son effet ([`EchecEffet::Indisponible`]),
//!   les autres restent utilisables. Les échecs sont mémorisés jusqu'à
//!   [`oublier_echec`] (choix d'un effet dans le menu).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::core::{w, Interface};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_FEATURE_LEVEL_10_0,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11Multithread, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE,
    D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_WRITE_DISCARD, D3D11_RASTERIZER_DESC, D3D11_SAMPLER_DESC, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionDevice2, IDCompositionTarget, IDCompositionVisual2,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIFactory2, IDXGIOutput, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::capture::{texte_bornes, DerniereTrame};
use super::catalogue::EffetAnimation;
use super::courbes::Piste;
use super::d3d_commun::{
    compiler, en_pmv2, etape, multifil, refus, sortie, verifier_adaptateur, EchecD3d, VerrouD3d,
};
use super::effets_shader::{self, ConstantesEffet};
use super::erreur::ErreurAnimation;
use super::gpu::ContexteGpu;
use super::hlsl_effets::VS_HLSL;
use super::jelly::taille_max_texture;
use super::plan::{PlanShader, Sens};
use super::surimpression;
use crate::logging;

/// Longueur maximale du message du compilateur conservé (caractères).
const MAX_MESSAGE_COMPILATEUR: usize = 2000;
/// Échecs consécutifs du rendu tolérés avant d'abandonner l'effet.
const MAX_ECHECS_CONSECUTIFS: u32 = 3;

/// Échec de préparation ou de rendu d'un effet à shader.
#[derive(Debug, Clone)]
pub enum EchecEffet {
    /// Shader non compilé / refusé, ou base du pipeline impossible
    /// (`global` : compilateur absent, adaptateur logiciel, niveau de
    /// fonctionnalité... — tous les effets à shader sont concernés).
    /// Mémorisé jusqu'à [`oublier_echec`].
    Indisponible {
        effet: EffetAnimation,
        global: bool,
        etape: String,
        motif: String,
    },
    /// Propre à cette fenêtre / cette image (taille, ressource, fenêtre,
    /// composition, rendu répété en échec) : cette animation seulement.
    Ponctuel { etape: String, motif: String },
    /// Périphérique graphique perdu : le contexte GPU doit être recréé.
    PeripheriquePerdu(ErreurAnimation),
}

impl std::fmt::Display for EchecEffet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EchecEffet::Indisponible {
                effet,
                global,
                etape,
                motif,
            } => write!(
                f,
                "effet « {} » indisponible{} — étape « {etape} » : {motif}",
                effet.nom(),
                if *global { " (pipeline commun)" } else { "" }
            ),
            EchecEffet::Ponctuel { etape, motif } => {
                write!(f, "échec ponctuel — étape « {etape} » : {motif}")
            }
            EchecEffet::PeripheriquePerdu(e) => write!(f, "périphérique perdu : {e}"),
        }
    }
}

/// Classe un échec d'étape propre à une animation : périphérique perdu
/// si son HRESULT l'indique, sinon ponctuel.
fn ponctuel(e: EchecD3d) -> EchecEffet {
    if e.peripherique_perdu() {
        if let Some(err) = e.erreur {
            return EchecEffet::PeripheriquePerdu(ErreurAnimation::Win32(err));
        }
    }
    EchecEffet::Ponctuel {
        etape: e.etape,
        motif: e.motif,
    }
}

/// État de rendu d'une animation à shader après un battement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtatRendu {
    /// Image intermédiaire rendue : d'autres suivront.
    EnCours,
    /// Dernière image (progression finale) rendue à l'instant.
    DerniereImage,
    /// Plus rien à rendre (dernière image déjà présentée).
    DejaFini,
}

// ---------------------------------------------------------------------------
// Cache par périphérique
// ---------------------------------------------------------------------------

/// Objets communs à tous les effets (un jeu par périphérique).
struct Base {
    vs: ID3D11VertexShader,
    echantillonneur: ID3D11SamplerState,
    rasterisation: ID3D11RasterizerState,
    fabrique: IDXGIFactory2,
}

enum EtatBase {
    Vide,
    Pret(Rc<Base>),
    Impossible { etape: String, motif: String },
}

enum EtatPs {
    Pret(ID3D11PixelShader),
    Impossible { etape: String, motif: String },
}

struct Cache {
    /// Pointeur brut de l'`ID3D11Device` auquel le cache se rapporte.
    appareil: usize,
    base: EtatBase,
    ps: HashMap<EffetAnimation, EtatPs>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache {
        appareil: 0,
        base: EtatBase::Vide,
        ps: HashMap::new(),
    });
}

/// Exécute `f` sur le cache (jamais de panique : `None` si déjà emprunté).
fn avec_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> Option<R> {
    CACHE.with(|c| c.try_borrow_mut().ok().map(|mut g| f(&mut g)))
}

/// Vrai si l'effet est connu indisponible (shader ou base en échec
/// mémorisé) pour le périphérique courant.
pub fn est_indisponible(effet: EffetAnimation) -> bool {
    avec_cache(|c| {
        matches!(c.base, EtatBase::Impossible { .. })
            || matches!(c.ps.get(&effet), Some(EtatPs::Impossible { .. }))
    })
    .unwrap_or(false)
}

/// Vrai si le pixel shader de l'effet est déjà compilé pour `d3d`.
pub fn est_pret(d3d: &ID3D11Device, effet: EffetAnimation) -> bool {
    let appareil = d3d.as_raw() as usize;
    avec_cache(|c| {
        c.appareil == appareil
            && matches!(c.base, EtatBase::Pret(_))
            && matches!(c.ps.get(&effet), Some(EtatPs::Pret(_)))
    })
    .unwrap_or(false)
}

/// Oublie l'échec mémorisé de l'effet (et l'échec global de la base) : la
/// prochaine utilisation retentera la compilation. Note : l'échec du
/// chargement de `d3dcompiler_47.dll` lui-même reste mémorisé pour le
/// processus (`d3d_commun::charger_d3dcompile`).
pub fn oublier_echec(effet: EffetAnimation) {
    avec_cache(|c| {
        if matches!(c.base, EtatBase::Impossible { .. }) {
            c.base = EtatBase::Vide;
        }
        if matches!(c.ps.get(&effet), Some(EtatPs::Impossible { .. })) {
            c.ps.remove(&effet);
        }
    });
}

/// Remet le cache à zéro si le périphérique a changé.
fn aligner_cache(appareil: usize) {
    avec_cache(|c| {
        if c.appareil != appareil {
            c.appareil = appareil;
            c.base = EtatBase::Vide;
            c.ps.clear();
        }
    });
}

/// Construit la base du pipeline. L'erreur porte `true` si l'échec est
/// définitif pour ce périphérique.
unsafe fn construire_base(d3d: &ID3D11Device) -> Result<Base, (EchecD3d, bool)> {
    const C: &str = "Effets GPU (base du pipeline)";
    let debut = Instant::now();
    let definitif = |e: EchecD3d| (e, true);
    let passager = |e: EchecD3d| (e, false);

    let niveau = d3d.GetFeatureLevel();
    if niveau.0 < D3D_FEATURE_LEVEL_10_0.0 {
        return Err(definitif(refus(
            C,
            "niveau de fonctionnalité",
            format!("niveau de fonctionnalité Direct3D {:#x} < 10_0", niveau.0),
        )));
    }
    let fabrique = verifier_adaptateur(C, d3d, debut)?;

    let vs_compile = compiler(
        VS_HLSL,
        "effets_vs.hlsl",
        "vs_plein_ecran",
        "vs_4_0",
        MAX_MESSAGE_COMPILATEUR,
    )
    .map_err(|m| definitif(refus(C, "D3DCompile (VS plein écran)", m)))?;
    logging::info(&format!(
        "{C} : D3DCompile VS plein écran OK ({} octets, t = {} µs){}.",
        vs_compile.code.len(),
        debut.elapsed().as_micros(),
        texte_avertissements(&vs_compile.avertissements)
    ));
    let mut vs = None;
    etape(
        C,
        true,
        "CreateVertexShader",
        debut,
        d3d.CreateVertexShader(&vs_compile.code, None, Some(&mut vs)),
    )
    .map_err(definitif)?;
    let vs = sortie(C, "CreateVertexShader", vs).map_err(definitif)?;

    // Bilinéaire, bords bloqués.
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
        "{C} : prête en {} ms (niveau de fonctionnalité {:#x}).",
        debut.elapsed().as_millis(),
        niveau.0
    ));
    Ok(Base {
        vs,
        echantillonneur,
        rasterisation,
        fabrique,
    })
}

fn texte_avertissements(a: &str) -> String {
    if a.is_empty() {
        String::new()
    } else {
        format!(" — avertissements du compilateur : {a}")
    }
}

/// Base en cache pour `d3d` (construite au besoin).
unsafe fn base(d3d: &ID3D11Device, effet: EffetAnimation) -> Result<Rc<Base>, EchecEffet> {
    aligner_cache(d3d.as_raw() as usize);
    let en_cache = avec_cache(|c| match &c.base {
        EtatBase::Pret(b) => Some(Ok(b.clone())),
        EtatBase::Impossible { etape, motif } => Some(Err(EchecEffet::Indisponible {
            effet,
            global: true,
            etape: format!("{etape} (échec mémorisé)"),
            motif: motif.clone(),
        })),
        EtatBase::Vide => None,
    })
    .flatten();
    if let Some(r) = en_cache {
        return r;
    }
    match construire_base(d3d) {
        Ok(b) => {
            let b = Rc::new(b);
            avec_cache(|c| c.base = EtatBase::Pret(b.clone()));
            Ok(b)
        }
        Err((e, definitif)) => {
            if e.peripherique_perdu() {
                return Err(ponctuel(e));
            }
            if definitif {
                logging::avertir(&format!(
                    "Effets GPU : base du pipeline indisponible pour ce périphérique (étape « {} » \
                     : {}) — échec mémorisé, tous les effets à shader sont indisponibles.",
                    e.etape, e.motif
                ));
                avec_cache(|c| {
                    c.base = EtatBase::Impossible {
                        etape: e.etape.clone(),
                        motif: e.motif.clone(),
                    }
                });
                Err(EchecEffet::Indisponible {
                    effet,
                    global: true,
                    etape: e.etape,
                    motif: e.motif,
                })
            } else {
                Err(ponctuel(e))
            }
        }
    }
}

/// Pixel shader de l'effet en cache (compilé au besoin). Renvoie aussi la
/// durée de compilation si elle vient d'avoir lieu.
unsafe fn pixel_shader(
    d3d: &ID3D11Device,
    effet: EffetAnimation,
) -> Result<(ID3D11PixelShader, Option<Duration>), EchecEffet> {
    let en_cache = avec_cache(|c| match c.ps.get(&effet) {
        Some(EtatPs::Pret(ps)) => Some(Ok(ps.clone())),
        Some(EtatPs::Impossible { etape, motif }) => Some(Err(EchecEffet::Indisponible {
            effet,
            global: false,
            etape: format!("{etape} (échec mémorisé)"),
            motif: motif.clone(),
        })),
        None => None,
    })
    .flatten();
    if let Some(r) = en_cache {
        return r.map(|ps| (ps, None));
    }
    let contexte = format!("Effet GPU « {} » (pixel shader)", effet.nom());
    let c = contexte.as_str();
    let debut = Instant::now();
    let memoriser = |etape: &str, motif: &str| {
        avec_cache(|cache| {
            cache.ps.insert(
                effet,
                EtatPs::Impossible {
                    etape: etape.to_string(),
                    motif: motif.to_string(),
                },
            )
        });
        EchecEffet::Indisponible {
            effet,
            global: false,
            etape: etape.to_string(),
            motif: motif.to_string(),
        }
    };
    let (Some(parametres), Some(source)) = (
        effets_shader::parametres(effet),
        effets_shader::source_ps(effet),
    ) else {
        let e = refus(c, "source HLSL", "effet sans shader".into());
        return Err(memoriser(&e.etape, &e.motif));
    };
    let compile = match compiler(
        &source,
        parametres.nom_source,
        "ps_effet",
        "ps_4_0",
        MAX_MESSAGE_COMPILATEUR,
    ) {
        Ok(v) => v,
        Err(m) => {
            let e = refus(c, &format!("D3DCompile ({})", parametres.nom_source), m);
            logging::avertir(&format!(
                "{c} : échec mémorisé, cet effet est indisponible (les autres effets restent \
                 utilisables)."
            ));
            return Err(memoriser(&e.etape, &e.motif));
        }
    };
    let duree_compilation = debut.elapsed();
    logging::info(&format!(
        "{c} : D3DCompile {} OK ({} octets, t = {} µs){}.",
        parametres.nom_source,
        compile.code.len(),
        duree_compilation.as_micros(),
        texte_avertissements(&compile.avertissements)
    ));
    let mut ps = None;
    if let Err(e) = etape(
        c,
        true,
        "CreatePixelShader",
        debut,
        d3d.CreatePixelShader(&compile.code, None, Some(&mut ps)),
    ) {
        if e.peripherique_perdu() {
            return Err(ponctuel(e));
        }
        return Err(memoriser(&e.etape, &e.motif));
    }
    let ps = match sortie(c, "CreatePixelShader", ps) {
        Ok(ps) => ps,
        Err(e) => return Err(memoriser(&e.etape, &e.motif)),
    };
    avec_cache(|cache| cache.ps.insert(effet, EtatPs::Pret(ps.clone())));
    Ok((ps, Some(debut.elapsed())))
}

/// Base + pixel shader de l'effet.
unsafe fn pipeline(
    d3d: &ID3D11Device,
    effet: EffetAnimation,
) -> Result<(Rc<Base>, ID3D11PixelShader, Option<Duration>), EchecEffet> {
    let b = base(d3d, effet)?;
    let (ps, duree) = pixel_shader(d3d, effet)?;
    Ok((b, ps, duree))
}

/// Compile (si nécessaire) la base et le pixel shader de l'effet. Renvoie
/// la durée de la compilation (nulle si déjà en cache).
pub fn prechauffer(gpu: &ContexteGpu, effet: EffetAnimation) -> Result<Duration, EchecEffet> {
    let debut = Instant::now();
    let (_, _, compile) = unsafe { pipeline(&gpu.d3d, effet)? };
    Ok(if compile.is_some() {
        debut.elapsed()
    } else {
        Duration::ZERO
    })
}

// ---------------------------------------------------------------------------
// Rendu d'une animation
// ---------------------------------------------------------------------------

/// Surimpression d'un effet à shader. La fenêtre est détruite au `Drop`.
pub struct RenduEffet {
    id: u32,
    effet: EffetAnimation,
    fenetre: HWND,
    contexte: ID3D11DeviceContext,
    multithread: Option<ID3D11Multithread>,
    dcomp: IDCompositionDevice2,
    base: Rc<Base>,
    ps: ID3D11PixelShader,
    constantes: ID3D11Buffer,
    vue_image: ID3D11ShaderResourceView,
    swapchain: IDXGISwapChain1,
    vue_cible: ID3D11RenderTargetView,
    taille_cible: (u32, u32),
    valeurs: ConstantesEffet,
    progression: Piste,
    duree: f64,
    debut: Instant,
    fini: bool,
    echecs_consecutifs: u32,
    rendus: u32,
    _cible: IDCompositionTarget,
    _visuel: IDCompositionVisual2,
    _texture: ID3D11Texture2D,
}

/// Garde de la fenêtre pendant la création (détruite si la création
/// échoue après `CreateWindowExW`).
struct FenetrePartielle(HWND);

impl Drop for FenetrePartielle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}

impl RenduEffet {
    /// Crée la surimpression au-dessus de la fenêtre (bornes de
    /// `trame.bornes`, marge du plan), copie l'image (GPU -> GPU), rend la
    /// première image (progression à t = 0) PUIS valide l'arbre
    /// DirectComposition et affiche la fenêtre.
    pub unsafe fn creer(
        gpu: &ContexteGpu,
        id: u32,
        sens: Sens,
        trame: &DerniereTrame,
        plan: &PlanShader,
        graine: f32,
    ) -> Result<RenduEffet, EchecEffet> {
        let debut = Instant::now();
        let effet = plan.effet;
        let ctx = format!("Effet GPU #{id} « {} » ({sens:?})", effet.nom());
        let c = ctx.as_str();
        let d3d = &gpu.d3d;

        let (base, ps, compile) = pipeline(d3d, effet)?;
        let detaille = compile.is_some();

        let (largeur, hauteur) = trame.taille;
        let marge = plan.marge_px.max(0);
        let max = taille_max_texture(d3d.GetFeatureLevel().0) as i64;

        // --- Texture source WGC (reste sur le GPU).
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
        // Le pool peut différer du contenu (redimensionnement récent).
        let largeur = largeur.min(desc_src.Width);
        let hauteur = hauteur.min(desc_src.Height);
        let lc = largeur as i64 + 2 * marge as i64;
        let hc = hauteur as i64 + 2 * marge as i64;
        if largeur == 0 || hauteur == 0 || lc > max || hc > max {
            return Err(ponctuel(refus(
                c,
                "taille",
                format!("taille invalide {largeur}x{hauteur} (marge {marge}, maximum {max} px)"),
            )));
        }
        if desc_src.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(ponctuel(refus(
                c,
                "texture WGC",
                format!("format de texture WGC inattendu {:?}", desc_src.Format),
            )));
        }
        let (lc, hc) = (lc as u32, hc as u32);
        let mt = multifil(&gpu.contexte, d3d);

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
        )
        .map_err(ponctuel)?;
        let texture = sortie(c, "CreateTexture2D (image)", texture).map_err(ponctuel)?;
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
            // La session WGC peut être fermée juste après : copie soumise.
            gpu.contexte.Flush();
        }
        let mut vue_image = None;
        etape(
            c,
            detaille,
            "CreateShaderResourceView",
            debut,
            d3d.CreateShaderResourceView(&texture, None, Some(&mut vue_image)),
        )
        .map_err(ponctuel)?;
        let vue_image = sortie(c, "CreateShaderResourceView", vue_image).map_err(ponctuel)?;

        // --- Constantes (dynamiques : progression à chaque image).
        let valeurs = ConstantesEffet {
            taille_cible: [lc as f32, hc as f32],
            taille_image: [largeur as f32, hauteur as f32],
            marge: [marge as f32, marge as f32],
            progression: plan.progression.valeur_a(0.0).clamp(0.0, 1.0),
            graine: if graine.is_finite() {
                graine.clamp(0.0, 0.999_999)
            } else {
                0.0
            },
            rayon: plan.rayon_coins.max(0.0),
            sens: match sens {
                Sens::Fermeture => 0.0,
                Sens::Ouverture => 1.0,
            },
            opacite: 1.0,
            reserve: 0.0,
        };
        let desc_cb = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<ConstantesEffet>() as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut constantes = None;
        etape(
            c,
            detaille,
            "CreateBuffer (constantes)",
            debut,
            d3d.CreateBuffer(&desc_cb, None, Some(&mut constantes)),
        )
        .map_err(ponctuel)?;
        let constantes = sortie(c, "CreateBuffer (constantes)", constantes).map_err(ponctuel)?;

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
            base.fabrique
                .CreateSwapChainForComposition(d3d, &desc_sc, None::<&IDXGIOutput>),
        )
        .map_err(ponctuel)?;
        let tampon_sc: ID3D11Texture2D =
            etape(c, detaille, "GetBuffer(0)", debut, swapchain.GetBuffer(0)).map_err(ponctuel)?;
        let mut vue_cible = None;
        etape(
            c,
            detaille,
            "CreateRenderTargetView",
            debut,
            d3d.CreateRenderTargetView(&tampon_sc, None, Some(&mut vue_cible)),
        )
        .map_err(ponctuel)?;
        let vue_cible = sortie(c, "CreateRenderTargetView", vue_cible).map_err(ponctuel)?;
        drop(tampon_sc);

        // --- Fenêtre de surimpression.
        let b = trame.bornes;
        let x = b.left - marge;
        let y = b.top - marge;
        let instance = etape(
            c,
            detaille,
            "GetModuleHandleW",
            debut,
            GetModuleHandleW(None),
        )
        .map_err(ponctuel)?;
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
        )
        .map_err(ponctuel)?;
        let mut garde = FenetrePartielle(fenetre);

        // Première image AVANT l'affichage (pas d'image vide à l'écran).
        let rendu = Rendu {
            contexte: &gpu.contexte,
            multithread: mt.as_ref(),
            base: &base,
            ps: &ps,
            constantes: &constantes,
            vue_image: &vue_image,
            vue_cible: &vue_cible,
            swapchain: &swapchain,
            taille_cible: (lc, hc),
        };
        let resultat_premier = etape(
            c,
            true,
            "premier rendu + Present",
            debut,
            rendu.dessiner(&valeurs),
        );
        if let Err(e) = resultat_premier {
            delier_sous_verrou(&gpu.contexte, mt.as_ref());
            return Err(ponctuel(e));
        }

        let suite = (|| -> Result<(IDCompositionDevice2, IDCompositionTarget, IDCompositionVisual2), EchecD3d> {
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
            etape(c, detaille, "SetContent(swapchain)", debut, visuel.SetContent(&swapchain))?;
            etape(c, detaille, "SetRoot", debut, cible.SetRoot(&visuel))?;
            etape(c, detaille, "Commit", debut, dev.Commit())?;
            Ok((dev, cible, visuel))
        })();
        let (dev, cible_dcomp, visuel) = match suite {
            Ok(v) => v,
            Err(e) => {
                delier_sous_verrou(&gpu.contexte, mt.as_ref());
                return Err(ponctuel(e));
            }
        };

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
        // Création réussie : la fenêtre passe sous la garde de RenduEffet.
        garde.0 = HWND::default();
        drop(garde);

        logging::info(&format!(
            "{c} : surimpression créée en {} ms — image {largeur}x{hauteur}, swapchain {lc}x{hc} \
             (marge {marge} px, flip-séquentielle 2 tampons, alpha prémultiplié), fenêtre ({x}, \
             {y}), bornes cible {}, durée {:.0} ms, graine {:.4}{}.",
            debut.elapsed().as_millis(),
            texte_bornes(&b),
            plan.duree * 1000.0,
            valeurs.graine,
            if detaille {
                ", shader compilé pour l'occasion"
            } else {
                ""
            }
        ));

        Ok(RenduEffet {
            id,
            effet,
            fenetre,
            contexte: gpu.contexte.clone(),
            multithread: mt,
            dcomp: dev,
            base,
            ps,
            constantes,
            vue_image,
            swapchain,
            vue_cible,
            taille_cible: (lc, hc),
            valeurs,
            progression: plan.progression.clone(),
            duree: plan.duree,
            debut: Instant::now(),
            fini: false,
            echecs_consecutifs: 0,
            rendus: 1,
            _cible: cible_dcomp,
            _visuel: visuel,
            _texture: texture,
        })
    }

    /// Fenêtre de surimpression (détruite au `Drop`).
    pub fn fenetre(&self) -> HWND {
        self.fenetre
    }

    /// Vrai si la dernière image a été présentée (plus rien à rendre).
    pub fn est_fini(&self) -> bool {
        self.fini
    }

    /// Nombre d'images présentées (journal).
    pub fn rendus(&self) -> u32 {
        self.rendus
    }

    /// Rend l'image correspondant à l'instant présent. Un échec isolé est
    /// toléré (`EnCours`, réessayé au battement suivant) ; erreur après
    /// [`MAX_ECHECS_CONSECUTIFS`] échecs consécutifs, ou immédiatement si
    /// le périphérique est perdu.
    pub unsafe fn dessiner_maintenant(&mut self) -> Result<EtatRendu, EchecEffet> {
        if self.fini {
            return Ok(EtatRendu::DejaFini);
        }
        let t = self.debut.elapsed().as_secs_f64();
        let derniere = t >= self.duree;
        let p = if derniere {
            self.progression.valeur_finale()
        } else {
            self.progression.valeur_a(t)
        };
        self.valeurs.progression = if p.is_finite() {
            p.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let rendu = Rendu {
            contexte: &self.contexte,
            multithread: self.multithread.as_ref(),
            base: &self.base,
            ps: &self.ps,
            constantes: &self.constantes,
            vue_image: &self.vue_image,
            vue_cible: &self.vue_cible,
            swapchain: &self.swapchain,
            taille_cible: self.taille_cible,
        };
        match rendu.dessiner(&self.valeurs) {
            Ok(()) => {
                self.echecs_consecutifs = 0;
                self.rendus = self.rendus.saturating_add(1);
                if derniere {
                    self.fini = true;
                    Ok(EtatRendu::DerniereImage)
                } else {
                    Ok(EtatRendu::EnCours)
                }
            }
            Err(e) => {
                let code = e.code().0 as u32;
                if super::jelly::hresult_peripherique_perdu(code) {
                    logging::avertir(&format!(
                        "Effet GPU #{} « {} » : périphérique perdu au rendu (HRESULT {code:#010x}).",
                        self.id,
                        self.effet.nom()
                    ));
                    return Err(EchecEffet::PeripheriquePerdu(ErreurAnimation::Win32(e)));
                }
                self.echecs_consecutifs += 1;
                logging::avertir(&format!(
                    "Effet GPU #{} « {} » : échec du rendu (HRESULT {code:#010x} : {}), échec \
                     consécutif {}/{MAX_ECHECS_CONSECUTIFS}.",
                    self.id,
                    self.effet.nom(),
                    e.message(),
                    self.echecs_consecutifs
                ));
                if self.echecs_consecutifs >= MAX_ECHECS_CONSECUTIFS {
                    return Err(EchecEffet::Ponctuel {
                        etape: "rendu (Map / Draw / Present)".into(),
                        motif: format!("HRESULT {code:#010x} ({})", e.message()),
                    });
                }
                Ok(EtatRendu::EnCours)
            }
        }
    }
}

impl Drop for RenduEffet {
    fn drop(&mut self) {
        unsafe {
            // Retire la swapchain du visuel et valide AVANT de détruire la
            // fenêtre (erreurs ignorées : périphérique éventuellement perdu).
            let _ = self._visuel.SetContent(None::<&windows::core::IUnknown>);
            let _ = self.dcomp.Commit();
            delier_sous_verrou(&self.contexte, self.multithread.as_ref());
            // Erreur ignorée : la fenêtre a pu être détruite avec le processus.
            let _ = DestroyWindow(self.fenetre);
        }
    }
}

/// Délie NOS liaisons du contexte immédiat partagé (vue de rendu, image,
/// constantes, nuanceurs) sans `ClearState` global, sous la section
/// critique, puis soumet.
unsafe fn delier_sous_verrou(contexte: &ID3D11DeviceContext, mt: Option<&ID3D11Multithread>) {
    let _verrou = VerrouD3d::prendre(mt);
    contexte.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
    contexte.PSSetShaderResources(0, Some(&[None]));
    contexte.PSSetConstantBuffers(0, Some(&[None]));
    contexte.Flush();
}

/// Références nécessaires à un rendu.
struct Rendu<'a> {
    contexte: &'a ID3D11DeviceContext,
    multithread: Option<&'a ID3D11Multithread>,
    base: &'a Base,
    ps: &'a ID3D11PixelShader,
    constantes: &'a ID3D11Buffer,
    vue_image: &'a ID3D11ShaderResourceView,
    vue_cible: &'a ID3D11RenderTargetView,
    swapchain: &'a IDXGISwapChain1,
    taille_cible: (u32, u32),
}

impl Rendu<'_> {
    /// Écrit les constantes, efface (transparent), dessine le triangle
    /// plein écran et présente. Tout l'état est reposé à chaque fois (le
    /// contexte immédiat est partagé avec le Jelly et la capture), sous la
    /// section critique du périphérique.
    unsafe fn dessiner(&self, valeurs: &ConstantesEffet) -> windows::core::Result<()> {
        let _verrou = VerrouD3d::prendre(self.multithread);
        let ctx = self.contexte;
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(
            self.constantes,
            0,
            D3D11_MAP_WRITE_DISCARD,
            0,
            Some(&mut map),
        )?;
        if map.pData.is_null() {
            ctx.Unmap(self.constantes, 0);
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_POINTER,
                "Map du tampon de constantes sans pointeur",
            ));
        }
        std::ptr::copy_nonoverlapping(
            valeurs as *const ConstantesEffet as *const u8,
            map.pData as *mut u8,
            std::mem::size_of::<ConstantesEffet>(),
        );
        ctx.Unmap(self.constantes, 0);

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
        ctx.RSSetState(&self.base.rasterisation);
        // Mélange désactivé : le shader écrit directement la couleur
        // prémultipliée (fond effacé à transparent).
        ctx.OMSetBlendState(None::<&ID3D11BlendState>, None, 0xffff_ffff);

        // Triangle plein écran : ni tampon de sommets ni disposition.
        ctx.IASetInputLayout(None::<&ID3D11InputLayout>);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let constantes = [Some(self.constantes.clone())];
        ctx.VSSetShader(&self.base.vs, None);
        ctx.PSSetShader(self.ps, None);
        ctx.PSSetConstantBuffers(0, Some(&constantes));
        ctx.PSSetShaderResources(0, Some(&[Some(self.vue_image.clone())]));
        ctx.PSSetSamplers(0, Some(&[Some(self.base.echantillonneur.clone())]));

        ctx.Draw(3, 0);
        // Intervalle 0 : jamais d'attente de la synchronisation verticale
        // sur le fil de l'interface (cadence = fil de battement).
        self.swapchain.Present(0, DXGI_PRESENT(0)).ok()
    }
}
