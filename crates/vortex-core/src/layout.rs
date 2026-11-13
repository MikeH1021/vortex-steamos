//! Where each file of an unpacked archive belongs, per game.
//!
//! Mod authors pack archives every way imaginable. One wraps everything in a
//! folder named after the mod, the next starts at the game folder, a third
//! ships loose files. Each game has rules for telling these apart. The
//! Witcher 3 and Stardew Valley rules follow the Vortex extensions for those
//! games. Cyberpunk 2077 has no extension in the Vortex tree, so its rules
//! follow the folder conventions its modding community documents.

use std::path::{Path, PathBuf};

use crate::deploy::{walk_files, ModError};
use crate::games::{Kind, Rule, When};

/// One file to install: where it is in the archive and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Path inside the unpacked archive.
    pub from: PathBuf,
    /// Named folder of the game, or "" for its mod folder.
    pub target: String,
    /// Path inside that folder.
    pub to: PathBuf,
}

/// Name of the game-root target that games with a mod loader define.
pub(crate) const ROOT: &str = "root";

pub(crate) fn plan(
    kind: Kind,
    rules: &[Rule],
    package: &Path,
    name: &str,
) -> Result<Vec<Placement>, ModError> {
    let files: Vec<String> = walk_files(package)?
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    plan_files(kind, rules, &files, name)
}

/// Lay out an archive given as a list of its files.
pub(crate) fn plan_files(
    kind: Kind,
    rules: &[Rule],
    files: &[String],
    name: &str,
) -> Result<Vec<Placement>, ModError> {
    let placements = match kind {
        Kind::Generic | Kind::Bethesda => by_rules(rules, files, name)?,
        Kind::Witcher3 => witcher3(files),
        Kind::Cyberpunk2077 => cyberpunk(files),
        Kind::Stardew => stardew(files, name),
    };
    if placements.is_empty() {
        return Err(ModError::new(match kind {
            Kind::Witcher3 => format!("{name} does not look like a Witcher 3 mod. No mods, dlc, bin, or content folder was found in it."),
            Kind::Cyberpunk2077 => format!("{name} does not look like a Cyberpunk 2077 mod. No archive, bin, r6, red4ext, engine, or mods folder was found in it."),
            Kind::Stardew => format!("{name} does not look like a Stardew Valley mod. No manifest.json was found in it."),
            _ => format!("{name} has no files to install."),
        }));
    }
    Ok(placements)
}

// --- Games described by rules ------------------------------------------------

fn by_rules(rules: &[Rule], files: &[String], name: &str) -> Result<Vec<Placement>, ModError> {
    for rule in rules {
        let (when, placed) = match rule {
            Rule::Reject { message, when } => {
                if applies(when, files) {
                    return Err(ModError::new(message.clone()));
                }
                continue;
            }
            Rule::Marker {
                file,
                ext,
                up,
                keep_folder,
                to,
                target,
                when,
            } => (
                when,
                marker(files, file, ext, *up, *keep_folder, to, target, name),
            ),
            Rule::Anchor {
                dirs,
                keep,
                to,
                target,
                when,
            } => (when, anchor(files, dirs, *keep, to, target)),
            Rule::Flat {
                ext,
                to,
                target,
                when,
            } => (when, flat(files, ext, to, target)),
            Rule::AsIs {
                strip_wrapper,
                to,
                target,
                when,
            } => (when, whole(files, *strip_wrapper, to, target)),
        };
        if !placed.is_empty() && applies(when, files) {
            return Ok(placed);
        }
    }
    Ok(as_is(files))
}

fn applies(when: &When, files: &[String]) -> bool {
    let lower: Vec<Vec<String>> = files.iter().map(|file| lower_parts(file)).collect();
    let name = |parts: &Vec<String>| parts.last().cloned().unwrap_or_default();
    let ends = |list: &[String], parts: &Vec<String>| {
        list.iter()
            .any(|ext| name(parts).ends_with(&ext.to_lowercase()))
    };
    let file_ok = when.has_file.is_empty()
        || lower.iter().any(|parts| {
            when.has_file
                .iter()
                .any(|wanted| wanted.to_lowercase() == name(parts))
        });
    let ext_ok = when.has_ext.is_empty() || lower.iter().any(|parts| ends(&when.has_ext, parts));
    let dir_ok = when.has_dir.is_empty()
        || lower.iter().any(|parts| {
            parts[..parts.len() - 1].iter().any(|part| {
                when.has_dir
                    .iter()
                    .any(|wanted| &wanted.to_lowercase() == part)
            })
        });
    let none_ok = !lower.iter().any(|parts| ends(&when.no_ext, parts));
    file_ok && ext_ok && dir_ok && none_ok
}

