//! WP-15: a persistent mood vector - energy, happiness, boredom, affection,
//! annoyance, each 0..1 - that drifts on its own (a daily energy curve,
//! boredom rising with inactivity, annoyance decaying toward zero) and gets
//! nudged by real events (a reaction spoken, a hush, Mario actually talking to
//! her). tone.py reads it to build the one-line tone instruction; server.py
//! also lets it bias ambient-loop weights and which idle micro-expression
//! shows (reaction_table.py).
//!
//! Clock-injectable (`now_fn`/`clock_fn`) like presence.py/hush.py, so
//! drift/decay/the daily reset can be tested without a real wall-clock wait.
//! Persistence (mood.json under `%APPDATA%\Felt`) is a thin load/save pair at
//! the bottom, same shape as wardrobe.py's own load/save - kept OUT of the
//! state itself so tests can exercise [`MoodState`] purely in memory.
//!
//! Deliberate scope decision: elapsed time for drift is measured from this
//! PROCESS's own last tick (via `now_fn`, monotonic), not from a persisted
//! wall-clock timestamp. A restart does NOT fast-forward boredom/annoyance
//! across however long felt-animation was closed - "she was bored while the
//! app wasn't even running" isn't a real feeling to model, and the daily
//! reset below already handles the overnight case (annoyance cleared, boredom
//! eased) without needing to reconstruct exactly how long she was closed for.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The five mood axes, each nominally 0..1.
pub const MOOD_KEYS: [&str; 5] = ["energy", "happiness", "boredom", "affection", "annoyance"];

/// Python's `_DEFAULT_MOOD`, one entry per [`MOOD_KEYS`] axis.
fn default_mood() -> BTreeMap<String, f64> {
    BTreeMap::from([
        ("energy".to_string(), 0.5),
        ("happiness".to_string(), 0.6),
        ("boredom".to_string(), 0.2),
        ("affection".to_string(), 0.5),
        ("annoyance".to_string(), 0.0),
    ])
}

fn is_mood_key(key: &str) -> bool {
    MOOD_KEYS.contains(&key)
}

/// Mirror of Python's `_default_mood_path`: `mood.json` under
/// `%APPDATA%\Felt`, falling back to `~/AppData/Roaming/Felt` when `APPDATA`
/// is unset or empty. Snapshotted at import time in the Python; computed on
/// demand here.
pub fn default_mood_path() -> PathBuf {
    let base = match env::var_os("APPDATA").filter(|v| !v.is_empty()) {
        Some(appdata) => PathBuf::from(appdata),
        None => {
            let home = env::var_os("HOME")
                .or_else(|| env::var_os("USERPROFILE"))
                .map_or_else(|| PathBuf::from("."), PathBuf::from);
            home.join("AppData").join("Roaming")
        }
    };
    base.join("Felt").join("mood.json")
}

/// Target energy by hour of day - WP-15's own words: "low 1-7am, peak
/// afternoon." Hand-authored against those actual clock hours, not a
/// formula/sine wave standing in for them.
const ENERGY_CURVE: [f64; 24] = [
    0.35, 0.18, 0.10, 0.08, 0.08, 0.12, 0.20, 0.30, 0.45, 0.60, 0.70, 0.78, // 0..=11
    0.82, 0.85, 0.85, 0.82, 0.78, 0.72, 0.65, 0.60, 0.55, 0.50, 0.45, 0.40, // 12..=23
];

// Drift rates. Energy chases a moving hourly target on a time constant
// (tau) - it closes roughly 63% of the remaining gap every `tau` seconds.
// Boredom/annoyance don't chase a target, so they use flat per-second
// rates instead.
const ENERGY_TAU_S: f64 = 20.0 * 60.0; // ~20 min to mostly catch up to the hour's target
const BOREDOM_RISE_PER_S: f64 = 1.0 / (45.0 * 60.0); // 0 -> 1 over ~45 idle minutes
const ANNOYANCE_TAU_S: f64 = 30.0 * 60.0; // WP-15: "annoyance decays over ~30 min"

/// Clamp to 0..=1, exactly as the Python did (`max(0.0, min(1.0, value))`).
/// Not `f64::clamp`: that returns NaN for a NaN input, while the Python
/// order yields 1.0.
#[allow(clippy::manual_clamp)]
fn clamp01(value: f64) -> f64 {
    value.min(1.0).max(0.0)
}

