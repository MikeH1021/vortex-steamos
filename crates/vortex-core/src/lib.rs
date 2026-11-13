//! Steam library, Proton prefix, mod deployment, and plugin lists for Bethesda games.
//!
//! The Godot shell talks to this crate through `vortex-cli`. Upstream Vortex
//! in the sibling `Vortex/` tree is the behavior reference and is not linked.

mod deploy;
mod fomod;
mod games;
mod install;
mod layout;
mod nexus;
mod plugins;
mod proton;
mod settings;
mod steam;
mod vdf;

pub use deploy::{
    deploy, link_method, link_method_for_paths, load_mods, move_mod, purge, remove_mod,
    set_enabled, staging_dir, toggle_mod, DeployReport, DeployRoots, LinkMethod, ModEntry,
    ModError, NexusRef,
};
pub use fomod::{
    install_fomod, preview_fomod, FomodChoice, FomodInstall, FomodNote, FomodPlan, InstalledFiles,
};
pub use games::{
    all_games, game_by_id, game_ids, native_plugin_names, Base, Extender, GameDef, Kind, ModPath,
    PluginRules, Rule, When,
};
pub use install::{
    discard_download, incoming_dir, install_incoming, install_source, install_with, is_archive,
    list_downloads, preview_source, DownloadItem, DownloadMeta, FailedInstall, IncomingOutcome,
    InstallReport, InstallRules, ARCHIVE_EXTENSIONS,
};
pub use nexus::{
    clear_key, current_key, download_mod_file, list_mod_files, search_mods, store_key,
    validate_account, Account, ClearedKey, FileList, FixtureTransport, LiveTransport, NexusError,
    NexusFile, NexusMod, NexusTransport, NxmGrant, NxmLink, SavedDownload, SearchReport,
};
pub use plugins::{
    apply_classic_timestamps, classic_load_order_time, move_plugin, read_lists, reconcile_plugins,
    set_plugin_enabled, write_loadorder_txt, write_plugins_txt, Plugin, PluginFiles, PluginFormat,
    LIST_HEADER,
};
pub use proton::{compat_tool_name, proton_layout, ProtonLayout};
pub use settings::{merge_settings, SettingsReport};
pub use steam::{
    default_steam_candidates, discover, discover_roots, steam_candidates, DiscoverReport,
    InstalledGame,
};
pub use vdf::{parse, Document, ParseError, Value};
