# Handoff

Everything needed to pick this project up on another machine: what exists, how it was built, what has and has not been verified, and what to do next. The README is for users and contributors. This file is the build's memory.

Last updated 2026-11-10, at version 0.2.0.

## The one thing to know first

**Nothing here has run on SteamOS, Bazzite, or any Linux machine.** All work was done on macOS (Apple Silicon). The Rust engine is tested by unit tests and by driving the CLI against a pretend Steam library. The UI was run in Godot on macOS with scripted input and screenshots. The Linux release build has never been produced, and the GitHub workflows have never run. The first job on a Linux machine is the checklist under "Verify on a device".

## What this is

A controller-first mod manager for SteamOS and Bazzite, in the spirit of Nexus Mods' Vortex. It is an independent project, not a fork: no upstream code is linked or copied. The upstream Vortex source is used as a behaviour reference and as the input to a script that reads game facts out of its extensions.

Goals the owner set, in order: a user-friendly open source app to manage game mods, download from Nexus Mods and sign in with a Nexus account, catch "Mod Manager Download" links the way Vortex does, run natively on SteamOS and Bazzite, and look and navigate like Steam Big Picture with a gamepad.

## Set up on a new machine

```sh
git clone <this repo> vortex-steamos
cd vortex-steamos
cargo test                      # 67 tests
cargo build -p vortex-cli
godot --path ui                 # Godot 4.3+; built and tested with 4.7.2
```

Toolchain used: Rust 1.99.1, Godot 4.7.2, Node 20.11 (only for the importer).

To run the whole app with no Steam, no game, and no network, on macOS or Linux:

```sh
eval "$(scripts/dev-sandbox.sh)"
cargo build -p vortex-cli
godot --path ui
godot --path ui -- 'nxm://skyrimspecialedition/mods/667/files/1234?key=abc&expires=1'
godot --headless --path ui --script ../scripts/ui-smoke.gd      # scripted run through the UI
```

Do not run `cargo test` in a shell where the sandbox variables are exported. `VORTEX_NEXUS_KEY` being set makes `nexus::tests::env_key_overrides_the_file` fail. Use a fresh shell, or `env -u VORTEX_NEXUS_KEY cargo test`.

### The upstream Vortex checkout

Only the importer needs it. It expects the checkout next to this repository:

```sh
git clone https://github.com/Nexus-Mods/Vortex ../Vortex
git -C ../Vortex checkout c892135a438280d0a9a6fda308f10942ec2eff73   # the commit the game data was read from (2026-11-06)
npm install esbuild            # anywhere; point NODE_PATH at it. Lets the importer read TypeScript extensions
NODE_PATH=/path/to/node_modules node scripts/import-vortex-games.cjs --facts facts.json
```

On the original machine the layout was `vortex_steamos/app` (this repository) and `vortex_steamos/Vortex` (upstream).

## Architecture

```
crates/vortex-core     the engine, a Rust library
  games.rs             game definitions, loaded from games/*.json. Rule and When types
  games/core.json      games with code behind them (Bethesda, Witcher 3, Cyberpunk, Stardew)
  games/ported.json    games ported from Vortex extensions into install rules
  games/generated.json written by the importer. Games that only need a mod folder
  games/cases.json     example archives per game: files in, placements out. Run by cargo test
  steam.rs             find Steam libraries and installed games, cover art, Linux builds
  proton.rs            paths inside a game's Proton prefix, matched without regard to case
  install.rs           unpack archives (zip, 7z, rar via system tools), FOMOD, stage a mod
  layout.rs            where each file of an archive goes: rule engine plus per-game functions
  fomod.rs             FOMOD installer: steps, groups, flags, conditions
  deploy.rs            hardlink staged mods into game folders, conflicts, backups, purge
  plugins.rs           plugins.txt and loadorder.txt for Bethesda games
  nexus.rs             Nexus Mods API: search, files, downloads, nxm:// links, API key storage
crates/vortex-cli      every engine action as a command that prints JSON
ui/                    Godot 4 shell. scripts/shell.gd draws everything, scripts/core_client.gd
                       runs vortex-cli on a worker thread
packaging/             launcher script, desktop entry, install.sh, uninstall.sh
scripts/               build-release.sh, dev-sandbox.sh, import-vortex-games.cjs, ui-smoke.gd
docs/workflows/        ci.yml and release.yml, parked. See the README there for how to turn them on
docs/porting/          the brief the game ports were written from, and the per-game report
```

The UI never touches Steam, the game folder, or Nexus itself. It calls `vortex-cli` for everything and renders the JSON. That keeps the engine testable without a GUI and lets the UI be rewritten freely, which has happened twice.

### On-disk state

- `<steam library>/steamapps/vortex/<game id>/` is a game's staging folder, on the same drive as the game so hardlinks work.
  - `mods/<mod id>/` holds a mod's files. Files for a named target sit in `mods/<mod id>/.targets/<target>/`.
  - `mods.json` is the mod list in priority order. `deployment.json` records every link placed, so they can be removed.
  - `incoming/` receives downloads. `archives/` keeps installed archives. `.extract/` is unpacking scratch space.
