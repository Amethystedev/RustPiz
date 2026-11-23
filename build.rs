//! Script de build :
//!
//! 1. embarque l'icône et le manifeste Windows dans l'exécutable via
//!    `windres` (MinGW), afin que l'icône soit chargée par le chemin standard
//!    `LoadIconW` (systray) ET visible sur le fichier .exe dans l'Explorateur ;
//! 2. (v0.6.0) construit la DLL `rustpiz_runtime.dll` (crate
//!    `crates/rustpiz-runtime`, portée de RustHawk) par un cargo imbriqué et
//!    la copie dans `OUT_DIR/rustpiz_runtime.dll`, embarquée ensuite par
//!    `include_bytes!` (exécutable autonome : aucune DLL à livrer à côté).
//!
//! L'objet COFF produit par windres est passé directement à l'éditeur de
//! liens (`cargo:rustc-link-arg`) : contrairement à une archive statique,
//! un objet donné en ligne de commande est toujours inclus au lien, même
//! s'il ne contient que des ressources et aucun symbole.
//!
//! Cargo imbriqué : dossier cible dédié sous `OUT_DIR` (aucun conflit de
//! verrou avec le cargo appelant), `--offline --locked` (aucune
//! résolution, aucune écriture de Cargo.lock), variables d'environnement de
//! cargo/rustc de l'appelant retirées (sinon `RUSTFLAGS`, `CARGO_TARGET_DIR`,
//! `CARGO_BUILD_TARGET`… fuiraient). Aucune récursion : `rustpiz-runtime` ne
//! dépend pas de `rustpiz`, donc ce script n'est pas relancé ; une variable
//! sentinelle le garantit en plus. Les chemins locaux (racine du projet,
//! `CARGO_HOME`) sont remplacés dans la DLL par `--remap-path-prefix`.
//!
//! Chaîne d'outils de la DLL (v0.6.1) : la DLL de RustHawk, qui fonctionnait
//! sur le build 26300, était compilée pour `x86_64-pc-windows-msvc` (cargo-xwin,
//! CRT statique). Pour éliminer cette différence, la DLL est construite en
//! MSVC par `cargo xwin` quand c'est possible SANS téléchargement (cache
//! cargo-xwin déjà présent), sinon par repli en GNU/MinGW (avertissement
//! `cargo:warning`). `RUSTPIZ_RUNTIME_CHAINE_FORCEE=gnu|msvc` force un choix
//! (`msvc` forcé : échec bloquant ; toute autre valeur non vide : échec de
//! build explicite). La chaîne retenue est exposée à l'exécutable
//! (`RUSTPIZ_RUNTIME_CHAINE`) et écrite dans journal.log. L'exécutable
//! lui-même reste en GNU/MinGW (`.cargo/config.toml`).
//!
//! Construction MSVC : sortie d'erreur complète écrite dans
//! `OUT_DIR/dll_msvc_erreur.log` (citée, tronquée, dans l'avertissement de
//! repli), délai maximal de 20 min (sous-processus tué au-delà), lien
//! reproductible (`/Brepro`). Toute DLL produite (MSVC ou GNU) doit exporter
//! `DllGetClassObject` et `DllCanUnloadNow` (table d'exports PE relue).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Cible de l'exécutable et cible de repli de la DLL.
const CIBLE_GNU: &str = "x86_64-pc-windows-gnu";
/// Cible préférée de la DLL (identique à RustHawk).
const CIBLE_MSVC: &str = "x86_64-pc-windows-msvc";
/// Force la chaîne de la DLL : `gnu` ou `msvc` (sinon : MSVC puis repli GNU).
const VAR_CHAINE_FORCEE: &str = "RUSTPIZ_RUNTIME_CHAINE_FORCEE";

/// Délai maximal de la construction MSVC imbriquée (sous-processus tué au-delà).
const DELAI_MAX_MSVC: Duration = Duration::from_secs(20 * 60);
/// Journal complet de la sortie d'erreur de la construction MSVC.
const JOURNAL_MSVC: &str = "dll_msvc_erreur.log";
/// Exports exigés de la DLL (point d'entrée COM du TAP XAML).
const EXPORTS_REQUIS: [&str; 2] = ["DllGetClassObject", "DllCanUnloadNow"];

