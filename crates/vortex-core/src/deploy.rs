//! Stage mods and link them into a game's `Data` folder.
//!
//! Hardlinks are used when the staging folder and the game are on the same
//! filesystem. A file that is already in `Data` and was not put there by this
//! app is left alone, which keeps the base game masters intact.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkMethod {
    Hardlink,
    Symlink,
}

#[derive(Debug)]
pub struct ModError {
    message: String,
}

impl ModError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ModError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ModError {}

impl From<io::Error> for ModError {
    fn from(err: io::Error) -> Self {
        Self::new(err.to_string())
    }
}

impl From<serde_json::Error> for ModError {
    fn from(err: serde_json::Error) -> Self {
        Self::new(err.to_string())
    }
}

/// Where a mod came from on Nexus Mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NexusRef {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub nexus: Option<NexusRef>,
    /// Which file of the Nexus page this is, such as "Main file" or a patch.
    #[serde(default)]
    pub file: String,
    /// File name of the source archive kept in `<staging>/archives`.
    #[serde(default)]
    pub archive: Option<String>,
    #[serde(default)]
    pub files: u64,
}

impl ModEntry {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: true,
            version: String::new(),
            nexus: None,
            file: String::new(),
            archive: None,
            files: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeployReport {
    pub method: LinkMethod,
    /// Files newly linked into the game folder.
    pub linked: usize,
    /// Links taken out because their mod is off, gone, or lost a conflict.
    pub removed: usize,
    /// Links that were already in place.
    pub unchanged: usize,
    /// Files more than one enabled mod provides. The mod lower in the list wins.
    pub conflicts: usize,
    /// Files left alone because the game folder already has its own copy.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModState {
    mods: Vec<ModEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DeploymentManifest {
    files: Vec<DeployedFile>,
    /// Folders this app created inside the game's mod folder, relative to it.
    #[serde(default)]
    dirs: Vec<String>,
    /// The same for each named target.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    target_dirs: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeployedFile {
    /// Path inside the game folder, spelled the way it is on disk.
    relative: String,
    /// Id of the mod that owns the link.
    source: String,
    /// Path inside the mod, when its spelling differs from `relative`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_relative: String,
    /// Inode of a hardlink when it was placed. It identifies the link as ours
    /// after the staged file is gone or replaced. Zero for a symlink.
    #[serde(default, skip_serializing_if = "is_zero")]
    ino: u64,
    /// Which of the game's folders the link is in. Empty for the mod folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    target: String,
    /// True when the game's own file was moved aside to make room.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    backup: bool,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl DeployedFile {
    fn source_path(&self, staging: &Path) -> PathBuf {
        let inside = if self.source_relative.is_empty() {
            &self.relative
        } else {
            &self.source_relative
        };
        target_dir(staging, &self.source, &self.target).join(inside)
    }
}

pub fn link_method(staging_device: u64, game_device: u64) -> LinkMethod {
    if staging_device == game_device {
        LinkMethod::Hardlink
    } else {
        LinkMethod::Symlink
    }
}

/// `<library>/steamapps/vortex/<game id>`.
pub fn staging_dir(library: &Path, game_id: &str) -> PathBuf {
    library.join("steamapps").join("vortex").join(game_id)
}

pub fn link_method_for_paths(staging: &Path, game_data: &Path) -> io::Result<LinkMethod> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let staging_device = fs::metadata(staging)?.dev();
        let game_device = fs::metadata(game_data)?.dev();
        Ok(link_method(staging_device, game_device))
    }
    #[cfg(not(unix))]
    {
        let _ = (staging, game_data);
        Ok(LinkMethod::Symlink)
    }
}

pub fn mod_dir(staging: &Path, id: &str) -> PathBuf {
    staging.join("mods").join(id)
}

