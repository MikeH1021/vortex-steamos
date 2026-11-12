//! Put a downloaded archive or folder into the staging directory.
//!
//! The result is a mod whose files are relative to the game's `Data` folder.
//! A simple archive dropped in `<staging>/incoming` is installed on the next
//! sync and moved to `<staging>/archives`. An installer that still has a real
//! choice stays in `incoming` until the shell asks.
//!
//! Archives are unpacked under `<staging>/.extract`, on the same drive as the
//! mods. `/tmp` on SteamOS is RAM, and a texture pack does not fit there.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::deploy::{self, copy_file, resolve_case, walk_files, ModEntry, ModError, NexusRef};
use crate::fomod::{self, FomodChoice, FomodNote, FomodPlan, InstalledFiles};
use crate::games::Kind;
use crate::layout;

/// Archive types this app installs. `.rar` needs `bsdtar`, `7z`, or `unrar`.
pub const ARCHIVE_EXTENSIONS: &[&str] = &[".zip", ".7z", ".rar"];

/// Folders a Bethesda game reads from `Data`. An archive whose only top-level
/// folder is one of these is already laid out for `Data`.
const DATA_FOLDERS: &[&str] = &[
    "textures",
    "meshes",
    "scripts",
    "interface",
    "sound",
    "music",
    "strings",
    "video",
    "materials",
    "grass",
    "seq",
    "shadersfx",
    "lodsettings",
    "skse",
    "sfse",
    "geometries",
    "particles",
    "terrain",
    "planetdata",
    "f4se",
    "nvse",
    "obse",
    "fose",
    "mcm",
    "source",
    "tools",
    "facegen",
    "fonts",
    "menus",
    "trees",
    "shaders",
    "distantlod",
    "dialogueviews",
    "calientetools",
    "netscriptframework",
    "config",
    "vis",
    "programs",
];

/// What the installer has to know about the game a mod is for.
#[derive(Debug, Clone)]
pub struct InstallRules {
    pub kind: Kind,
    /// How archives are laid out, for games described by rules.
    pub install: Vec<crate::games::Rule>,
    /// The game's install folder. A mod loader's setup may read from it.
    pub game_root: Option<PathBuf>,
}

impl InstallRules {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            install: Vec::new(),
            game_root: None,
        }
    }

    pub fn bethesda() -> Self {
        Self::new(Kind::Bethesda)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub installed: ModEntry,
    pub notes: Vec<FomodNote>,
}

/// Written next to a download so the installed mod remembers its Nexus page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadMeta {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    #[serde(default)]
    pub mod_name: String,
    /// The file's own title on the mod page.
    #[serde(default)]
    pub file_title: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadItem {
    pub name: String,
    pub path: PathBuf,
    pub module_name: String,
    pub needs_choices: bool,
    pub size: u64,
    pub nexus: Option<NexusRef>,
    /// Why this archive cannot be installed, when it cannot.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedInstall {
    pub name: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncomingOutcome {
    pub installed: Vec<InstallReport>,
    pub pending: Vec<DownloadItem>,
    pub failed: Vec<FailedInstall>,
}

pub fn incoming_dir(staging: &Path) -> PathBuf {
    staging.join("incoming")
}

fn extract_dir(staging: &Path) -> PathBuf {
    staging.join(".extract")
}

/// Path of the metadata file that travels with `archive`.
pub fn meta_path(archive: &Path) -> PathBuf {
    let mut name = archive
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".vortex.json");
    archive.with_file_name(name)
}

pub fn write_meta(archive: &Path, meta: &DownloadMeta) -> Result<(), ModError> {
    deploy::write_atomic(&meta_path(archive), &serde_json::to_string_pretty(meta)?)?;
    Ok(())
}

fn read_meta(archive: &Path) -> Option<DownloadMeta> {
    let text = fs::read_to_string(meta_path(archive)).ok()?;
    let meta: DownloadMeta = serde_json::from_str(&text).ok()?;
    (meta.mod_id != 0 && !meta.domain.is_empty()).then_some(meta)
}

pub fn list_downloads(
    staging: &Path,
    files: &InstalledFiles,
    rules: &InstallRules,
) -> Result<Vec<DownloadItem>, ModError> {
    let incoming = incoming_dir(staging);
    if !incoming.is_dir() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for path in incoming_paths(&incoming)? {
        items.push(describe(staging, &path, files, rules));
    }
    Ok(items)
}

/// Install everything in `incoming` that does not need a choice.
///
/// One broken archive does not stop the rest. It is reported in `failed` and
/// stays in `incoming`.
pub fn install_incoming(
    staging: &Path,
    files: &InstalledFiles,
    rules: &InstallRules,
) -> Result<IncomingOutcome, ModError> {
    let incoming = incoming_dir(staging);
    let mut outcome = IncomingOutcome {
        installed: Vec::new(),
        pending: Vec::new(),
        failed: Vec::new(),
    };
    if !incoming.is_dir() {
        return Ok(outcome);
    }
    for path in incoming_paths(&incoming)? {
        let item = describe(staging, &path, files, rules);
        if let Some(error) = item.error {
            outcome.failed.push(FailedInstall {
                name: item.name,
                error,
            });
            continue;
        }
        if item.needs_choices {
            outcome.pending.push(item);
            continue;
        }
        match install_with(staging, &path, files, &[], rules) {
            Ok(report) => outcome.installed.push(report),
            Err(err) => outcome.failed.push(FailedInstall {
                name: item.name,
                error: err.to_string(),
            }),
        }
    }
    prune_extracted(staging);
    Ok(outcome)
}

/// Install a Bethesda-style archive with the recommended options.
pub fn install_source(staging: &Path, source: &Path) -> Result<InstallReport, ModError> {
    install_with(
        staging,
        source,
        &InstalledFiles::empty(),
        &[],
        &InstallRules::bethesda(),
    )
}

