//! JSON commands for the Godot shell.
//!
//! stdout is one JSON document. Diagnostics go to stderr.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Serialize;
use vortex_core::{
    all_games, clear_key, current_key, deploy, discard_download, discover, discover_roots,
    download_mod_file, game_by_id, game_ids, incoming_dir, install_incoming, install_with,
    list_downloads, list_mod_files, load_mods, move_mod, move_plugin, native_plugin_names,
    preview_source, purge, read_lists, reconcile_plugins, remove_mod, search_mods,
    set_plugin_enabled, store_key, toggle_mod, validate_account, DeployReport, DownloadItem,
    FailedInstall, FixtureTransport, FomodChoice, GameDef, InstallRules, InstalledFiles,
    InstalledGame, LiveTransport, ModEntry, NexusTransport, NxmLink, Plugin, PluginFiles,
    PluginRules, SavedDownload,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("discover") => print_json(&discover()),
        Some("games") => print_json(
            &all_games()
                .iter()
                .map(|game| SupportedGame {
                    id: &game.id,
                    name: &game.name,
                    nexus_domain: &game.nexus_domain,
                })
                .collect::<Vec<_>>(),
        ),
        Some("sync") => {
            let game = require_game(&args)?;
            print_json(&activate(&game, true)?)
        }
        Some("toggle") => {
            let game = require_game(&args)?;
            let id = flag(&args, "--mod").ok_or("toggle requires --mod")?;
            toggle_mod(&game.staging_path, &id).map_err(text)?;
            print_json(&activate(&game, false)?)
        }
        Some("move") => {
            let game = require_game(&args)?;
            let id = flag(&args, "--mod").ok_or("move requires --mod")?;
            move_mod(&game.staging_path, &id, require_i64(&args, "--by")?).map_err(text)?;
            print_json(&activate(&game, false)?)
        }
        Some("remove") => {
            let game = require_game(&args)?;
            let id = flag(&args, "--mod").ok_or("remove requires --mod")?;
            let keep = args.iter().any(|arg| arg == "--keep-archive");
            ready(&game)?;
            remove_mod(&game.staging_path, &game.deploy_roots(), &id, keep).map_err(text)?;
            print_json(&activate(&game, false)?)
        }
        Some("purge") => {
            let game = require_game(&args)?;
            print_json(&purge(&game.staging_path, &game.deploy_roots()).map_err(text)?)
        }
        Some("install") => {
            let game = require_game(&args)?;
            let source = flag(&args, "--from").ok_or("install requires --from")?;
            let choices = load_choices(&args)?;
            ready(&game)?;
            let report = install_with(
                &game.staging_path,
                source.as_ref(),
                &installed_files(&game),
                &choices,
                &rules(&game),
            )
            .map_err(text)?;
            let mut sync = activate(&game, false)?;
            sync.installed.push(report.installed);
            print_json(&sync)
        }
        Some("downloads") => {
            let game = require_game(&args)?;
            print_json(&downloads_report(&game)?)
        }
        Some("discard") => {
            let game = require_game(&args)?;
            let source = flag(&args, "--from").ok_or("discard requires --from")?;
            discard_download(&game.staging_path, source.as_ref()).map_err(text)?;
            print_json(&downloads_report(&game)?)
        }
        Some("fomod") => {
            let game = require_game(&args)?;
            let source = flag(&args, "--from").ok_or("fomod requires --from")?;
            let choices = load_choices(&args)?;
            let plan = preview_source(
                &game.staging_path,
                source.as_ref(),
                &installed_files(&game),
                &choices,
            )
            .map_err(text)?;
            print_json(&plan)
        }
        Some("plugins") => plugins(&args[1..]),
        Some("nexus") => nexus(&args[1..]),
        Some("-V") | Some("--version") => {
            println!("vortex-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("-h") | Some("--help") | None => {
            print!("{}", help_text());
            Ok(())
        }
        Some(other) => Err(format!("unknown command '{other}'\n\n{}", help_text())),
    }
}

