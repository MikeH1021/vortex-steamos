//! Find supported games in local Steam libraries.
//!
//! Extra libraries (a Deck microSD, a Bazzite second drive) come from
//! `steamapps/libraryfolders.vdf`. The game is installed when its app manifest
//! and executable are both present.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::deploy::{self, DeployRoots, LinkMethod};
use crate::games::{self, all_games, Base, GameDef, Kind, ModPath};
use crate::plugins::PluginFormat;
use crate::proton::{self, ProtonLayout};
use crate::vdf::{self, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ScriptExtender {
    pub name: String,
    pub loader: String,
    pub installed: bool,
}

/// Pictures of a game that Steam has already downloaded for its library.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GameArt {
    /// The tall cover, 600 by 900.
    pub capsule: Option<PathBuf>,
    /// The wide backdrop.
    pub hero: Option<PathBuf>,
    /// The small wide banner.
    pub header: Option<PathBuf>,
}

/// Look in Steam's library cache. Its layout has changed over the years:
/// `<appid>_library_600x900.jpg` at first, later a folder per app, and now
/// often one more folder with a hashed name inside that.
fn find_art(steam_root: &Path, app_id: u32) -> GameArt {
    let cache = steam_root.join("appcache").join("librarycache");
    let find = |names: &[&str]| -> Option<PathBuf> {
        for name in names {
            let flat = cache.join(format!("{app_id}_{name}"));
            if flat.is_file() {
                return Some(flat);
            }
        }
        let folder = cache.join(app_id.to_string());
        let mut folders = vec![folder.clone()];
        if let Ok(entries) = std::fs::read_dir(&folder) {
            folders.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir()),
            );
        }
        for name in names {
            for dir in &folders {
                let path = dir.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        None
    };
    GameArt {
        capsule: find(&[
            "library_600x900.jpg",
            "library_capsule.jpg",
            "library_600x900.png",
        ]),
        hero: find(&["library_hero.jpg", "library_hero.png"]),
        header: find(&["header.jpg", "library_header.jpg"]),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledGame {
    pub id: String,
    pub name: String,
    pub steam_app_id: u32,
    pub nexus_domain: String,
    pub kind: Kind,
    /// True when Steam installed the game's own Linux build, not the Windows one.
    pub native: bool,
    pub plugin_format: Option<PluginFormat>,
    pub library_path: PathBuf,
    pub install_path: PathBuf,
    pub exe_path: PathBuf,
    /// The folder mods are linked into.
    pub data_path: PathBuf,
    /// Other folders an installer can put files in, by name.
    pub targets: BTreeMap<String, PathBuf>,
    pub merge_mods: bool,
    pub staging_path: PathBuf,
    pub link_method: Option<LinkMethod>,
    pub native_plugins: Vec<String>,
    pub proton: ProtonLayout,
    /// True when the game must be started once before it can be modded,
    /// because its mod folder is inside a Proton prefix that is not there yet.
    pub needs_first_launch: bool,
    pub script_extender: Option<ScriptExtender>,
    pub notes: String,
    /// Settings files mods add lines to, by name, where they are on disk.
    pub settings: BTreeMap<String, PathBuf>,
    pub art: GameArt,
}

impl InstalledGame {
    pub fn deploy_roots(&self) -> DeployRoots {
        DeployRoots {
            main: self.data_path.clone(),
            targets: self.targets.clone(),
            merge_mods: self.merge_mods,
            // The Bethesda games keep their masters in the mod folder, and
            // nothing should ever stand in for those.
            replace_existing: self.kind != Kind::Bethesda,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoverReport {
    /// Steam client locations we considered, whether or not they exist.
    pub candidates: Vec<String>,
    /// Candidates that contain a `steamapps` directory.
    pub roots: Vec<String>,
    pub games: Vec<InstalledGame>,
    pub warnings: Vec<String>,
}

pub fn steam_candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".steam/debian-installation"),
        home.join("snap/steam/common/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("Library/Application Support/Steam"),
    ]
}

pub fn default_steam_candidates() -> Vec<PathBuf> {
    match std::env::var("HOME") {
        Ok(home) => steam_candidates(Path::new(&home)),
        Err(_) => Vec::new(),
    }
}

pub fn discover() -> DiscoverReport {
    if let Ok(root) = std::env::var("VORTEX_STEAM_ROOT") {
        if !root.is_empty() {
            return discover_roots(&[PathBuf::from(root)]);
        }
    }
    discover_roots(&default_steam_candidates())
}

pub fn discover_roots(candidates: &[PathBuf]) -> DiscoverReport {
    let mut warnings = Vec::new();
    let roots = existing_roots(candidates);
    let mut games = Vec::new();
    let mut seen_installs = HashSet::new();
    for root in &roots {
        let (found, mut root_warnings) = discover_in_root(root);
        warnings.append(&mut root_warnings);
        for game in found {
            if seen_installs.insert(game.install_path.clone()) {
                games.push(game);
            }
        }
    }
    DiscoverReport {
        candidates: display_paths(candidates),
        roots: display_paths(&roots),
        games,
        warnings,
    }
}

pub fn discover_in_root(steam_root: &Path) -> (Vec<InstalledGame>, Vec<String>) {
    let mut warnings = Vec::new();
    let libraries = libraries_of(steam_root, &mut warnings);
    let tool_config = std::fs::read_to_string(steam_root.join("config/config.vdf")).ok();
    let mut games = Vec::new();
    let mounted: Vec<PathBuf> = libraries
        .iter()
        .filter(|library| library.is_dir())
        .cloned()
        .collect();
    for library in libraries {
        if !library.is_dir() {
            warnings.push(format!(
                "Steam library is not mounted: {}",
                library.display()
            ));
            continue;
        }
        for def in all_games() {
            for app_id in &def.steam {
                match find_game(
                    steam_root,
                    &library,
                    &mounted,
                    def,
                    *app_id,
                    tool_config.as_deref(),
                ) {
                    Ok(Some(game)) => {
                        games.push(game);
                        break;
                    }
                    Ok(None) => {}
                    Err(err) => warnings.push(err),
                }
            }
        }
    }
    (games, warnings)
}

fn existing_roots(candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for candidate in candidates {
        if !candidate.join("steamapps").is_dir() {
            continue;
        }
        let key = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if seen.insert(key.clone()) {
            roots.push(key);
        }
    }
    roots
}

fn libraries_of(steam_root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut libraries = vec![steam_root.to_path_buf()];
    for relative in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        let path = steam_root.join(relative);
        match std::fs::read_to_string(&path) {
            Ok(text) => match library_paths(&text) {
                Ok(found) => libraries.extend(found),
                Err(err) => warnings.push(format!("{}: {err}", path.display())),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
    dedup_paths(libraries)
}

fn library_paths(text: &str) -> Result<Vec<PathBuf>, vdf::ParseError> {
    let doc = vdf::parse(text)?;
    let Value::Object(entries) = &doc.root else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for (key, value) in entries {
        match value {
            Value::String(path) if is_index(key) => push_library(&mut paths, path),
            Value::Object(_) => {
                if let Some(path) = value.get_ignore_ascii_case("path").and_then(Value::as_str) {
                    push_library(&mut paths, path);
                }
            }
            _ => {}
        }
    }
    Ok(paths)
}

fn is_index(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|ch| ch.is_ascii_digit())
}

fn push_library(paths: &mut Vec<PathBuf>, path: &str) {
    if !path.is_empty() {
        paths.push(PathBuf::from(path));
    }
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

struct Manifest {
    app_id: u32,
    install_dir: String,
    state_flags: u32,
}

fn parse_manifest(text: &str) -> Result<Manifest, vdf::ParseError> {
    let doc = vdf::parse(text)?;
    let app_id = required_string(&doc.root, "appid")?;
    let install_dir = required_string(&doc.root, "installdir")?;
    let state_flags = doc
        .root
        .get_ignore_ascii_case("StateFlags")
        .and_then(Value::as_str)
        .unwrap_or("0");
    let app_id = app_id.parse::<u32>().map_err(|_| vdf::ParseError {
        message: format!("appid '{app_id}' is not a number"),
        line: 1,
        column: 1,
    })?;
    let state_flags = state_flags.parse::<u32>().unwrap_or(0);
    Ok(Manifest {
        app_id,
        install_dir,
        state_flags,
    })
}

fn required_string(root: &Value, key: &str) -> Result<String, vdf::ParseError> {
    root.get_ignore_ascii_case(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| vdf::ParseError {
            message: format!("manifest has no {key}"),
            line: 1,
            column: 1,
        })
}

fn safe_install_dir(name: &str) -> Option<&str> {
    let unsafe_path = name.is_empty()
        || name.contains("..")
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':');
    if unsafe_path {
        None
    } else {
        Some(name)
    }
}

fn find_game(
    steam_root: &Path,
    library: &Path,
    libraries: &[PathBuf],
    game: &GameDef,
    app_id: u32,
    config_vdf: Option<&str>,
) -> Result<Option<InstalledGame>, String> {
    let manifest_path = library
        .join("steamapps")
        .join(format!("appmanifest_{app_id}.acf"));
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", manifest_path.display())),
    };
    let manifest =
        parse_manifest(&text).map_err(|err| format!("{}: {err}", manifest_path.display()))?;
    if manifest.app_id != app_id {
        return Err(format!(
            "{} declares app id {}, expected {app_id}",
            manifest_path.display(),
            manifest.app_id,
        ));
    }
    let Some(install_dir) = safe_install_dir(&manifest.install_dir) else {
        return Err(format!(
            "{} has an install dir we will not follow: {}",
            manifest_path.display(),
            manifest.install_dir
        ));
    };
    let install_path = library.join("steamapps").join("common").join(install_dir);
    let find = |relative: &str| {
        deploy::resolve_case(&install_path, Path::new(relative)).filter(|path| path.is_file())
    };
    // A Windows build wins when both are there: that is what Proton runs.
    let windows_exe = find(&game.executable)
        .or_else(|| game.other_executables.iter().find_map(|path| find(path)));
    let native_exe = game.native_executable.as_deref().and_then(find);
    let native = windows_exe.is_none() && native_exe.is_some();
    let Some(exe_path) = windows_exe.or(native_exe) else {
        if manifest.state_flags & 4 == 0 {
            return Ok(None);
        }
        // Steam installs a game's Linux build when it has one. A game whose
        // Linux build is not described here ends up in this branch.
        return Err(format!(
            "{} is installed, but its Windows program {} is not there. If Steam installed a Linux build, force Proton for the game under Properties, Compatibility to mod it.",
            game.name, game.executable
        ));
    };
    if native && game.uses_prefix() {
        return Err(format!(
            "{} is installed as its Linux build, which keeps mods somewhere this app does not know. To mod it, force Proton for the game in Steam under Properties, Compatibility.",
            game.name
        ));
    }

    let tool = config_vdf.and_then(|text| proton::compat_tool_name(text, app_id).ok().flatten());
    let proton = proton::proton_layout(
        prefix_library(library, libraries, app_id),
        game,
        app_id,
        tool,
    );
    let place = |path: &ModPath| {
        let base = proton
            .base_dir(path.base)
            .unwrap_or_else(|| install_path.clone());
        proton::existing_case(&base, Path::new(&path.path))
    };
    let data_path = place(&game.mod_path);
    let targets: BTreeMap<String, PathBuf> = game
        .targets
        .iter()
        .map(|(name, path)| (name.clone(), place(path)))
        .collect();
    let settings: BTreeMap<String, PathBuf> = game
        .settings
        .iter()
        .map(|file| (file.name.clone(), place(&file.path)))
        .collect();
    // Mods are staged in the library the game was in when the first one was
    // installed. A game Steam has since moved to another library keeps its
    // mods where they are, and they are linked across from there.
    let own_staging = deploy::staging_dir(library, &game.id);
    let staging_path = if own_staging.join("mods.json").is_file() {
        own_staging
    } else {
        libraries
            .iter()
            .map(|other| deploy::staging_dir(other, &game.id))
            .find(|other| other.join("mods.json").is_file())
            .unwrap_or(own_staging)
    };
    let in_prefix = game.mod_path.base != Base::Game
        || game.targets.values().any(|path| path.base != Base::Game);
    Ok(Some(InstalledGame {
        id: game.id.clone(),
        name: game.name.clone(),
        steam_app_id: app_id,
        nexus_domain: game.nexus_domain.clone(),
        kind: game.kind,
        native,
        plugin_format: game.plugins.as_ref().map(|rules| rules.format),
        script_extender: game.extender.as_ref().map(|extender| ScriptExtender {
            name: extender.name.clone(),
            loader: extender.loader.clone(),
            installed: find(&extender.loader).is_some(),
        }),
        native_plugins: games::native_plugin_names(game, Some(&install_path)),
        link_method: deploy::link_method_for_paths(library, &install_path).ok(),
        needs_first_launch: in_prefix && !proton.prefix_exists,
        notes: game.notes.clone(),
        settings,
        art: find_art(steam_root, app_id),
        merge_mods: game.merge_mods,
        proton,
        library_path: library.to_path_buf(),
        install_path,
        exe_path,
        data_path,
        targets,
        staging_path,
    }))
}

/// Steam normally keeps the prefix next to the game. Older installs keep every
/// prefix in the first library, so look there when the game's own has none.
fn prefix_library<'a>(library: &'a Path, libraries: &'a [PathBuf], app_id: u32) -> &'a Path {
    let has_prefix = |root: &Path| {
        root.join("steamapps")
            .join("compatdata")
            .join(app_id.to_string())
            .join("pfx")
            .is_dir()
    };
    if has_prefix(library) {
        return library;
    }
    libraries
        .iter()
        .map(PathBuf::as_path)
        .find(|other| has_prefix(other))
        .unwrap_or(library)
}

fn display_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn legacy_libraryfolders_uses_indexed_path_strings() {
        let text = r#"
            "LibraryFolders"
            {
                "TimeNextStatsReport" "123"
                "ContentStatsID" "abc"
                "1" "/run/media/mmcblk0p1/SteamLibrary"
            }
        "#;
        let paths = library_paths(text).unwrap();
        assert_eq!(
            paths,
            vec![PathBuf::from("/run/media/mmcblk0p1/SteamLibrary")]
        );
    }

    #[test]
    fn finds_both_games_and_the_proton_prefix() {
        let temp =
            TempDir(std::env::temp_dir().join(format!("vortex-steam-{}", std::process::id())));
        let steam = temp.0.join("Steam");
        let sd = temp.0.join("sd");
        let skyrim = steam.join("steamapps/common/Skyrim");
        let skyrim_se = sd.join("steamapps/common/Skyrim Special Edition");
        fs::create_dir_all(steam.join("config")).unwrap();
        fs::create_dir_all(skyrim.join("Data")).unwrap();
        fs::create_dir_all(skyrim_se.join("Data")).unwrap();
        fs::create_dir_all(
            sd.join("steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition"),
        )
        .unwrap();
        fs::write(skyrim.join("TESV.exe"), b"").unwrap();
        fs::write(skyrim_se.join("SkyrimSE.exe"), b"").unwrap();
        fs::write(skyrim_se.join("skse64_loader.exe"), b"").unwrap();
        fs::write(skyrim_se.join("Skyrim.ccc"), "ccExtra.esl\n").unwrap();
        fs::write(
            steam.join("steamapps/appmanifest_72850.acf"),
            manifest("72850", "Skyrim", "4"),
        )
        .unwrap();
        fs::write(
            sd.join("steamapps/appmanifest_489830.acf"),
            manifest("489830", "Skyrim Special Edition", "4"),
        )
        .unwrap();
        let sd_text = sd.display().to_string();
        let steam_text = steam.display().to_string();
        fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                r#""libraryfolders"
{{
    "0"
    {{
        "path" "{steam_text}"
    }}
    "1"
    {{
        "path" "{sd_text}"
    }}
    "2"
    {{
        "path" "/this/does/not/exist/vortex-test-library"
    }}
}}
"#
            ),
        )
        .unwrap();
        fs::write(
            steam.join("config/config.vdf"),
            r#""InstallConfigStore"
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
                    }
                }
            }
        }
    }
}
"#,
        )
        .unwrap();

        let report = discover_roots(std::slice::from_ref(&steam));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("not mounted")),
            "warnings: {:?}",
            report.warnings
        );
        assert_eq!(report.games.len(), 2, "warnings: {:?}", report.warnings);

        let classic = report
            .games
            .iter()
            .find(|game| game.id == "skyrim")
            .unwrap();
        assert!(!classic.proton.prefix_exists);
        let extender = classic.script_extender.as_ref().unwrap();
        assert!(extender.loader == "skse_loader.exe");
        assert!(!extender.installed);
        assert_eq!(classic.link_method, Some(LinkMethod::Hardlink));

        let special = report
            .games
            .iter()
            .find(|game| game.id == "skyrimse")
            .unwrap();
        assert!(special.proton.prefix_exists);
        assert_eq!(special.proton.tool.as_deref(), Some("proton_experimental"));
        assert!(special.script_extender.as_ref().unwrap().installed);
        assert!(!special.native && !special.needs_first_launch);
        assert!(special.art.capsule.is_none());
        assert!(special
            .native_plugins
            .iter()
            .any(|name| name == "ccExtra.esl"));
        assert!(special.staging_path.ends_with("steamapps/vortex/skyrimse"));
        assert!(special
            .proton
            .plugins_txt
            .as_ref()
            .unwrap()
            .ends_with("AppData/Local/Skyrim Special Edition/Plugins.txt"));
    }

    #[test]
    fn linux_builds_prefix_folders_and_nested_programs_are_understood() {
        let root = tempfile::tempdir().unwrap();
        let steam = root.path().join("Steam");
        let apps = steam.join("steamapps");
        let add = |app_id: &str, dir: &str, program: &str| {
            let exe = apps.join("common").join(dir).join(program);
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(exe, b"").unwrap();
            fs::write(
                apps.join(format!("appmanifest_{app_id}.acf")),
                manifest(app_id, dir, "4"),
            )
            .unwrap();
        };
        // Stardew Valley ships a Linux build with a differently named program.
        add("413150", "Stardew Valley", "StardewValley");
        // Cyberpunk keeps its program two folders down and mods in the game folder.
        add("1091500", "Cyberpunk 2077", "bin/x64/Cyberpunk2077.exe");
        // BattleTech keeps mods in Documents, which is inside the prefix.
        add("637090", "BATTLETECH", "BattleTech.exe");
        // Grimrock's Linux build has no prefix for its Documents folder.
        add("207170", "Legend of Grimrock", "Grimrock.bin.x86");

        // Steam keeps cover art in two layouts, depending on its age.
        let cache = steam.join("appcache/librarycache");
        fs::create_dir_all(cache.join("413150/abc123")).unwrap();
        fs::write(cache.join("413150/abc123/library_600x900.jpg"), b"jpg").unwrap();
        fs::write(cache.join("413150/library_hero.jpg"), b"jpg").unwrap();
        fs::write(cache.join("1091500_library_600x900.jpg"), b"jpg").unwrap();

        let report = discover_roots(std::slice::from_ref(&steam));
        let game = |id: &str| report.games.iter().find(|game| game.id == id);
        let art = &game("stardewvalley").unwrap().art;
        assert!(art
            .capsule
            .as_ref()
            .unwrap()
            .ends_with("abc123/library_600x900.jpg"));
        assert!(art
            .hero
            .as_ref()
            .unwrap()
            .ends_with("413150/library_hero.jpg"));
        assert!(art.header.is_none());
        assert!(game("cyberpunk2077").unwrap().art.capsule.is_some());

        let stardew = game("stardewvalley").unwrap();
        assert!(stardew.native);
        assert!(!stardew.needs_first_launch);
        assert!(stardew.data_path.ends_with("Stardew Valley/Mods"));
        assert!(stardew.targets["root"].ends_with("common/Stardew Valley"));
        assert!(stardew.proton.plugins_txt.is_none());

        let cyberpunk = game("cyberpunk2077").unwrap();
        assert!(!cyberpunk.native);
        assert_eq!(cyberpunk.data_path, cyberpunk.install_path);

        let battletech = game("battletech").unwrap();
        assert!(battletech.needs_first_launch);
        assert!(battletech.data_path.ends_with(
            "compatdata/637090/pfx/drive_c/users/steamuser/Documents/My Games/BattleTech/mods"
        ));

        assert!(game("grimrock").is_none());
        assert!(
            report
                .warnings
                .iter()
                .any(|text| text.contains("force Proton")),
            "{:?}",
            report.warnings
        );
    }

    fn manifest(app_id: &str, install_dir: &str, flags: &str) -> String {
        format!(
            r#""AppState"
{{
    "appid" "{app_id}"
    "installdir" "{install_dir}"
    "StateFlags" "{flags}"
}}
"#
        )
    }
}
