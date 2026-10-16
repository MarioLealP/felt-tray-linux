//! WP-17: game launched-or-closed, detected automatically (Mario's call,
//! 2026-09-26 review: "use the window title and recognize every game, no
//! need to specify").
//!
//! A process counts as a game when it has a visible, non-tool top-level
//! window AND any of:
//!   (a) its exe lives under a game library root - every Steam library's
//!       steamapps\common (libraryfolders.vdf), each Epic manifest's
//!       InstallLocation, each GOG install path, plus games.toml `roots`;
//!   (b) its full exe path is in Windows' own game list
//!       (HKCU\System\GameConfigStore\Children\*\MatchedExeFullPath);
//!   (c) its exe name is in games.toml's manual `games` list;
//! and it isn't ignored (launchers/helpers/crash handlers/anti-cheat/
//! overlays/tools, browsers + Discord, Felt itself, games.toml `ignore`).
//!
//! Same split as before: the OS-touching half (registry/VDF/manifest
//! reading, window/process enumeration) lives behind the unimplemented
//! [`GameSourceDiscovery`] and [`ProcessProbe`] traits; every decision
//! ([`classify_process`], [`is_ignored_exe`], [`clean_title`],
//! [`detect_games`], [`GameTracker`]) is pure and unit-tested with
//! synthetic inputs.
//!
//! Tracking is keyed by the normalized exe path, so a changing window title
//! never fakes a stop+start; the display name sent to felt-core is the
//! cleaned window title captured when the game started. Once a game is
//! tracked it stays "running" while any process with that exe is alive,
//! even if its window briefly disappears (loading screens, mode switches).

#![forbid(unsafe_code)]

pub mod classify;
pub mod config;
pub mod detect;
pub mod os;
pub mod paths;
pub mod title;

use std::path::PathBuf;

pub use classify::{
    DEFAULT_IGNORE_NAMES, DEFAULT_IGNORE_PATTERNS, GameSources, NEVER_GAMES, classify_process,
    is_ignored_exe,
};
pub use config::{
    GamesConfig, build_sources, load_games, load_games_config, parse_libraryfolders_vdf,
};
pub use detect::{GameTracker, ProcSample, detect_games, diff_running_games};
pub use os::{
    GameDetector, GameSourceDiscovery, ProcInfo, ProcessProbe, WindowSample, discover_sources,
    felt_root,
};
pub use paths::{exe_name_of, fnmatch_case, is_under, norm_path};
pub use title::clean_title;

/// Poll cadence for the running-games check.
pub const POLL_INTERVAL_S: f64 = 5.0;
/// How often the library sources (Steam/Epic/GOG/games.toml) are re-read.
pub const SOURCES_REFRESH_S: f64 = 10.0 * 60.0;

/// The shipped games.toml (the felt-animation tree's own file - read,
/// never copied).
pub fn default_games_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/games.toml")
}