/// `to` joined with more path parts, skipping `to` when it is empty.
fn under(to: &str, rest: &[&str]) -> PathBuf {
    let mut path = PathBuf::new();
    for part in to.split('/').filter(|part| !part.is_empty()) {
        path.push(part);
    }
    for part in rest {
        path.push(part);
    }
    path
}

#[allow(clippy::too_many_arguments)]
fn marker(
    files: &[String],
    names: &[String],
    exts: &[String],
    up: usize,
    keep_folder: bool,
    to: &str,
    target: &str,
    mod_name: &str,
) -> Vec<Placement> {
    let mut roots: Vec<Vec<&str>> = Vec::new();
    for file in files {
        let mut parts: Vec<&str> = file.split('/').collect();
        let name = parts.pop().unwrap_or("").to_lowercase();
        let hit = names.iter().any(|wanted| wanted.to_lowercase() == name)
            || exts.iter().any(|ext| name.ends_with(&ext.to_lowercase()));
        if !hit {
            continue;
        }
        parts.truncate(parts.len().saturating_sub(up));
        if !roots.contains(&parts) {
            roots.push(parts);
        }
    }
    // A marker inside another mod's folder belongs to the outer mod.
    roots.sort_by_key(|root| root.len());
    let mut outer: Vec<Vec<&str>> = Vec::new();
    for root in roots {
        if !outer
            .iter()
            .any(|kept| root.len() >= kept.len() && root[..kept.len()] == kept[..])
        {
            outer.push(root);
        }
    }
    files
        .iter()
        .filter_map(|file| {
            let parts: Vec<&str> = file.split('/').collect();
            let root = outer
                .iter()
                .find(|root| parts.len() > root.len() && parts[..root.len()] == root[..])?;
            let mut rest: Vec<&str> = Vec::new();
            if keep_folder {
                rest.push(root.last().copied().unwrap_or(mod_name));
            }
            rest.extend(&parts[root.len()..]);
            Some(place(file, target, under(to, &rest)))
        })
        .collect()
}

fn anchor(files: &[String], dirs: &[String], keep: bool, to: &str, target: &str) -> Vec<Placement> {
    let dirs: Vec<String> = dirs.iter().map(|dir| dir.to_lowercase()).collect();
    let mut base: Option<Vec<String>> = None;
    for file in files {
        let lower = lower_parts(file);
        let folders = &lower[..lower.len() - 1];
        if let Some(index) = folders.iter().position(|part| dirs.contains(part)) {
            if base.as_ref().is_none_or(|found| index < found.len()) {
                base = Some(lower[..index].to_vec());
            }
        }
    }
    let Some(base) = base else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|file| {
            let parts: Vec<&str> = file.split('/').collect();
            let lower = lower_parts(file);
            let inside = lower.len() > base.len() + 1
                && lower[..base.len()] == base[..]
                && dirs.contains(&lower[base.len()]);
            let from = if keep { base.len() } else { base.len() + 1 };
            inside.then(|| place(file, target, under(to, &parts[from..])))
        })
        .collect()
}

fn flat(files: &[String], exts: &[String], to: &str, target: &str) -> Vec<Placement> {
    files
        .iter()
        .filter(|file| {
            let lower = file.to_lowercase();
            exts.iter().any(|ext| lower.ends_with(&ext.to_lowercase()))
        })
        .map(|file| {
            let name = file.rsplit('/').next().unwrap_or(file);
            place(file, target, under(to, &[name]))
        })
        .collect()
}

fn whole(files: &[String], strip_wrapper: bool, to: &str, target: &str) -> Vec<Placement> {
    let first = |file: &String| file.split('/').next().map(str::to_string);
    let wrapped = strip_wrapper
        && !files.is_empty()
        && files.iter().all(|file| file.contains('/'))
        && files.iter().all(|file| first(file) == first(&files[0]));
    files
        .iter()
        .map(|file| {
            let parts: Vec<&str> = file.split('/').collect();
            let rest = if wrapped { &parts[1..] } else { &parts[..] };
            place(file, target, under(to, rest))
        })
        .collect()
}

