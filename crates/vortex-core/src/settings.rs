//! Lines a mod needs in one of the game's own settings files.
//!
//! A Witcher 3 mod with a menu or a hotkey ships a snippet, such as
//! `MCME.input.settings`, and its instructions say to paste it into
//! `input.settings` in the Documents folder. On Windows the player does that
//! by hand. Here the snippets of every enabled mod are merged into the file,
//! section by section, and taken out again when the mod is turned off.
//!
//! Only lines this app added are ever removed. They are remembered in
//! `settings.json` in the staging folder. The file as it was before the first
//! change is kept next to it as `<name>.vortex_backup`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::deploy::{self, ModEntry, ModError};

/// What a merge did, for the shell to tell the player.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SettingsReport {
    /// The settings file, such as `input.settings`.
    pub name: String,
    pub path: PathBuf,
    /// Lines put in and taken out by this run.
    pub added: usize,
    pub removed: usize,
    /// Lines of ours that are in the file after this run.
    pub total: usize,
    /// Mods that brought lines for this file.
    pub mods: Vec<String>,
    /// Why nothing could be done, when something should have been.
    pub warning: Option<String>,
}

/// Lines this app has put into each settings file: file -> section -> lines.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    files: BTreeMap<String, Vec<(String, String)>>,
}

fn record_path(staging: &Path) -> PathBuf {
    staging.join("settings.json")
}

fn load_record(staging: &Path) -> Record {
    fs::read_to_string(record_path(staging))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Whether `file_name` is a snippet for the settings file `name`:
/// `input.settings`, `MyMod.input.settings`, or either with `.txt` added.
pub fn is_snippet(file_name: &str, name: &str) -> bool {
    let lower = file_name.to_lowercase();
    let lower = lower.strip_suffix(".txt").unwrap_or(&lower);
    let name = name.to_lowercase();
    lower == name
        || lower
            .strip_suffix(&name)
            .is_some_and(|front| front.ends_with('.') || front.ends_with('_') || front.ends_with('-'))
}

/// `(section, line)` pairs of a snippet, in order. Comments, blank lines, and
/// anything before the first section are left out.
fn parse_snippet(text: &str) -> Vec<(String, String)> {
    let mut section = None;
    let mut lines = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') || line.starts_with("//")
        {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = Some(line[1..line.len() - 1].trim().to_string());
        } else if let Some(section) = &section {
            // A binding or a value. Prose from a readme-style snippet is not.
            if line.contains('=') {
                lines.push((section.clone(), line.to_string()));
            }
        }
    }
    lines
}

fn snippets_in(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            snippets_in(&path, name, found);
        } else if meta.is_file()
            && path
                .file_name()
                .is_some_and(|file| is_snippet(&file.to_string_lossy(), name))
        {
            found.push(path);
        }
    }
}

/// Bring `target` in line with the snippets of the enabled mods.
pub fn merge_settings(
    staging: &Path,
    mods: &[ModEntry],
    name: &str,
    target: &Path,
    game_running: bool,
) -> Result<SettingsReport, ModError> {
    let mut report = SettingsReport {
        name: name.to_string(),
        path: target.to_path_buf(),
        ..Default::default()
    };
    let mut wanted: Vec<(String, String)> = Vec::new();
    for entry in mods.iter().filter(|entry| entry.enabled) {
        let mut files = Vec::new();
        snippets_in(&deploy::mod_dir(staging, &entry.id), name, &mut files);
        let mut brought = false;
        for file in files {
            let Ok(bytes) = fs::read(&file) else {
                continue;
            };
            for pair in parse_snippet(&String::from_utf8_lossy(&bytes)) {
                brought = true;
                if !wanted.contains(&pair) {
                    wanted.push(pair);
                }
            }
        }
        if brought {
            report.mods.push(entry.name.clone());
        }
    }
    let mut record = load_record(staging);
    let ours = record.files.remove(name).unwrap_or_default();
    if wanted.is_empty() && ours.is_empty() {
        return Ok(report);
    }
    if !target.is_file() {
        record.files.insert(name.to_string(), ours);
        report.warning = Some(format!(
            "{name} is not there yet. Start the game once so it writes its settings, then come back and the mods' key bindings are added."
        ));
        return Ok(report);
    }
    let bytes = fs::read(target)?;
    let Ok(text) = String::from_utf8(bytes) else {
        record.files.insert(name.to_string(), ours);
        report.warning = Some(format!(
            "{name} is not plain text, so the mods' lines were not added to it."
        ));
        return Ok(report);
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // Take out what we added for mods that are off or gone now.
    for (section, line) in &ours {
        if wanted.contains(&(section.clone(), line.clone())) {
            continue;
        }
        if let Some((start, end)) = section_span(&lines, section) {
            if let Some(at) = (start..end).find(|&at| lines[at].trim() == line) {
                lines.remove(at);
                report.removed += 1;
            }
        }
    }
    // Put in what is missing. A line that is already there and is not on
    // record was the player's or the game's, and stays theirs.
    let mut kept: Vec<(String, String)> = Vec::new();
    for (section, line) in &wanted {
        let pair = (section.clone(), line.clone());
        let span = section_span(&lines, section);
        let present = span.is_some_and(|(start, end)| lines[start..end].iter().any(|have| have.trim() == line));
        if present {
            if ours.contains(&pair) {
                kept.push(pair);
            }
            continue;
        }
        match span {
            Some((start, end)) => {
                // After the section's last line, before the blank lines
                // that separate it from the next one.
                let mut at = end;
                while at > start && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, line.clone());
            }
            None => {
                if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{section}]"));
                lines.push(line.clone());
                lines.push(String::new());
            }
        }
        report.added += 1;
        kept.push(pair);
    }
    report.total = kept.len();

    if game_running && (report.added > 0 || report.removed > 0) {
        // The game holds its settings in memory and writes them back when it
        // exits, which would undo the change. Nothing is touched until then.
        report.added = 0;
        report.removed = 0;
        report.total = ours.len();
        report.warning = Some(format!(
            "The game is running, so {name} was left alone. Close the game and the mods' lines are sorted out the next time this screen is opened."
        ));
        return Ok(report);
    }
    if report.added > 0 || report.removed > 0 {
        let backup = target.with_file_name(format!("{name}.vortex_backup"));
        if fs::symlink_metadata(&backup).is_err() {
            fs::copy(target, &backup)?;
        }
        let mut out = lines.join(newline);
        out.push_str(newline);
        deploy::write_atomic(target, &out)?;
    }
    if !kept.is_empty() {
        record.files.insert(name.to_string(), kept);
    }
    if record.files.is_empty() {
        let _ = fs::remove_file(record_path(staging));
    } else {
        fs::create_dir_all(staging)?;
        deploy::write_atomic(&record_path(staging), &serde_json::to_string_pretty(&record)?)?;
    }
    Ok(report)
}

