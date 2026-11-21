//! Capture du contenu d'une fenêtre par Windows.Graphics.Capture (WGC).
//!
//! Les images arrivent sous forme de textures Direct3D 11 qui restent sur
//! le GPU : aucun `BitBlt`, aucune copie vers la mémoire centrale.
//! Seule la dernière image est conservée (partagée avec le rappel WGC).
//!
//! **Fil de capture (v0.3.2)** : les sessions sont créées sur un fil
//! dédié ([`Atelier`], appartement MTA, son propre périphérique WinRT
//! construit sur le MÊME périphérique Direct3D 11) et non plus sur le fil
//! de la boucle de messages. Retour de test v0.3.1 : le fil principal n'est
//! jamais revenu de la création de la session de suivi du Bloc-notes
//! (journal arrêté net) ; un blocage ou une lenteur de WGC ne peut
//! désormais plus geler l'application :
//! - suivi de fermeture : demande ASYNCHRONE ([`Atelier::demander`]), le
//!   résultat revient par `WM_SESSION_CREEE` ([`recuperer_resultats`]) ;
//! - ouverture : attente bornée ([`demarrer_session`], délai
//!   [`DELAI_CREATION_MS`]) ;
//! - fil bloqué plus de [`SEUIL_BLOCAGE_MS`] : il est abandonné et un
//!   nouveau fil est lancé ([`MAX_RELANCES`] fois au plus) ; une
//!   surveillance périodique (module parent) journalise l'étape bloquée ;
//! - si le fil est indisponible (lancement ou périphérique WinRT
//!   impossibles), repli sur la création directe dans le fil appelant
//!   (comportement v0.3.1).
//!
//! Journalisation : une ligne AVANT chaque sous-étape risquée de la
//! création (un blocage est ainsi localisable), une ligne de fin, erreurs
//! avec l'étape et le HRESULT, première image (délai, taille, bornes),
//! arrêt (nombre d'images). Aucune ligne par image (pas de spam).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::core::{factory, AgileReference, IInspectable, Interface, Ref};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
    GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, RPC_E_CHANGED_MODE, WPARAM};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Multithread, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::eligibilite;
use super::erreur::ErreurAnimation;
use super::gpu::ContexteGpu;
use super::attente;
use super::{WM_SESSION_CREEE, WM_TRAME_PRETE};
use crate::logging;

/// Nombre de tampons du pool : un pour l'image conservée, un pour la suivante.
const TAMPONS: i32 = 2;
/// Intervalle minimal entre deux images (100 ms, en unités de 100 ns).
const INTERVALLE_MIN: i64 = 100 * 10_000;
/// Attente maximale d'une création synchrone (ouverture) par le fil de
/// capture. Une création normale prend quelques dizaines de ms.
pub const DELAI_CREATION_MS: u64 = 700;
/// Au-delà, une création en cours est considérée comme bloquée : les
/// nouvelles demandes sont refusées immédiatement.
pub const SEUIL_BLOCAGE_MS: u64 = 1500;
/// Demandes au plus en file (en cours comprise).
const MAX_EN_FILE: usize = 3;
/// Relances au plus d'un fil de capture bloqué (chaque fil abandonné reste
/// bloqué dans son appel Windows : borne stricte).
const MAX_RELANCES: u32 = 3;

/// Noms des étapes de création (index = numéro d'étape ; 0 = fil libre).
const ETAPES: [&str; 12] = [
    "libre",
    "IsSupported",
    "CreateForWindow",
    "Size",
    "CreateFreeThreaded (pool)",
    "CreateCaptureSession",
    "SetIsCursorCaptureEnabled",
    "SetIsBorderRequired",
    "SetMinUpdateInterval",
    "gestionnaire + FrameArrived",
    "StartCapture",
    "terminée",
];
const NB_ETAPES_UTILES: usize = ETAPES.len() - 2;

/// Dernière image capturée d'une fenêtre.
pub struct DerniereTrame {
    /// Image WGC (sa texture reste sur le GPU tant que l'objet vit).
    pub trame: Direct3D11CaptureFrame,
    /// Rectangle visible de la fenêtre au moment de la capture
    /// (`DWMWA_EXTENDED_FRAME_BOUNDS`, pixels physiques).
    pub bornes: RECT,
    /// Taille utile du contenu (pixels).
    pub taille: (u32, u32),
    /// Numéro de l'image dans sa session (1 = première ; égal à
    /// [`SessionCapture::trames_recues`] quand c'est la plus récente).
    pub numero: u64,
}

impl Drop for DerniereTrame {
    fn drop(&mut self) {
        // Rend immédiatement le tampon au pool (erreur ignorée).
        let _ = self.trame.Close();
    }
}

/// Image partagée entre le fil WGC (écriture) et la boucle de messages (lecture).
pub type TramePartagee = Arc<Mutex<Option<DerniereTrame>>>;