fn place(from: &str, target: &str, to: impl Into<PathBuf>) -> Placement {
    Placement {
        from: PathBuf::from(from),
        target: target.to_string(),
        to: to.into(),
    }
}

fn as_is(files: &[String]) -> Vec<Placement> {
    files.iter().map(|file| place(file, "", file)).collect()
}

fn lower_parts(file: &str) -> Vec<String> {
    file.split('/').map(str::to_lowercase).collect()
}

fn join(parts: &[&str]) -> PathBuf {
    parts.iter().collect()
}

// --- The Witcher 3 ---------------------------------------------------------

/// Everything is placed relative to the game folder.
///
/// - `…/mods/modName/…` and `…/dlc/dlcName/…` keep their place, whatever
///   folders the author wrapped around them.
/// - `…/bin/…` holds menu definitions and goes to the game's `bin`.
/// - A bare `modName/content/…` goes under `mods`, or under `dlc` when the
///   folder name starts with "dlc".
fn witcher3(files: &[String]) -> Vec<Placement> {
    let mut placements = Vec::new();
    for file in files {
        let parts: Vec<&str> = file.split('/').collect();
        let lower = lower_parts(file);
        // Authors include a vanilla copy of the menu files "to restore later".
        if lower.iter().any(|part| part.contains("backup")) {
            continue;
        }
        let folders = &lower[..lower.len() - 1];
        // Key bindings to paste into input.settings. Kept with the mod, out
        // of the game folder, wherever in the archive the author put them.
        let inside_a_mod = folders.iter().any(|part| part == "mods" || part == "content");
        if !inside_a_mod && crate::settings::is_snippet(&lower[lower.len() - 1], "input.settings") {
            placements.push(place(file, "", format!(".settings/{}", parts[parts.len() - 1])));
            continue;
        }
        let anchor = |wanted: &str| folders.iter().position(|part| part == wanted);
        if let Some(index) = anchor("mods")
            .or_else(|| anchor("dlc"))
            .or_else(|| anchor("bin"))
        {
            // A folder directly under mods/ or dlc/ is required. `mods/readme.txt` is not a mod.
            if lower[index] == "bin" || parts.len() > index + 2 {
                let mut to = vec![lower[index].as_str()];
                to.extend(&parts[index + 1..]);
                placements.push(place(file, "", join(&to)));
            }
            continue;
        }
        if let Some(content) = folders.iter().position(|part| part == "content") {
            if content == 0 {
                continue;
            }
            let owner = parts[content - 1];
            let folder = if lower[content - 1].starts_with("dlc") {
                "dlc"
            } else {
                "mods"
            };
            // The game only loads mod folders whose name starts with "mod".
            let fixed;
            let owner = if folder == "mods" && !lower[content - 1].starts_with("mod") {
                fixed = format!("mod{owner}");
                fixed.as_str()
            } else {
                owner
            };
            let mut to = vec![folder, owner];
            to.extend(&parts[content..]);
            placements.push(place(file, "", join(&to)));
        }
    }
    placements
}

// --- Cyberpunk 2077 --------------------------------------------------------

/// Folders of the game that mods add files to.
const CYBERPUNK_ROOTS: &[&str] = &["archive", "bin", "engine", "r6", "red4ext", "mods", "tools"];