/// Lines of the section called `name`: from just after its header to just
/// before the next header. Section names are matched without regard to case.
fn section_span(lines: &[String], name: &str) -> Option<(usize, usize)> {
    let is_header = |line: &str| {
        let line = line.trim();
        line.starts_with('[') && line.ends_with(']')
    };
    let header = lines.iter().position(|line| {
        let line = line.trim();
        is_header(line) && line[1..line.len() - 1].trim().eq_ignore_ascii_case(name)
    })?;
    let end = lines[header + 1..]
        .iter()
        .position(|line| is_header(line))
        .map_or(lines.len(), |offset| header + 1 + offset);
    Some((header + 1, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, enabled: bool) -> ModEntry {
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"{id}","enabled":{enabled}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn snippets_are_told_by_name() {
        assert!(is_snippet("input.settings", "input.settings"));
        assert!(is_snippet("MCME.input.settings", "input.settings"));
        assert!(is_snippet("MCME_AR.input.settings.txt", "input.settings"));
        assert!(is_snippet("mod_input.settings", "input.settings"));
        assert!(!is_snippet("user.settings", "input.settings"));
        assert!(!is_snippet("myinput.settings", "input.settings"));
        assert!(!is_snippet("input.settings.bak", "input.settings"));
    }

    #[test]
    fn bindings_go_in_and_come_out_with_the_mod() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let target = root.path().join("input.settings");
        let original = "[Exploration]\r\nIK_Z=(Action=CastSign)\r\nIK_X=(Action=Mine)\r\n\r\n[Combat]\r\nIK_Q=(Action=Dodge)\r\n";
        fs::write(&target, original).unwrap();
        let snippet = staging.join("mods/menu/mods/modMenu");
        fs::create_dir_all(&snippet).unwrap();
        fs::write(
            snippet.join("Menu.input.settings"),
            "; paste into input.settings\n[Exploration]\nIK_Z=(Action=OpenMenu)\nIK_X=(Action=Mine)\n\n[MenuBase]\nIK_Up=(Action=MenuUp)\n",
        )
        .unwrap();

        let on = [entry("menu", true)];
        let report = merge_settings(&staging, &on, "input.settings", &target, false).unwrap();
        assert_eq!((report.added, report.removed, report.total), (2, 0, 2));
        assert_eq!(report.mods, ["menu"]);
        let merged = fs::read_to_string(&target).unwrap();
        assert_eq!(
            merged,
            "[Exploration]\r\nIK_Z=(Action=CastSign)\r\nIK_X=(Action=Mine)\r\nIK_Z=(Action=OpenMenu)\r\n\r\n[Combat]\r\nIK_Q=(Action=Dodge)\r\n\r\n[MenuBase]\r\nIK_Up=(Action=MenuUp)\r\n\r\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("input.settings.vortex_backup")).unwrap(),
            original
        );

        // Nothing more to do the second time.
        let again = merge_settings(&staging, &on, "input.settings", &target, false).unwrap();
        assert_eq!((again.added, again.removed, again.total), (0, 0, 2));

        // Not while the game has the file in memory.
        let off = [entry("menu", false)];
        let report = merge_settings(&staging, &off, "input.settings", &target, true).unwrap();
        assert!(report.warning.is_some());
        assert_eq!(fs::read_to_string(&target).unwrap(), merged);

        // Off: our lines go, the player's own identical line stays.
        let report = merge_settings(&staging, &off, "input.settings", &target, false).unwrap();
        assert_eq!((report.added, report.removed, report.total), (0, 2, 0));
        let cleaned = fs::read_to_string(&target).unwrap();
        assert!(cleaned.contains("IK_X=(Action=Mine)"));
        assert!(!cleaned.contains("OpenMenu") && !cleaned.contains("MenuUp"));
        assert!(!staging.join("settings.json").exists());
    }

    #[test]
    fn a_game_that_has_not_run_yet_gets_a_warning() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("staging");
        let snippet = staging.join("mods/menu");
        fs::create_dir_all(&snippet).unwrap();
        fs::write(snippet.join("input.settings"), "[Exploration]\nIK_Z=(Action=OpenMenu)\n").unwrap();
        let report = merge_settings(
            &staging,
            &[entry("menu", true)],
            "input.settings",
            &root.path().join("input.settings"),
            false,
        )
        .unwrap();
        assert!(report.warning.is_some());
        assert_eq!(report.added, 0);
    }
}