pub fn install_with(
    staging: &Path,
    source: &Path,
    files: &InstalledFiles,
    choices: &[FomodChoice],
    rules: &InstallRules,
) -> Result<InstallReport, ModError> {
    let scratch = scratch_dir(staging, source);
    let installed = install_prepared(staging, source, files, choices, rules, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    let mut report = installed?;
    let _ = fs::remove_dir_all(extract_dir(staging).join(path_key(source)));
    if is_inside_incoming(staging, source) {
        let kept = archive_source(staging, source)?;
        let name = kept
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        if name.is_some() {
            let mut mods = deploy::load_mods(staging)?;
            if let Some(entry) = mods
                .iter_mut()
                .find(|entry| entry.id == report.installed.id)
            {
                entry.archive = name.clone();
                deploy::save_mods(staging, &mods)?;
            }
            report.installed.archive = name;
        }
    }
    Ok(report)
}

/// Working space for one install, next to the unpacked archive.
fn scratch_dir(staging: &Path, source: &Path) -> PathBuf {
    extract_dir(staging).join(format!("{}.stage", path_key(source)))
}

/// Delete an archive waiting in `incoming` without installing it.
pub fn discard_download(staging: &Path, source: &Path) -> Result<(), ModError> {
    if !is_inside_incoming(staging, source) {
        return Err(ModError::new(
            "Only archives in the incoming folder can be discarded.",
        ));
    }
    let _ = fs::remove_dir_all(extract_dir(staging).join(path_key(source)));
    let _ = fs::remove_file(meta_path(source));
    if source.is_dir() {
        fs::remove_dir_all(source)?;
    } else {
        fs::remove_file(source)?;
    }
    Ok(())
}

/// Resolve the installer the shell should show for `source`.
pub fn preview_source(
    staging: &Path,
    source: &Path,
    files: &InstalledFiles,
    choices: &[FomodChoice],
) -> Result<FomodPlan, ModError> {
    require_source(source)?;
    let (raw, _) = materialize(staging, source)?;
    let Some(root) = fomod_base(&raw) else {
        return Ok(FomodPlan {
            name: display_name(source),
            needs_choices: false,
            steps: Vec::new(),
        });
    };
    let xml = read_xml(&module_config(&root)?)?;
    fomod::preview_fomod(&root, &xml, files, choices)
}

fn module_config(root: &Path) -> Result<PathBuf, ModError> {
    fomod::find_module_config(root)
        .ok_or_else(|| ModError::new("The installer's ModuleConfig.xml is missing."))
}

/// The folder that holds `fomod/ModuleConfig.xml`, looking through wrapper
/// folders such as `Campfire 1.12/`.
fn fomod_base(raw: &Path) -> Option<PathBuf> {
    let mut current = raw.to_path_buf();
    for _ in 0..4 {
        if fomod::find_module_config(&current).is_some() {
            return Some(current);
        }
        let entries: Vec<_> = fs::read_dir(&current)
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        if entries.len() != 1 || !entries[0].path().is_dir() {
            return None;
        }
        current = entries[0].path();
    }
    None
}

fn require_source(source: &Path) -> Result<(), ModError> {
    if source.exists() {
        Ok(())
    } else {
        Err(ModError::new(format!(
            "mod source does not exist: {}",
            source.display()
        )))
    }
}

/// FOMOD files are usually UTF-8 and sometimes UTF-16 with a byte order mark.
fn read_xml(path: &Path) -> Result<String, ModError> {
    let bytes = fs::read(path)?;
    let utf16 = |big_endian: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if big_endian {
                    u16::from_be_bytes(*pair)
                } else {
                    u16::from_le_bytes(*pair)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let text = match bytes.as_slice() {
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    };
    // The declared encoding no longer matches once the text is decoded.
    Ok(strip_xml_declaration(&text).to_string())
}

fn strip_xml_declaration(text: &str) -> &str {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<?xml") {
        if let Some(end) = trimmed.find("?>") {
            return &trimmed[end + 2..];
        }
    }
    trimmed
}

fn install_prepared(
    staging: &Path,
    source: &Path,
    files: &InstalledFiles,
    choices: &[FomodChoice],
    rules: &InstallRules,
    scratch: &Path,
) -> Result<InstallReport, ModError> {
    require_source(source)?;
    let (raw, extracted) = materialize(staging, source)?;
    let meta = read_meta(source);
    let mut mods = deploy::load_mods(staging)?;
    let mut name = meta
        .as_ref()
        .map(|meta| meta.mod_name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| display_name(source));
    let id = unique_id(&mods, &slug(&name));
    let dest = deploy::mod_dir(staging, &id);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    let installed = (|| -> Result<Vec<FomodNote>, ModError> {
        let mut notes = Vec::new();
        // An installer picks the files first. Its output is then laid out
        // like any other archive for the game.
        let mut package = raw.clone();
        // Files that came out of our own extraction can be moved by linking.
        // A folder the user pointed at is copied so the two stay independent.
        let mut may_link = extracted;
        if let Some(root) = fomod_base(&raw) {
            let xml = read_xml(&module_config(&root)?)?;
            let picked = if rules.kind == Kind::Bethesda {
                dest.clone()
            } else {
                scratch.join("fomod")
            };
            let install = fomod::install_fomod_with(&root, &xml, &picked, files, choices)?;
            if meta.is_none() && install.name != "Mod" {
                name = install.name;
            }
            notes = install.notes;
            if rules.kind == Kind::Bethesda {
                return Ok(notes);
            }
            package = picked;
            may_link = true;
        } else if rules.kind == Kind::Bethesda {
            let package = package_root(&raw);
            let content = content_root(&package);
            copy_loose(&content, &dest, may_link)?;
            stage_game_folder_files(&package, &content, &dest, may_link)?;
            return Ok(notes);
        }
        let listing = file_list(&package)?;
        let placements = match layout::smapi_payload(&listing) {
            Some(payload) if rules.kind == Kind::Stardew => {
                // SMAPI's download is an installer program. Its Linux files
                // are a zip inside it, and that is all that is needed.
                package = scratch.join("smapi");
                fs::create_dir_all(&package)?;
                extract_zip(&raw.join(payload), &package)?;
                may_link = true;
                let placed = layout::smapi(&file_list(&package)?);
                if placed.is_empty() {
                    return Err(ModError::new(
                        "That SMAPI download has no Linux files in it. Download SMAPI again.",
                    ));
                }
                placed
            }
            _ => layout::plan(rules.kind, &rules.install, &package, &name)?,
        };
        for item in &placements {
            let from = package.join(&item.from);
            let to = deploy::target_dir(staging, &id, &item.target).join(&item.to);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            if !(may_link && fs::hard_link(&from, &to).is_ok()) {
                copy_file(&from, &to)?;
            }
        }
        if rules.kind == Kind::Stardew {
            finish_smapi(&deploy::target_dir(staging, &id, layout::ROOT), rules)?;
        }
        Ok(notes)
    })();
    let notes = match installed {
        Ok(notes) => notes,
        Err(err) => {
            let _ = fs::remove_dir_all(&dest);
            return Err(err);
        }
    };
    let count = count_files(&dest)?;
    if count == 0 {
        let _ = fs::remove_dir_all(&dest);
        return Err(ModError::new(format!(
            "{} has no files to install.",
            item_name(source)
        )));
    }
    let mut entry = ModEntry::new(id, name);
    entry.files = count;
    if let Some(meta) = meta {
        entry.version = meta.version;
        entry.file = meta.file_title;
        entry.nexus = Some(NexusRef {
            domain: meta.domain,
            mod_id: meta.mod_id,
            file_id: meta.file_id,
        });
    }
    // Downloading the same Nexus file again is a reinstall. The mod keeps its
    // place in the list and whether it is on.
    let earlier = entry.nexus.as_ref().and_then(|nexus| {
        mods.iter()
            .position(|other| other.nexus.as_ref() == Some(nexus))
    });
    if let Some(index) = earlier {
        let old = mods[index].clone();
        let old_dir = deploy::mod_dir(staging, &old.id);
        if old_dir.exists() {
            fs::remove_dir_all(&old_dir)?;
        }
        fs::rename(&dest, &old_dir)?;
        if let Some(name) = old.archive.as_deref().and_then(deploy::safe_relative) {
            let _ = fs::remove_file(staging.join("archives").join(name));
        }
        entry.id = old.id;
        entry.enabled = old.enabled;
        mods[index] = entry.clone();
    } else {
        mods.push(entry.clone());
    }
    deploy::save_mods(staging, &mods)?;
    Ok(InstallReport {
        installed: entry,
        notes,
    })
}

fn file_list(root: &Path) -> Result<Vec<String>, ModError> {
    Ok(walk_files(root)?
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect())
}

/// Files of a staged mod, including those meant for the game's other folders.
fn count_files(mod_root: &Path) -> Result<u64, ModError> {
    if !mod_root.is_dir() {
        return Ok(0);
    }
    let mut count = walk_files(mod_root)?.len();
    if let Ok(targets) = fs::read_dir(mod_root.join(deploy::TARGETS_DIR)) {
        for target in targets.flatten() {
            count += walk_files(&target.path())?.len();
        }
    }
    Ok(count as u64)
}

/// SMAPI's own installer does two more things after copying its files. The
/// loader must be runnable, and it reads the game's dependency list under its
/// own name.
fn finish_smapi(root: &Path, rules: &InstallRules) -> Result<(), ModError> {
    let loader = root.join("StardewModdingAPI");
    if !loader.is_file() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&loader, fs::Permissions::from_mode(0o755))?;
    }
    if let Some(game) = &rules.game_root {
        if let Some(deps) = resolve_case(game, Path::new("Stardew Valley.deps.json")) {
            fs::copy(deps, root.join("StardewModdingAPI.deps.json"))?;
        }
    }
    Ok(())
}