/// Session de capture continue d'une fenêtre.
pub struct SessionCapture {
    /// Fenêtre capturée (valeur brute du HWND).
    pub hwnd: isize,
    derniere: TramePartagee,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    jeton: i64,
    /// Dernières bornes connues de la fenêtre (rafraîchies par le gestionnaire
    /// d'images et par [`SessionCapture::rafraichir_bornes`]).
    bornes: Arc<Mutex<RECT>>,
    /// Positionné à la fermeture : le gestionnaire ne stocke plus d'image.
    ferme: Arc<AtomicBool>,
    /// Nombre d'images valides reçues (incrémenté par le gestionnaire).
    compteur: Arc<AtomicU64>,
    /// Instant de la dernière image stockée, en ms depuis `debut`, + 1
    /// (0 = aucune image).
    derniere_ms: Arc<AtomicU64>,
    /// Rôle de la session (pour le journal : « suivi », « ouverture #3 »...).
    role: String,
    debut: Instant,
}

/// Suivi des étapes d'une création : journal + état partagé du fil.
struct Etapes<'a> {
    role: &'a str,
    hwnd: isize,
    fil: Option<&'a EtatFil>,
    courante: Cell<usize>,
}

impl Etapes<'_> {
    fn passer(&self, n: usize) {
        self.courante.set(n);
        if let Some(f) = self.fil {
            f.marquer_etape(n);
        }
        let nom = ETAPES.get(n).copied().unwrap_or("?");
        logging::info(&format!(
            "Capture WGC [{}] {:#x} : étape {n}/{NB_ETAPES_UTILES} {nom}…",
            self.role, self.hwnd
        ));
    }
    fn nom_courant(&self) -> &'static str {
        ETAPES.get(self.courante.get()).copied().unwrap_or("?")
    }
}

impl SessionCapture {
    /// Crée la session de capture de `hwnd` dans le fil COURANT avec le
    /// périphérique WinRT `appareil`. Si `notifier` vaut
    /// `Some((fenetre, id))`, `WM_TRAME_PRETE` (wparam = id) est posté à
    /// `fenetre` à la première image. Journalise chaque étape, le succès
    /// ou l'échec (étape + HRESULT). `fil` : état du fil de capture à tenir
    /// à jour (surveillance d'un blocage), `None` hors fil de capture.
    fn creer(
        appareil: &IDirect3DDevice,
        hwnd: isize,
        notifier: Option<(isize, u32)>,
        role: &str,
        fil: Option<&EtatFil>,
    ) -> Result<Self, ErreurAnimation> {
        let etapes = Etapes {
            role,
            hwnd,
            fil,
            courante: Cell::new(0),
        };
        let debut = Instant::now();
        match Self::creer_interne(appareil, hwnd, notifier, role, &etapes) {
            Ok(s) => {
                logging::info(&format!(
                    "Capture WGC [{role}] démarrée sur {hwnd:#x} en {} ms (bornes {}, fil {}).",
                    debut.elapsed().as_millis(),
                    texte_bornes(&s.bornes()),
                    if fil.is_some() { "de capture" } else { "principal" }
                ));
                Ok(s)
            }
            Err(e) => {
                logging::avertir(&format!(
                    "Capture WGC [{role}] impossible sur {hwnd:#x} à l'étape « {} » après {} ms : {e}",
                    etapes.nom_courant(),
                    debut.elapsed().as_millis()
                ));
                Err(e)
            }
        }
    }

