//! Proton prefix paths for a Windows game installed by Steam on Linux.
//!
//! The prefix appears after the game has launched once. Plugin files live in
//! the prefix, not next to the executable.
//!
//! Skyrim Special Edition reads
//! `AppData/Local/Skyrim Special Edition/plugins.txt`.
//! Classic Skyrim reads `AppData/Local/Skyrim/plugins.txt`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::games::{Base, GameDef};
use crate::vdf::{self, ParseError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtonLayout {
    pub compatdata: PathBuf,
    pub prefix: PathBuf,
    /// The Windows user's profile folder inside the prefix.
    pub user: PathBuf,
    /// Plugin list files, for games that have them.
    pub plugins_txt: Option<PathBuf>,
    pub loadorder_txt: Option<PathBuf>,
    pub my_games: Option<PathBuf>,
    pub prefix_exists: bool,
    pub tool: Option<String>,
}

impl ProtonLayout {
    /// The Windows folder `base` names, inside the prefix. `None` for the
    /// game's own folder, which is not part of the prefix.
    pub fn base_dir(&self, base: Base) -> Option<PathBuf> {
        let relative = match base {
            Base::Game => return None,
            Base::Documents => Path::new("Documents").to_path_buf(),
            Base::AppdataLocal => Path::new("AppData").join("Local"),
            Base::AppdataRoaming => Path::new("AppData").join("Roaming"),
        };
        Some(existing_case(&self.user, &relative))
    }
}

/// Paths inside the prefix for `app_id`.
///
/// Windows does not care about letter case and Linux does. The game writes
/// `Plugins.txt`, older tools write `plugins.txt`. Whatever is already on disk
/// is the name that is used, so a second file never appears next to the first.
pub fn proton_layout(
    library: &Path,
    game: &GameDef,
    app_id: u32,
    tool: Option<String>,
) -> ProtonLayout {
    let compatdata = library
        .join("steamapps")
        .join("compatdata")
        .join(app_id.to_string());
    let prefix = compatdata.join("pfx");
    let user = prefix.join("drive_c").join("users").join("steamuser");
    let app_data = game.plugins.as_ref().map(|rules| {
        existing_case(
            &user,
            &Path::new("AppData").join("Local").join(&rules.app_dir),
        )
    });
    ProtonLayout {
        plugins_txt: app_data
            .as_ref()
            .map(|dir| existing_case(dir, Path::new("Plugins.txt"))),
        loadorder_txt: app_data
            .as_ref()
            .map(|dir| existing_case(dir, Path::new("loadorder.txt"))),
        my_games: game.plugins.as_ref().map(|rules| {
            existing_case(
                &user,
                &Path::new("Documents").join("My Games").join(&rules.app_dir),
            )
        }),
        prefix_exists: prefix.is_dir(),
        compatdata,
        prefix,
        user,
        tool,
    }
}

/// Follow `relative` under `root`, matching each part without regard to case
/// while it exists. Parts that are not on disk yet keep the spelling given.
pub(crate) fn existing_case(root: &Path, relative: &Path) -> PathBuf {
    let mut current = root.to_path_buf();
    let mut on_disk = current.is_dir();
    for component in relative.components() {
        let std::path::Component::Normal(wanted) = component else {
            continue;
        };
        if on_disk {
            if let Some(found) = crate::deploy::resolve_case(&current, Path::new(wanted)) {
                current = found;
                on_disk = current.is_dir();
                continue;
            }
            on_disk = false;
        }
        current.push(wanted);
    }
    current
}

/// Read `CompatToolMapping/<appid>/name` from Steam's `config.vdf`.
pub fn compat_tool_name(config_vdf: &str, app_id: u32) -> Result<Option<String>, ParseError> {
    let doc = vdf::parse(config_vdf)?;
    let app_id = app_id.to_string();
    let name = doc
        .root
        .dig(&[
            "Software",
            "Valve",
            "Steam",
            "CompatToolMapping",
            &app_id,
            "name",
        ])
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::game_by_id;

    #[test]
    fn se_plugins_live_in_the_proton_user_profile() {
        let layout = proton_layout(
            Path::new("/steam"),
            game_by_id("skyrimse").unwrap(),
            489830,
            None,
        );
        assert_eq!(
            layout.plugins_txt.unwrap(),
            Path::new("/steam/steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition/Plugins.txt")
        );
        assert_eq!(
            layout.my_games.unwrap(),
            Path::new("/steam/steamapps/compatdata/489830/pfx/drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition")
        );
    }

    #[test]
    fn classic_skyrim_uses_its_own_appdata_folder() {
        let layout = proton_layout(
            Path::new("/steam"),
            game_by_id("skyrim").unwrap(),
            72850,
            None,
        );
        assert!(layout.plugins_txt.unwrap().ends_with(
            "compatdata/72850/pfx/drive_c/users/steamuser/AppData/Local/Skyrim/Plugins.txt"
        ));
    }

    #[test]
    fn an_existing_lowercase_list_is_reused() {
        let root = tempfile::tempdir().unwrap();
        let app_data = root.path().join(
            "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/skyrim special edition",
        );
        std::fs::create_dir_all(&app_data).unwrap();
        std::fs::write(app_data.join("plugins.txt"), "").unwrap();
        let layout = proton_layout(root.path(), game_by_id("skyrimse").unwrap(), 489830, None);
        assert!(layout.prefix_exists);
        assert_eq!(
            layout.plugins_txt.clone().unwrap(),
            app_data.join("plugins.txt")
        );
        assert_eq!(
            layout.loadorder_txt.clone().unwrap(),
            app_data.join("loadorder.txt")
        );
        assert_eq!(
            layout.base_dir(Base::Documents).unwrap(),
            layout.user.join("Documents")
        );
        let plain = proton_layout(
            root.path(),
            game_by_id("cyberpunk2077").unwrap(),
            1091500,
            None,
        );
        assert!(plain.plugins_txt.is_none());
    }

    #[test]
    fn reads_the_configured_proton_tool() {
        let text = r#"
            "InstallConfigStore"
            {
                "Software"
                {
                    "Valve"
                    {
                        "Steam"
                        {
                            "CompatToolMapping"
                            {
                                "489830"
                                {
                                    "name" "proton_experimental"
                                    "config" ""
                                }
                            }
                        }
                    }
                }
            }
        "#;
        assert_eq!(
            compat_tool_name(text, 489830).unwrap().as_deref(),
            Some("proton_experimental")
        );
        assert_eq!(compat_tool_name(text, 72850).unwrap(), None);
    }
}