fn describe(
    staging: &Path,
    path: &Path,
    files: &InstalledFiles,
    _rules: &InstallRules,
) -> DownloadItem {
    let meta = read_meta(path);
    let mut item = DownloadItem {
        name: item_name(path),
        path: path.to_path_buf(),
        module_name: meta
            .as_ref()
            .map(|meta| meta.mod_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| display_name(path)),
        needs_choices: false,
        size: fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        nexus: meta.map(|meta| NexusRef {
            domain: meta.domain,
            mod_id: meta.mod_id,
            file_id: meta.file_id,
        }),
        error: None,
    };
    let plan = match lists_fomod(path) {
        Ok(Some(false)) => return item,
        Ok(_) => preview_source(staging, path, files, &[]),
        Err(err) => Err(err),
    };
    match plan {
        Ok(plan) => {
            item.needs_choices = plan.needs_choices;
            if item.nexus.is_none() && plan.name != "Mod" && !plan.steps.is_empty() {
                item.module_name = plan.name;
            }
        }
        Err(err) => item.error = Some(err.to_string()),
    }
    item
}

fn incoming_paths(incoming: &Path) -> Result<Vec<PathBuf>, ModError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(incoming)? {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() || is_archive(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Move an installed source into `<staging>/archives`. Returns where it went.
fn archive_source(staging: &Path, source: &Path) -> Result<PathBuf, ModError> {
    let archives = staging.join("archives");
    fs::create_dir_all(&archives)?;
    let target = unique_path(&archives, source);
    move_path(source, &target)?;
    let _ = fs::remove_file(meta_path(source));
    Ok(target)
}

fn is_inside_incoming(staging: &Path, source: &Path) -> bool {
    let Ok(source) = source.canonicalize() else {
        return false;
    };
    let Ok(incoming) = incoming_dir(staging).canonicalize() else {
        return false;
    };
    source.parent() == Some(incoming.as_path())
}

fn item_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string())
}