pub fn load_mods(staging: &Path) -> Result<Vec<ModEntry>, ModError> {
    let path = staging.join("mods.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let state: ModState = serde_json::from_str(&fs::read_to_string(&path)?)
        .map_err(|err| ModError::new(format!("{}: {err}", path.display())))?;
    Ok(state.mods)
}

pub fn save_mods(staging: &Path, mods: &[ModEntry]) -> Result<(), ModError> {
    fs::create_dir_all(staging)?;
    let state = ModState {
        mods: mods.to_vec(),
    };
    write_atomic(
        &staging.join("mods.json"),
        &serde_json::to_string_pretty(&state)?,
    )?;
    Ok(())
}

/// Write `text` next to `path` and rename it into place, so a crash or a full
/// disk never leaves half a file behind.
pub(crate) fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let written = fs::File::create(&temp).and_then(|mut file| {
        use std::io::Write;
        file.write_all(text.as_bytes())?;
        // Renaming before the data is on disk can leave an empty file behind
        // after a power cut.
        file.sync_all()
    });
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn no_mod(id: &str) -> ModError {
    ModError::new(format!("no mod with id '{id}'"))
}

pub fn set_enabled(staging: &Path, id: &str, enabled: bool) -> Result<ModEntry, ModError> {
    let mut mods = load_mods(staging)?;
    let entry = mods
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| no_mod(id))?;
    entry.enabled = enabled;
    let updated = entry.clone();
    save_mods(staging, &mods)?;
    Ok(updated)
}

pub fn toggle_mod(staging: &Path, id: &str) -> Result<ModEntry, ModError> {
    let enabled = load_mods(staging)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| no_mod(id))?
        .enabled;
    set_enabled(staging, id, !enabled)
}

/// Move a mod `by` places in the list. A mod lower in the list wins when two
/// mods ship the same file. Returns the new position.
pub fn move_mod(staging: &Path, id: &str, by: i64) -> Result<usize, ModError> {
    let mut mods = load_mods(staging)?;
    let from = mods
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| no_mod(id))?;
    let last = mods.len() as i64 - 1;
    let to = (from as i64 + by).clamp(0, last) as usize;
    if to != from {
        let entry = mods.remove(from);
        mods.insert(to, entry);
        save_mods(staging, &mods)?;
    }
    Ok(to)
}

/// The folders a game's mods are linked into.
#[derive(Debug, Clone)]
pub struct DeployRoots {
    /// The game's mod folder.
    pub main: PathBuf,
    /// Other folders an installer can address by name, such as the game's
    /// root for a mod loader that sits next to the executable.
    pub targets: BTreeMap<String, PathBuf>,
    /// False when each mod gets a folder of its own inside `main`.
    pub merge_mods: bool,
    /// True when a mod may stand in for a file the game shipped. The game's
    /// file is kept beside it as `<name>.vortex_backup` and put back when the
    /// mod is turned off. False leaves the game's files alone.
    pub replace_existing: bool,
}

impl DeployRoots {
    /// One mod folder that every mod is merged into.
    pub fn single(main: impl Into<PathBuf>) -> Self {
        Self {
            main: main.into(),
            targets: BTreeMap::new(),
            merge_mods: true,
            replace_existing: false,
        }
    }

    /// The main folder under the name "", then the named ones.
    fn each(&self) -> impl Iterator<Item = (&str, &Path)> {
        std::iter::once(("", self.main.as_path())).chain(
            self.targets
                .iter()
                .map(|(name, path)| (name.as_str(), path.as_path())),
        )
    }
}

/// Where a mod keeps the files meant for `target`. Files for a named target
/// sit in a hidden folder, so the walk over the main files passes them by.
pub(crate) fn target_dir(staging: &Path, id: &str, target: &str) -> PathBuf {
    let root = mod_dir(staging, id);
    if target.is_empty() {
        root
    } else {
        root.join(TARGETS_DIR).join(target)
    }
}

pub(crate) const TARGETS_DIR: &str = ".targets";

/// Take a mod's links out of the game folder, then delete its staged files.
///
/// The source archive in `<staging>/archives` goes too unless `keep_archive`.
pub fn remove_mod(
    staging: &Path,
    roots: &DeployRoots,
    id: &str,
    keep_archive: bool,
) -> Result<ModEntry, ModError> {
    let mut mods = load_mods(staging)?;
    let index = mods
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| no_mod(id))?;
    mods[index].enabled = false;
    save_mods(staging, &mods)?;
    deploy(staging, roots)?;
    let entry = mods.remove(index);
    let dir = mod_dir(staging, &entry.id);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    if !keep_archive {
        if let Some(name) = entry.archive.as_deref().and_then(safe_relative) {
            let archive = staging.join("archives").join(name);
            let still_used = mods.iter().any(|other| other.archive == entry.archive);
            if !still_used && fs::symlink_metadata(&archive).is_ok() {
                if archive.is_dir() {
                    fs::remove_dir_all(&archive)?;
                } else {
                    fs::remove_file(&archive)?;
                }
            }
        }
    }
    save_mods(staging, &mods)?;
    Ok(entry)
}

