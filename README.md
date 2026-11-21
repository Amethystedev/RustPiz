# RustPiz

**RustPiz** (contraction de *Rust* et *Compiz*) est une application Windows native
en **Rust** qui habille les fenêtres de Windows 11 d'effets modernes : transparence,
effets DWM (Mica, Acrylique, Verre), animations d'ouverture/fermeture et effet
**Jelly** (fenêtres gélatineuses au déplacement), le tout rendu par le GPU.
Successeur de l'ancien projet « Transparent Windows » (inspiré du vieil outil
« System Transparency »).

- Pilotage depuis l'**icône de la zone de notification**, des **raccourcis clavier
  globaux** et une **fenêtre de paramètres native**.
- Aucune dépendance à .NET, exécutable autonome (~1 Mo).
- Conçu pour Windows 11 (build ≥ 22000, vérifié avec 26H2), x64.

## Installation

1. Copiez `rustpiz.exe` où vous voulez (ex. `C:\Outils\`).
2. Double-cliquez : une **notification** confirme le démarrage et une icône
   apparaît dans la zone de notification (éventuellement sous le chevron « ^ »).
3. Au premier lancement, la configuration est créée dans
   `%APPDATA%\RustPiz\config.toml`.

Pour un démarrage automatique : placez un raccourci vers l'exécutable dans
`shell:startup` (`Win+R` → `shell:startup`).

> ⚠️ Au premier lancement, **SmartScreen** peut afficher un avertissement
> « éditeur inconnu » (binaire non signé) : « Informations complémentaires » →
> « Exécuter quand même ».

## Compilation (pour les développeurs)

```bash
cargo check
cargo build --release
```

La cible par défaut est `x86_64-pc-windows-gnu` (voir `.cargo/config.toml`).
L'exécutable est généré dans
`target/x86_64-pc-windows-gnu/release/rustpiz.exe`.

## Utilisation

### Transparence automatique de l'Explorateur (activée par défaut)

- **Au démarrage** de RustPiz, toutes les fenêtres de l'Explorateur
  déjà ouvertes sont rendues transparentes avec l'opacité de base.
- **Chaque nouvelle fenêtre** de l'Explorateur ouverte ensuite est détectée
  (scan toutes les 2 secondes) et rendue transparente automatiquement.
- Vous pouvez ensuite **personnaliser chaque fenêtre** individuellement :
  `Ctrl+Alt+T` pour basculer, `Ctrl+Alt+Haut/Bas` pour ajuster l'opacité, et
  le menu **Effet DWM** pour Mica/Acrylique/Verre.
- Ce comportement se désactive via la case « Explorateur automatiquement
  transparent » dans les **Paramètres**, ou `transparence_explorateur = false`
  dans `config.toml`.

### Raccourcis clavier (par défaut)

Les raccourcis agissent sur la **fenêtre située sous le curseur de la souris** :

| Raccourci        | Action                                             |
| ---------------- | -------------------------------------------------- |
| `Ctrl+Alt+T`     | Activer / désactiver la transparence de la fenêtre |
| `Ctrl+Alt+Haut`  | Augmenter l'opacité (+10 %)                        |
| `Ctrl+Alt+Bas`   | Diminuer l'opacité (−10 %)                         |

### Menu de l'icône (clic droit)

- **Transparence activée** : interrupteur général (double-clic sur l'icône fait
  la même chose). La désactivation restaure toutes les fenêtres.
- **Opacité globale** : opacité appliquée par défaut (50 à 100 %).
- **Effet DWM (fenêtre sous le curseur)** : Aucun / Mica / Mica Alt / Acrylique /
  Verre. Après le choix, vous avez **3 secondes** pour placer le curseur sur la
  fenêtre cible.
- **Identifier la fenêtre sous le curseur…** : après 3 secondes, affiche le
  processus, la classe et le titre de la fenêtre visée, avec un exemple de
  profil prêt à copier dans `config.toml`.
- **Restaurer toutes les fenêtres** : remet l'opacité et les effets d'origine.
- **Paramètres…** : ouvre la fenêtre de paramètres native.
- **Recharger la configuration** : applique les modifications du fichier sans
  redémarrer.
- **Animation des fenêtres** : choix de l'effet, cases « Animer l'ouverture » et
  « Animer la fermeture » (voir plus bas).
- **Effet Jelly (déplacement des fenêtres)** : case à cocher d'activation, et
  sous-menu **Puissance du Jelly** (0 / 25 / 50 / 75 / 100 %, 0 = aucun mouvement).
- **Quitter** : restaure toutes les fenêtres puis ferme l'application.

### Fenêtre de paramètres

Opacité par défaut (curseur 10-100 %), activation au démarrage, et les trois
raccourcis clavier. **Enregistrer** met à jour `config.toml` et applique
immédiatement (les raccourcis sont réenregistrés à chaud).

## Effets DWM (Windows 11 22H2 et +)

| Effet        | Description                                              |
| ------------ | -------------------------------------------------------- |
| **Mica**     | Matière opaque teintée du fond d'écran                   |
| **Mica Alt** | Variante Mica plus prononcée (fenêtres à onglets)        |
| **Acrylique**| Flou translucide                                         |
| **Verre**    | Feuille de verre complète (`DwmExtendFrameIntoClientArea`) |

Les effets sont restaurés à leur état d'origine par « Restaurer toutes les
fenêtres » et à la fermeture de l'application.

## Effets d'ouverture/fermeture

Les fenêtres des autres applications peuvent être animées à l'ouverture et à
la fermeture.

- **Effets disponibles** : Aucun, Fondu, Zoom, Glissement, Rétrécissement,
  Télévision, Flou.
- **Menu** : clic droit sur l'icône, sous-menu « Animation des fenêtres »
  (choix de l'effet), puis « Animer l'ouverture des fenêtres » et « Animer la
  fermeture des fenêtres » (cases à cocher). Ces animations sont désactivées
  par défaut.
- **Clés de configuration** (racine de `config.toml`) : `effet_fenetre`
  (`"aucun"`, `"fondu"`, `"zoom"`, etc.), `effet_ouverture` et
  `effet_fermeture` (`true` / `false`).
- **Principe (GPU)** : le contenu de la fenêtre est capturé par
  Windows.Graphics.Capture (textures Direct3D 11 laissées sur le GPU), puis
  rejoué dans une fenêtre de surimpression DirectComposition dont les
  propriétés (opacité, échelle, décalage, flou) sont animées par le
  compositeur Windows. Aucune injection dans les autres processus.
- **Limites** :
  - la fermeture animée fonctionne surtout pour la fenêtre au premier plan
    (seule celle-ci est suivie en continu) ;
  - l'ouverture masque temporairement la fenêtre (transparence quasi totale)
    et attend qu'elle soit réellement peinte avant de jouer l'effet (v0.4.6) :
    la fenêtre apparaît donc avec un léger retard (environ 0,1 à 0,4 s, 0,75 s
    au plus) ; si aucune image exploitable n'arrive à temps, elle s'affiche
    sans animation ; si Windows refuse le masquage, l'animation d'ouverture
    est désactivée automatiquement pour la session ;
  - si le processus est tué pendant une animation d'ouverture, la fenêtre
    concernée pourrait rester masquée : la fermer puis la rouvrir ;
  - les effets de feu et de verre brisé ne sont pas inclus.

> Le dossier de configuration est `%APPDATA%\RustPiz`. Un ancien dossier
> `%APPDATA%\TransparentWindows` est migré automatiquement au premier lancement.

## Effet Jelly (v0.4)

Quand on **déplace une fenêtre** en glissant sa barre de titre, elle se déforme
élastiquement (traînée proportionnelle à la vitesse) puis revient à sa forme avec
un rebond amorti, façon [Jello](https://github.com/iamDecode/Jello) sur macOS.

- **Menu** : case « Effet Jelly (déplacement des fenêtres) » et sous-menu
  « Puissance du Jelly » (0 à 100 %). Désactivé par défaut ; désactivé ou à 0 %,
  il ne consomme rien.
- **Clés de configuration** : `jelly_actif` (`true` / `false`) et
  `jelly_puissance` (0 à 100, défaut 50).
- **Principe (100 % GPU)** : la fenêtre est capturée (Windows.Graphics.Capture), sa
  texture reste sur le GPU et est dessinée par Direct3D 11 sur un **vrai maillage
  courbe** simulé par un système masses-ressorts, puis présentée via une swapchain
  de composition DirectComposition, synchronisée sur le rafraîchissement de l'écran.
  Aucune injection dans les autres processus.
- **Limites** : le contenu de la fenêtre est figé pendant le glissement, sans ombre
  DWM ; un Snap, un redimensionnement ou un changement de DPI terminent l'effet
  sans rebond ; si le GPU matériel est indisponible ou le pipeline échoue, le Jelly
  est désactivé pour la session (notification + `journal.log`) ; si le processus est
  tué pendant l'effet, la fenêtre peut rester quasi invisible : la fermer/rouvrir.

## Configuration (`%APPDATA%\RustPiz\config.toml`)

```toml
opacite_defaut = 80            # opacité en % (10 = très transparent, 100 = opaque)
activer_au_demarrage = true
transparence_explorateur = true # Explorateur transparent automatiquement
effet_fenetre = "zoom"         # animation d'ouverture/fermeture
effet_ouverture = false
effet_fermeture = false
jelly_actif = false            # effet Jelly au déplacement des fenêtres
jelly_puissance = 50           # 0 à 100

[raccourcis]
bascule = "Ctrl+Alt+T"         # format : Ctrl/Alt/Shift/Win + touche
augmenter = "Ctrl+Alt+Haut"
diminuer = "Ctrl+Alt+Bas"

exclusions_classes = [ ... ]   # classes de fenêtres jamais modifiées
exclusions_processus = [ ... ] # processus jamais modifiés (ex. "monjeu.exe")

# Profils automatiques (appliqués aux nouvelles fenêtres, scan toutes les 2 s) :
[[profils]]
nom = "Explorateur Mica"
processus = "explorer.exe"     # critères : processus, classe, titre_contient
classe = "CabinetWClass"
effet_dwm = "mica"             # aucun | mica | micaalt | acrylique | verre

[[profils]]
nom = "Bloc-notes translucide"
processus = "notepad.exe"
opacite = 85
```

Après modification : **Recharger la configuration** dans le menu, ou redémarrer.

### Protections intégrées

Par défaut, l'application **ignore** : la barre des tâches, le menu Démarrer, le
centre de notifications, les menus contextuels du shell, le bureau, l'écran de
verrouillage et **toute fenêtre plein écran** (cas de la plupart des jeux).

## Journalisation

Les événements et erreurs sont consignés dans
`%APPDATA%\RustPiz\journal.log` (y compris la version de Windows
détectée). En cas de comportement inattendu, c'est le premier fichier à consulter.

## Limitations connues

- Certaines applications modernes (WinUI 3, Electron avec accélération GPU)
  peuvent afficher des artefacts avec `WS_EX_LAYERED` ou ignorer les effets DWM.
- L'effet « Verre » sur une fenêtre qui gère déjà ses marges DWM peut produire
  des rendus inattendus ; « Aucun » restaure l'état d'origine.
- Si le processus est tué brutalement (gestionnaire des tâches), les fenêtres
  modifiées ne peuvent pas être restaurées automatiquement ; fermer puis rouvrir
  la fenêtre concernée suffit. La fermeture de session Windows, elle, est gérée.

## Licence

MIT.