/// Everything is placed relative to the game folder.
///
/// - An archive that contains game folders (`archive`, `bin`, `r6`, …) is
///   copied from the level those folders are at. Files next to them, such as
///   a readme, are left out.
/// - A REDmod, recognised by its `info.json`, goes to `mods/<name>`.
/// - Loose `.archive` and `.xl` files go to `archive/pc/mod`.
fn cyberpunk(files: &[String]) -> Vec<Placement> {
    // The shallowest folder that has a game folder as its child.
    let mut base: Option<Vec<String>> = None;
    for file in files {
        let lower = lower_parts(file);
        let folders = &lower[..lower.len() - 1];
        if let Some(index) = folders
            .iter()
            .position(|part| CYBERPUNK_ROOTS.contains(&part.as_str()))
        {
            if base.as_ref().is_none_or(|found| index < found.len()) {
                base = Some(lower[..index].to_vec());
            }
        }
    }
    if let Some(base) = base {
        return files
            .iter()
            .filter_map(|file| {
                let parts: Vec<&str> = file.split('/').collect();
                let lower = lower_parts(file);
                let inside = lower.len() > base.len() + 1
                    && lower[..base.len()] == base[..]
                    && CYBERPUNK_ROOTS.contains(&lower[base.len()].as_str());
                inside.then(|| {
                    // Game folders are lowercase on disk. Keep them that way.
                    let mut to = vec![lower[base.len()].as_str()];
                    to.extend(&parts[base.len() + 1..]);
                    place(file, "", join(&to))
                })
            })
            .collect();
    }

    let redmods: Vec<Vec<&str>> = files
        .iter()
        .filter(|file| file.to_lowercase().ends_with("/info.json"))
        .map(|file| {
            let mut parts: Vec<&str> = file.split('/').collect();
            parts.pop();
            parts
        })
        .collect();
    if !redmods.is_empty() {
        return files
            .iter()
            .filter_map(|file| {
                let parts: Vec<&str> = file.split('/').collect();
                let owner = redmods
                    .iter()
                    .find(|root| parts.len() > root.len() && parts[..root.len()] == root[..])?;
                let mut to = vec!["mods"];
                to.extend(&parts[owner.len() - 1..]);
                Some(place(file, "", join(&to)))
            })
            .collect();
    }

    files
        .iter()
        .filter(|file| {
            let lower = file.to_lowercase();
            lower.ends_with(".archive") || lower.ends_with(".xl")
        })
        .map(|file| {
            let name = file.rsplit('/').next().unwrap_or(file);
            place(file, "", join(&["archive", "pc", "mod", name]))
        })
        .collect()
}

// --- Stardew Valley ----------------------------------------------------------

/// The SMAPI installer's payload for Linux, when `files` is the SMAPI download.
pub(crate) fn smapi_payload(files: &[String]) -> Option<&String> {
    files.iter().find(|file| {
        let lower = lower_parts(file);
        let name = lower.last().map(String::as_str).unwrap_or("");
        (name == "install.dat" || name == "linux-install.dat")
            && lower.iter().any(|part| part == "linux")
    })
}

/// SMAPI unpacked: everything from the folder holding `StardewModdingAPI`
/// goes next to the game.
pub(crate) fn smapi(files: &[String]) -> Vec<Placement> {
    let Some(loader) = files.iter().find(|file| {
        file.rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("StardewModdingAPI"))
    }) else {
        return Vec::new();
    };
    let base = &loader[..loader.len() - "StardewModdingAPI".len()];
    files
        .iter()
        .filter(|file| file.starts_with(base))
        .map(|file| place(file, ROOT, &file[base.len()..]))
        .collect()
}

