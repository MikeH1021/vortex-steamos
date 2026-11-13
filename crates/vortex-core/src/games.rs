//! The games this app knows how to mod.
//!
//! Games are data, in three files under `games/`:
//!
//! - `core.json`: games with code of their own behind them. The Bethesda
//!   games with their plugin lists, and games whose install logic is a
//!   function in `layout.rs`.
//! - `ported.json`: games whose Vortex extension had install logic, ported
//!   by hand into install rules. `cases.json` holds example archives for them.
//! - `generated.json`: written by `scripts/import-vortex-games.cjs` from an
//!   upstream Vortex checkout. Games that only need a mod folder.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::plugins::PluginFormat;

/// The folder a path starts from.
///
/// The Windows folders are inside the game's Proton prefix. A game running its
/// own Linux build has no prefix, and those folders do not apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    /// The game's install folder.
    Game,
    /// `Documents` of the Windows user.
    Documents,
    /// `AppData/Local`.
    AppdataLocal,
    /// `AppData/Roaming`.
    AppdataRoaming,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModPath {
    pub base: Base,
    /// Relative to `base`. Empty for the base folder itself.
    #[serde(default)]
    pub path: String,
}

/// How an archive is turned into files for this game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Unpack the archive into the mod folder as it is.
    #[default]
    Generic,
    /// `Data` layout, wrapper folders, plugin lists.
    Bethesda,
    Witcher3,
    Cyberpunk2077,
    Stardew,
}

/// When a rule applies. Every part that is given must be found in the archive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// A file with one of these names, anywhere.
    #[serde(default)]
    pub has_file: Vec<String>,
    /// A file whose name ends with one of these.
    #[serde(default)]
    pub has_ext: Vec<String>,
    /// A folder with one of these names, anywhere.
    #[serde(default)]
    pub has_dir: Vec<String>,
    /// No file whose name ends with one of these.
    #[serde(default)]
    pub no_ext: Vec<String>,
}

/// One way of laying an archive out for a game. A game lists several, and the
/// first one that finds something to install is used. An archive none of them
/// fits is unpacked into the mod folder as it is.
///
/// Names and extensions are matched without regard to case. `to` is a folder
/// inside the target, and `target` names one of the game's `targets`, or is
/// empty for the mod folder.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rule {
    /// A folder holding a marker file is one mod. It is installed under its
    /// own name, and an archive can hold several. Files outside are left out.
    Marker {
        /// Exact file names that mark a mod, such as `ModInfo.xml`.
        #[serde(default)]
        file: Vec<String>,
        /// Or endings, such as `.mod`.
        #[serde(default)]
        ext: Vec<String>,
        /// The mod's folder is this many levels above the marker's folder.
        #[serde(default)]
        up: usize,
        /// False to install the folder's contents without the folder itself.
        #[serde(default = "yes")]
        keep_folder: bool,
        #[serde(default)]
        to: String,
        #[serde(default)]
        target: String,
        #[serde(default)]
        when: When,
    },
    /// The archive is cut at the first level where one of these folders
    /// appears, and only those folders are installed.
    Anchor {
        dirs: Vec<String>,
        /// False to install what is inside the folder, without the folder.
        #[serde(default = "yes")]
        keep: bool,
        #[serde(default)]
        to: String,
        #[serde(default)]
        target: String,
        #[serde(default)]
        when: When,
    },
    /// Files with these endings are installed side by side, wherever they
    /// were in the archive. Everything else is left out.
    Flat {
        ext: Vec<String>,
        #[serde(default)]
        to: String,
        #[serde(default)]
        target: String,
        #[serde(default)]
        when: When,
    },
    /// The archive is refused with `message`. Needs a `when`. For archives
    /// that are a tool to run, not files to install.
    Reject { message: String, when: When },
    /// Everything is installed as it is.
    AsIs {
        /// True to drop a single folder that wraps the whole archive.
        #[serde(default)]
        strip_wrapper: bool,
        #[serde(default)]
        to: String,
        #[serde(default)]
        target: String,
        #[serde(default)]
        when: When,
    },
}

/// `plugins.txt` handling for the Bethesda games.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginRules {
    pub format: PluginFormat,
    /// Folder under `AppData/Local` and under `Documents/My Games`.
    pub app_dir: String,
    pub native: Vec<String>,
    /// Creation Club list next to the executable. Its entries are native too.
    #[serde(default)]
    pub ccc: Option<String>,
    pub extensions: Vec<String>,
}

/// A script extender or mod loader the game's mods commonly need.
#[derive(Debug, Clone, Deserialize)]
pub struct Extender {
    pub name: String,
    /// File in the game folder that shows it is installed.
    pub loader: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameDef {
    /// Id used on the command line and in the staging folder name.
    pub id: String,
    pub name: String,
    /// Steam app ids that install this game. The first one is the usual one.
    pub steam: Vec<u32>,
    /// The Windows program, relative to the install folder.
    pub executable: String,
    /// Other places the Windows program can be, for editions that moved it.
    #[serde(default)]
    pub other_executables: Vec<String>,
    /// The program of the game's own Linux build, when it has one.
    #[serde(default)]
    pub native_executable: Option<String>,
    /// Where mod files go.
    pub mod_path: ModPath,
    pub nexus_domain: String,
    /// False when every mod gets a folder of its own inside the mod folder.
    #[serde(default = "yes")]
    pub merge_mods: bool,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub plugins: Option<PluginRules>,
    #[serde(default)]
    pub extender: Option<Extender>,
    /// Other folders a mod can put files in, by name. An installer chooses them.
    #[serde(default)]
    pub targets: BTreeMap<String, ModPath>,
    /// How archives are laid out, for games of the generic kind.
    #[serde(default)]
    pub install: Vec<Rule>,
    /// Something the player has to know or do that the app cannot do for them.
    #[serde(default)]
    pub notes: String,
    /// Settings files of the game that mods bring lines for. A mod ships a
    /// snippet named after the file, and its lines are merged in.
    #[serde(default)]
    pub settings: Vec<SettingsFile>,
}

/// One settings file mods add lines to, such as The Witcher 3's
/// `input.settings` with its key bindings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SettingsFile {
    /// The file's name. `MyMod.input.settings` is a snippet for `input.settings`.
    pub name: String,
    pub path: ModPath,
}