fn text(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[derive(Serialize)]
struct SupportedGame {
    id: &'static str,
    name: &'static str,
    nexus_domain: &'static str,
}

#[derive(Serialize)]
struct SyncReport {
    game: InstalledGame,
    mods: Vec<ModEntry>,
    plugins: Vec<Plugin>,
    plugin_warning: Option<String>,
    deploy: DeployReport,
    installed: Vec<ModEntry>,
    pending: Vec<DownloadItem>,
    failed: Vec<FailedInstall>,
    incoming: PathBuf,
}

#[derive(Serialize)]
struct DownloadsReport {
    incoming: PathBuf,
    items: Vec<DownloadItem>,
}

#[derive(Serialize)]
struct PluginsReport {
    plugins: Vec<Plugin>,
}

#[derive(Serialize)]
struct NxmReport {
    game: String,
    game_name: String,
    #[serde(flatten)]
    saved: SavedDownload,
}

const NEEDS_PREFIX: &str = "Launch the game once from Steam so Proton creates its prefix. Mods are deployed, and the plugin list will be written after that.";

fn rules(game: &InstalledGame) -> InstallRules {
    InstallRules {
        kind: game.kind,
        install: game_by_id(&game.id)
            .map(|def| def.install.clone())
            .unwrap_or_default(),
        game_root: Some(game.install_path.clone()),
    }
}

/// A game whose mod folder is inside its Proton prefix cannot take mods
/// until the prefix exists.
fn ready(game: &InstalledGame) -> Result<(), String> {
    if game.needs_first_launch {
        Err(format!(
            "Launch {} once from Steam first. Its mod folder is created on the first launch.",
            game.name
        ))
    } else {
        Ok(())
    }
}

/// The plugin list files of a game that has them and has been launched.
fn plugin_files<'a>(
    game: &'a InstalledGame,
    rules: &'a PluginRules,
    extensions: &'a [&'a str],
) -> Option<PluginFiles<'a>> {
    if !game.proton.prefix_exists {
        return None;
    }
    Some(PluginFiles {
        data_dir: &game.data_path,
        plugins_txt: game.proton.plugins_txt.as_deref()?,
        loadorder_txt: game.proton.loadorder_txt.as_deref()?,
        format: rules.format,
        native: &game.native_plugins,
        extensions,
    })
}

fn extensions(rules: &PluginRules) -> Vec<&str> {
    rules.extensions.iter().map(String::as_str).collect()
}

fn definition(game: &InstalledGame) -> Result<&'static GameDef, String> {
    game_by_id(&game.id).ok_or_else(|| format!("unknown game '{}'", game.id))
}

fn downloads_report(game: &InstalledGame) -> Result<DownloadsReport, String> {
    let items =
        list_downloads(&game.staging_path, &installed_files(game), &rules(game)).map_err(text)?;
    Ok(DownloadsReport {
        incoming: incoming_dir(&game.staging_path),
        items,
    })
}

fn activate(game: &InstalledGame, take_incoming: bool) -> Result<SyncReport, String> {
    ready(game)?;
    let files = installed_files(game);
    let rules = rules(game);
    let (installed, pending, failed) = if take_incoming {
        let outcome = install_incoming(&game.staging_path, &files, &rules).map_err(text)?;
        (
            outcome
                .installed
                .into_iter()
                .map(|report| report.installed)
                .collect(),
            outcome.pending,
            outcome.failed,
        )
    } else {
        // Nothing is installed here, but what is waiting is still reported so
        // the shell's picture of the incoming folder stays whole.
        let mut pending = Vec::new();
        let mut failed = Vec::new();
        for item in list_downloads(&game.staging_path, &files, &rules).map_err(text)? {
            match item.error {
                Some(error) => failed.push(FailedInstall {
                    name: item.name,
                    error,
                }),
                None if item.needs_choices => pending.push(item),
                None => {}
            }
        }
        (Vec::new(), pending, failed)
    };
    let deploy_report = deploy(&game.staging_path, &game.deploy_roots()).map_err(text)?;
    let definition = definition(game)?;
    let (plugins, plugin_warning) = match &definition.plugins {
        None => (Vec::new(), None),
        Some(rules) => match plugin_files(game, rules, &extensions(rules)) {
            Some(lists) => (
                reconcile_plugins(
                    lists.data_dir,
                    lists.plugins_txt,
                    lists.loadorder_txt,
                    lists.format,
                    lists.native,
                    lists.extensions,
                )
                .map_err(text)?,
                None,
            ),
            None => (Vec::new(), Some(NEEDS_PREFIX.to_string())),
        },
    };
    let mods = load_mods(&game.staging_path).map_err(text)?;
    Ok(SyncReport {
        game: game.clone(),
        mods,
        plugins,
        plugin_warning,
        deploy: deploy_report,
        installed,
        pending,
        failed,
        incoming: incoming_dir(&game.staging_path),
    })
}