/// Energy eases toward the hour's [`ENERGY_CURVE`] target over `elapsed_s`
/// (exponential approach on a ~20 min time constant). `elapsed_s <= 0` is a
/// no-op.
pub fn drift_energy(energy: f64, hour: i32, elapsed_s: f64) -> f64 {
    if elapsed_s <= 0.0 {
        return energy;
    }
    let target = ENERGY_CURVE[hour.rem_euclid(24) as usize];
    let factor = 1.0 - (-elapsed_s / ENERGY_TAU_S).exp();
    clamp01(energy + (target - energy) * factor)
}

/// Boredom rises linearly with idle time, clamped to 1. `elapsed_s <= 0`
/// is a no-op.
pub fn drift_boredom(boredom: f64, elapsed_s: f64) -> f64 {
    if elapsed_s <= 0.0 {
        return boredom;
    }
    clamp01(boredom + BOREDOM_RISE_PER_S * elapsed_s)
}

/// Annoyance decays exponentially toward zero (~30 min time constant),
/// clamped at 0. `elapsed_s <= 0` is a no-op.
pub fn drift_annoyance(annoyance: f64, elapsed_s: f64) -> f64 {
    if elapsed_s <= 0.0 {
        return annoyance;
    }
    clamp01(annoyance * (-elapsed_s / ANNOYANCE_TAU_S).exp())
}

/// Minimal wall-clock reading: the only parts of `datetime.datetime` this
/// module ever looked at - `clock_fn().hour` and `clock_fn().date().isoformat()`.
/// Defined here because chrono is not an allowed dependency for this port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
}

impl DateTime {
    /// A wall-clock instant at `hour` (0..=23) on the given calendar date.
    pub fn new(year: i32, month: u32, day: u32, hour: u32) -> Self {
        Self {
            year,
            month,
            day,
            hour,
        }
    }