fn yes() -> bool {
    true
}

impl GameDef {
    /// Whether any of this game's folders live in the Proton prefix.
    pub fn uses_prefix(&self) -> bool {
        self.plugins.is_some()
            || self.mod_path.base != Base::Game
            || self.targets.values().any(|path| path.base != Base::Game)
    }
}

/// Every game, hand-written ones first.
pub fn all_games() -> &'static [GameDef] {
    static GAMES: OnceLock<Vec<GameDef>> = OnceLock::new();
    GAMES.get_or_init(|| {
        let mut games: Vec<GameDef> = Vec::new();
        // Earlier files win when two define the same game.
        for (file, text) in [
            ("core.json", include_str!("../games/core.json")),
            ("ported.json", include_str!("../games/ported.json")),
            ("generated.json", include_str!("../games/generated.json")),
        ] {
            let more: Vec<GameDef> = serde_json::from_str(text)
                .unwrap_or_else(|err| panic!("games/{file} is not valid: {err}"));
            for game in more {
                if !games.iter().any(|known| known.id == game.id) {
                    games.push(game);
                }
            }
        }
        games
    })
}

/// Look a game up by its id. `skyrim-se` is accepted for `skyrimse`.
pub fn game_by_id(id: &str) -> Option<&'static GameDef> {
    let wanted = id.trim().to_ascii_lowercase();
    let squeezed = wanted.replace('-', "");
    all_games()
        .iter()
        .find(|game| game.id == wanted)
        .or_else(|| all_games().iter().find(|game| game.id == squeezed))
}

/// Ids joined for an error message.
pub fn game_ids() -> String {
    all_games()
        .iter()
        .map(|game| game.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Base masters plus any Creation Club names listed in the game install.
pub fn native_plugin_names(game: &GameDef, install_path: Option<&Path>) -> Vec<String> {
    let Some(rules) = &game.plugins else {
        return Vec::new();
    };
    let mut names = rules.native.clone();
    let Some(install_path) = install_path else {
        return names;
    };
    let Some(file_name) = &rules.ccc else {
        return names;
    };
    let Some(list) = crate::deploy::resolve_case(install_path, Path::new(file_name)) else {
        return names;
    };
    let Ok(text) = std::fs::read_to_string(list) else {
        return names;
    };
    for line in text.lines() {
        let name = line.trim();
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        if !names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_are_the_steam_ids() {
        assert_eq!(game_by_id("skyrim").unwrap().steam, [72850]);
        assert_eq!(game_by_id("skyrimse").unwrap().steam, [489830]);
        assert_eq!(game_by_id("skyrim-se").unwrap().id, "skyrimse");
        assert_eq!(game_by_id("fallout4").unwrap().steam, [377160]);
        assert_eq!(game_by_id("witcher3").unwrap().kind, Kind::Witcher3);
        assert!(game_by_id("tetris").is_none());
    }

    #[test]
    fn every_definition_is_usable() {
        let games = all_games();
        assert!(games.len() > 20, "only {} games loaded", games.len());
        for (index, game) in games.iter().enumerate() {
            assert!(!game.steam.is_empty(), "{} has no Steam id", game.id);
            assert!(!game.executable.is_empty(), "{} has no program", game.id);
            for path in std::iter::once(&game.mod_path).chain(game.targets.values()) {
                let safe =
                    path.path.is_empty() || crate::deploy::safe_relative(&path.path).is_some();
                assert!(safe, "{} has an unsafe folder: {}", game.id, path.path);
            }
            for rule in &game.install {
                let (to, target) = match rule {
                    Rule::Reject { message, when } => {
                        assert!(!message.is_empty() && *when != When::default());
                        continue;
                    }
                    Rule::Marker { to, target, .. }
                    | Rule::Anchor { to, target, .. }
                    | Rule::Flat { to, target, .. }
                    | Rule::AsIs { to, target, .. } => (to, target),
                };
                assert!(
                    to.is_empty() || crate::deploy::safe_relative(to).is_some(),
                    "{} has a rule with an unsafe folder: {to}",
                    game.id
                );
                assert!(
                    target.is_empty() || game.targets.contains_key(target),
                    "{} has a rule for a target it does not define: {target}",
                    game.id
                );
            }
            assert_eq!(
                game.plugins.is_some(),
                game.kind == Kind::Bethesda,
                "{}: plugin lists and the Bethesda layout go together",
                game.id
            );
            for other in &games[index + 1..] {
                assert_ne!(game.id, other.id);
            }
        }
    }

    #[test]
    fn only_prefix_games_ask_for_a_prefix() {
        assert!(game_by_id("skyrimse").unwrap().uses_prefix());
        assert!(!game_by_id("cyberpunk2077").unwrap().uses_prefix());
        assert!(!game_by_id("stardewvalley").unwrap().uses_prefix());
    }
}