fn installed_files(game: &InstalledGame) -> InstalledFiles {
    let mut enabled = game.native_plugins.clone();
    let rules = game_by_id(&game.id).and_then(|def| def.plugins.as_ref());
    if let (Some(rules), Some(list)) = (rules, &game.proton.plugins_txt) {
        if let Ok(bytes) = std::fs::read(list) {
            let text = String::from_utf8_lossy(&bytes);
            for plugin in read_lists(&text, None, rules.format, &game.native_plugins) {
                if plugin.enabled {
                    enabled.push(plugin.name);
                }
            }
        }
    }
    InstalledFiles::new(&game.data_path, enabled)
}

fn load_choices(args: &[String]) -> Result<Vec<FomodChoice>, String> {
    let Some(path) = flag(args, "--choices") else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?;
    serde_json::from_str(&text).map_err(|err| format!("{path}: {err}"))
}

fn discovered(args: &[String]) -> Vec<InstalledGame> {
    match flag(args, "--root") {
        Some(root) => discover_roots(&[PathBuf::from(root)]).games,
        None => discover().games,
    }
}

fn require_game(args: &[String]) -> Result<InstalledGame, String> {
    let name =
        flag(args, "--game").ok_or(format!("--game is required. Use one of: {}.", game_ids()))?;
    let def = game_by_id(&name).ok_or(format!(
        "unknown game '{name}'. Use one of: {}.",
        game_ids()
    ))?;
    discovered(args)
        .into_iter()
        .find(|game| game.id == def.id)
        .ok_or_else(|| format!("No {} install found in Steam.", def.name))
}

/// Writes download progress to a file the shell polls. Never more than a few
/// times a second, so a fast connection does not turn into disk churn.
fn progress_writer(args: &[String]) -> impl FnMut(u64, Option<u64>) {
    let path = flag(args, "--progress");
    let mut last: Option<Instant> = None;
    move |received, total| {
        let Some(path) = &path else {
            return;
        };
        let done = total == Some(received);
        if !done && last.is_some_and(|at| at.elapsed() < Duration::from_millis(150)) {
            return;
        }
        last = Some(Instant::now());
        let _ = std::fs::write(
            path,
            format!(
                "{{\"received\":{received},\"total\":{}}}",
                total.unwrap_or(0)
            ),
        );
    }
}