/// Link every enabled mod into the game's folders, then remove links from
/// mods that are no longer enabled. Files this app did not create are not deleted.
pub fn deploy(staging: &Path, roots: &DeployRoots) -> Result<DeployReport, ModError> {
    let mods = load_mods(staging)?;
    deploy_mods(staging, roots, &mods)
}

/// Remove every link this app placed. Mods stay installed.
pub fn purge(staging: &Path, roots: &DeployRoots) -> Result<DeployReport, ModError> {
    deploy_mods(staging, roots, &[])
}

struct Planned {
    mod_id: String,
    source: PathBuf,
    source_relative: String,
    /// Path under the root, before it is matched to the spelling on disk.
    wanted: String,
}

fn deploy_mods(
    staging: &Path,
    roots: &DeployRoots,
    mods: &[ModEntry],
) -> Result<DeployReport, ModError> {
    fs::create_dir_all(staging)?;
    let mut previous = load_manifest(staging)?;
    let mut report = DeployReport {
        method: LinkMethod::Hardlink,
        linked: 0,
        removed: 0,
        unchanged: 0,
        conflicts: 0,
        skipped: Vec::new(),
    };
    let mut manifest = DeploymentManifest::default();
    for (target, root) in roots.each() {
        let (old_files, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut previous.files)
            .into_iter()
            .partition(|file| file.target == target);
        previous.files = rest;
        let old_dirs = if target.is_empty() {
            std::mem::take(&mut previous.dirs)
        } else {
            previous.target_dirs.remove(target).unwrap_or_default()
        };
        // A target only comes into being when a mod has something for it.
        let wanted = mods
            .iter()
            .any(|entry| entry.enabled && target_dir(staging, &entry.id, target).is_dir());
        if !root.is_dir() {
            if !wanted && old_files.is_empty() {
                continue;
            }
            fs::create_dir_all(root).map_err(|err| {
                ModError::new(format!("could not create {}: {err}", root.display()))
            })?;
        }
        let method = link_method_for_paths(staging, root)?;
        if target.is_empty() {
            report.method = method;
        }
        let nest = target.is_empty() && !roots.merge_mods;
        let (files, dirs) = deploy_root(
            staging,
            target,
            root,
            roots.replace_existing,
            nest,
            method,
            mods,
            &old_files,
            old_dirs,
            &mut report,
        )?;
        manifest.files.extend(files);
        if target.is_empty() {
            manifest.dirs = dirs;
        } else if !dirs.is_empty() {
            manifest.target_dirs.insert(target.to_string(), dirs);
        }
    }
    // Links under a target this game no longer has stay on record, so a later
    // run that knows the target again can still clean them up.
    manifest.files.extend(previous.files);
    manifest.target_dirs.extend(previous.target_dirs);
    save_manifest(staging, &manifest)?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn deploy_root(
    staging: &Path,
    target: &str,
    data: &Path,
    replace_existing: bool,
    nest: bool,
    method: LinkMethod,
    mods: &[ModEntry],
    previous: &[DeployedFile],
    mut dirs: Vec<String>,
    report: &mut DeployReport,
) -> Result<(Vec<DeployedFile>, Vec<String>), ModError> {
    // Windows treats `Textures/a.dds` and `textures/A.DDS` as one file, so the
    // plan is keyed without regard to case and a later mod replaces an earlier one.
    let mut planned: HashMap<String, Planned> = HashMap::new();
    let mut order = Vec::new();
    for entry in mods.iter().filter(|entry| entry.enabled) {
        let root = target_dir(staging, &entry.id, target);
        if !root.is_dir() {
            continue;
        }
        for relative in walk_files(&root)? {
            let source_relative = relative.to_string_lossy().replace('\\', "/");
            let wanted = if nest {
                format!("{}/{source_relative}", entry.id)
            } else {
                source_relative.clone()
            };
            let key = wanted.to_lowercase();
            let plan = Planned {
                mod_id: entry.id.clone(),
                source: root.join(&relative),
                source_relative,
                wanted,
            };
            if let Some(earlier) = planned.insert(key.clone(), plan) {
                if earlier.mod_id != entry.id {
                    report.conflicts += 1;
                }
            } else {
                order.push(key);
            }
        }
    }

    let mut kept: HashMap<String, DeployedFile> = HashMap::new();
    for old in previous {
        let Some(dest) = safe_join(data, &old.relative) else {
            continue;
        };
        if fs::symlink_metadata(&dest).is_err() {
            continue;
        }
        let old_source = old.source_path(staging);
        if !placed_by_us(&dest, old, &old_source) {
            continue;
        }
        // Still the right file only if it links to what is staged now. A mod
        // that was reinstalled keeps its path and gets new files.
        let key = old.relative.to_lowercase();
        let same_winner = planned.get(&key).is_some_and(|plan| {
            plan.mod_id == old.source
                && plan.source == old_source
                && is_our_link(&dest, &old_source)
        });
        if same_winner {
            kept.insert(key, old.clone());
        } else {
            fs::remove_file(&dest)?;
            report.removed += 1;
            let saved = backup_path(&dest);
            if old.backup && fs::symlink_metadata(&saved).is_ok() {
                fs::rename(&saved, &dest)?;
            }
        }
    }

    let mut files = Vec::new();
    let mut names = CaseMap::new(data);
    for key in &order {
        if let Some(file) = kept.remove(key) {
            files.push(file);
            report.unchanged += 1;
            continue;
        }
        let plan = &planned[key];
        let Some(relative) = names.resolve(&plan.wanted) else {
            report.skipped.push(plan.wanted.clone());
            continue;
        };
        let dest = data.join(&relative);
        // A backup beside one of our links is the game's file we moved aside,
        // whether or not the record of doing so survived. Going by what is on
        // disk keeps the original restorable after a lost or stale manifest.
        let saved = backup_path(&dest);
        let mut backup = replace_existing && fs::symlink_metadata(&saved).is_ok();
        let present = fs::symlink_metadata(&dest).ok();
        if present.is_some() && is_our_link(&dest, &plan.source) {
            report.unchanged += 1;
        } else {
            if let Some(found) = &present {
                // The game's own file. Step around it, or move it aside.
                if !(replace_existing && dest.is_file()) {
                    report.skipped.push(relative);
                    continue;
                }
                if backup {
                    // A plain file where our link used to be: Steam updated or
                    // repaired the game. That file is the original now, and
                    // the older backup is out of date. Anything that may still
                    // be a link of ours is left alone.
                    if found.file_type().is_symlink() || link_count(found) != 1 {
                        report.skipped.push(relative);
                        continue;
                    }
                    fs::remove_file(&saved)?;
                }
                fs::rename(&dest, &saved)?;
                backup = true;
            } else if let Some(parent) = Path::new(&relative).parent() {
                // A file where a mod wants a folder (or the other way round)
                // costs that one file, not the whole deploy.
                if create_dirs(data, parent, &mut dirs).is_err() {
                    report.skipped.push(relative);
                    continue;
                }
            }
            place_link(method, &plan.source, &dest).map_err(|err| {
                ModError::new(format!("could not place {}: {err}", dest.display()))
            })?;
            report.linked += 1;
        }
        files.push(DeployedFile {
            source_relative: if plan.source_relative == relative {
                String::new()
            } else {
                plan.source_relative.clone()
            },
            ino: hardlink_inode(&dest),
            relative,
            source: plan.mod_id.clone(),
            target: target.to_string(),
            backup,
        });
    }
    Ok((files, prune_dirs(data, dirs)))
}

/// Spells a mod's path the way the game folder already spells it.
///
/// A mod that ships `textures/` must land in the game's existing `Textures/`.
/// On Linux two folders that differ only by case can both exist, and the game
/// then sees only one of them.
struct CaseMap {
    root: PathBuf,
    dirs: HashMap<PathBuf, HashMap<String, String>>,
}

impl CaseMap {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            dirs: HashMap::new(),
        }
    }

    fn resolve(&mut self, relative: &str) -> Option<String> {
        let relative = safe_relative(relative)?;
        let mut current = PathBuf::new();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                continue;
            };
            let part = part.to_string_lossy().to_string();
            let root = &self.root;
            let listing = self.dirs.entry(current.clone()).or_insert_with(|| {
                let mut names = HashMap::new();
                if let Ok(entries) = fs::read_dir(root.join(&current)) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        names.entry(name.to_lowercase()).or_insert(name);
                    }
                }
                names
            });
            let actual = listing.entry(part.to_lowercase()).or_insert(part).clone();
            current.push(actual);
        }
        Some(current.to_string_lossy().replace('\\', "/"))
    }
}