/// An archive that will not open is nearly always a download that was cut short.
fn damaged(path: &Path, err: impl std::fmt::Display) -> ModError {
    ModError::new(format!(
        "{} is damaged or incomplete. Download it again. ({err})",
        item_name(path)
    ))
}

fn lower_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// `Some(false)` when the archive index shows there is no installer, so the
/// archive does not have to be unpacked to find out. `None` when only
/// unpacking can tell.
fn lists_fomod(path: &Path) -> Result<Option<bool>, ModError> {
    if path.is_dir() {
        return Ok(Some(fomod_base(path).is_some()));
    }
    let name = lower_name(path);
    if name.ends_with(".zip") {
        return zip_lists_fomod(path).map(Some);
    }
    if name.ends_with(".7z") {
        return sevenz_lists_fomod(path).map(Some);
    }
    Ok(None)
}

fn is_module_config_name(name: &str) -> bool {
    name.replace('\\', "/")
        .to_ascii_lowercase()
        .ends_with("fomod/moduleconfig.xml")
}

fn zip_lists_fomod(path: &Path) -> Result<bool, ModError> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| damaged(path, err))?;
    let mut found = false;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        let Ok(name) = entry.name() else {
            continue;
        };
        if is_module_config_name(&name) {
            found = true;
            break;
        }
    }
    Ok(found)
}

fn sevenz_lists_fomod(path: &Path) -> Result<bool, ModError> {
    let archive = sevenz_rust2::Archive::open(path).map_err(|err| damaged(path, err))?;
    Ok(archive
        .files
        .iter()
        .any(|entry| is_module_config_name(&entry.name)))
}

/// The folder to install from, and whether this app unpacked it.
fn materialize(staging: &Path, source: &Path) -> Result<(PathBuf, bool), ModError> {
    if source.is_dir() {
        return Ok((source.to_path_buf(), false));
    }
    Ok((cache_extract(staging, source)?, true))
}

fn cache_extract(staging: &Path, source: &Path) -> Result<PathBuf, ModError> {
    let meta = fs::metadata(source)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|time| time.as_secs())
        .unwrap_or(0);
    let stamp_text = format!("{modified} {}\n{}", meta.len(), source.display());
    let dir = extract_dir(staging).join(path_key(source));
    let stamp = dir.join(".vortex-ready");
    if stamp.is_file() && fs::read_to_string(&stamp).ok().as_deref() == Some(stamp_text.as_str()) {
        return Ok(dir);
    }
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    if let Err(err) = extract_archive(source, &dir) {
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }
    fs::write(&stamp, stamp_text)?;
    Ok(dir)
}