/// Variable posée pour le cargo imbriqué : si ce script s'exécutait quand même
/// dans ce contexte, il ne relancerait pas de construction (pas de récursion).
const SENTINELLE: &str = "RUSTPIZ_BUILD_IMBRIQUE";

/// Chaîne d'outils de la DLL.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chaine {
    Msvc,
    Gnu,
}

impl Chaine {
    fn cible(self) -> &'static str {
        match self {
            Chaine::Msvc => CIBLE_MSVC,
            Chaine::Gnu => CIBLE_GNU,
        }
    }
}

fn main() {
    // Ce projet ne vise que Windows (x86_64-pc-windows-gnu) : on ne tente
    // le lien de ressources que pour cette cible.
    let cible = std::env::var("TARGET").unwrap_or_default();
    if !cible.contains("windows") {
        println!("cargo:warning=Cible non-Windows ({cible}) : ressources ignorées.");
        return;
    }

    let dossier_sortie = std::env::var("OUT_DIR").expect("OUT_DIR indéfini");
    let objet = format!("{dossier_sortie}/ressources.o");

    let statut = Command::new("x86_64-w64-mingw32-windres")
        .args(["-i", "assets/ressources.rc", "-o", &objet])
        .status()
        .expect("x86_64-w64-mingw32-windres introuvable (paquet mingw-w64 requis)");
    assert!(
        statut.success(),
        "échec de windres sur assets/ressources.rc"
    );

    // Objet COFF passé tel quel à l'éditeur de liens : toujours inclus.
    println!("cargo:rustc-link-arg={objet}");

    // Recompiler si une ressource change.
    println!("cargo:rerun-if-changed=assets/ressources.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");

    construire_dll_runtime(Path::new(&dossier_sortie));
}

/// Construit `rustpiz_runtime.dll` en release (MSVC de préférence, repli
/// GNU) et la copie dans `OUT_DIR/rustpiz_runtime.dll`.
fn construire_dll_runtime(dossier_sortie: &Path) {
    println!("cargo:rerun-if-changed=crates");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.cargo/config.toml");
    println!("cargo:rerun-if-env-changed={SENTINELLE}");
    println!("cargo:rerun-if-env-changed={VAR_CHAINE_FORCEE}");
    println!("cargo:rerun-if-env-changed=XWIN_CACHE_DIR");
    println!("cargo:rerun-if-env-changed=HOME");
    println!("cargo:rerun-if-env-changed=XDG_CACHE_HOME");
    // Marqueur DONE du cache cargo-xwin : seulement s'il existe (un chemin
    // absent ferait relancer ce script à chaque construction).
    for done in marqueurs_cache_xwin() {
        if done.is_file() {
            println!("cargo:rerun-if-changed={}", done.display());
        }
    }

    let destination = dossier_sortie.join("rustpiz_runtime.dll");
    if std::env::var_os(SENTINELLE).is_some() {
        // Ne devrait jamais arriver (voir l'en-tête) ; on évite toute boucle.
        assert!(
            destination.is_file(),
            "build.rs relancé dans le cargo imbriqué sans DLL disponible"
        );
        println!("cargo:rustc-env=RUSTPIZ_RUNTIME_CHAINE=inconnue (build imbriqué)");
        return;
    }

    let debut = Instant::now();
    let forcee = std::env::var(VAR_CHAINE_FORCEE).unwrap_or_default();
    let forcee = forcee.trim().to_ascii_lowercase();
    let (octets, description) = match forcee.as_str() {
        "gnu" => (
            construire(Chaine::Gnu, dossier_sortie).unwrap_or_else(|e| panic!("{e}")),
            format!("{CIBLE_GNU} (GNU/MinGW, forcé par {VAR_CHAINE_FORCEE})"),
        ),
        "msvc" => (
            construire(Chaine::Msvc, dossier_sortie).unwrap_or_else(|e| panic!("{e}")),
            format!("{CIBLE_MSVC} (MSVC, CRT statique, forcé par {VAR_CHAINE_FORCEE})"),
        ),
        "" => match construire(Chaine::Msvc, dossier_sortie) {
            Ok(octets) => (
                octets,
                format!("{CIBLE_MSVC} (MSVC via cargo-xwin, CRT statique, comme RustHawk)"),
            ),
            Err(raison) => {
                println!(
                    "cargo:warning=rustpiz_runtime.dll : construction MSVC impossible ({}) ; \
                     repli sur {CIBLE_GNU} (GNU/MinGW).",
                    tronquer(&raison.replace(['\n', '\r'], " "), 600)
                );
                (
                    construire(Chaine::Gnu, dossier_sortie).unwrap_or_else(|e| panic!("{e}")),
                    format!("{CIBLE_GNU} (GNU/MinGW, repli : MSVC indisponible)"),
                )
            }
        },
        autre => panic!(
            "{VAR_CHAINE_FORCEE}={autre:?} : valeur inconnue (attendu : `gnu`, `msvc`, ou variable \
             absente/vide pour MSVC avec repli GNU)"
        ),
    };

    // Le profil release (`strip = "symbols"`) retire déjà les symboles :
    // un `strip` MinGW supplémentaire ne changeait rien (même taille).
    std::fs::write(&destination, &octets).unwrap_or_else(|e| {
        panic!(
            "copie de la DLL vers {} impossible : {e}",
            destination.display()
        )
    });
    let taille = std::fs::metadata(&destination)
        .map(|m| m.len())
        .unwrap_or(0);
    println!("cargo:rustc-env=RUSTPIZ_RUNTIME_CHAINE={description}");
    eprintln!(
        "rustpiz_runtime.dll : {taille} octets, {description} (construite/vérifiée en {:.1} s)",
        debut.elapsed().as_secs_f64()
    );
}

