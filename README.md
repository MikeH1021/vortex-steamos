# Vortex for SteamOS and Bazzite

An open source, controller-first mod manager for SteamOS and Bazzite. It runs as a native Linux app in Game Mode and on the desktop. It finds your Steam games, downloads mods from Nexus Mods, installs them, and keeps the game's plugin list in order. The games themselves stay Windows builds and run through Proton.

> **Early days.** The app has been built and run on one Bazzite desktop: it found a real Steam library, signed in to Nexus Mods, and downloaded, installed, and deployed mods for Skyrim from both the in-app search and a Mod Manager Download link. It has not yet been run on a Steam Deck, on SteamOS itself, or in Game Mode, and most of the supported games have never had a real mod put through them. Expect things to break. Back up your game and save folders before letting it touch them. This notice will come down once the checklist in [docs/HANDOFF.md](docs/HANDOFF.md) has been worked through on a device.

This is an independent project. It is not made or endorsed by Nexus Mods. The tornado icon is the Vortex icon from the upstream Vortex source, which is published under the GPL-3.0; the name and logo belong to Nexus Mods. The `Vortex/` directory next to this one is the upstream Nexus Mods Vortex source, kept as a behavior reference. This app does not link against it or modify it.

## Supported games

Games installed through Steam. Run `vortex-cli games` for the full list.

- **With plugin lists and load order:** Skyrim Special Edition, Skyrim, Skyrim VR, Enderal, Starfield, Fallout 4, Fallout 4 VR, Fallout: New Vegas, Fallout 3 (including Game of the Year), and Oblivion.
- **With install rules of their own:** The Witcher 3, Cyberpunk 2077, and Stardew Valley. See the notes below.
- **Ported with install rules (47):** 7 Days to Die, A Hat in Time, Baldur's Gate 3, Blade & Sorcery, Bloodstained: Ritual of the Night, Code Vein, Conan Exiles, Dark Souls II, Darkest Dungeon, Dawn of Man, Dragon Age: Origins, Dragon's Dogma, Elex, Factorio, Galactic Civilizations III, GreedFall, Kenshi, Kingdom Come: Deliverance, Microsoft Flight Simulator, Monster Hunter: World, Morrowind, Mount & Blade, Mount & Blade: Warband, Mount & Blade: With Fire and Sword, Neverwinter Nights: Enhanced Edition, No Man's Sky, Palworld, Pillars of Eternity II: Deadfire, Prison Architect, RimWorld, Sekiro, Spyro Reignited Trilogy, Star Wars: KOTOR, Star Wars: KOTOR II, Surviving Mars, Team Fortress 2, The Witcher, The Witcher 2, Torchlight II, Total War: Three Kingdoms, Vampire: The Masquerade Bloodlines, War Thunder, Wolcen: Lords Of Mayhem, X Rebirth, X4: Foundations, XCOM 2, XCOM 2: War of the Chosen.
- **Imported from Vortex, mods unpacked into the game's mod folder as they are (18):** BattleTech, Breaking Wheel, Dark Souls, Divinity: Original Sin 2 Definitive Edition, Divinity: Original Sin 2 Original Edition, Dragon Age 2, Garden Paws, Grim Dawn, Kerbal Space Program, Legend of Grimrock, Oxygen Not Included, Pathfinder: Kingmaker, Pathfinder: Wrath of the Righteous, Shadowrun Returns, Starbound, State of Decay, The Elder Scrolls Online, The Sims 3.

The ported games reproduce where Vortex puts each kind of mod archive. What Vortex does beyond that for some of them is not here: writing a game's load order file, merging two mods that change the same file, and running helper tools. Where that matters, the app says what to do by hand under "Good to know" on the game's Mods screen.

Some games keep mods or their plugin list inside the Proton prefix, which Steam creates the first time the game runs. Launch those once from Steam before modding them. The Games screen says which ones are waiting.

### The Witcher 3

Mods, DLC folders, and menu files are recognised however the author packed them and placed under `mods`, `dlc`, and `bin` in the game folder. The game loads mods in alphabetical order of their folder names. A mod that ships key bindings to paste into `input.settings` (a file such as `MyMod.input.settings`) has them added to the game's `input.settings` for you, and taken out again when the mod is turned off. The file as it was is kept next to it as `input.settings.vortex_backup`. Close the game first: it rewrites that file when it exits. Not handled: the Script Merger tool (needed when two mods change the same script), and mods that ask you to paste lines into `user.settings`.

`patches/witcher3-mcmr/` holds script fixes for two specific mods, Multi Companion Mod Remastered and its Autonomous Romance add-on, whose companions would not run, catch up, or speak on the machine this was tested on. They are not applied by the app. Its README says what they do and how to apply them.