fn nexus(args: &[String]) -> Result<(), String> {
    let transport = nexus_transport()?;
    match args.first().map(String::as_str) {
        Some("status") => {
            let Some(key) = current_key().map_err(text)? else {
                return print_json(&vortex_core::Account {
                    signed_in: false,
                    name: String::new(),
                    premium: false,
                });
            };
            let account = validate_account(transport.as_ref(), &key).map_err(text)?;
            print_json(&account)
        }
        Some("store-key") => {
            let source = flag(args, "--from").ok_or("nexus store-key requires --from <file>")?;
            let account = store_key(Path::new(&source), transport.as_ref()).map_err(text)?;
            print_json(&account)
        }
        Some("clear-key") => {
            let cleared = clear_key().map_err(text)?;
            print_json(&cleared)
        }
        Some("search") => {
            let game = require_game(args)?;
            let query = flag(args, "--query").unwrap_or_default();
            let key = current_key().map_err(text)?;
            let report = search_mods(
                transport.as_ref(),
                &game.nexus_domain,
                &query,
                key.as_deref(),
            )
            .map_err(text)?;
            print_json(&report)
        }
        Some("files") => {
            let game = require_game(args)?;
            let mod_id = require_u64(args, "--mod")?;
            let key = require_nexus_key()?;
            let files = list_mod_files(transport.as_ref(), &game.nexus_domain, mod_id, &key)
                .map_err(text)?;
            print_json(&files)
        }
        Some("download") => {
            let game = require_game(args)?;
            let mod_id = require_u64(args, "--mod")?;
            let file_id = require_u64(args, "--file")?;
            let key = require_nexus_key()?;
            let saved = download_mod_file(
                transport.as_ref(),
                &NxmLink::direct(&game.nexus_domain, mod_id, file_id),
                &incoming_dir(&game.staging_path),
                &key,
                &mut progress_writer(args),
            )
            .map_err(text)?;
            print_json(&saved)
        }
        Some("nxm") => {
            let link = match (flag(args, "--from"), flag(args, "--url")) {
                (Some(path), _) => {
                    std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?
                }
                (None, Some(url)) => url,
                (None, None) => return Err("nexus nxm requires --from <file> or --url".into()),
            };
            let link = NxmLink::parse(&link).map_err(text)?;
            let prefer = flag(args, "--prefer").unwrap_or_default();
            let mut games: Vec<InstalledGame> = discovered(args)
                .into_iter()
                .filter(|game| game.nexus_domain == link.domain)
                .collect();
            // Two games can share a Nexus page, such as Skyrim SE and Skyrim VR.
            games.sort_by_key(|game| game.id != prefer);
            let Some(game) = games.into_iter().next() else {
                let known = all_games()
                    .iter()
                    .find(|game| game.nexus_domain == link.domain);
                return Err(match known {
                    Some(def) => format!(
                        "That download is for {}, which is not installed in Steam.",
                        def.name
                    ),
                    None => format!(
                        "That download is for '{}', a game this app does not manage yet.",
                        link.domain
                    ),
                });
            };
            let key = require_nexus_key()?;
            let saved = download_mod_file(
                transport.as_ref(),
                &link,
                &incoming_dir(&game.staging_path),
                &key,
                &mut progress_writer(args),
            )
            .map_err(text)?;
            print_json(&NxmReport {
                game: game.id,
                game_name: game.name,
                saved,
            })
        }
        _ => Err(help_text()),
    }
}

fn nexus_transport() -> Result<Box<dyn NexusTransport>, String> {
    if let Ok(path) = std::env::var("VORTEX_NEXUS_FIXTURE") {
        if !path.is_empty() {
            return Ok(Box::new(FixtureTransport::open(path)));
        }
    }
    Ok(Box::new(LiveTransport::new()))
}

fn require_nexus_key() -> Result<String, String> {
    current_key()
        .map_err(text)?
        .ok_or_else(|| "Sign in to Nexus first. Open Downloads, then Nexus account.".to_string())
}

fn require_u64(args: &[String], name: &str) -> Result<u64, String> {
    let text = flag(args, name).ok_or_else(|| format!("nexus requires {name}"))?;
    text.parse::<u64>()
        .map_err(|_| format!("{name} must be a number"))
}

fn require_i64(args: &[String], name: &str) -> Result<i64, String> {
    let text = flag(args, name).ok_or_else(|| format!("{name} is required"))?;
    text.parse::<i64>()
        .map_err(|_| format!("{name} must be a whole number"))
}

