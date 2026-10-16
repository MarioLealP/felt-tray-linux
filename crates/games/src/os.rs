//! The OS-touching half: registry/VDF/manifest discovery, window and
//! process enumeration. Every Win32/psutil call sits behind a trait that
//! this crate leaves unimplemented; the orchestration around it is pure.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::classify::GameSources;
use crate::config::{build_sources, load_games_config};
use crate::detect::{ProcSample, detect_games};
use crate::paths::norm_path;

/// One process's window observation: its best visible top-level window.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSample {
    pub area: i64,
    pub title: String,
}

/// Reading Steam/Epic/GOG installs and Windows' own game list (winreg +
/// libraryfolders.vdf + Epic manifests). Unimplemented on purpose: the
/// real implementation is Win32 registry/filesystem access. Each method
/// mirrors the Python `_steam_libraries`/`_epic_locations`/... helpers
/// and fails soft - errors become `Err`, which [`discover_sources`]
/// turns into empty lists.
pub trait GameSourceDiscovery {
    /// Every Steam library root (`HKCU\Software\Valve\Steam` +
    /// libraryfolders.vdf).
    fn steam_libraries(&self) -> Result<Vec<String>, String>;
    /// Epic manifests' `InstallLocation`s.
    fn epic_locations(&self) -> Result<Vec<String>, String>;
    /// GOG install paths.
    fn gog_paths(&self) -> Result<Vec<String>, String>;
    /// GameConfigStore `MatchedExeFullPath` entries.
    fn game_config_store_exes(&self) -> Result<Vec<String>, String>;
}

/// The umbrella directory holding the felt-animation tree (Python's
/// `E:\Felt`): anything running from under it is Felt itself, never a game.
pub fn felt_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../")
}

/// Read every library source; each one fails soft to empty.
pub fn discover_sources(games_path: &Path, discovery: &dyn GameSourceDiscovery) -> GameSources {
    let safe = |result: Result<Vec<String>, String>| result.unwrap_or_default();
    let steam = safe(discovery.steam_libraries());
    let epic = safe(discovery.epic_locations());
    let gog = safe(discovery.gog_paths());
    let known = safe(discovery.game_config_store_exes());
    let steam_refs: Vec<&str> = steam.iter().map(String::as_str).collect();
    let epic_refs: Vec<&str> = epic.iter().map(String::as_str).collect();
    let gog_refs: Vec<&str> = gog.iter().map(String::as_str).collect();
    let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
    let felt = felt_root().to_string_lossy().into_owned();
    build_sources(
        &load_games_config(games_path),
        &steam_refs,
        &epic_refs,
        &gog_refs,
        &known_refs,
        &[&felt],
    )
}

/// (name, exe path); exe is `None` when the path cannot be read.
pub type ProcInfo = (String, Option<String>);

/// Process/window enumeration for [`GameDetector::sample`] (psutil +
/// EnumWindows on Windows). Unimplemented on purpose.
pub trait ProcessProbe {
    fn pids(&self) -> Vec<u32>;
    /// `None` if the process vanished or access was denied.
    fn process_info(&self, pid: u32) -> Option<ProcInfo>;
    /// Per PID, its largest visible, unowned, non-tool, non-cloaked
    /// top-level window (title preferring a titled one).
    fn visible_windows_by_pid(&self) -> HashMap<u32, WindowSample>;
}

/// Owns the OS sampling: refreshes GameSources every ~10 min, keeps a
/// per-PID (name, exe) cache so exe lookups only happen for new PIDs, and
/// never panics on AccessDenied/vanished processes. Clock and discovery
/// are injectable so tests are deterministic.
pub struct GameDetector<N, D> {
    games_path: PathBuf,
    refresh_s: f64,
    now_fn: N,
    discover_fn: D,
    sources: Option<GameSources>,
    loaded_at: f64,
    pid_cache: HashMap<u32, ProcInfo>,
    own_pid: u32,
}

impl<N: Fn() -> f64, D: FnMut(&Path) -> GameSources> GameDetector<N, D> {
    pub fn new(refresh_s: f64, now_fn: N, discover_fn: D) -> Self {
        Self::with_games_path(crate::default_games_path(), refresh_s, now_fn, discover_fn)
    }

    pub fn with_games_path(games_path: PathBuf, refresh_s: f64, now_fn: N, discover_fn: D) -> Self {
        Self {
            games_path,
            refresh_s,
            now_fn,
            discover_fn,
            sources: None,
            loaded_at: 0.0,
            pid_cache: HashMap::new(),
            own_pid: std::process::id(),
        }
    }

    pub fn sources(&mut self) -> &GameSources {
        let now = (self.now_fn)();
        if self.sources.is_none() || now - self.loaded_at >= self.refresh_s {
            self.sources = Some((self.discover_fn)(&self.games_path));
            self.loaded_at = now;
        }
        self.sources.as_ref().unwrap()
    }

    fn cached_info(&mut self, probe: &dyn ProcessProbe, pid: u32) -> Option<ProcInfo> {
        if let Some(cached) = self.pid_cache.get(&pid) {
            return Some(cached.clone());
        }
        let info = probe.process_info(pid)?;
        self.pid_cache.insert(pid, info.clone());
        Some(info)
    }

    /// One poll: {exe path key: display name} of running games.
    pub fn sample(
        &mut self,
        tracked: &HashSet<String>,
        probe: &dyn ProcessProbe,
    ) -> HashMap<String, String> {
        let sources = self.sources().clone();
        let alive: HashSet<u32> = probe.pids().into_iter().collect();
        self.pid_cache.retain(|pid, _| alive.contains(pid));
        let windows = probe.visible_windows_by_pid();
        let mut pids: HashSet<u32> = windows.keys().copied().collect();
        for (pid, (name, exe)) in &self.pid_cache {
            let key = match exe {
                Some(e) if !e.is_empty() => norm_path(e),
                _ => format!("name:{}", name.to_lowercase()),
            };
            if tracked.contains(&key) {
                pids.insert(*pid);
            }
        }
        let mut pid_list: Vec<u32> = pids
            .into_iter()
            .filter(|pid| *pid != self.own_pid && *pid != 0 && alive.contains(pid))
            .collect();
        pid_list.sort_unstable(); // deterministic sample order
        let mut samples = Vec::new();
        for pid in pid_list {
            let Some((name, exe)) = self.cached_info(probe, pid) else {
                continue;
            };
            let window = windows.get(&pid);
            samples.push(ProcSample {
                pid,
                name,
                exe_path: exe,
                has_window: window.is_some(),
                title: window.map(|w| w.title.clone()),
            });
        }
        detect_games(&samples, &sources, tracked)
    }
}