/// Each folder with a `manifest.json` is one mod and becomes a folder in
/// `Mods`. An archive can hold several. Files outside those folders are left
/// out. A replacement for the game's own `Content` goes next to the game.
fn stardew(files: &[String], name: &str) -> Vec<Placement> {
    let mut roots: Vec<Vec<&str>> = files
        .iter()
        .filter(|file| {
            let lower = lower_parts(file);
            lower.last().is_some_and(|name| name == "manifest.json")
                && !lower.iter().any(|part| part == "locale")
        })
        .map(|file| {
            let mut parts: Vec<&str> = file.split('/').collect();
            parts.pop();
            parts
        })
        .collect();
    // A mod inside another mod's folder belongs to the outer one.
    roots.sort_by_key(|root| root.len());
    let mut outer: Vec<Vec<&str>> = Vec::new();
    for root in roots {
        if !outer
            .iter()
            .any(|kept| root.len() >= kept.len() && root[..kept.len()] == kept[..])
        {
            outer.push(root);
        }
    }
    if outer.is_empty() {
        return files
            .iter()
            .filter_map(|file| {
                let parts: Vec<&str> = file.split('/').collect();
                let index = parts
                    .iter()
                    .position(|part| part.eq_ignore_ascii_case("Content"))?;
                (index + 1 < parts.len()).then(|| place(file, ROOT, join(&parts[index..])))
            })
            .collect();
    }
    files
        .iter()
        .filter_map(|file| {
            let parts: Vec<&str> = file.split('/').collect();
            let root = outer
                .iter()
                .find(|root| parts.len() > root.len() && parts[..root.len()] == root[..])?;
            let folder = root.last().copied().unwrap_or(name);
            let mut to = vec![folder];
            to.extend(&parts[root.len()..]);
            Some(place(file, "", join(&to)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ported game ships examples of archives and where their files go.
    #[test]
    fn ported_games_place_their_example_archives() {
        #[derive(serde::Deserialize)]
        struct Case {
            game: String,
            #[serde(default)]
            name: String,
            files: Vec<String>,
            expect: Vec<(String, String)>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../games/cases.json")).expect("cases.json is valid");
        assert!(!cases.is_empty());
        for (index, case) in cases.iter().enumerate() {
            let game = crate::games::game_by_id(&case.game)
                .unwrap_or_else(|| panic!("case {index}: no game '{}'", case.game));
            let name = if case.name.is_empty() {
                "Mod"
            } else {
                &case.name
            };
            let placed = plan_files(game.kind, &game.install, &case.files, name)
                .map(|placed| moves(&placed))
                .unwrap_or_default();
            let mut expect = case.expect.clone();
            expect.sort();
            assert_eq!(placed, expect, "{} case {index} ({name})", case.game);
        }
    }

    fn moves(placements: &[Placement]) -> Vec<(String, String)> {
        let mut list: Vec<(String, String)> = placements
            .iter()
            .map(|item| {
                let to = item.to.to_string_lossy().replace('\\', "/");
                let to = if item.target.is_empty() {
                    to
                } else {
                    format!("[{}] {to}", item.target)
                };
                (item.from.to_string_lossy().to_string(), to)
            })
            .collect();
        list.sort();
        list
    }

    fn files(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| item.to_string()).collect()
    }

    fn pair(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }

    #[test]
    fn witcher3_finds_the_mod_however_it_is_wrapped() {
        let placed = witcher3(&files(&[
            "Better Torches v2/Mods/modBetterTorches/content/blob0.bundle",
            "Better Torches v2/DLC/dlcTorches/content/metadata.store",
            "Better Torches v2/bin/config/r4game/user_config_matrix/pc/torches.xml",
            "Better Torches v2/backup/bin/config/r4game/user_config_matrix/pc/torches.xml",
            "Better Torches v2/readme.txt",
        ]));
        assert_eq!(
            moves(&placed),
            [
                pair(
                    "Better Torches v2/DLC/dlcTorches/content/metadata.store",
                    "dlc/dlcTorches/content/metadata.store"
                ),
                pair(
                    "Better Torches v2/Mods/modBetterTorches/content/blob0.bundle",
                    "mods/modBetterTorches/content/blob0.bundle"
                ),
                pair(
                    "Better Torches v2/bin/config/r4game/user_config_matrix/pc/torches.xml",
                    "bin/config/r4game/user_config_matrix/pc/torches.xml"
                ),
            ]
        );
    }

    #[test]
    fn witcher3_bare_content_folders_get_a_home() {
        let placed = witcher3(&files(&[
            "modHDRework/content/texture.cache",
            "dlcArmor/content/blob0.bundle",
            "FastTravel/content/scripts/game/map.ws",
            "notes.txt",
        ]));
        assert_eq!(
            moves(&placed),
            [
                pair(
                    "FastTravel/content/scripts/game/map.ws",
                    "mods/modFastTravel/content/scripts/game/map.ws"
                ),
                pair(
                    "dlcArmor/content/blob0.bundle",
                    "dlc/dlcArmor/content/blob0.bundle"
                ),
                pair(
                    "modHDRework/content/texture.cache",
                    "mods/modHDRework/content/texture.cache"
                ),
            ]
        );
        assert!(witcher3(&files(&["readme.txt", "mods/readme.txt"])).is_empty());
        // A snippet beside the mod folders is kept, but not for the game folder.
        assert_eq!(
            moves(&witcher3(&files(&["Menu Mod/Menu.input.settings.txt"]))),
            [pair("Menu Mod/Menu.input.settings.txt", ".settings/Menu.input.settings.txt")]
        );
    }

    #[test]
    fn cyberpunk_starts_at_the_game_folders() {
        let placed = cyberpunk(&files(&[
            "Cool Jacket/archive/pc/mod/jacket.archive",
            "Cool Jacket/r6/tweaks/jacket.yaml",
            "Cool Jacket/Bin/x64/plugins/cyber_engine_tweaks/mods/jacket/init.lua",
            "Cool Jacket/README.md",
        ]));
        assert_eq!(
            moves(&placed),
            [
                pair(
                    "Cool Jacket/Bin/x64/plugins/cyber_engine_tweaks/mods/jacket/init.lua",
                    "bin/x64/plugins/cyber_engine_tweaks/mods/jacket/init.lua"
                ),
                pair(
                    "Cool Jacket/archive/pc/mod/jacket.archive",
                    "archive/pc/mod/jacket.archive"
                ),
                pair("Cool Jacket/r6/tweaks/jacket.yaml", "r6/tweaks/jacket.yaml"),
            ]
        );
    }

    #[test]
    fn cyberpunk_redmods_and_loose_archives() {
        let redmod = cyberpunk(&files(&[
            "NightCity/info.json",
            "NightCity/archives/night.archive",
            "NightCity/tweaks/night.tweak",
        ]));
        assert_eq!(
            moves(&redmod),
            [
                pair(
                    "NightCity/archives/night.archive",
                    "mods/NightCity/archives/night.archive"
                ),
                pair("NightCity/info.json", "mods/NightCity/info.json"),
                pair(
                    "NightCity/tweaks/night.tweak",
                    "mods/NightCity/tweaks/night.tweak"
                ),
            ]
        );
        let loose = cyberpunk(&files(&["hair.archive", "hair.archive.xl", "preview.png"]));
        assert_eq!(
            moves(&loose),
            [
                pair("hair.archive", "archive/pc/mod/hair.archive"),
                pair("hair.archive.xl", "archive/pc/mod/hair.archive.xl"),
            ]
        );
        assert!(cyberpunk(&files(&["readme.txt"])).is_empty());
    }

    #[test]
    fn stardew_installs_each_manifest_folder() {
        let placed = stardew(
            &files(&[
                "Pack 1.2/[CP] Farm/manifest.json",
                "Pack 1.2/[CP] Farm/assets/farm.png",
                "Pack 1.2/FarmCore/manifest.json",
                "Pack 1.2/FarmCore/FarmCore.dll",
                "Pack 1.2/FarmCore/i18n/locale/manifest.json",
                "Pack 1.2/how to install.txt",
            ]),
            "Pack",
        );
        assert_eq!(
            moves(&placed),
            [
                pair("Pack 1.2/FarmCore/FarmCore.dll", "FarmCore/FarmCore.dll"),
                pair(
                    "Pack 1.2/FarmCore/i18n/locale/manifest.json",
                    "FarmCore/i18n/locale/manifest.json"
                ),
                pair("Pack 1.2/FarmCore/manifest.json", "FarmCore/manifest.json"),
                pair(
                    "Pack 1.2/[CP] Farm/assets/farm.png",
                    "[CP] Farm/assets/farm.png"
                ),
                pair(
                    "Pack 1.2/[CP] Farm/manifest.json",
                    "[CP] Farm/manifest.json"
                ),
            ]
        );
        // A manifest at the top of the archive takes the mod's name.
        let flat = stardew(&files(&["manifest.json", "Mod.dll"]), "Lookup Anything");
        assert_eq!(
            moves(&flat),
            [
                pair("Mod.dll", "Lookup Anything/Mod.dll"),
                pair("manifest.json", "Lookup Anything/manifest.json"),
            ]
        );
        let xnb = stardew(&files(&["Retexture/Content/Animals/cat.xnb"]), "Retexture");
        assert_eq!(
            moves(&xnb),
            [pair(
                "Retexture/Content/Animals/cat.xnb",
                "[root] Content/Animals/cat.xnb"
            )]
        );
    }

    #[test]
    fn smapi_is_recognised_and_goes_next_to_the_game() {
        let download = files(&[
            "SMAPI 4.1 installer/install on Linux.sh",
            "SMAPI 4.1 installer/internal/linux/install.dat",
            "SMAPI 4.1 installer/internal/windows/install.dat",
        ]);
        assert_eq!(
            smapi_payload(&download).unwrap(),
            "SMAPI 4.1 installer/internal/linux/install.dat"
        );
        assert!(smapi_payload(&files(&["Mod/manifest.json"])).is_none());
        let placed = smapi(&files(&[
            "StardewModdingAPI",
            "StardewModdingAPI.dll",
            "Mods/ConsoleCommands/manifest.json",
            "smapi-internal/config.json",
        ]));
        assert_eq!(placed.len(), 4);
        assert!(placed.iter().all(|item| item.target == ROOT));
        assert_eq!(
            moves(&placed)[0].1,
            "[root] Mods/ConsoleCommands/manifest.json"
        );
    }
}