/// Marqueurs DONE possibles du cache cargo-xwin (SDK Windows et CRT MSVC
/// déjà téléchargés).
fn marqueurs_cache_xwin() -> Vec<PathBuf> {
    [
        std::env::var_os("XWIN_CACHE_DIR").map(PathBuf::from),
        std::env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("cargo-xwin")),
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("cargo-xwin")),
    ]
    .into_iter()
    .flatten()
    .flat_map(|d| [d.join("xwin").join("DONE"), d.join("DONE")])
    .collect()
}

fn cache_xwin_present() -> bool {
    marqueurs_cache_xwin().iter().any(|d| d.is_file())
}

/// Coupe `texte` à `max` caractères (avec « … »).
fn tronquer(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        texte.to_owned()
    } else {
        let mut court: String = texte.chars().take(max).collect();
        court.push('…');
        court
    }
}

/// Construit la DLL pour `chaine` ; renvoie l'image PE ou la raison d'échec.
fn construire(chaine: Chaine, dossier_sortie: &Path) -> Result<Vec<u8>, String> {
    let racine =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR indéfini"));
    let dossier_cible = dossier_sortie.join("runtime-target");
    // Binaire cargo de l'appelant (même chaîne d'outils) ; repli sur le PATH.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let hote_msvc = std::env::var("HOST").is_ok_and(|h| h.ends_with("windows-msvc"));

    let mut commande = Command::new(cargo);
    commande.current_dir(&racine);
    if chaine == Chaine::Msvc && !hote_msvc {
        // Depuis Linux : cargo-xwin, mais jamais de téléchargement depuis un
        // script de build (cache déjà présent exigé).
        if !cache_xwin_present() {
            return Err(
                "cache cargo-xwin absent (~/.cache/cargo-xwin/xwin/DONE) : aucun téléchargement \
                 depuis build.rs ; lancez une fois `cargo xwin build --target x86_64-pc-windows-msvc` \
                 pour le créer"
                    .to_owned(),
            );
        }
        commande.arg("xwin");
    }
    commande
        .args([
            "build",
            "--release",
            "-p",
            "rustpiz-runtime",
            "--target",
            chaine.cible(),
            "--offline",
            "--locked",
        ])
        .arg("--target-dir")
        .arg(&dossier_cible);

    // Nettoyage de l'environnement hérité : tout ce que cargo pose pour un
    // script de build ou qui modifierait la compilation imbriquée.
    for (cle, _) in std::env::vars_os() {
        let Some(cle) = cle.to_str() else { continue };
        let a_retirer = (cle.starts_with("CARGO_") && cle != "CARGO_HOME")
            || cle.starts_with("DEP_")
            || cle.starts_with("RUSTC_")
            || matches!(
                cle,
                "CARGO"
                    | "RUSTC"
                    | "RUSTDOC"
                    | "RUSTFLAGS"
                    | "RUSTDOCFLAGS"
                    | "TARGET"
                    | "HOST"
                    | "OUT_DIR"
                    | "PROFILE"
                    | "OPT_LEVEL"
                    | "DEBUG"
                    | "NUM_JOBS"
                    | "__CARGO_FIX_PLZ"
            );
        if a_retirer {
            commande.env_remove(cle);
        }
    }
    commande.env(SENTINELLE, "1");

    // Chemins de la machine de construction retirés de la DLL (messages de
    // panique, informations de débogage) : `--remap-path-prefix`, passé par
    // `CARGO_ENCODED_RUSTFLAGS` (séparateur 0x1F, chemins avec espaces sûrs ;
    // cargo-xwin y ajoute ses propres options).
    let mut remplacements = vec![(racine.clone(), "rustpiz")];
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(cargo_home) = cargo_home {
        remplacements.push((cargo_home, "cargo"));
    }
    let mut drapeaux: Vec<String> = remplacements
        .iter()
        .filter_map(|(chemin, nom)| {
            chemin
                .to_str()
                .map(|c| format!("--remap-path-prefix={c}={nom}"))
        })
        .collect();
    if chaine == Chaine::Msvc {
        // Comme RustHawk/.cargo/config.toml : CRT statique (aucun
        // redistribuable Visual C++ requis dans explorer). LNK4099 : PDB des
        // bibliothèques MSVC absents, sans conséquence.
        drapeaux.push("-Ctarget-feature=+crt-static".to_owned());
        drapeaux.push("-Clink-arg=/IGNORE:4099".to_owned());
        // Lien reproductible : horodatage PE dérivé du contenu.
        drapeaux.push("-Clink-arg=/Brepro".to_owned());
    }
    commande.env("CARGO_ENCODED_RUSTFLAGS", drapeaux.join("\u{1f}"));

    let ligne = format!(
        "`cargo {}build --release -p rustpiz-runtime --target {} --offline --locked`",
        if chaine == Chaine::Msvc && !hote_msvc { "xwin " } else { "" },
        chaine.cible()
    );
    let statut = if chaine == Chaine::Msvc {
        executer_msvc(&mut commande, dossier_sortie, &ligne)?
    } else {
        commande.status().map_err(|e| {
            format!(
                "impossible de lancer cargo ({e}) pour construire rustpiz_runtime.dll \
                 (cargo imbriqué, voir l'en-tête de build.rs)"
            )
        })?
    };
    if !statut.success() {
        return Err(format!(
            "échec de la construction de rustpiz_runtime.dll par le cargo imbriqué \
             ({ligne}, dossier {}, {statut}).\n\
             Lisez les erreurs de compilation ci-dessus. Si une dépendance est absente du \
             cache local (mode --offline), lancez d'abord `cargo fetch --locked` à la racine \
             du projet, puis relancez la construction ; si Cargo.lock doit changer, mettez-le \
             à jour explicitement (`cargo update -p <crate>`) car --locked interdit toute \
             modification.",
            dossier_cible.display()
        ));
    }

    let produite = dossier_cible
        .join(chaine.cible())
        .join("release")
        .join("rustpiz_runtime.dll");
    let octets = std::fs::read(&produite)
        .map_err(|e| format!("DLL introuvable après construction ({}) : {e}", produite.display()))?;
    if !(octets.len() > 4096 && octets.starts_with(b"MZ")) {
        return Err(format!("{} n'est pas une image PE valide", produite.display()));
    }
    let exports = exports_pe(&octets)
        .map_err(|e| format!("{} : table d'exports illisible ({e})", produite.display()))?;
    let manquants: Vec<&str> = EXPORTS_REQUIS
        .iter()
        .copied()
        .filter(|requis| !exports.iter().any(|e| e == requis))
        .collect();
    if !manquants.is_empty() {
        return Err(format!(
            "{} n'exporte pas {} (exports trouvés : {})",
            produite.display(),
            manquants.join(", "),
            tronquer(&exports.join(", "), 300)
        ));
    }
    Ok(octets)
}

