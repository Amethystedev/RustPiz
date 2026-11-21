//! Script de build : embarque l'icône et le manifeste Windows dans
//! l'exécutable via `windres` (MinGW), afin que l'icône soit chargée par
//! le chemin standard `LoadIconW` (systray) ET visible sur le fichier
//! .exe dans l'Explorateur.
//!
//! L'objet COFF produit par windres est passé directement à l'éditeur de
//! liens (`cargo:rustc-link-arg`) : contrairement à une archive statique,
//! un objet donné en ligne de commande est toujours inclus au lien, même
//! s'il ne contient que des ressources et aucun symbole.

use std::process::Command;

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
    assert!(statut.success(), "échec de windres sur assets/ressources.rc");

    // Objet COFF passé tel quel à l'éditeur de liens : toujours inclus.
    println!("cargo:rustc-link-arg={objet}");

    // Recompiler si une ressource change.
    println!("cargo:rerun-if-changed=assets/ressources.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
}