fn create_dirs(data: &Path, relative: &Path, created: &mut Vec<String>) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in relative.components() {
        current.push(component);
        let path = data.join(&current);
        if path.is_dir() {
            continue;
        }
        fs::create_dir(&path)?;
        let text = current.to_string_lossy().replace('\\', "/");
        if !created.contains(&text) {
            created.push(text);
        }
    }
    Ok(())
}

/// Delete folders this app made that are empty again. Returns the ones still in use.
fn prune_dirs(data: &Path, mut dirs: Vec<String>) -> Vec<String> {
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.matches('/').count()));
    let mut remaining = Vec::new();
    for dir in dirs {
        let Some(path) = safe_join(data, &dir) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        // `remove_dir` refuses a folder that still has anything in it.
        if fs::remove_dir(&path).is_err() {
            remaining.push(dir);
        }
    }
    remaining.sort();
    remaining
}

pub(crate) fn walk_files(root: &Path) -> Result<Vec<PathBuf>, ModError> {
    let mut files = Vec::new();
    walk_rec(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_rec(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), ModError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.eq_ignore_ascii_case("fomod") {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk_rec(root, &path, files)?;
        } else if metadata.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

pub(crate) fn copy_file(source: &Path, dest: &Path) -> Result<(), ModError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, dest)?;
    Ok(())
}