/// Lance la construction MSVC : sortie d'erreur COMPLÈTE dans
/// `OUT_DIR/dll_msvc_erreur.log` (fichier, donc aucun tube à vider), délai
/// maximal [`DELAI_MAX_MSVC`] (sous-processus tué au-delà). En cas d'échec,
/// l'erreur cite la fin du journal (tronquée) et son chemin.
fn executer_msvc(
    commande: &mut Command,
    dossier_sortie: &Path,
    ligne: &str,
) -> Result<std::process::ExitStatus, String> {
    let journal = dossier_sortie.join(JOURNAL_MSVC);
    let fichier = std::fs::File::create(&journal)
        .map_err(|e| format!("{} : création impossible ({e})", journal.display()))?;
    let mut enfant = commande
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fichier))
        .spawn()
        .map_err(|e| format!("impossible de lancer {ligne} : {e}"))?;
    let debut = Instant::now();
    let statut = loop {
        match enfant.try_wait() {
            Ok(Some(statut)) => break statut,
            Ok(None) if debut.elapsed() >= DELAI_MAX_MSVC => {
                let _ = enfant.kill();
                let _ = enfant.wait();
                return Err(format!(
                    "{ligne} : délai maximal de {} min dépassé, construction interrompue \
                     (journal complet : {})",
                    DELAI_MAX_MSVC.as_secs() / 60,
                    journal.display()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => {
                let _ = enfant.kill();
                let _ = enfant.wait();
                return Err(format!("{ligne} : attente impossible ({e})"));
            }
        }
    };
    if statut.success() {
        // Rien à garder : le journal n'est conservé qu'en cas d'échec.
        let _ = std::fs::remove_file(&journal);
        return Ok(statut);
    }
    let erreurs = std::fs::read(&journal)
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .unwrap_or_default();
    let fin: Vec<&str> = erreurs
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(3)
        .collect();
    let fin: Vec<&str> = fin.into_iter().rev().collect();
    Err(format!(
        "{ligne} : {statut} — {} (journal complet : {})",
        tronquer(&fin.join(" | "), 400),
        journal.display()
    ))
}

/// Noms exportés d'une image PE32+ (lecture minimale, bornée et sans
/// panique : toute incohérence est une erreur).
fn exports_pe(image: &[u8]) -> Result<Vec<String>, String> {
    let u16_a = |o: usize| -> Result<u16, String> {
        image
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| format!("lecture hors de l'image à {o:#x}"))
    };
    let u32_a = |o: usize| -> Result<u32, String> {
        image
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("lecture hors de l'image à {o:#x}"))
    };
    let pe = u32_a(0x3C)? as usize;
    if image.get(pe..pe + 4) != Some(b"PE\0\0".as_slice()) {
        return Err("signature PE absente".to_owned());
    }
    let sections = u16_a(pe + 6)? as usize;
    let taille_optionnel = u16_a(pe + 20)? as usize;
    let optionnel = pe + 24;
    if u16_a(optionnel)? != 0x20B {
        return Err("image non PE32+".to_owned());
    }
    if u32_a(optionnel + 108)? < 1 {
        return Err("aucun répertoire de données".to_owned());
    }
    let rva_exports = u32_a(optionnel + 112)? as usize;
    if rva_exports == 0 {
        return Ok(Vec::new());
    }
    let table = optionnel + taille_optionnel;
    let vers_fichier = |rva: usize| -> Result<usize, String> {
        for i in 0..sections {
            let s = table + i * 40;
            let taille_virtuelle = u32_a(s + 8)? as usize;
            let adresse = u32_a(s + 12)? as usize;
            let taille_brute = u32_a(s + 16)? as usize;
            let brut = u32_a(s + 20)? as usize;
            let etendue = taille_virtuelle.max(taille_brute);
            if rva >= adresse && rva < adresse + etendue {
                return Ok(brut + (rva - adresse));
            }
        }
        Err(format!("RVA {rva:#x} hors des sections"))
    };
    let repertoire = vers_fichier(rva_exports)?;
    let nombre = u32_a(repertoire + 24)? as usize;
    if nombre > 65_536 {
        return Err(format!("{nombre} noms exportés : incohérent"));
    }
    let noms = vers_fichier(u32_a(repertoire + 32)? as usize)?;
    let mut resultat = Vec::with_capacity(nombre);
    for i in 0..nombre {
        let debut = vers_fichier(u32_a(noms + i * 4)? as usize)?;
        let reste = image.get(debut..).ok_or("nom exporté hors de l'image")?;
        let fin = reste
            .iter()
            .take(512)
            .position(|&o| o == 0)
            .ok_or("nom exporté non terminé")?;
        resultat.push(String::from_utf8_lossy(&reste[..fin]).into_owned());
    }
    Ok(resultat)
}