/// Drop unpacked copies of archives that are gone, such as a wizard the user
/// backed out of before deleting the download.
fn prune_extracted(staging: &Path) {
    let Ok(entries) = fs::read_dir(extract_dir(staging)) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let source = fs::read_to_string(dir.join(".vortex-ready"))
            .ok()
            .and_then(|text| text.lines().nth(1).map(PathBuf::from));
        if !source.is_some_and(|source| source.exists()) {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

fn path_key(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn copy_loose(source: &Path, dest: &Path, may_link: bool) -> Result<(), ModError> {
    fs::create_dir_all(dest)?;
    for relative in walk_files(source)? {
        let from = source.join(&relative);
        let to = dest.join(&relative);
        if may_link {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::hard_link(&from, &to).is_ok() {
                continue;
            }
        }
        copy_file(&from, &to)?;
    }
    Ok(())
}

/// Programs and libraries a Bethesda mod ships for the game folder itself:
/// a script extender's loader, an ENB or engine-fix DLL. The game never reads
/// these from `Data`, so they are staged for the game folder.
const GAME_FOLDER_ENDINGS: &[&str] = &[".exe", ".dll", ".asi"];

fn stage_game_folder_files(
    package: &Path,
    content: &Path,
    dest: &Path,
    may_link: bool,
) -> Result<(), ModError> {
    let Ok(entries) = fs::read_dir(package) else {
        return Ok(());
    };
    let root = dest.join(deploy::TARGETS_DIR).join("root");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        let path = entry.path();
        if !path.is_file() || !GAME_FOLDER_ENDINGS.iter().any(|end| lower.ends_with(end)) {
            continue;
        }
        fs::create_dir_all(&root)?;
        let to = root.join(&name);
        if !(may_link && fs::hard_link(&path, &to).is_ok()) {
            copy_file(&path, &to)?;
        }
        // With no `Data` folder in the archive, the same file was just laid
        // out for `Data` as well. It does not belong there.
        if content == package {
            let _ = fs::remove_file(dest.join(&name));
        }
    }
    Ok(())
}

fn content_root(package: &Path) -> PathBuf {
    if let Some(data) = resolve_case(package, Path::new("Data")) {
        if data.is_dir() {
            return data;
        }
    }
    package.to_path_buf()
}

fn is_data_folder(name: &str) -> bool {
    DATA_FOLDERS
        .iter()
        .any(|folder| folder.eq_ignore_ascii_case(name))
}

/// Step through wrapper folders such as `Campfire 1.12/` to the mod itself.
fn package_root(package: &Path) -> PathBuf {
    let mut current = package.to_path_buf();
    for _ in 0..3 {
        if fomod::find_module_config(&current).is_some() {
            return current;
        }
        if resolve_case(&current, Path::new("Data")).is_some_and(|path| path.is_dir()) {
            return current;
        }
        let Ok(entries) = fs::read_dir(&current) else {
            return current;
        };
        let useful: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        let wrapper = useful.len() == 1
            && useful[0].path().is_dir()
            && !is_data_folder(&useful[0].file_name().to_string_lossy());
        if !wrapper {
            return current;
        }
        current = useful[0].path();
    }
    current
}

fn extract_archive(source: &Path, dest: &Path) -> Result<(), ModError> {
    let name = lower_name(source);
    if name.ends_with(".7z") {
        sevenz_rust2::decompress_file(source, dest).map_err(|err| {
            ModError::new(format!("could not extract {}: {err}", item_name(source)))
        })?;
        return Ok(());
    }
    if name.ends_with(".zip") {
        return extract_zip(source, dest);
    }
    extract_external(source, dest)
}

/// Unpack with a tool from the system. SteamOS and Bazzite both ship `bsdtar`,
/// which reads RAR.
fn extract_external(source: &Path, dest: &Path) -> Result<(), ModError> {
    let mut output_flag = std::ffi::OsString::from("-o");
    output_flag.push(dest);
    let mut dest_slash = dest.as_os_str().to_os_string();
    dest_slash.push("/");
    let attempts: [(&str, Vec<&std::ffi::OsStr>); 5] = [
        (
            "bsdtar",
            vec![
                "-xf".as_ref(),
                source.as_ref(),
                "-C".as_ref(),
                dest.as_ref(),
            ],
        ),
        (
            "7z",
            vec![
                "x".as_ref(),
                "-y".as_ref(),
                "-bd".as_ref(),
                output_flag.as_ref(),
                source.as_ref(),
            ],
        ),
        (
            "7zz",
            vec![
                "x".as_ref(),
                "-y".as_ref(),
                "-bd".as_ref(),
                output_flag.as_ref(),
                source.as_ref(),
            ],
        ),
        (
            "unrar",
            vec![
                "x".as_ref(),
                "-y".as_ref(),
                "-idq".as_ref(),
                source.as_ref(),
                dest_slash.as_ref(),
            ],
        ),
        (
            "unar",
            vec![
                "-quiet".as_ref(),
                "-force-overwrite".as_ref(),
                "-output-directory".as_ref(),
                dest.as_ref(),
                source.as_ref(),
            ],
        ),
    ];
    let mut failure = None;
    for (tool, args) in attempts {
        let run = Command::new(tool)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match run {
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => failure = Some(format!("{tool}: {err}")),
            Ok(output) if output.status.success() => {
                drop_links(dest)?;
                return Ok(());
            }
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stderr);
                let line = text.lines().rev().find(|line| !line.trim().is_empty());
                failure = Some(format!("{tool}: {}", line.unwrap_or("failed").trim()));
                // A failed tool can leave part of the archive behind.
                let _ = fs::remove_dir_all(dest);
                fs::create_dir_all(dest)?;
            }
        }
    }
    Err(ModError::new(match failure {
        Some(detail) => format!("could not extract {} ({detail})", item_name(source)),
        None => format!(
            "{} needs bsdtar, 7z, or unrar to unpack, and none of them is installed.",
            item_name(source)
        ),
    }))
}

/// An outside tool may have unpacked symlinks. Nothing a mod needs is one, and
/// one that points out of the folder must never be followed.
fn drop_links(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            fs::remove_file(&path)?;
        } else if meta.is_dir() {
            drop_links(&path)?;
        }
    }
    Ok(())
}

fn extract_zip(source: &Path, dest: &Path) -> Result<(), ModError> {
    let file = fs::File::open(source)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| damaged(source, err))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| {
            ModError::new(format!(
                "could not read {} entry {index}: {err}",
                item_name(source)
            ))
        })?;
        let raw_name = entry
            .name()
            .map(|name| name.into_owned())
            .unwrap_or_else(|_| "<unreadable>".to_string());
        let Some(relative) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            return Err(ModError::new(format!(
                "archive entry escapes the folder: {raw_name}"
            )));
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(&target)?;
        io::copy(&mut entry, &mut output)?;
        // Keep the executable bit. A mod loader's launcher needs it.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            if mode & 0o111 != 0 {
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    Ok(())
}

pub fn is_archive(path: &Path) -> bool {
    let name = lower_name(path);
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn archive_name(source: &Path) -> String {
    let stem = if source.is_dir() {
        source.file_name()
    } else {
        source.file_stem()
    };
    stem.and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Mod")
        .to_string()
}

/// A readable name from a file name. Nexus appends the mod id, version, and a
/// timestamp: `SkyUI_5_2_SE-12604-5-2SE.7z` reads better as `SkyUI 5 2 SE`.
fn display_name(source: &Path) -> String {
    let stem = archive_name(source);
    let bytes = stem.as_bytes();
    let mut cut = stem.len();
    let mut index = 0;
    while let Some(found) = stem[index..].find('-') {
        let start = index + found;
        let digits = bytes[start + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let after = start + 1 + digits;
        let tail_ok = stem[after..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
        if digits > 0 && bytes.get(after) == Some(&b'-') && tail_ok && start >= 3 {
            cut = start;
            break;
        }
        index = start + 1;
    }
    let name = stem[..cut].replace('_', " ");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        stem
    } else {
        name
    }
}

pub(crate) fn slug(name: &str) -> String {
    let mut id = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if (ch == ' ' || ch == '-' || ch == '_') && !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "mod".to_string()
    } else {
        id
    }
}

fn unique_id(mods: &[ModEntry], base: &str) -> String {
    if !mods.iter().any(|entry| entry.id == base) {
        return base.to_string();
    }
    let mut number = 2;
    loop {
        let candidate = format!("{base}-{number}");
        if !mods.iter().any(|entry| entry.id == candidate) {
            return candidate;
        }
        number += 1;
    }
}

fn unique_path(directory: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("archive"));
    let candidate = directory.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let extension = source
        .extension()
        .filter(|_| !source.is_dir())
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    directory.join(format!(
        "{}-{}{extension}",
        slug(&archive_name(source)),
        unique_suffix()
    ))
}