    fn creer_interne(
        appareil: &IDirect3DDevice,
        hwnd: isize,
        notifier: Option<(isize, u32)>,
        role: &str,
        etapes: &Etapes<'_>,
    ) -> Result<Self, ErreurAnimation> {
        let debut = Instant::now();
        etapes.passer(1);
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err(ErreurAnimation::CaptureNonSupportee);
        }
        etapes.passer(2);
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForWindow(HWND(hwnd as *mut core::ffi::c_void))? };
        etapes.passer(3);
        let taille = item.Size()?;
        if taille.Width <= 0 || taille.Height <= 0 {
            return Err(ErreurAnimation::TailleInvalide);
        }

        etapes.passer(4);
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            appareil,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            TAMPONS,
            taille,
        )?;
        etapes.passer(5);
        let session = match pool.CreateCaptureSession(&item) {
            Ok(s) => s,
            Err(e) => {
                let _ = pool.Close();
                return Err(e.into());
            }
        };

        // Pas de curseur dans l'image.
        etapes.passer(6);
        let _ = session.SetIsCursorCaptureEnabled(false);
        // OBLIGATOIRE : sans cela Windows dessine une bordure jaune autour de
        // la fenêtre capturée, inacceptable pour un effet visuel permanent.
        // Disponible à partir de Windows 11 (IGraphicsCaptureSession3) ;
        // vérifié fonctionnel sur Windows 11 (retours de test v0.3.x).
        etapes.passer(7);
        if let Err(e) = session.SetIsBorderRequired(false) {
            logging::avertir(&format!(
                "Capture WGC [{role}] {hwnd:#x} : SetIsBorderRequired(false) refusé ({:#010x}).",
                e.code().0 as u32
            ));
            let _ = session.Close();
            let _ = pool.Close();
            return Err(ErreurAnimation::BordureImposee);
        }
        // Limite la fréquence de capture (Windows 11 24H2+) ; échec toléré.
        etapes.passer(8);
        let _ = session.SetMinUpdateInterval(TimeSpan {
            Duration: INTERVALLE_MIN,
        });

        etapes.passer(9);
        let derniere: TramePartagee = Arc::new(Mutex::new(None));
        let bornes_init = unsafe { eligibilite::bornes_fenetre(HWND(hwnd as *mut core::ffi::c_void)) }
            .unwrap_or_default();
        let bornes = Arc::new(Mutex::new(bornes_init));
        let ferme = Arc::new(AtomicBool::new(false));
        let compteur = Arc::new(AtomicU64::new(0));
        let derniere_ms = Arc::new(AtomicU64::new(0));
        let gestionnaire = match creer_gestionnaire(
            appareil,
            hwnd,
            taille,
            Partage {
                derniere: Arc::clone(&derniere),
                bornes: Arc::clone(&bornes),
                ferme: Arc::clone(&ferme),
                compteur: Arc::clone(&compteur),
                derniere_ms: Arc::clone(&derniere_ms),
            },
            notifier,
            role.to_string(),
            debut,
        ) {
            Ok(g) => g,
            Err(e) => {
                let _ = session.Close();
                let _ = pool.Close();
                return Err(e);
            }
        };
        let jeton = match pool.FrameArrived(&gestionnaire) {
            Ok(j) => j,
            Err(e) => {
                let _ = session.Close();
                let _ = pool.Close();
                return Err(e.into());
            }
        };
        etapes.passer(10);
        if let Err(e) = session.StartCapture() {
            let _ = pool.RemoveFrameArrived(jeton);
            let _ = session.Close();
            let _ = pool.Close();
            return Err(e.into());
        }
        etapes.passer(11);
        Ok(SessionCapture {
            hwnd,
            derniere,
            pool,
            session,
            jeton,
            bornes,
            ferme,
            compteur,
            derniere_ms,
            role: role.to_string(),
            debut,
        })
    }

    /// Nombre d'images valides reçues depuis le démarrage.
    pub fn trames_recues(&self) -> u64 {
        self.compteur.load(Ordering::Relaxed)
    }

    /// Âge de la session (ms).
    pub fn age_ms(&self) -> u128 {
        self.debut.elapsed().as_millis()
    }

    /// Âge de la dernière image reçue (ms), `None` si aucune image.
    pub fn age_derniere_image_ms(&self) -> Option<u64> {
        let v = self.derniere_ms.load(Ordering::Relaxed);
        if v == 0 {
            return None;
        }
        let maintenant = self.debut.elapsed().as_millis() as u64;
        Some(maintenant.saturating_sub(v - 1))
    }

    /// Vrai si une image est conservée (prête pour une fermeture).
    pub fn a_une_image(&self) -> bool {
        self.derniere
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Relit les bornes de la fenêtre (si elle existe encore) ; sinon les
    /// dernières bornes mémorisées sont conservées.
    pub fn rafraichir_bornes(&self) {
        let nouvelles =
            unsafe { eligibilite::bornes_fenetre(HWND(self.hwnd as *mut core::ffi::c_void)) };
        if let Some(b) = nouvelles {
            *self.bornes.lock().unwrap_or_else(PoisonError::into_inner) = b;
        }
    }

    /// Dernières bornes connues de la fenêtre.
    pub fn bornes(&self) -> RECT {
        *self.bornes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Retire la dernière image capturée (s'il y en a une).
    pub fn prendre_trame(&self) -> Option<DerniereTrame> {
        self.derniere
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Remet `trame` (prise par [`SessionCapture::prendre_trame`] pour
    /// l'analyser) comme dernière image, SAUF si une image plus récente est
    /// arrivée entre-temps ou si la session est fermée : `trame` est alors
    /// libérée (rendue au pool).
    pub fn remettre_trame(&self, trame: DerniereTrame) {
        let rejet = {
            let mut g = self.derniere.lock().unwrap_or_else(PoisonError::into_inner);
            if g.is_none() && !self.ferme.load(Ordering::SeqCst) {
                *g = Some(trame);
                None
            } else {
                Some(trame)
            }
        };
        drop(rejet);
    }
}

impl Drop for SessionCapture {
    fn drop(&mut self) {
        // Plus aucune image ne doit être stockée par le gestionnaire.
        self.ferme.store(true, Ordering::SeqCst);
        logging::info(&format!(
            "Capture WGC [{}] arrêtée sur {:#x} : {} image(s) reçue(s) en {} ms.",
            self.role,
            self.hwnd,
            self.trames_recues(),
            self.debut.elapsed().as_millis()
        ));
        // Toutes les erreurs sont ignorées : la fenêtre peut déjà être détruite.
        let _ = self.pool.RemoveFrameArrived(self.jeton);
        let _ = self.session.Close();
        let _ = self.pool.Close();
        // Libère l'image conservée tout de suite.
        drop(self.prendre_trame());
    }
}

// ---------------------------------------------------------------------------
// Fil de capture
// ---------------------------------------------------------------------------

/// État du fil de capture, lisible depuis le fil principal (surveillance).
pub struct EtatFil {
    base: Instant,
    /// Jeton de la demande en cours (0 = libre).
    jeton: AtomicU64,
    hwnd: AtomicIsize,
    etape: AtomicUsize,
    /// Début de l'étape courante (ms depuis `base`).
    etape_depuis_ms: AtomicU64,
    /// Début de la demande en cours (ms depuis `base`).
    demande_depuis_ms: AtomicU64,
    /// Demandes envoyées et non terminées (en cours comprise).
    en_file: AtomicUsize,
}

impl EtatFil {
    fn nouveau() -> Self {
        EtatFil {
            base: Instant::now(),
            jeton: AtomicU64::new(0),
            hwnd: AtomicIsize::new(0),
            etape: AtomicUsize::new(0),
            etape_depuis_ms: AtomicU64::new(0),
            demande_depuis_ms: AtomicU64::new(0),
            en_file: AtomicUsize::new(0),
        }
    }
    fn maintenant_ms(&self) -> u64 {
        self.base.elapsed().as_millis() as u64
    }
    fn commencer(&self, jeton: u64, hwnd: isize) {
        let t = self.maintenant_ms();
        self.hwnd.store(hwnd, Ordering::SeqCst);
        self.demande_depuis_ms.store(t, Ordering::SeqCst);
        self.etape_depuis_ms.store(t, Ordering::SeqCst);
        self.etape.store(0, Ordering::SeqCst);
        self.jeton.store(jeton, Ordering::SeqCst);
    }
    fn marquer_etape(&self, n: usize) {
        self.etape_depuis_ms.store(self.maintenant_ms(), Ordering::SeqCst);
        self.etape.store(n, Ordering::SeqCst);
    }
    fn terminer(&self) {
        self.jeton.store(0, Ordering::SeqCst);
        self.etape.store(0, Ordering::SeqCst);
        decrementer(&self.en_file);
    }
}

/// Décrémente sans passer sous zéro.
fn decrementer(compteur: &AtomicUsize) {
    let mut courant = compteur.load(Ordering::SeqCst);
    while courant > 0 {
        match compteur.compare_exchange_weak(courant, courant - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(v) => courant = v,
        }
    }
}

/// Création en cours sur le fil de capture (pour le journal).
#[derive(Debug, Clone)]
pub struct CreationEnCours {
    pub jeton: u64,
    pub hwnd: isize,
    pub etape: &'static str,
    pub depuis_ms: u64,
    pub etape_depuis_ms: u64,
    pub en_file: usize,
}

/// Où renvoyer le résultat d'une demande.
enum Reponse {
    /// Création synchrone : l'appelant attend (avec délai) sur ce canal.
    Attente(SyncSender<Result<SessionCapture, ErreurAnimation>>),
    /// Création asynchrone : résultat déposé dans [`RESULTATS`] puis
    /// `WM_SESSION_CREEE` posté à cette fenêtre.
    Message(isize),
}

struct Demande {
    jeton: u64,
    hwnd: isize,
    notifier: Option<(isize, u32)>,
    role: String,
    reponse: Reponse,
}

/// Résultats des créations asynchrones (jeton, résultat), relevés par le
/// fil principal à la réception de `WM_SESSION_CREEE`.
type Resultats = VecDeque<(u64, Result<SessionCapture, ErreurAnimation>)>;
static RESULTATS: Mutex<Resultats> = Mutex::new(VecDeque::new());

/// Refus du fil de capture.
pub enum RefusAtelier {
    /// Fil arrêté (lancement ou périphérique WinRT impossibles) : repli
    /// possible sur la création directe.
    Arrete,
    /// Fil occupé / bloqué, ou délai d'attente dépassé (texte pour le journal).
    Occupe(String),
}

/// Un fil de capture lancé (canal d'envoi + état partagé).
struct Fil {
    envoi: Sender<Demande>,
    etat: Arc<EtatFil>,
}

/// Lance un fil de capture sur le périphérique Direct3D `d3d`.
fn lancer_fil(d3d: &ID3D11Device) -> Option<Fil> {
    let (envoi, reception) = mpsc::channel::<Demande>();
    let etat = Arc::new(EtatFil::nouveau());
    let etat_fil = Arc::clone(&etat);
    let d3d = d3d.clone();
    match std::thread::Builder::new()
        .name("TW-capture-WGC".into())
        .spawn(move || fil_capture(d3d, reception, etat_fil))
    {
        Ok(_) => Some(Fil { envoi, etat }),
        Err(e) => {
            logging::erreur(&format!("Fil de capture WGC impossible à lancer : {e}."));
            None
        }
    }
}

/// Fil dédié à la création des sessions WGC. S'il reste bloqué plus de
/// [`SEUIL_BLOCAGE_MS`] dans une création, il est abandonné (il finira ou
/// non ; son résultat éventuel sera libéré) et un nouveau fil est lancé,
/// au plus [`MAX_RELANCES`] fois.
pub struct Atelier {
    d3d: ID3D11Device,
    fil: RefCell<Fil>,
    relances: Cell<u32>,
}

impl Atelier {
    /// Lance le fil de capture sur le périphérique Direct3D `d3d`.
    pub fn lancer(d3d: &ID3D11Device) -> Option<Atelier> {
        let fil = lancer_fil(d3d);
        if fil.is_none() {
            logging::erreur("Sessions WGC créées dans le fil principal (fil de capture absent).");
        }
        Some(Atelier {
            d3d: d3d.clone(),
            fil: RefCell::new(fil?),
            relances: Cell::new(0),
        })
    }

    fn etat(&self) -> Option<Arc<EtatFil>> {
        self.fil.try_borrow().ok().map(|f| Arc::clone(&f.etat))
    }

    /// Création en cours, s'il y en a une.
    pub fn creation_en_cours(&self) -> Option<CreationEnCours> {
        let etat = self.etat()?;
        let jeton = etat.jeton.load(Ordering::SeqCst);
        if jeton == 0 {
            return None;
        }
        let maintenant = etat.maintenant_ms();
        Some(CreationEnCours {
            jeton,
            hwnd: etat.hwnd.load(Ordering::SeqCst),
            etape: ETAPES
                .get(etat.etape.load(Ordering::SeqCst))
                .copied()
                .unwrap_or("?"),
            depuis_ms: maintenant.saturating_sub(etat.demande_depuis_ms.load(Ordering::SeqCst)),
            etape_depuis_ms: maintenant.saturating_sub(etat.etape_depuis_ms.load(Ordering::SeqCst)),
            en_file: etat.en_file.load(Ordering::SeqCst),
        })
    }

    /// Refus éventuel AVANT l'envoi : file pleine ou création bloquée
    /// (dans ce dernier cas, relance d'un nouveau fil si possible).
    fn verifier_disponible(&self) -> Result<(), RefusAtelier> {
        if let Some(c) = self.creation_en_cours() {
            if c.depuis_ms >= SEUIL_BLOCAGE_MS {
                let n = self.relances.get();
                if n < MAX_RELANCES {
                    if let Some(nouveau) = lancer_fil(&self.d3d) {
                        if let Ok(mut fil) = self.fil.try_borrow_mut() {
                            *fil = nouveau;
                            self.relances.set(n + 1);
                            logging::avertir(&format!(
                                "Fil de capture WGC bloqué sur {:#x} depuis {} ms à l'étape « {} » : \
                                 fil abandonné, nouveau fil lancé (relance {}/{MAX_RELANCES}).",
                                c.hwnd,
                                c.depuis_ms,
                                c.etape,
                                n + 1
                            ));
                            return Ok(());
                        }
                    }
                }
                return Err(RefusAtelier::Occupe(format!(
                    "création sur {:#x} bloquée depuis {} ms à l'étape « {} » (relances épuisées)",
                    c.hwnd, c.depuis_ms, c.etape
                )));
            }
        }
        let n = self.etat().map(|e| e.en_file.load(Ordering::SeqCst)).unwrap_or(0);
        if n >= MAX_EN_FILE {
            return Err(RefusAtelier::Occupe(format!("{n} demande(s) déjà en file")));
        }
        Ok(())
    }

    fn envoyer(&self, demande: Demande) -> Result<(), RefusAtelier> {
        let Ok(fil) = self.fil.try_borrow() else {
            return Err(RefusAtelier::Occupe("fil de capture momentanément inaccessible".into()));
        };
        fil.etat.en_file.fetch_add(1, Ordering::SeqCst);
        if fil.envoi.send(demande).is_err() {
            decrementer(&fil.etat.en_file);
            return Err(RefusAtelier::Arrete);
        }
        Ok(())
    }

    /// Demande asynchrone : le résultat arrivera par `WM_SESSION_CREEE`
    /// posté à `fenetre` (voir [`recuperer_resultats`]).
    pub fn demander(
        &self,
        jeton: u64,
        hwnd: isize,
        role: &str,
        fenetre: isize,
    ) -> Result<(), RefusAtelier> {
        self.verifier_disponible()?;
        self.envoyer(Demande {
            jeton,
            hwnd,
            notifier: None,
            role: role.to_string(),
            reponse: Reponse::Message(fenetre),
        })
    }

    /// Demande synchrone avec attente bornée à `delai_ms`. En cas de
    /// dépassement, la session éventuellement créée plus tard est
    /// libérée par le fil de capture lui-même.
    fn attendre(
        &self,
        hwnd: isize,
        notifier: Option<(isize, u32)>,
        role: &str,
        delai_ms: u64,
    ) -> Result<Result<SessionCapture, ErreurAnimation>, RefusAtelier> {
        self.verifier_disponible()?;
        let (envoi, reception) = mpsc::sync_channel(1);
        self.envoyer(Demande {
            jeton: 0,
            hwnd,
            notifier,
            role: role.to_string(),
            reponse: Reponse::Attente(envoi),
        })?;
        match reception.recv_timeout(Duration::from_millis(delai_ms)) {
            Ok(r) => Ok(r),
            Err(RecvTimeoutError::Timeout) => {
                let detail = self
                    .creation_en_cours()
                    .map(|c| format!(" (étape « {} » depuis {} ms)", c.etape, c.etape_depuis_ms))
                    .unwrap_or_default();
                Err(RefusAtelier::Occupe(format!(
                    "création sur {hwnd:#x} non terminée en {delai_ms} ms{detail}"
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(RefusAtelier::Arrete),
        }
    }
}

/// Démarre une session de façon SYNCHRONE (ouverture) : par le fil de
/// capture avec attente bornée, ou directement dans le fil appelant si le
/// fil de capture est indisponible.
pub fn demarrer_session(
    gpu: &ContexteGpu,
    hwnd: isize,
    notifier: Option<(isize, u32)>,
    role: &str,
) -> Result<SessionCapture, ErreurAnimation> {
    if let Some(atelier) = gpu.atelier.as_ref() {
        match atelier.attendre(hwnd, notifier, role, DELAI_CREATION_MS) {
            Ok(r) => return r,
            Err(RefusAtelier::Occupe(t)) => {
                logging::avertir(&format!("Capture WGC [{role}] sur {hwnd:#x} refusée : {t}."));
                return Err(ErreurAnimation::FilCapture(t));
            }
            Err(RefusAtelier::Arrete) => logging::avertir(
                "Fil de capture WGC arrêté : création dans le fil principal (repli).",
            ),
        }
    }
    SessionCapture::creer(&gpu.winrt, hwnd, notifier, role, None)
}

/// Variante STRICTEMENT bornée de [`demarrer_session`] (effet Jelly, au
/// milieu d'un glisser) : attente de `delai_ms` au plus par le fil de
/// capture, et JAMAIS de repli sur la création directe dans le fil
/// appelant (non bornée) — fil de capture absent ou arrêté : abandon
/// journalisé ([`ErreurAnimation::FilCapture`]). `demarrer_session`
/// (ouverture) est inchangée.
pub fn demarrer_session_bornee(
    gpu: &ContexteGpu,
    hwnd: isize,
    notifier: Option<(isize, u32)>,
    role: &str,
    delai_ms: u64,
) -> Result<SessionCapture, ErreurAnimation> {
    let texte = match gpu.atelier.as_ref() {
        Some(atelier) => match atelier.attendre(hwnd, notifier, role, delai_ms) {
            Ok(r) => return r,
            Err(RefusAtelier::Occupe(t)) => t,
            Err(RefusAtelier::Arrete) => {
                "fil de capture arrêté (pas de repli dans le fil principal)".to_string()
            }
        },
        None => "fil de capture absent (pas de repli dans le fil principal)".to_string(),
    };
    logging::avertir(&format!("Capture WGC [{role}] sur {hwnd:#x} abandonnée : {texte}."));
    Err(ErreurAnimation::FilCapture(texte))
}

/// Démarre une session directement dans le fil appelant (repli du suivi
/// quand le fil de capture est arrêté).
pub fn demarrer_directement(
    gpu: &ContexteGpu,
    hwnd: isize,
    role: &str,
) -> Result<SessionCapture, ErreurAnimation> {
    SessionCapture::creer(&gpu.winrt, hwnd, None, role, None)
}

/// Relève les résultats des créations asynchrones.
pub fn recuperer_resultats() -> Vec<(u64, Result<SessionCapture, ErreurAnimation>)> {
    RESULTATS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
        .collect()
}

/// Périphérique WinRT propre au fil de capture (même périphérique D3D 11 :
/// les textures produites sont utilisables par la surimpression).
fn creer_appareil_winrt(d3d: &ID3D11Device) -> windows::core::Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = d3d.cast()?;
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast() }
}

/// Boucle du fil de capture (appartement MTA, vit autant que le contexte
/// GPU ; jamais de `RoUninitialize` : les objets WGC créés ici doivent
/// rester utilisables). Ne panique jamais.
fn fil_capture(d3d: ID3D11Device, reception: Receiver<Demande>, etat: Arc<EtatFil>) {
    unsafe {
        match RoInitialize(RO_INIT_MULTITHREADED) {
            Ok(()) => {}
            Err(e) if e.code() == RPC_E_CHANGED_MODE => {}
            Err(e) => logging::avertir(&format!(
                "Fil de capture WGC : RoInitialize(MTA) a échoué ({:#010x}).",
                e.code().0 as u32
            )),
        }
    }
    let appareil = match creer_appareil_winrt(&d3d) {
        Ok(a) => a,
        Err(e) => {
            logging::erreur(&format!(
                "Fil de capture WGC : périphérique WinRT impossible ({:#010x} : {}) — \
                 création dans le fil principal.",
                e.code().0 as u32,
                e.message()
            ));
            return; // `reception` libéré : les envois échoueront -> repli.
        }
    };
    logging::info("Fil de capture WGC prêt (appartement MTA).");
    while let Ok(d) = reception.recv() {
        etat.commencer(d.jeton.max(1), d.hwnd);
        let resultat = SessionCapture::creer(&appareil, d.hwnd, d.notifier, &d.role, Some(&etat));
        etat.terminer();
        match d.reponse {
            Reponse::Attente(canal) => {
                if let Err(retour) = canal.send(resultat) {
                    logging::info(&format!(
                        "Capture WGC [{}] sur {:#x} terminée après le délai d'attente : abandonnée.",
                        d.role, d.hwnd
                    ));
                    drop(retour);
                }
            }
            Reponse::Message(fenetre) => {
                RESULTATS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back((d.jeton, resultat));
                let poste = unsafe {
                    PostMessageW(
                        Some(HWND(fenetre as *mut core::ffi::c_void)),
                        WM_SESSION_CREEE,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                if let Err(e) = poste {
                    logging::avertir(&format!(
                        "Fil de capture WGC : PostMessage(WM_SESSION_CREEE) a échoué ({:#010x}).",
                        e.code().0 as u32
                    ));
                }
            }
        }
    }
    logging::info("Fil de capture WGC arrêté.");
}

/// Garde-fou de contenu de l'attente d'ouverture (v0.4.6) : l'image
/// `trame` est-elle transparente, uniforme ou porteuse de contenu ?
/// Quelques blocs de pixels ([`attente::blocs_echantillons`], grille 5x5 de
/// blocs 16x4 au plus) sont copiés GPU -> GPU vers une petite texture
/// STAGING, lue une seule fois par le CPU (`Map(READ)`) ; la décision est
/// prise par la fonction pure [`attente::classer`]. Appelée seulement au
/// moment d'une décision (image stable ou plafond), jamais à chaque image.
/// `Err(texte)` si l'échantillonnage échoue (l'appelant traite alors
/// l'image comme porteuse de contenu).
pub unsafe fn analyser_trame(
    gpu: &ContexteGpu,
    trame: &DerniereTrame,
) -> Result<attente::ContenuImage, String> {
    let texte = |quoi: &str, e: windows::core::Error| {
        format!("{quoi} : {} (HRESULT {:#010x})", e.message(), e.code().0 as u32)
    };
    let surface = trame.trame.Surface().map_err(|e| texte("Surface WGC", e))?;
    let acces: IDirect3DDxgiInterfaceAccess =
        surface.cast().map_err(|e| texte("IDirect3DDxgiInterfaceAccess", e))?;
    let source: ID3D11Texture2D = acces.GetInterface().map_err(|e| texte("texture WGC", e))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    source.GetDesc(&mut desc);
    if desc.SampleDesc.Count != 1 {
        return Err(format!("texture multi-échantillonnée ({})", desc.SampleDesc.Count));
    }
    let format_bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
        || desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        || desc.Format == DXGI_FORMAT_B8G8R8A8_TYPELESS;
    if !format_bgra {
        return Err(format!("format de texture inattendu ({})", desc.Format.0));
    }
    let largeur = trame.taille.0.min(desc.Width);
    let hauteur = trame.taille.1.min(desc.Height);
    let blocs = attente::blocs_echantillons(largeur, hauteur);
    if blocs.is_empty() {
        return Err(format!("image sans pixel ({largeur}x{hauteur})"));
    }
    let lignes = attente::BLOC_HAUTEUR * blocs.len() as u32;
    let desc_lecture = D3D11_TEXTURE2D_DESC {
        Width: attente::BLOC_LARGEUR,
        Height: lignes,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut lecture: Option<ID3D11Texture2D> = None;
    gpu.d3d
        .CreateTexture2D(&desc_lecture, None, Some(&mut lecture))
        .map_err(|e| texte("CreateTexture2D (STAGING)", e))?;
    let lecture = lecture.ok_or_else(|| "CreateTexture2D (STAGING) sans texture".to_string())?;

    // Section critique du périphérique partagé (fil WGC, Jelly) autour de
    // la copie et de la lecture.
    let multifil = gpu.contexte.cast::<ID3D11Multithread>().ok();
    if let Some(m) = multifil.as_ref() {
        m.Enter();
    }
    let resultat = (|| -> Result<attente::ContenuImage, String> {
        for (i, b) in blocs.iter().enumerate() {
            let boite = D3D11_BOX {
                left: b.gauche,
                top: b.haut,
                front: 0,
                right: b.droite,
                bottom: b.bas,
                back: 1,
            };
            gpu.contexte.CopySubresourceRegion(
                &lecture,
                0,
                0,
                attente::BLOC_HAUTEUR * i as u32,
                0,
                &source,
                0,
                Some(&boite),
            );
        }
        let mut carte = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.contexte
            .Map(&lecture, 0, D3D11_MAP_READ, 0, Some(&mut carte))
            .map_err(|e| texte("Map(READ)", e))?;
        if carte.pData.is_null() {
            gpu.contexte.Unmap(&lecture, 0);
            return Err("Map(READ) sans pointeur".into());
        }
        let pas = carte.RowPitch as usize;
        let utile = attente::BLOC_LARGEUR as usize * 4;
        if pas < utile {
            gpu.contexte.Unmap(&lecture, 0);
            return Err(format!("pas de ligne incohérent ({pas})"));
        }
        // Taille lisible garantie : (lignes - 1) pas complets + 1 ligne utile.
        let longueur = pas * (lignes as usize - 1) + utile;
        let octets = std::slice::from_raw_parts(carte.pData as *const u8, longueur);
        let dimensions: Vec<(u32, u32)> =
            blocs.iter().map(|b| (b.largeur(), b.hauteur())).collect();
        let contenu = attente::classer(octets, pas, &dimensions);
        gpu.contexte.Unmap(&lecture, 0);
        contenu.ok_or_else(|| "données de la texture de lecture incohérentes".to_string())
    })();
    if let Some(m) = multifil.as_ref() {
        m.Leave();
    }
    resultat
}

/// Texte « (g, h)-(d, b) LxH » d'un rectangle, pour le journal.
pub fn texte_bornes(r: &RECT) -> String {
    format!(
        "({}, {})-({}, {}) {}x{}",
        r.left,
        r.top,
        r.right,
        r.bottom,
        r.right - r.left,
        r.bottom - r.top
    )
}

/// Données partagées entre la session (boucle de messages) et le
/// gestionnaire (fil WGC).
struct Partage {
    derniere: TramePartagee,
    bornes: Arc<Mutex<RECT>>,
    ferme: Arc<AtomicBool>,
    compteur: Arc<AtomicU64>,
    derniere_ms: Arc<AtomicU64>,
}

/// Crée le gestionnaire `FrameArrived`, exécuté sur un fil WGC.
///
/// Contraintes : `Send + 'static`, ne jamais paniquer (une panique dans un
/// rappel COM interrompt le processus), aucun accès à l'état global
/// `thread_local!` (autre fil) — uniquement les `Arc` partagés et le
/// journal (verrou statique, sûr depuis n'importe quel fil).
fn creer_gestionnaire(
    appareil: &IDirect3DDevice,
    hwnd: isize,
    taille_initiale: SizeInt32,
    partage: Partage,
    notifier: Option<(isize, u32)>,
    role: String,
    debut: Instant,
) -> Result<TypedEventHandler<Direct3D11CaptureFramePool, IInspectable>, ErreurAnimation> {
    let Partage {
        derniere,
        bornes: bornes_partagees,
        ferme,
        compteur,
        derniere_ms,
    } = partage;
    // Le périphérique WinRT doit pouvoir être utilisé depuis le fil WGC
    // (pour `Recreate`) : on passe par une référence agile.
    let appareil: AgileReference<IDirect3DDevice> = AgileReference::new(appareil)?;
    let taille_pool = Mutex::new(taille_initiale);
    let deja_notifie = AtomicBool::new(false);
    // Session sans notification = suivi de fermeture : ses images ne sont
    // pas conservées tant que la fenêtre est masquée par une de nos
    // animations (alpha 0 / alpha 1), pour ne jamais rejouer en fermeture
    // une image de la fenêtre quasi invisible. Lecture sans verrou
    // (`masquage::masquee_sans_verrou`) : aucun effet sur les autres
    // sessions ni sur les fenêtres non masquées.
    let suivi = notifier.is_none();

    Ok(TypedEventHandler::new(
        move |expediteur: Ref<Direct3D11CaptureFramePool>, _: Ref<IInspectable>| {
            let Some(pool) = expediteur.as_ref() else {
                return Ok(());
            };
            let Ok(trame) = pool.TryGetNextFrame() else {
                return Ok(());
            };
            if ferme.load(Ordering::SeqCst) {
                let _ = trame.Close();
                return Ok(());
            }
            let Ok(contenu) = trame.ContentSize() else {
                let _ = trame.Close();
                return Ok(());
            };
            if contenu.Width <= 0 || contenu.Height <= 0 {
                let _ = trame.Close();
                return Ok(());
            }

            // La fenêtre a changé de taille : on recrée le pool à la nouvelle
            // taille et on abandonne cette image (la suivante sera correcte).
            {
                let mut courante = taille_pool.lock().unwrap_or_else(PoisonError::into_inner);
                if contenu.Width != courante.Width || contenu.Height != courante.Height {
                    let _ = trame.Close();
                    if let Ok(dispositif) = appareil.resolve() {
                        if pool
                            .Recreate(
                                &dispositif,
                                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                                TAMPONS,
                                contenu,
                            )
                            .is_ok()
                        {
                            *courante = contenu;
                        }
                    }
                    return Ok(());
                }
            }

            // Suivi de fermeture : image ignorée (sans toucher à la
            // dernière image conservée) pendant un masquage temporaire.
            if suivi && super::masquage::masquee_sans_verrou(hwnd) {
                let _ = trame.Close();
                return Ok(());
            }

            let bornes = match unsafe {
                eligibilite::bornes_fenetre(HWND(hwnd as *mut core::ffi::c_void))
            } {
                Some(b) => {
                    *bornes_partagees.lock().unwrap_or_else(PoisonError::into_inner) = b;
                    b
                }
                None => *bornes_partagees.lock().unwrap_or_else(PoisonError::into_inner),
            };
            let ecoule = debut.elapsed().as_millis() as u64;
            let precedent = compteur.fetch_add(1, Ordering::Relaxed);
            let nouvelle = DerniereTrame {
                trame,
                bornes,
                taille: (contenu.Width as u32, contenu.Height as u32),
                numero: precedent.saturating_add(1),
            };
            if precedent == 0 {
                logging::info(&format!(
                    "Capture WGC [{role}] : 1re image de {hwnd:#x} après {ecoule} ms (contenu {}x{}, \
                     bornes {}).",
                    contenu.Width,
                    contenu.Height,
                    texte_bornes(&bornes)
                ));
            }
            derniere_ms.store(ecoule.saturating_add(1), Ordering::Relaxed);
            // L'ancienne image est libérée (Drop -> Close) hors du verrou.
            // Le drapeau est revérifié sous le verrou : `Drop` le positionne
            // avant sa dernière `prendre_trame`.
            let ancienne = {
                let mut g = derniere.lock().unwrap_or_else(PoisonError::into_inner);
                if ferme.load(Ordering::SeqCst) {
                    Some(nouvelle)
                } else {
                    g.replace(nouvelle)
                }
            };
            drop(ancienne);

            if let Some((fenetre, id)) = notifier {
                if !deja_notifie.swap(true, Ordering::SeqCst) {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(fenetre as *mut core::ffi::c_void)),
                            WM_TRAME_PRETE,
                            WPARAM(id as usize),
                            LPARAM(0),
                        );
                    }
                }
            }
            Ok(())
        },
    ))
}