fn plugins(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("read") => plugins_read(args),
        Some(action @ ("toggle" | "move")) => {
            let game = require_game(args)?;
            let name = flag(args, "--plugin").ok_or("plugins needs --plugin <file name>")?;
            let def = definition(&game)?;
            let Some(rules) = &def.plugins else {
                return Err(format!("{} does not use a plugin list.", game.name));
            };
            let extensions = extensions(rules);
            let Some(files) = plugin_files(&game, rules, &extensions) else {
                return Err(NEEDS_PREFIX.to_string());
            };
            let plugins = if action == "toggle" {
                set_plugin_enabled(&files, &name, None)
            } else {
                move_plugin(&files, &name, require_i64(args, "--by")?)
            }
            .map_err(text)?;
            print_json(&PluginsReport { plugins })
        }
        _ => Err(help_text()),
    }
}

fn plugins_read(args: &[String]) -> Result<(), String> {
    let game_name = flag(args, "--game").ok_or("plugins read requires --game")?;
    let plugins_path = flag(args, "--plugins").ok_or("plugins read requires --plugins")?;
    let def = game_by_id(&game_name).ok_or(format!(
        "unknown game '{game_name}'. Use one of: {}.",
        game_ids()
    ))?;
    let plugins_txt =
        std::fs::read_to_string(&plugins_path).map_err(|err| format!("{plugins_path}: {err}"))?;
    let loadorder_txt = match flag(args, "--loadorder") {
        Some(path) => Some(std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?),
        None => None,
    };
    let install = flag(args, "--install");
    let Some(rules) = &def.plugins else {
        return Err(format!("{} does not use a plugin list.", def.name));
    };
    let natives = native_plugin_names(def, install.as_deref().map(Path::new));
    let plugins = read_lists(
        &plugins_txt,
        loadorder_txt.as_deref(),
        rules.format,
        &natives,
    );
    print_json(&plugins)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(value).map_err(text)?);
    Ok(())
}

fn help_text() -> String {
    format!(
        "\
vortex-cli discover
vortex-cli games
vortex-cli sync --game <game>
vortex-cli toggle --game <game> --mod <id>
vortex-cli move --game <game> --mod <id> --by <places>
vortex-cli remove --game <game> --mod <id> [--keep-archive]
vortex-cli purge --game <game>
vortex-cli downloads --game <game>
vortex-cli discard --game <game> --from <archive in incoming>
vortex-cli fomod --game <game> --from <archive> [--choices <choices.json>]
vortex-cli install --game <game> --from <archive-or-folder> [--choices <choices.json>]
vortex-cli plugins toggle --game <game> --plugin <name>
vortex-cli plugins move --game <game> --plugin <name> --by <places>
vortex-cli plugins read --game <game> --plugins <plugins.txt> [--loadorder <loadorder.txt>] [--install <game dir>]
vortex-cli nexus status
vortex-cli nexus store-key --from <key-file>
vortex-cli nexus clear-key
vortex-cli nexus search --game <game> [--query <name>]
vortex-cli nexus files --game <game> --mod <id>
vortex-cli nexus download --game <game> --mod <id> --file <id> [--progress <file>]
vortex-cli nexus nxm --url <nxm link> | --from <file holding the link> [--prefer <game>] [--progress <file>]

<game> is one of: {}.
Run `vortex-cli games` for their names.

sync installs archives from the game's incoming folder that do not need a choice, links enabled mods into Data, and writes plugins.txt when the Proton prefix exists. Installers with options stay in incoming until install --choices.
purge takes every link this app made back out of the game folder. Mods stay installed and come back with sync.
nexus nxm handles a Mod Manager Download link from the Nexus website. It works for free accounts.
Search does not need an API key. File lists and downloads do. Pass the key with `nexus store-key --from` or VORTEX_NEXUS_KEY. The key is stored at $XDG_CONFIG_HOME/vortex-steamos/nexus.key (mode 0600) and is not accepted as a command argument. Adult mods are left out of search results.
Set VORTEX_STEAM_ROOT, or pass --root, to point discovery at a specific Steam folder.
",
        game_ids()
    )
}
