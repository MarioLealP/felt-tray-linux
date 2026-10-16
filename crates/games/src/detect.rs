//! Pure detection over process samples, plus the path-keyed tracker.

use std::collections::{HashMap, HashSet};

use crate::classify::{GameSources, classify_process};
use crate::paths::norm_path;
use crate::title::clean_title;

/// One OS process observation at poll time.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
    pub has_window: bool,
    pub title: Option<String>,
}

impl ProcSample {
    pub fn new(
        pid: u32,
        name: &str,
        exe_path: Option<&str>,
        has_window: bool,
        title: Option<&str>,
    ) -> Self {
        Self {
            pid,
            name: name.to_string(),
            exe_path: exe_path.map(str::to_string),
            has_window,
            title: title.map(str::to_string),
        }
    }
}

/// Pure: {exe path key: display name} of games running in this poll.
/// A process whose key is already tracked stays counted without needing a
/// window (hysteresis), so a vanishing window doesn't fake a stop.
pub fn detect_games(
    samples: &[ProcSample],
    sources: &GameSources,
    still_tracked: &HashSet<String>,
) -> HashMap<String, String> {
    let mut games = HashMap::new();
    for s in samples {
        let key = match s.exe_path.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => norm_path(p),
            None => format!("name:{}", s.name.to_lowercase()),
        };
        if games.contains_key(&key) {
            continue;
        }
        if still_tracked.contains(&key)
            || classify_process(s.exe_path.as_deref(), Some(&s.name), s.has_window, sources)
        {
            // Python: clean_title(s.title, s.exe_path or s.name)
            let exe_or_name = s
                .exe_path
                .as_deref()
                .filter(|p| !p.is_empty())
                .unwrap_or(&s.name);
            let display = clean_title(s.title.as_deref(), Some(exe_or_name));
            games.insert(key, display);
        }
    }
    games
}

/// Pure: (started, stopped) key sets between two poll snapshots.
pub fn diff_running_games(
    previous: &HashSet<String>,
    current: &HashSet<String>,
) -> (HashSet<String>, HashSet<String>) {
    let started = current.difference(previous).cloned().collect();
    let stopped = previous.difference(current).cloned().collect();
    (started, stopped)
}

/// Clock-injectable - tracks each running game (keyed by exe path) with
/// its start time and the display name captured at start, so the stop
/// event reports the same name plus a real duration.
pub struct GameTracker<F> {
    now_fn: F,
    running: HashMap<String, (f64, String)>, // key -> (start, display name)
}

impl<F: Fn() -> f64> GameTracker<F> {
    pub fn new(now_fn: F) -> Self {
        Self {
            now_fn,
            running: HashMap::new(),
        }
    }

    pub fn running_keys(&self) -> HashSet<String> {
        self.running.keys().cloned().collect()
    }

    pub fn running_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .running
            .values()
            .map(|(_, name)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// Feed one poll's {key: display name}. Returns (started_names,
    /// [(stopped_name, duration_s), ...]), sorted for determinism.
    pub fn update(
        &mut self,
        current: &HashMap<String, String>,
    ) -> (Vec<String>, Vec<(String, f64)>) {
        let now = (self.now_fn)();
        let previous = self.running_keys();
        let current_keys: HashSet<String> = current.keys().cloned().collect();
        let (started, stopped) = diff_running_games(&previous, &current_keys);
        for key in &started {
            self.running
                .insert(key.clone(), (now, current[key].clone()));
        }
        let mut stopped_with_duration = Vec::new();
        for key in &stopped {
            let (start_time, name) = self.running.remove(key).unwrap();
            stopped_with_duration.push((name, now - start_time));
        }
        let mut started_names: Vec<String> = started.iter().map(|k| current[k].clone()).collect();
        started_names.sort();
        stopped_with_duration.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        (started_names, stopped_with_duration)
    }
}
