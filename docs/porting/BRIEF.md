# Porting Vortex game extensions to JSON game definitions

You are porting game support from the upstream Nexus Mods Vortex source into a
different mod manager (Rust + Godot, for SteamOS / Bazzite Linux). In the new
app a game is DATA: one JSON object. Your job is to read each assigned Vortex
extension and write the JSON object that reproduces what its installers do,
plus example archives that prove it.

Upstream extensions: ../Vortex/extensions/games/<extension>/src/
Facts already read automatically from each extension (id, name, Steam ids,
executable, mod folder): the file written by `node scripts/import-vortex-games.cjs --facts facts.json`
(entries have "extension", "game" (may be missing when it could not be read), "custom", "reason").
Use the facts as the starting point and correct them from the source when wrong.

This brief was written for workers that only produced a JSON file. When porting by hand, add the game to `crates/vortex-core/games/ported.json` and its cases to `games/cases.json`, then run `cargo test`.


## Game object

```json
{
  "id": "rimworld",                       // Vortex game id, lowercase
  "name": "RimWorld",
  "steam": [294100],                      // Steam app ids (numbers). Required. Skip the game if it has none.
  "executable": "RimWorldWin64.exe",      // Windows program, relative to the install folder, forward slashes
  "native_executable": "RimWorldLinux",   // ONLY if the game has a native Linux build on Steam with a different program name, and you are confident of the name. Otherwise omit.
  "mod_path": { "base": "game", "path": "Mods" },
  "nexus_domain": "rimworld",             // details.nexusPageId if set, else the id
  "merge_mods": true,                     // false = every mod gets its own folder (named after the mod) inside mod_path
  "targets": { "root": { "base": "game", "path": "" } },   // optional extra folders rules can address by name
  "install": [ ...rules... ],             // optional, see below
  "notes": "Plain sentences for the player."   // optional
}
```

`base` is one of:
- `game`            the install folder
- `documents`       the Windows user's Documents folder (inside the Proton prefix on Linux)
- `appdata_local`   AppData/Local
- `appdata_roaming` AppData/Roaming
Nothing else exists (no LocalLow, no registry, no absolute paths). If a game
needs another location, skip it and say why.

`path` is relative, forward slashes, no `..`, may be "".

## Install rules

`install` is a list. Rules are tried in order; the FIRST rule that applies and
places at least one file wins, and only its placements are installed. If no
rule matches, the archive is unpacked into mod_path exactly as it is (wrapper
folders included). All name/extension matching ignores case. `to` is a folder
inside the target (default ""). `target` is "" for mod_path or a key of
`targets`. Every rule may carry `"when"` (see below).

1. marker — a folder that holds a marker file is one mod.
   `{"type":"marker", "file":["ModInfo.xml"], "ext":[".mod"], "up":0, "keep_folder":true, "to":"", "target":""}`
   - A file matches if its name equals one of `file` OR ends with one of `ext`.
   - The mod's folder is the marker's folder, `up` levels higher (RimWorld: About/About.xml with up=1).
   - A marker inside another matched folder belongs to the outer one.
   - keep_folder=true: files go to `<to>/<mod folder name>/<rest>`. If the marker is at the top of the archive, the mod's display name is used as the folder name.
   - keep_folder=false: files go to `<to>/<rest>` (contents only).
   - Files outside any matched folder are dropped. Several mods per archive are fine.

2. anchor — cut the archive at the level where a known folder first appears.
   `{"type":"anchor", "dirs":["nativePC"], "keep":true, "to":"", "target":""}`
   - Finds the shallowest folder whose name is in `dirs`. Only files under folders named in `dirs` at that level are installed; siblings (readme etc.) are dropped.
   - keep=true: `<to>/<dir>/<rest>` (the dir keeps the archive's spelling). keep=false: `<to>/<rest>`.

3. flat — install files with given endings side by side, dropping all folders.
   `{"type":"flat", "ext":[".pak"], "to":"", "target":""}`
   - Each matching file goes to `<to>/<file name>`. Everything else is dropped.

4. as_is — install everything unchanged.
   `{"type":"as_is", "strip_wrapper":false, "to":"", "target":""}`
   - strip_wrapper=true drops a single top-level folder that wraps the whole archive.

`when` (optional, on any rule): `{"has_file":["dinput8.dll"], "has_ext":[".dll"], "has_dir":["bin"], "no_ext":[".pak"]}`
Each given list must be satisfied (has_*: at least one match anywhere in the archive; no_ext: no file matches).

What the engine does NOT have: load order files, file merging, running tools,
editing ini/xml/json, per-file exceptions, renaming files, reading file
contents, user dialogs, registry. Deployment is hardlinks into the target
folder; files that already exist in the game and are not ours are never
overwritten. FOMOD installers are handled before rules run (rules then apply to
the FOMOD's output).

## How to port

For each extension: read its src (index and any installer/modtype files).
Work out, for each registerInstaller (in priority order) and registerModType,
what archive shape it accepts and where files end up, then express that as
rules. Mod types that send files somewhere other than mod_path become
`targets` + a rule with `target`. Be faithful to upstream; where the rule
language cannot express something, pick the closest safe behaviour and say so
in `notes` (if the player needs to know) and in your report.

If mods for the game cannot work at all without something the engine lacks
(for example mods are only loaded when listed in a file the manager must
write), do NOT silently include it. Either include it with a clear `notes`
sentence telling the player what they must do by hand, or skip it.
registerLoadOrder / registerMerge by themselves are not a reason to skip when
the game still loads mods without them; mention the loss in `notes` only if it
matters to the player.

`notes` style: plain complete sentences addressed to a player, no jargon, no
markdown, at most three sentences. Only things the player must know or do.

Linux: games run through Proton unless a native Linux build exists. Do not
invent Linux paths. Windows-only helper tools the extension launches are out
of scope.

## Example cases (required)

For every game give at least 2 cases (more for games with several rules):
```json
{ "game": "rimworld", "name": "Cool Mod",
  "files": ["CoolMod-1.2/About/About.xml", "CoolMod-1.2/Defs/x.xml", "readme.txt"],
  "expect": [["CoolMod-1.2/About/About.xml", "CoolMod-1.2/About/About.xml"],
             ["CoolMod-1.2/Defs/x.xml", "CoolMod-1.2/Defs/x.xml"]] }
```
`files` lists every file in a pretend archive (files only, forward slashes).
`expect` lists [from, to] for every file that gets installed; dropped files are
simply absent. `to` is relative to mod_path, or written `[target] path` for a
named target, e.g. `[root] dinput8.dll`. `name` is the mod's display name (only
matters when a marker sits at the archive top). Trace the rules by hand
carefully — these cases are run as tests against the real engine, and a wrong
expectation fails the build. Base them on archive shapes the upstream code (or
its tests) clearly handles.

## Output file

Valid JSON, exactly:
```json
{ "games": [ ... ], "cases": [ ... ],
  "skipped": [ { "extension": "game-x", "reason": "..." } ],
  "report": [ { "id": "rimworld", "confidence": "high|medium|low", "lost": "what upstream does that this port does not, one line" } ] }
```
Validate it with `python3 -c "import json;json.load(open(PATH))"` before finishing.
Your final message: one line per game (ported / skipped and why). Nothing else.