### Cyberpunk 2077

Mods are placed from the level of the game's own folders (`archive`, `bin`, `r6`, `red4ext`, `engine`, `mods`). Loose `.archive` files go to `archive/pc/mod`, and REDmods to `mods`. Two things Steam has to be told:

- Framework mods (Cyber Engine Tweaks, RED4ext) need `WINEDLLOVERRIDES="winmm,version=n,b" %command%` in the game's launch options.
- REDmods load only when REDmod is switched on in the game's launcher.

### Stardew Valley

Steam installs Stardew Valley's own Linux build, and that is what this app mods. Each folder with a `manifest.json` becomes a mod in `Mods`. SMAPI, the mod loader nearly every mod needs, installs like a mod: download it from its Nexus page and the app puts it next to the game. Then set the game's launch options in Steam to the path of `StardewModdingAPI` in the game folder, in quotes, followed by `%command%`.

## Install

Download the latest `vortex-steamos-…-linux-x86_64.tar.gz` from the releases page. On a Steam Deck, switch to Desktop Mode first. Then:

```sh
tar -xzf vortex-steamos-*-linux-x86_64.tar.gz
cd vortex-steamos-*-linux-x86_64
./install.sh
```

Everything goes into your home folder (`~/.local/share/vortex-steamos`). Nothing needs root and nothing touches the read-only system image, so the app survives system updates. The installer also adds the app to your Steam library, with cover art and an icon, so it appears under Non-Steam in Game Mode and in Big Picture. Restart Steam to see the artwork. Pass `--no-steam` to skip that.

The installer also makes the app the handler for `nxm://` links. Those are the links behind the **Mod Manager Download** button on nexusmods.com. Pass `--no-links` to skip that.

To remove the app, run `~/.local/share/vortex-steamos/uninstall.sh`. Your mods and downloads are left in place.

## Using it

The app is laid out like Steam's Big Picture and is driven entirely from a controller. The footer always shows what each button does on the current screen.

| Button | Does |
| --- | --- |
| D-pad or left stick | Move the highlight |
| A | Select |
| B | Back |
| X | Turn the highlighted mod or plugin on or off. Discard a download |
| Y | Details |
| LB / RB | Previous or next tab |
| LT / RT | Jump five rows in a long list |

The Games tab is a shelf of your installed games, using the cover art Steam already has. Highlight a text box and Steam's on-screen keyboard opens. Steam + X opens it by hand. A keyboard works too: arrows, Enter, Esc, X, Y, Q, E, Page Up, Page Down.

### Sign in

Open **Downloads**, then **Nexus account**. Nexus signs third-party apps in with a personal API key:

1. Press **Get your key**. Your Nexus account's API keys page opens in the browser. Copy the Personal API Key at the bottom.
2. Paste it into the app and press **Sign in**.

The key is checked with Nexus and saved at `~/.config/vortex-steamos/nexus.key`, readable only by you. It is never shown again, never logged, and never passed on a command line.

### Get mods

There are three ways, and they all end the same: the mod is installed and switched on.

- **From the Nexus website.** Browse nexusmods.com in your browser and press **Mod Manager Download** on a file. The browser hands the link to this app, which downloads and installs it. This works with free and Premium accounts, and is the same flow Vortex uses on Windows.
- **From inside the app.** **Downloads → Search Nexus Mods** finds mods by name and lists their files. With Premium, pressing A on a file downloads it right there. With a free account, pressing A opens that file's download page in the browser, and the file comes back to the app when you press Slow Download.
- **From a file you already have.** Put a `.zip`, `.7z`, or `.rar` in the incoming folder shown under **Downloads → Check the incoming folder**.

A mod with an installer (FOMOD) opens a wizard so you can pick its options. Required and recommended options start selected, and later steps react to what you picked and to the plugins you already have.

### Manage mods

- **Mods** lists what is installed. X turns a mod on or off. A opens its options: move it up or down, open its Nexus page, or remove it.
- When two mods ship the same file, the mod lower in the list wins. Moving a mod changes who wins.
- **Plugins** is the load order the game uses, top to bottom. X turns a plugin on or off, and A lets you move it earlier or later. Base game plugins stay on and stay first. A plugin whose mod is turned off and on again comes back where it was.

Turning a mod off takes its files out of the game folder and keeps the mod installed. For the Bethesda games, a file that belongs to the game itself is never replaced. For other games a mod may stand in for a game file: the original is kept next to it as `<name>.vortex_backup` and put back when the mod is turned off.

## How it works