pub(crate) fn safe_relative(value: &str) -> Option<PathBuf> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(normalized.trim().trim_matches('/'));
    if path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub(crate) fn resolve_case(root: &Path, relative: &Path) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    if !current.is_dir() {
        return None;
    }
    for component in relative.components() {
        let Component::Normal(wanted) = component else {
            continue;
        };
        let wanted = wanted.to_string_lossy();
        let found = fs::read_dir(&current).ok()?.find_map(|entry| {
            let entry = entry.ok()?;
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&wanted)
                .then(|| entry.path())
        })?;
        current = found;
    }
    Some(current)
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    Some(root.join(safe_relative(relative)?))
}

fn backup_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".vortex_backup");
    PathBuf::from(name)
}

/// Inode of `path` when it is a regular file, which for us means a hardlink.
fn hardlink_inode(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.is_file() {
                return meta.ino();
            }
        }
    }
    let _ = path;
    0
}

/// Whether the file at `dest` is the link recorded in `record`.
fn placed_by_us(dest: &Path, record: &DeployedFile, source: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(dest) else {
        return false;
    };
    if meta.file_type().is_symlink() || record.ino == 0 {
        return is_our_link(dest, source);
    }
    meta.is_file() && hardlink_inode(dest) == record.ino
}

fn is_our_link(dest: &Path, source: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(dest) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return fs::read_link(dest)
            .ok()
            .is_some_and(|target| target == source || paths_match(&target, source));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(source_meta) = fs::metadata(source) {
            return metadata.dev() == source_meta.dev() && metadata.ino() == source_meta.ino();
        }
    }
    false
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn place_link(method: LinkMethod, source: &Path, dest: &Path) -> Result<(), ModError> {
    match method {
        LinkMethod::Hardlink => match fs::hard_link(source, dest) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
                symlink(source, dest)?;
                Ok(())
            }
            Err(err) => Err(err.into()),
        },
        LinkMethod::Symlink => {
            symlink(source, dest)?;
            Ok(())
        }
    }
}

fn symlink(source: &Path, dest: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, dest)
    }
    #[cfg(not(unix))]
    {
        std::os::windows::fs::symlink_file(source, dest)
    }
}

fn load_manifest(staging: &Path) -> Result<DeploymentManifest, ModError> {
    let path = staging.join("deployment.json");
    if !path.is_file() {
        return Ok(DeploymentManifest::default());
    }
    match serde_json::from_str(&fs::read_to_string(&path)?) {
        Ok(manifest) => Ok(manifest),
        Err(_) => {
            // A cut-off record (power loss, full disk) must not lock the
            // player out of their mods. Links still in place are recognised
            // again by what they point at.
            let _ = fs::rename(&path, staging.join("deployment.json.corrupt"));
            Ok(DeploymentManifest::default())
        }
    }
}