- `~/.config/vortex-steamos/nexus.key` is the Nexus API key, mode 0600.
- `~/.local/state/vortex-steamos/links/*.nxm` is the queue of download links. `ui.pid` marks a running window.
- Installed app: `~/.local/share/vortex-steamos/`, desktop entry `vortex-steamos.desktop`.

## Decisions and why

- **Rust engine behind a CLI, Godot shell in front.** Godot gives a controller-native UI that runs in gamescope. Rust gives a static binary that runs on both SteamOS (Arch) and Bazzite (Fedora). This split predates the work recorded here.
- **Hardlinks, staging on the game's drive.** Mods cost no extra space and a Deck's SD card library stays self-contained. A different drive falls back to symlinks.
- **Deploy is case-insensitive.** A mod's `textures/` merges into the game's `Textures/`. Windows games under Proton see only one of two folders that differ by case. This is the most common way Linux modding silently fails.
- **Link ownership is recorded by inode** in `deployment.json`, so links are still recognised after a mod's staged files are replaced or deleted.
- **Archives unpack under the staging folder, not `/tmp`.** `/tmp` is RAM on SteamOS.
- **Games are data.** Adding a game should not need Rust. The rule language (`marker`, `anchor`, `flat`, `as_is`, `reject`, with `when`) covers most Vortex installers. A game that needs more gets a function in `layout.rs`.
- **Bethesda games never have their own files replaced. Other games can**, with the original kept as `<name>.vortex_backup` and restored when the mod is turned off. Many non-Bethesda mods only work by replacing shipped files.
- **Sign-in is a personal API key.** Nexus single sign-on and OAuth need an application id issued by Nexus Mods. Reusing Vortex's id would impersonate their app, so it was not done. Registering this app with Nexus is an open decision for the owner.
- **`nxm://` links go through a queue folder.** The desktop entry runs `packaging/vortex-steamos`, which writes the link to the queue and exits if a window is already running. The window polls the queue once a second. No sockets, no D-Bus.
- **The API key and download grants never appear on a command line.** They are passed in files.
- **The upstream importer runs the extensions** in Node against a fake Vortex API and records what they register. Reading them with regular expressions was tried first and picked up the wrong ids.
- **The repository root is `app/`** of the original working folder. The upstream Vortex clone is not part of this repository.
- **The name.** "Vortex" is Nexus Mods' product name. The README states the project is independent. Renaming before any public release is an open decision.

## How the game list was built

78 games in three groups.

1. **Hand-written with code (13, `core.json`):** the Bethesda games (plugin tables taken from Vortex's `gamebryo_plugin_management`), The Witcher 3 and Stardew Valley (install logic ported from the Vortex extensions), Cyberpunk 2077 and Starfield (no real Vortex extension exists; written from each community's documented conventions).
2. **Ported into rules (47, `ported.json`):** 46 were produced by four parallel AI workers, each given `docs/porting/BRIEF.md` and a batch of upstream extensions, returning JSON plus example archives. Palworld was added by hand afterwards, from community conventions, since Vortex only has a stub for it.
3. **Imported automatically (18, `generated.json`):** extensions that only state facts.

How much to trust group 2:

- Every example in `cases.json` (131 of them) passes against the real engine. That proves the rules do what the examples say. It does not prove the rules match real mods, because the same worker wrote both from the same reading of the source.
- `docs/porting/report.json` has each worker's own confidence rating per game (11 high, 32 medium, 6 low) and one line on what upstream does that the port does not.
- Some details came from a worker's memory, not from the source. Known ones: the Darkest Dungeon executable path, native Linux executable names (7 Days to Die, RimWorld, Darkest Dungeon, Witcher 2, Team Fortress 2), the Baldur's Gate 3 and Sekiro launch options, the Morrowind launcher instruction, the Torchlight 2 folder casing.
- Load order files, file merging, and helper tools were not ported for any game. Where that matters the game's `notes` tell the player what to do by hand.

