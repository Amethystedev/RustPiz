# Avis de licence : crates portées de RustHawk (GPL-3.0-only)

Les crates de ce dossier (`rustpiz-theme`, `rustpiz-thumbnails`,
`rustpiz-xaml`, `rustpiz-protocol`, `rustpiz-runtime`) et les thèmes du
dossier `themes/` à la racine du dépôt sont **portés de RustHawk**
(`rusthawk-*`), lui-même dérivé des mods Windhawk **Windows 11 Taskbar
Styler** et **Taskbar Thumbnail Size** (Ramen Software / m417z et
contributeurs), publiés sous **GPL v3**. Les thèmes proviennent du source GPL
du Styler et du *Windows 11 Taskbar Styling Guide* (thème Surface : Jimmy,
JimmyLlancaMelo).

Ces crates et ces thèmes restent sous **GPL-3.0-only** (métadonnée `license`
de chaque `Cargo.toml`). Modifications apportées lors du portage :
renommage `rusthawk` -> `rustpiz` (crates, modules, tube nommé
`\\.\pipe\rustpiz.<pid>`, message `RustPiz_RunOnUiThread`, dossier
`%LOCALAPPDATA%\RustPiz`), CLSID COM propre à RustPiz, intégration à
l'espace de travail Cargo de RustPiz. L'outil `gen-bindings` n'est pas porté
(`rustpiz-runtime/src/bindings.rs` est le fichier généré, repris tel quel).

Le paquet racine `rustpiz` est lui aussi sous licence GPL-3.0-only (décision de
l'utilisateur, v0.6.0 ; auparavant MIT). L'exécutable `rustpiz.exe` embarque
la DLL `rustpiz_runtime.dll` et les thèmes GPL-3 ; sa distribution respecte
donc la GPL v3 (texte complet :
<https://www.gnu.org/licenses/gpl-3.0.html>, et `RustHawk/LICENSE`).