    /// `YYYY-MM-DD`, matching `datetime.date.isoformat()`.
    pub fn date_iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// The live vector plus when it was last ticked. server.py owns one
/// instance and calls tick() before every read (GET /tone, ambient-loop
/// weighting, resting-face-adjacent picks) so drift is always caught up
/// to "now" rather than only advancing once per event.
///
/// `now_fn` is the monotonic process clock used for elapsed-time math;
/// `clock_fn` is the wall clock used for hour-of-day and the daily reset.
pub struct MoodState<N, C> {
    /// The live values: one entry per [`MOOD_KEYS`] axis, each nominally 0..1.
    /// Public (and mutated by tests) like the Python's `values` dict.
    pub values: BTreeMap<String, f64>,
    now_fn: N,
    clock_fn: C,
    last_tick: f64,
    // None (not today's date) so the very first tick() adopts today as
    // the baseline without immediately firing a "new day" reset - a
    // fresh process start already IS a clean slate, it doesn't also
    // need the reset's side effects applied on top.
    last_reset_date: Option<String>,
}

impl<N, C> MoodState<N, C>
where
    N: Fn() -> f64,
    C: Fn() -> DateTime,
{
    /// The live vector starts from the default mood; `values` overrides
    /// known keys only ([`MOOD_KEYS`]) - anything else is ignored.
    pub fn new(values: Option<BTreeMap<String, f64>>, now_fn: N, clock_fn: C) -> Self {
        let mut merged = default_mood();
        if let Some(values) = values {
            for (key, value) in values {
                if is_mood_key(&key) {
                    merged.insert(key, value);
                }
            }
        }
        let last_tick = now_fn();
        Self {
            values: merged,
            now_fn,
            clock_fn,
            last_tick,
            last_reset_date: None,
        }
    }

    /// Advance drift to "now": energy toward the hour's target, boredom up,
    /// annoyance down, then the once-per-day partial reset if the date rolled.
    pub fn tick(&mut self) {
        let now = (self.now_fn)();
        let elapsed = now - self.last_tick;
        self.last_tick = now;
        let hour = (self.clock_fn)().hour;
        let energy = self.values["energy"];
        let boredom = self.values["boredom"];
        let annoyance = self.values["annoyance"];
        self.values.insert(
            "energy".to_string(),
            drift_energy(energy, hour as i32, elapsed),
        );
        self.values
            .insert("boredom".to_string(), drift_boredom(boredom, elapsed));
        self.values
            .insert("annoyance".to_string(), drift_annoyance(annoyance, elapsed));
        self.maybe_daily_reset();
    }

    fn maybe_daily_reset(&mut self) {
        let today = (self.clock_fn)().date_iso();
        if self.last_reset_date.is_none() {
            self.last_reset_date = Some(today);
            return;
        }
        if self.last_reset_date.as_deref() == Some(today.as_str()) {
            return;
        }
        self.last_reset_date = Some(today);
        // "A partial reset each morning" (WP-15). A new day wipes any
        // overnight grudge outright (annoyance), eases boredom (waking up
        // to a new day is itself a change of scene), and pulls happiness
        // gently toward a pleasant baseline rather than carrying over
        // however she happened to feel at 11:59pm. Affection is
        // deliberately NOT touched here - it's the slow-building
        // "relationship" axis, not a daily transient like the other four.
        self.values.insert("annoyance".to_string(), 0.0);
        self.values.insert(
            "boredom".to_string(),
            (self.values["boredom"] - 0.3).max(0.0),
        );
        self.values.insert(
            "happiness".to_string(),
            self.values["happiness"] * 0.5 + 0.6 * 0.5,
        );
    }

    // -- event nudges (WP-15's own list) -----------------------------------

    /// A reaction was actually spoken on screen.
    pub fn on_reaction_spoken(&mut self) {
        let happiness = self.values["happiness"];
        let boredom = self.values["boredom"];
        self.values
            .insert("happiness".to_string(), clamp01(happiness + 0.03));
        self.values
            .insert("boredom".to_string(), clamp01(boredom - 0.10));
    }

    /// She was hushed.
    pub fn on_hush(&mut self) {
        let annoyance = self.values["annoyance"];
        self.values
            .insert("annoyance".to_string(), clamp01(annoyance + 0.15));
    }

    /// Mario actually prompting her (voice/text chat, not an unprompted
    /// screen reaction) - "Mario talking to her +affection/+happiness." Also
    /// counts as a real event against boredom, same reasoning as
    /// [`Self::on_reaction_spoken`].
    pub fn on_talked_to(&mut self) {
        let affection = self.values["affection"];
        let happiness = self.values["happiness"];
        let boredom = self.values["boredom"];
        self.values
            .insert("affection".to_string(), clamp01(affection + 0.02));
        self.values
            .insert("happiness".to_string(), clamp01(happiness + 0.03));
        self.values
            .insert("boredom".to_string(), clamp01(boredom - 0.20));
    }

    /// POST /mood/debug - WP-15's own Accept criterion: "force the state via
    /// a debug endpoint" so tone/ambient/face changes can be checked at
    /// different moods without waiting on real drift. Unknown keys are
    /// ignored; known ones are clamped to 0..1.
    pub fn set_debug(&mut self, overrides: &BTreeMap<String, f64>) {
        for (key, value) in overrides {
            if is_mood_key(key) {
                self.values.insert(key.clone(), clamp01(*value));
            }
        }
    }

    /// A snapshot copy, not a live reference.
    pub fn as_dict(&self) -> BTreeMap<String, f64> {
        self.values.clone()
    }
}

/// Load `mood.json` from `path`, overlaying known keys ([`MOOD_KEYS`]) onto
/// the defaults. A missing file or unparseable JSON yields the defaults -
/// the Python logs the parse error, this port silently falls back.
pub fn load_mood_state(path: &Path) -> BTreeMap<String, f64> {
    let mut out = default_mood();
    if !path.exists() {
        return out;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return out;
    };
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    let Some(obj) = raw.as_object() else {
        return out;
    };
    for key in MOOD_KEYS {
        // Only JSON numbers become values here; the Python would keep any
        // JSON type it found under a known key (and blow up later in the
        // drift math). Ports drop non-numeric values instead.
        if let Some(value) = obj.get(key).and_then(|v| v.as_f64()) {
            out.insert(key.to_string(), value);
        }
    }
    out
}

/// Write `values` to `path` as pretty JSON (2-space indent, like the
/// Python's `json.dumps(..., indent=2)`), creating the parent directory.
pub fn save_mood_state(values: &BTreeMap<String, f64>, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(values).map_err(io::Error::other)?;
    fs::write(path, text)
}