fn link_count(metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        1
    }
}

fn save_manifest(staging: &Path, manifest: &DeploymentManifest) -> Result<(), ModError> {
    write_atomic(
        &staging.join("deployment.json"),
        &serde_json::to_string(manifest)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_device_hardlinks() {
        assert_eq!(link_method(7, 7), LinkMethod::Hardlink);
        assert_eq!(link_method(7, 8), LinkMethod::Symlink);
    }

    #[test]
    fn staging_stays_inside_the_steam_library() {
        let path = staging_dir(Path::new("/run/media/mmcblk0p1/SteamLibrary"), "skyrimse");
        assert_eq!(
            path,
            Path::new("/run/media/mmcblk0p1/SteamLibrary/steamapps/vortex/skyrimse")
        );
    }

    #[test]
    fn later_mod_wins_and_disabling_it_removes_only_our_link() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("Skyrim.esm"), b"vanilla").unwrap();
        save_mod(&staging, "a", "A.esp", b"from-a");
        save_mod(&staging, "b", "A.esp", b"from-b");
        save_mod(&staging, "b", "Skyrim.esm", b"replaced");
        save_mods(&staging, &[entry("a", true), entry("b", true)]).unwrap();

        let report = deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(report.method, LinkMethod::Hardlink);
        assert_eq!(report.linked, 1);
        assert_eq!(report.conflicts, 1);
        assert_eq!(fs::read(data.join("A.esp")).unwrap(), b"from-b");
        assert_eq!(fs::read(data.join("Skyrim.esm")).unwrap(), b"vanilla");
        assert!(report.skipped.iter().any(|path| path == "Skyrim.esm"));

        let again = deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!((again.linked, again.removed, again.unchanged), (0, 0, 1));

        set_enabled(&staging, "b", false).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(fs::read(data.join("A.esp")).unwrap(), b"from-a");
        assert_eq!(fs::read(data.join("Skyrim.esm")).unwrap(), b"vanilla");

        set_enabled(&staging, "a", false).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert!(!data.join("A.esp").exists());
        assert_eq!(fs::read(data.join("Skyrim.esm")).unwrap(), b"vanilla");
    }

    #[test]
    fn a_mod_folder_merges_into_the_games_spelling() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(data.join("Textures")).unwrap();
        fs::write(data.join("Textures/vanilla.dds"), b"v").unwrap();
        save_mod(&staging, "a", "textures/armor/Iron.dds", b"a");
        save_mod(&staging, "b", "TEXTURES/Armor/iron.DDS", b"b");
        save_mod(&staging, "b", "meshes/new.nif", b"n");
        save_mods(&staging, &[entry("a", true), entry("b", true)]).unwrap();

        let report = deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(report.conflicts, 1);
        assert_eq!(report.linked, 2);
        // One folder, spelled the way the game had it. The later mod's file wins.
        assert_eq!(dir_names(&data), ["Textures", "meshes"]);
        assert_eq!(dir_names(&data.join("Textures")), ["Armor", "vanilla.dds"]);
        assert_eq!(dir_names(&data.join("Textures/Armor")), ["iron.DDS"]);
        assert_eq!(
            fs::read(data.join("Textures/Armor/iron.DDS")).unwrap(),
            b"b"
        );

        // Turning the winner off hands the file back to the other mod, in the
        // folder that is already there.
        set_enabled(&staging, "b", false).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(dir_names(&data.join("Textures")), ["Armor", "vanilla.dds"]);
        assert_eq!(dir_names(&data.join("Textures/Armor")), ["Iron.dds"]);
        assert_eq!(
            fs::read(data.join("Textures/Armor/Iron.dds")).unwrap(),
            b"a"
        );
        assert!(!data.join("meshes").exists());

        // Purging leaves the game folder as it was found.
        let purged = purge(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(purged.removed, 1);
        assert_eq!(dir_names(&data), ["Textures"]);
        assert_eq!(dir_names(&data.join("Textures")), ["vanilla.dds"]);
    }

    #[test]
    fn removing_a_mod_takes_its_links_files_and_archive() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(staging.join("archives")).unwrap();
        fs::write(staging.join("archives/A.zip"), b"zip").unwrap();
        save_mod(&staging, "a", "A.esp", b"from-a");
        save_mod(&staging, "b", "B.esp", b"from-b");
        let mut first = entry("a", true);
        first.archive = Some("A.zip".to_string());
        save_mods(&staging, &[first, entry("b", true)]).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();

        let gone = remove_mod(&staging, &DeployRoots::single(&data), "a", false).unwrap();
        assert_eq!(gone.id, "a");
        assert!(!data.join("A.esp").exists());
        assert!(data.join("B.esp").exists());
        assert!(!mod_dir(&staging, "a").exists());
        assert!(!staging.join("archives/A.zip").exists());
        let left = load_mods(&staging).unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].enabled);
        assert!(remove_mod(&staging, &DeployRoots::single(&data), "a", false).is_err());
    }

    #[test]
    fn replaced_and_deleted_mod_files_do_not_strand_links() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        save_mod(&staging, "a", "A.esp", b"old");
        save_mod(&staging, "b", "B.esp", b"b");
        save_mods(&staging, &[entry("a", true), entry("b", true)]).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();

        // A reinstall swaps the staged file for a new one at the same path.
        fs::remove_dir_all(mod_dir(&staging, "a")).unwrap();
        save_mod(&staging, "a", "A.esp", b"new");
        let report = deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!((report.removed, report.linked, report.unchanged), (1, 1, 1));
        assert_eq!(fs::read(data.join("A.esp")).unwrap(), b"new");
        assert!(report.skipped.is_empty());

        // Staged files deleted behind our back still get their links cleaned up.
        fs::remove_dir_all(mod_dir(&staging, "b")).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert!(!data.join("B.esp").exists());

        // A file the user put there in the meantime is theirs.
        fs::remove_file(data.join("A.esp")).unwrap();
        fs::write(data.join("A.esp"), b"mine").unwrap();
        let report = purge(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(fs::read(data.join("A.esp")).unwrap(), b"mine");
    }

    #[test]
    fn a_game_file_can_be_replaced_and_comes_back() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("game");
        fs::create_dir_all(data.join("rom")).unwrap();
        fs::write(data.join("rom/main.arc"), b"shipped").unwrap();
        save_mod(&staging, "a", "rom/main.arc", b"from-a");
        save_mod(&staging, "b", "rom/main.arc", b"from-b");
        save_mods(&staging, &[entry("a", true), entry("b", false)]).unwrap();
        let mut roots = DeployRoots::single(&data);
        roots.replace_existing = true;

        let report = deploy(&staging, &roots).unwrap();
        assert!(report.skipped.is_empty());
        assert_eq!(fs::read(data.join("rom/main.arc")).unwrap(), b"from-a");
        assert_eq!(
            fs::read(data.join("rom/main.arc.vortex_backup")).unwrap(),
            b"shipped"
        );
        // Another mod taking over keeps the one backup of the game's file.
        set_enabled(&staging, "b", true).unwrap();
        deploy(&staging, &roots).unwrap();
        assert_eq!(fs::read(data.join("rom/main.arc")).unwrap(), b"from-b");
        assert_eq!(
            fs::read(data.join("rom/main.arc.vortex_backup")).unwrap(),
            b"shipped"
        );
        deploy(&staging, &roots).unwrap();
        purge(&staging, &roots).unwrap();
        assert_eq!(fs::read(data.join("rom/main.arc")).unwrap(), b"shipped");
        assert_eq!(dir_names(&data.join("rom")), ["main.arc"]);
    }

    #[test]
    fn a_lost_record_does_not_lose_the_games_file() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("game");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("inputs.xml"), b"shipped").unwrap();
        save_mod(&staging, "a", "inputs.xml", b"from-a");
        save_mods(&staging, &[entry("a", true)]).unwrap();
        let mut roots = DeployRoots::single(&data);
        roots.replace_existing = true;
        deploy(&staging, &roots).unwrap();

        // The record is gone or cut short. The backup on disk still counts.
        fs::write(staging.join("deployment.json"), b"").unwrap();
        deploy(&staging, &roots).unwrap();
        set_enabled(&staging, "a", false).unwrap();
        deploy(&staging, &roots).unwrap();
        assert_eq!(fs::read(data.join("inputs.xml")).unwrap(), b"shipped");
        assert_eq!(dir_names(&data), ["inputs.xml"]);
    }

    #[test]
    fn a_game_update_becomes_the_new_original() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("game");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("inputs.xml"), b"shipped").unwrap();
        save_mod(&staging, "a", "inputs.xml", b"from-a");
        save_mods(&staging, &[entry("a", true)]).unwrap();
        let mut roots = DeployRoots::single(&data);
        roots.replace_existing = true;
        deploy(&staging, &roots).unwrap();

        // Steam writes a new file over our link.
        fs::remove_file(data.join("inputs.xml")).unwrap();
        fs::write(data.join("inputs.xml"), b"updated").unwrap();
        let report = deploy(&staging, &roots).unwrap();
        assert!(report.skipped.is_empty());
        assert_eq!(fs::read(data.join("inputs.xml")).unwrap(), b"from-a");
        purge(&staging, &roots).unwrap();
        assert_eq!(fs::read(data.join("inputs.xml")).unwrap(), b"updated");
        assert_eq!(dir_names(&data), ["inputs.xml"]);
    }

    #[test]
    fn a_file_where_a_folder_should_be_costs_only_that_file() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        save_mod(&staging, "a", "docs", b"a file");
        save_mod(&staging, "b", "docs/readme.txt", b"in a folder");
        save_mod(&staging, "b", "B.esp", b"plugin");
        save_mods(&staging, &[entry("a", true), entry("b", true)]).unwrap();
        let report = deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(report.skipped, ["docs/readme.txt"]);
        assert!(data.join("B.esp").is_file());
    }

    #[test]
    fn moving_a_mod_changes_who_wins() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        save_mod(&staging, "a", "A.esp", b"from-a");
        save_mod(&staging, "b", "A.esp", b"from-b");
        save_mods(&staging, &[entry("a", true), entry("b", true)]).unwrap();
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(move_mod(&staging, "a", 5).unwrap(), 1);
        deploy(&staging, &DeployRoots::single(&data)).unwrap();
        assert_eq!(fs::read(data.join("A.esp")).unwrap(), b"from-a");
        assert_eq!(move_mod(&staging, "a", -1).unwrap(), 0);
    }

    #[test]
    fn old_mod_lists_still_load() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("mods.json"),
            r#"{"mods":[{"id":"a","name":"A","enabled":false}]}"#,
        )
        .unwrap();
        let mods = load_mods(root.path()).unwrap();
        assert_eq!(mods[0].name, "A");
        assert!(mods[0].nexus.is_none());
    }

    #[test]
    fn a_mod_can_reach_a_second_folder_and_get_a_folder_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let game = root.path().join("Game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("Game.exe"), b"exe").unwrap();
        save_mod(&staging, "loader", ".targets/root/Loader.dll", b"dll");
        save_mod(&staging, "loader", "Bundled/a.txt", b"a");
        save_mod(&staging, "pack", "content.json", b"{}");
        save_mods(&staging, &[entry("loader", true), entry("pack", true)]).unwrap();
        let mut roots = DeployRoots::single(game.join("Mods"));
        roots.targets.insert("root".to_string(), game.clone());
        roots.merge_mods = false;

        let report = deploy(&staging, &roots).unwrap();
        assert_eq!(report.linked, 3);
        assert_eq!(fs::read(game.join("Loader.dll")).unwrap(), b"dll");
        // The mod folder did not exist, and each mod sits under its own name.
        assert_eq!(
            fs::read(game.join("Mods/loader/Bundled/a.txt")).unwrap(),
            b"a"
        );
        assert_eq!(
            fs::read(game.join("Mods/pack/content.json")).unwrap(),
            b"{}"
        );
        assert!(!game.join("Mods/.targets").exists());

        let gone = purge(&staging, &roots).unwrap();
        assert_eq!(gone.removed, 3);
        assert_eq!(dir_names(&game), ["Game.exe", "Mods"]);
        assert_eq!(dir_names(&game.join("Mods")), Vec::<String>::new());
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn entry(id: &str, enabled: bool) -> ModEntry {
        ModEntry {
            enabled,
            ..ModEntry::new(id, id)
        }
    }

    fn save_mod(staging: &Path, id: &str, file: &str, bytes: &[u8]) {
        let path = mod_dir(staging, id).join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