- Mods are staged in `<steam library>/steamapps/vortex/<game>/mods`, on the same drive as the game. A game later moved to another Steam library keeps its mods where they were staged. Enabled mods are hardlinked into the game's `Data` folder, so they take no extra space. A library on a different drive from its staging folder falls back to symlinks.
- Windows does not care about letter case and Linux does. A mod that ships `textures/` is merged into the game's existing `Textures/`, and `Plugins.txt` is written under whichever spelling the game already created. Without this a game silently ignores half of a mod.
- Archives are unpacked next to the staging folder, not in `/tmp`. On SteamOS `/tmp` is memory, and a texture pack does not fit there.
- Plugin lists are written the way each game expects. Special Edition and Fallout 4 use the `*Plugin.esp` format and leave base masters out. The older games list enabled plugins and take their order from file timestamps.
- `.rar` archives are unpacked with `bsdtar`, `7z`, or `unrar` from the system, whichever is there. Bazzite ships `7z` and `unrar`.
- A script extender or other mod that ships programs and libraries for the game folder itself (SKSE's loader, an engine-fix DLL) has those files placed next to the game's own program, and the rest in `Data`.

## What it does not do yet

- Starting a script extender. SKSE, F4SE, and the like are installed into the game folder, but Steam still starts the game's own program. For Skyrim and Skyrim Special Edition the Mods screen shows the launch option that points Steam at the loader. That line has not been tried in a game yet.
- ENB and other mods that put whole folders, or files other than `.exe`, `.dll`, and `.asi`, into the game folder of a Bethesda game. Install those by hand.
- Load order files, file merging, and helper tools for the games ported from Vortex. Baldur's Gate 3 mods, for one, are installed here and then switched on in the game's own mod manager.
- Games Vortex only finds through the Windows registry or another store, such as The Sims 4, World of Tanks, and Halo: The Master Chief Collection.
- Nexus Collections.
- Automatic load order sorting (LOOT). You order plugins yourself.
- Nexus single sign-on. That needs this app to be registered with Nexus Mods, so for now sign-in uses the personal API key described above.
- Adult mods are left out of search results. Download links for them from the website still work.

## Develop

- `crates/vortex-core` is the engine: Steam discovery, Proton prefixes, installing, deploying, plugin lists, and the Nexus client.
- `crates/vortex-core/games/` holds the game definitions. They are data, not code.
- `crates/vortex-cli` exposes the engine as commands that print JSON.
- `ui/` is the Godot 4 shell. It draws the interface and calls `vortex-cli` on a worker thread for everything else.
- `packaging/` holds the launcher, desktop entry, and installer that go into a release.

```sh
cargo test
cargo build -p vortex-cli
godot --path ui            # Godot 4.3 or newer
```

In the editor the window is 1280×800, the Deck's resolution. A Steam Deck session (`SteamDeck=1`) or gamescope goes fullscreen. A source checkout uses the `vortex-cli` in `target/`. Set `VORTEX_CLI` to use another one.

### Run it without Steam or a network

`scripts/dev-sandbox.sh` builds a pretend Steam library with two games and a canned Nexus under `target/sandbox`. It works on macOS and Linux:

```sh
eval "$(scripts/dev-sandbox.sh)"
cargo build -p vortex-cli
godot --path ui
godot --path ui -- 'nxm://skyrimspecialedition/mods/667/files/1234?key=abc&expires=1'
```

The second `godot` line plays the part of a browser opening a Mod Manager Download link.

The variables behind it are useful on their own. `VORTEX_STEAM_ROOT` points discovery at one Steam folder, `VORTEX_NEXUS_FIXTURE` swaps Nexus for a folder of JSON files, and `VORTEX_NEXUS_KEY` overrides the saved key.

### Add a game

A game is an entry in a JSON file: its name, Steam app id, executable, Nexus page, and the folder mods go in.

```json
{
  "id": "grimdawn",
  "name": "Grim Dawn",
  "steam": [219990],
  "executable": "Grim Dawn.exe",
  "mod_path": { "base": "game", "path": "mods" },
  "nexus_domain": "grimdawn"
}
```

`base` is `game` for the install folder, or `documents`, `appdata_local`, or `appdata_roaming` for those Windows folders, which this app finds inside the game's Proton prefix. Add `"native_executable"` when the game has a Linux build with a differently named program, and `"merge_mods": false` when each mod needs a folder of its own.

The definitions are in three files, and an earlier one wins when two define the same game:

- `games/core.json`: games with code behind them. The Bethesda games, and games whose install logic is a function in `crates/vortex-core/src/layout.rs`.
- `games/ported.json`: games whose Vortex extension had install logic, ported into install rules.
- `games/generated.json`: written by `node scripts/import-vortex-games.cjs`. It runs each game extension from the upstream Vortex checkout next to this folder against a pretend Vortex, and records what the extension registers. It lists every game it skips and why. With the `esbuild` package installed it reads the TypeScript extensions too, and `--facts out.json` dumps what it could read from every extension as a starting point for a port.

#### Install rules

Mod authors pack archives every which way, so a game can list rules for telling the shapes apart. The first rule that finds something to install is used. An archive no rule fits is unpacked into the mod folder as it is.

```json
"install": [
  { "type": "marker", "file": ["About.xml"], "up": 1 },
  { "type": "anchor", "dirs": ["nativePC"], "keep": false },
  { "type": "flat", "ext": [".pak"], "to": "~mods" },
  { "type": "reject", "message": "This is a tool, not a mod.", "when": { "has_file": ["patcher.exe"] } }
]
```

- `marker`: a folder holding the named file is one mod, installed under its own name. `up` climbs from the marker's folder. `"keep_folder": false` installs the contents only.
- `anchor`: cut the archive at the level where one of these folders appears, and install only those folders.
- `flat`: install files with these endings side by side, dropping their folders.
- `as_is`: install everything, optionally dropping one wrapper folder.
- `reject`: refuse the archive with a message.

A game can also name settings files of its own that mods bring lines for:

```json
"settings": [
  { "name": "input.settings", "path": { "base": "documents", "path": "The Witcher 3/input.settings" } }
]
```

A file in a mod called `input.settings`, or ending in `.input.settings` or `.input.settings.txt`, is then read as sections of lines, and each line is added to the same section of the game's file while the mod is on.

Any rule can carry `to` (a folder inside the mod folder), `target` (one of the game's named `targets`, such as the game root for a mod loader), and `when` (`has_file`, `has_ext`, `has_dir`, `no_ext`).

Every ported game has example archives in `games/cases.json`: a list of files, and where each is expected to go. `cargo test` runs them all, so a rule change that breaks a game shows up at once. A game that needs more than rules gets a function in `layout.rs`, as The Witcher 3, Stardew Valley, and Cyberpunk 2077 have. Vortex has no extension of its own for Cyberpunk 2077, Starfield, or Palworld (it points to community ones), so those follow the folder conventions each game's modding community documents.

### Build a release

Release archives are built on x86_64 Linux, with Godot's Linux export templates installed:

```sh
rustup target add x86_64-unknown-linux-musl
scripts/build-release.sh
```

`vortex-cli` is linked statically so one binary runs on both SteamOS and Bazzite. That needs a musl C compiler. On a system without one, such as Bazzite, build the binary in a container and hand it to the script:

```sh
podman run --rm -v "$PWD":/src:Z -w /src -e CARGO_TARGET_DIR=/src/target/container \
    docker.io/library/rust:alpine sh -c \
    'apk add --no-cache musl-dev && cargo build --release --locked --target x86_64-unknown-linux-musl -p vortex-cli'
VORTEX_CLI_BIN=target/container/x86_64-unknown-linux-musl/release/vortex-cli scripts/build-release.sh
```

Workflows that run the tests and build this archive on GitHub are in `docs/workflows/`, not yet switched on. Its README says how.

### The command line

Every action in the app is a `vortex-cli` command, which makes the engine easy to script and to test:

```sh
vortex-cli discover
vortex-cli sync --game skyrimse
vortex-cli toggle --game skyrimse --mod campfire
vortex-cli remove --game skyrimse --mod campfire
vortex-cli purge --game skyrimse
vortex-cli plugins move --game skyrimse --plugin Campfire.esp --by -1
vortex-cli nexus search --game fallout4 --query "armor"
vortex-cli nexus nxm --url 'nxm://skyrimspecialedition/mods/667/files/1234?key=…&expires=…'
```

`vortex-cli --help` lists all of them. `purge` takes every link this app made back out of a game folder and leaves the mods installed. Run it before uninstalling if you want the game folder exactly as Steam left it.

For an installer, `fomod` prints the steps and `install --choices choices.json` installs with your picks. `choices.json` is a list of `{ "step", "group", "plugins" }`. Add `"step_index"` and `"group_index"`, counted from 0, when an installer has two steps or groups with the same name. Game-version and script-extender checks inside an installer are treated as satisfied.

## Project status and history

`docs/HANDOFF.md` records how this was built, the decisions behind it, what has and has not been verified on a real device, and what to do next. Read it before picking the project up on a new machine.

## License

`GPL-3.0-or-later`, see `LICENSE.md`. The code here is original. The license matches upstream Vortex so behavior ported from that tree later stays compatible with it.