Not ported, and why: The Sims 4, World of Tanks, Neverwinter Nights (original) and 2, Daggerfall Unity, Untitled Goose Game (no Steam id in Vortex), Halo: The Master Chief Collection (needs a LocalLow folder and file edits), Nehrim (tied to Oblivion's install). Vortex's stubs for Mount & Blade II, Subnautica, Subnautica: Below Zero, Devil May Cry 5, and Resident Evil 2 and 3 are still uncovered.

## Verify on a device

In rough order. Each of these is written and untested.

1. **Build.** `scripts/build-release.sh` on x86_64 Linux. The GitHub workflows that would do this are parked in `docs/workflows/` and not active. Check the Godot export preset works with real export templates and that the musl build of `vortex-cli` links.
2. **CI.** `ci.yml` has never run, and is parked with the release workflow. The deploy tests depend on a case-sensitive file system, which macOS does not have, so Linux is where they mean something.
3. **Install.** `install.sh` on SteamOS and on Bazzite: files land in `~/.local/share/vortex-steamos`, `xdg-mime` registers the `nxm` handler, `steamos-add-to-steam` adds the shortcut.
4. **Discovery** against a real Steam library, including a game on an SD card and a game whose Proton prefix is in a different library.
5. **A real mod, end to end,** for Skyrim Special Edition: sign in, download, install, deploy, check `Plugins.txt` in the prefix, launch the game.
6. **`nxm://` from a browser**, including the Flatpak Firefox that ships on the Deck, with the app closed and with it open.
7. **The free-account flow.** The app opens `https://www.nexusmods.com/<game>/mods/<id>?tab=files&file_id=<id>&nmm=1`. That URL shape is from memory.
8. **Game Mode and Big Picture:** fullscreen detection (`SteamDeck`, `GAMESCOPE_WAYLAND_DISPLAY`, `SteamTenfoot`), a real controller, opening the browser with `steam://openurl/`, opening the keyboard with `steam://open/keyboard`.
9. **Cover art** from `appcache/librarycache` in its real layouts.
10. **Running under Steam's runtime:** `core_client.gd` unsets `LD_PRELOAD` and restores `LD_LIBRARY_PATH` from `SYSTEM_LD_LIBRARY_PATH` for child processes. The variable names are from memory.
11. **`.rar` archives** through `bsdtar` on both systems.
12. **Instructions shown to players:** Stardew Valley launch options for SMAPI, `WINEDLLOVERRIDES` for Cyberpunk 2077 and Palworld, the `StarfieldCustom.ini` lines.
13. **Starfield's plugin list.** This app always writes it. Vortex leaves that off by default for Starfield because the game's Creations menu edits the same file.

## Known gaps

- Bethesda mods that install outside `Data`: script extenders, ENB.
- Nexus Collections, mod update checks, endorsements.
- Automatic load order sorting (LOOT).
- Nexus single sign-on (see Decisions).
- Adult mods are hidden from search. Their download links from the website still work.
- The Steam shortcut created by the installer has no artwork.
- No Flatpak. The app installs to the home folder from a tarball.
- A download cannot be paused or resumed, and there is one at a time.
- Witcher 3: no Script Merger, no `mods.settings` load order, no merging of `input.settings` snippets.
- Stardew Valley: only the native Linux build was considered.

## Ideas for what is next

Suggested order: device verification first, then whatever it breaks.

- More games that fit the existing rules. Unreal Engine `~mods` games (Hogwarts Legacy, S.T.A.L.K.E.R. 2, Black Myth: Wukong, Stellar Blade, Lies of P), BepInEx games (Subnautica, Valheim, Lethal Company), Oblivion Remastered, The Sims 4. Partly useful: Elden Ring and the other FromSoftware games (need Mod Engine 2 run through Proton), Capcom RE Engine games, Bannerlord. The owner said the current list is enough for now.
- Load order files for the games that need one written (Baldur's Gate 3 `modsettings.lsx`, XCOM 2, Conan Exiles, Kingdom Come: Deliverance). This would need a new rule type or per-game code.
- Root-folder mods for Bethesda games, using the `targets` mechanism that already exists.
- A Flatpak, and artwork for the Steam shortcut.

## Working on the UI without a device

`shell.gd` has hooks for capturing screens:

```sh
VORTEX_VIEW=mods VORTEX_SHOT=/tmp/mods.png godot --path ui
VORTEX_VIEW=search:campfire VORTEX_SHOT=/tmp/search.png godot --path ui
VORTEX_VIEW=busy VORTEX_SHOT=/tmp/busy.png VORTEX_SHOT_DELAY=1.5 godot --path ui
```

`VORTEX_VIEW` takes `games`, `mods`, `mod`, `plugins`, `plugin`, `downloads`, `search:<query>`, `files:<mod id>`, `account`, `wizard`, `busy`. The window opens on screen briefly. `scripts/ui-smoke.gd` drives the main paths headless and prints each step; any `SCRIPT ERROR` in its output is a failure.

To see cover art in the sandbox, put JPEGs at `target/sandbox/Steam/appcache/librarycache/<app id>/library_600x900.jpg` and `library_hero.jpg`.

## History of the build

The engine, CLI, and a first Godot shell for Skyrim and Skyrim Special Edition existed at the start. The work recorded here, in order:

1. Core fixes and features: case-insensitive deploy, extraction off `/tmp`, removal of a 60-second download limit, reinstall instead of duplicate, `.rar`, remove and reorder mods, plugin toggle and reorder, download progress, `nxm://` links, six more Bethesda games. UI rewritten so nothing blocks. Packaging, CI, and the dev sandbox added.
2. Engine generalised: games as data, optional plugin lists, named deploy targets, mod folders inside the Proton prefix, native Linux builds. Importer written. The Witcher 3, Cyberpunk 2077, and Stardew Valley added.
3. Rule language added. 46 games ported from Vortex by parallel workers. Replacing shipped files with backups added.
4. Starfield and Palworld added.
5. UI rebuilt in the Steam Big Picture style: tab bar, cover shelf, button glyph footer, explicit focus wiring. Installer adds the app to Steam by default.