fn move_path(source: &Path, dest: &Path) -> Result<(), ModError> {
    if fs::rename(source, dest).is_ok() {
        return Ok(());
    }
    if source.is_dir() {
        for relative in walk_files(source)? {
            copy_file(&source.join(&relative), &dest.join(&relative))?;
        }
        fs::remove_dir_all(source)?;
    } else {
        fs::copy(source, dest)?;
        fs::remove_file(source)?;
    }
    Ok(())
}

fn unique_suffix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_with_a_data_folder_lands_in_staging() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let zip_path = root.path().join("Campfire.zip");
        write_zip(
            &zip_path,
            &[("Data/Campfire.esm", b"camp"), ("readme.txt", b"hi")],
        );
        let report = install_source(&staging, &zip_path).unwrap();
        assert_eq!(report.installed.id, "campfire");
        assert!(report.installed.enabled);
        let installed = fs::read(staging.join("mods/campfire/Campfire.esm")).unwrap();
        assert_eq!(installed, b"camp");
        assert!(!staging.join("mods/campfire/readme.txt").exists());
        assert_eq!(report.installed.files, 1);
        // Nothing unpacked is left behind once the mod is in place.
        let leftovers = fs::read_dir(staging.join(".extract"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_lone_textures_folder_is_not_mistaken_for_a_wrapper() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let loose = root.path().join("Better Rocks.zip");
        write_zip(&loose, &[("textures/rocks/rock.dds", b"dds")]);
        install_source(&staging, &loose).unwrap();
        assert!(staging
            .join("mods/better-rocks/textures/rocks/rock.dds")
            .is_file());

        let wrapped = root.path().join("Wrapped.zip");
        write_zip(&wrapped, &[("Wrapped v2/meshes/a.nif", b"nif")]);
        install_source(&staging, &wrapped).unwrap();
        assert!(staging.join("mods/wrapped/meshes/a.nif").is_file());
    }

    #[test]
    fn nexus_file_names_become_readable_names() {
        let name = |text: &str| display_name(Path::new(text));
        assert_eq!(name("SkyUI_5_2_SE-12604-5-2SE.7z"), "SkyUI 5 2 SE");
        assert_eq!(
            name("Campfire 1.12.1SEVR Release-667-1-12-1SEVR-1576028570.zip"),
            "Campfire 1.12.1SEVR Release"
        );
        assert_eq!(name("Mod-2.zip"), "Mod-2");
        assert_eq!(name("A-10-Warthog.zip"), "A-10-Warthog");
        assert_eq!(name("Campfire.zip"), "Campfire");
    }

    #[test]
    fn a_download_remembers_its_nexus_page_and_archive() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let incoming = incoming_dir(&staging);
        fs::create_dir_all(&incoming).unwrap();
        let archive = incoming.join("Campfire-667-1-12.zip");
        write_zip(&archive, &[("Campfire.esm", b"camp")]);
        write_meta(
            &archive,
            &DownloadMeta {
                domain: "skyrimspecialedition".into(),
                mod_id: 667,
                file_id: 9,
                mod_name: "Campfire - Complete Camping System".into(),
                file_title: "Campfire Main".into(),
                version: "1.12".into(),
            },
        )
        .unwrap();
        let listed = list_downloads(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].module_name, "Campfire - Complete Camping System");
        assert_eq!(listed[0].nexus.as_ref().unwrap().mod_id, 667);

        let outcome = install_incoming(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        let entry = &outcome.installed[0].installed;
        assert_eq!(entry.name, "Campfire - Complete Camping System");
        assert_eq!(entry.id, "campfire-complete-camping-system");
        assert_eq!(entry.version, "1.12");
        assert_eq!(entry.nexus.as_ref().unwrap().file_id, 9);
        assert_eq!(entry.archive.as_deref(), Some("Campfire-667-1-12.zip"));
        assert!(staging.join("archives/Campfire-667-1-12.zip").is_file());
        assert!(!meta_path(&archive).exists());
        let saved = deploy::load_mods(&staging).unwrap();
        assert_eq!(saved[0].archive.as_deref(), Some("Campfire-667-1-12.zip"));
        assert_eq!(saved[0].file, "Campfire Main");

        // The same file downloaded again replaces the mod instead of adding a twin.
        deploy::set_enabled(&staging, &entry.id, false).unwrap();
        write_zip(&archive, &[("Campfire.esm", b"camp2"), ("Extra.esp", b"x")]);
        let meta = DownloadMeta {
            domain: "skyrimspecialedition".into(),
            mod_id: 667,
            file_id: 9,
            mod_name: "Campfire - Complete Camping System".into(),
            file_title: "Campfire Main".into(),
            version: "1.12".into(),
        };
        write_meta(&archive, &meta).unwrap();
        let again = install_incoming(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(again.installed[0].installed.id, entry.id);
        let saved = deploy::load_mods(&staging).unwrap();
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].enabled);
        assert_eq!(saved[0].files, 2);
        assert_eq!(
            fs::read(staging.join("mods").join(&entry.id).join("Campfire.esm")).unwrap(),
            b"camp2"
        );
        let archives: Vec<_> = fs::read_dir(staging.join("archives")).unwrap().collect();
        assert_eq!(archives.len(), 1);

        // A different file from the same page is its own mod.
        write_zip(&archive, &[("Patch.esp", b"p")]);
        write_meta(
            &archive,
            &DownloadMeta {
                file_id: 10,
                ..meta
            },
        )
        .unwrap();
        install_incoming(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(deploy::load_mods(&staging).unwrap().len(), 2);
    }

    #[test]
    fn a_broken_archive_does_not_block_the_others() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let incoming = incoming_dir(&staging);
        fs::create_dir_all(&incoming).unwrap();
        fs::write(incoming.join("Broken.zip"), b"not a zip").unwrap();
        fs::write(incoming.join(".Half.zip.partial"), b"PK").unwrap();
        write_zip(&incoming.join("Good.zip"), &[("Good.esp", b"good")]);
        let outcome = install_incoming(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(outcome.installed.len(), 1);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].name, "Broken.zip");
        assert!(incoming.join("Broken.zip").is_file());

        let listed = list_downloads(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].error.is_some());
        discard_download(&staging, &incoming.join("Broken.zip")).unwrap();
        assert!(!incoming.join("Broken.zip").exists());
        assert!(discard_download(&staging, &root.path().join("staging/mods.json")).is_err());
    }

    fn install_for(kind: Kind, staging: &Path, archive: &Path) -> Result<InstallReport, ModError> {
        install_with(
            staging,
            archive,
            &InstalledFiles::empty(),
            &[],
            &InstallRules::new(kind),
        )
    }

    #[test]
    fn a_generic_game_gets_the_archive_as_it_is() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let archive = root.path().join("Big Pack.zip");
        write_zip(
            &archive,
            &[("BigPack/info.json", b"{}"), ("BigPack/a.pak", b"a")],
        );
        let report = install_for(Kind::Generic, &staging, &archive).unwrap();
        assert_eq!(report.installed.files, 2);
        // The wrapper folder is the mod. It is not peeled off.
        assert!(staging.join("mods/big-pack/BigPack/a.pak").is_file());
    }

    #[test]
    fn game_rules_decide_where_files_go() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");

        let witcher = root.path().join("Fast Travel.zip");
        write_zip(
            &witcher,
            &[
                (
                    "FastTravel v3/mods/modFastTravel/content/scripts/map.ws",
                    b"ws",
                ),
                ("FastTravel v3/readme.txt", b"hi"),
            ],
        );
        install_for(Kind::Witcher3, &staging, &witcher).unwrap();
        assert!(staging
            .join("mods/fast-travel/mods/modFastTravel/content/scripts/map.ws")
            .is_file());
        assert!(!staging.join("mods/fast-travel/readme.txt").exists());

        let cyberpunk = root.path().join("Hair.zip");
        write_zip(&cyberpunk, &[("hair.archive", b"a"), ("preview.png", b"p")]);
        install_for(Kind::Cyberpunk2077, &staging, &cyberpunk).unwrap();
        assert!(staging
            .join("mods/hair/archive/pc/mod/hair.archive")
            .is_file());

        let wrong = root.path().join("Notes.zip");
        write_zip(&wrong, &[("readme.txt", b"hi")]);
        let err = install_for(Kind::Cyberpunk2077, &staging, &wrong).unwrap_err();
        assert!(err.to_string().contains("Cyberpunk 2077 mod"), "{err}");
        assert!(!staging.join("mods/notes").exists());
        assert_eq!(deploy::load_mods(&staging).unwrap().len(), 2);
    }

    #[test]
    fn a_fomod_for_another_game_is_laid_out_after_the_choices() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let archive = root.path().join("Jacket.zip");
        let xml = br#"<config><moduleName>Jacket</moduleName><requiredInstallFiles><file source="files/jacket.archive" destination="jacket.archive"/></requiredInstallFiles></config>"#;
        write_zip(
            &archive,
            &[
                ("fomod/ModuleConfig.xml", xml),
                ("files/jacket.archive", b"a"),
            ],
        );
        let report = install_for(Kind::Cyberpunk2077, &staging, &archive).unwrap();
        assert_eq!(report.installed.name, "Jacket");
        assert!(staging
            .join("mods/jacket/archive/pc/mod/jacket.archive")
            .is_file());
        assert!(!staging.join(".extract").join("x").exists());
    }

    #[test]
    fn stardew_mods_and_smapi_install() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let game = root.path().join("Stardew Valley");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("Stardew Valley.deps.json"), b"{\"deps\":1}").unwrap();

        let pack = root.path().join("Farm Pack.zip");
        write_zip(
            &pack,
            &[
                ("Farm Pack/FarmCore/manifest.json", b"{}"),
                ("Farm Pack/FarmCore/FarmCore.dll", b"dll"),
                ("Farm Pack/readme.txt", b"hi"),
            ],
        );
        let report = install_for(Kind::Stardew, &staging, &pack).unwrap();
        assert_eq!(report.installed.files, 2);
        assert!(staging
            .join("mods/farm-pack/FarmCore/FarmCore.dll")
            .is_file());

        // The SMAPI download is an installer with a zip of Linux files inside.
        let payload = root.path().join("install.dat");
        write_zip(
            &payload,
            &[
                ("StardewModdingAPI", b"#!/bin/sh"),
                ("StardewModdingAPI.dll", b"dll"),
                ("Mods/ConsoleCommands/manifest.json", b"{}"),
            ],
        );
        let smapi = root.path().join("SMAPI 4.1.10-2400-4-1-10.zip");
        let dat = fs::read(&payload).unwrap();
        write_zip(
            &smapi,
            &[
                ("SMAPI 4.1.10 installer/install on Linux.sh", b"sh"),
                ("SMAPI 4.1.10 installer/internal/linux/install.dat", &dat),
                ("SMAPI 4.1.10 installer/internal/windows/install.dat", b"no"),
            ],
        );
        let rules = InstallRules {
            game_root: Some(game.clone()),
            ..InstallRules::new(Kind::Stardew)
        };
        let report = install_with(&staging, &smapi, &InstalledFiles::empty(), &[], &rules).unwrap();
        assert_eq!(report.installed.files, 4);
        let staged = staging
            .join("mods")
            .join(&report.installed.id)
            .join(".targets/root");
        assert!(staged.join("Mods/ConsoleCommands/manifest.json").is_file());
        assert_eq!(
            fs::read(staged.join("StardewModdingAPI.deps.json")).unwrap(),
            b"{\"deps\":1}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(staged.join("StardewModdingAPI"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }

        // Deployed, the loader lands next to the game and mods in Mods.
        let mut roots = deploy::DeployRoots::single(game.join("Mods"));
        roots.targets.insert("root".into(), game.clone());
        deploy::deploy(&staging, &roots).unwrap();
        assert!(game.join("StardewModdingAPI").is_file());
        assert!(game.join("Mods/FarmCore/FarmCore.dll").is_file());
        assert!(game.join("Mods/ConsoleCommands/manifest.json").is_file());
    }

    #[test]
    fn utf16_installers_are_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ModuleConfig.xml");
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><config><moduleName>Wide</moduleName></config>";
        let mut bytes = vec![0xff, 0xfe];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        let xml = read_xml(&path).unwrap();
        assert!(xml.starts_with("<config>"), "{xml}");
    }

    #[test]
    fn a_choice_installer_stays_in_incoming() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let incoming = incoming_dir(&staging);
        fs::create_dir_all(&incoming).unwrap();
        write_zip(&incoming.join("Plain.zip"), &[("Data/Plain.esp", b"plain")]);
        let xml = br#"<config><moduleName>Campfire</moduleName><installSteps><installStep name="Options"><optionalFileGroups><group name="Version" type="SelectExactlyOne"><plugins><plugin name="Full"><files><file source="full.txt" destination="full.txt"/></files><typeDescriptor><type name="Recommended"/></typeDescriptor></plugin><plugin name="Lite"><files><file source="lite.txt" destination="lite.txt"/></files><typeDescriptor><type name="Optional"/></typeDescriptor></plugin></plugins></group></optionalFileGroups></installStep></installSteps></config>"#;
        write_zip(
            &incoming.join("Campfire.zip"),
            &[
                ("fomod/ModuleConfig.xml", xml),
                ("full.txt", b"full"),
                ("lite.txt", b"lite"),
            ],
        );
        let outcome = install_incoming(
            &staging,
            &InstalledFiles::empty(),
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(outcome.installed.len(), 1);
        assert_eq!(outcome.installed[0].installed.id, "plain");
        assert_eq!(outcome.pending.len(), 1);
        assert_eq!(outcome.pending[0].module_name, "Campfire");
        assert!(incoming.join("Campfire.zip").is_file());
        assert!(!incoming.join("Plain.zip").exists());

        let choices = [FomodChoice {
            step: "Options".into(),
            group: "Version".into(),
            plugins: vec!["Lite".into()],
        }];
        let report = install_with(
            &staging,
            &incoming.join("Campfire.zip"),
            &InstalledFiles::empty(),
            &choices,
            &InstallRules::bethesda(),
        )
        .unwrap();
        assert_eq!(report.installed.name, "Campfire");
        assert_eq!(
            fs::read(staging.join("mods/campfire/lite.txt")).unwrap(),
            b"lite"
        );
        assert!(!staging.join("mods/campfire/full.txt").exists());
        assert!(!incoming.join("Campfire.zip").exists());
    }

    #[test]
    fn a_script_extender_is_staged_for_the_game_folder() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let archive = temp.path().join("skse_1_07_03.zip");
        write_zip(
            &archive,
            &[
                ("skse_1_07_03/skse_loader.exe", b"loader"),
                ("skse_1_07_03/skse_1_9_32.dll", b"dll"),
                ("skse_1_07_03/skse_readme.txt", b"docs"),
                ("skse_1_07_03/src/skse/main.cpp", b"source"),
                ("skse_1_07_03/Data/Scripts/actor.pex", b"script"),
            ],
        );
        let report = install_with(
            &staging,
            &archive,
            &InstalledFiles::empty(),
            &[],
            &InstallRules::bethesda(),
        )
        .unwrap();
        let staged = staging.join("mods").join(&report.installed.id);
        assert!(staged.join("Scripts/actor.pex").is_file());
        assert!(staged.join(".targets/root/skse_loader.exe").is_file());
        assert!(staged.join(".targets/root/skse_1_9_32.dll").is_file());
        assert!(!staged.join(".targets/root/skse_readme.txt").exists());
        assert!(!staged.join("skse_loader.exe").exists());

        // An archive of nothing but libraries has no `Data` folder to go by.
        let fixes = temp.path().join("EngineFixes.zip");
        write_zip(&fixes, &[("d3dx9_42.dll", b"a"), ("tbb.dll", b"b")]);
        let report = install_with(
            &staging,
            &fixes,
            &InstalledFiles::empty(),
            &[],
            &InstallRules::bethesda(),
        )
        .unwrap();
        let staged = staging.join("mods").join(&report.installed.id);
        assert!(staged.join(".targets/root/tbb.dll").is_file());
        assert!(!staged.join("tbb.dll").exists());
    }

    #[test]
    fn zip_slip_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let zip_path = root.path().join("evil.zip");
        write_zip(&zip_path, &[("../evil.txt", b"nope")]);
        let err = install_source(&root.path().join("staging"), &zip_path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("escapes"), "{message}");
        assert!(!root.path().join("evil.txt").exists());
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in files {
            zip.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut zip, bytes).unwrap();
        }
        zip.finish().unwrap();
    }
}
