//! WP-19: non-verbal vocalisations (yawn, giggle, sigh, ...) and filler
//! lines, played through audio.py at a real animation-tag's clip start or
//! while waiting on a slow reply. Same "folder = category, any file inside
//! counts, missing file degrades gracefully" convention as
//! reaction_table.py/signals/games.py - `E:\Felt\Sounds\<kind>\` (Mario's
//! own curated keepers, WP-19 Part 4) is a sibling of felt-character-assets'
//! own animations_lib, not inside this repo.

#![forbid(unsafe_code)]

mod py_str;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use py_str::py_str;

/// tag -> sound kind, or a list of kinds to pick between at random.
/// Python's `dict[str, str | list[str]]` from sounds.py / sounds.json.
pub type SoundMap = BTreeMap<String, SoundKind>;

/// A sounds.json value: one kind, or a list of kinds to pick between at
/// random (`"sleepy/yawn": "yawn"` vs `"emotion/laughing": ["laugh", "giggle"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundKind {
    /// A single kind.
    One(String),
    /// A list of kinds to pick between at random.
    Many(Vec<String>),
}

impl From<&str> for SoundKind {
    fn from(kind: &str) -> Self {
        SoundKind::One(kind.to_owned())
    }
}

impl<const N: usize> From<[&str; N]> for SoundKind {
    fn from(kinds: [&str; N]) -> Self {
        SoundKind::Many(kinds.iter().map(|k| (*k).to_owned()).collect())
    }
}

/// Minimal stand-in for `felt_animation.randomness.RandomLike` - just the two
/// methods sounds.py actually calls (`choice`, `shuffle`), made object-safe
/// by pinning the element types this crate picks between (sound paths) and
/// shuffles (lists of kinds). Production code falls back to a process-wide
/// default when no rng is injected, matching Python's `rng = rng or random`.
/// Tests inject a seeded [`SeededRng`] where the Python tests pass
/// `random.Random(seed)`.
pub trait RandomLike {
    /// Python's `random.choice(seq)`: one element of a non-empty sequence
    /// (panics on an empty sequence, like `random.choice` raising).
    fn choice(&self, seq: &[PathBuf]) -> PathBuf;

    /// Python's `random.shuffle(x)`: shuffle a sequence in place.
    fn shuffle(&self, x: &mut [String]);
}

/// Deterministic [`RandomLike`] (SplitMix64), so tests reproduce the role of
/// Python's `random.Random(seed)` without depending on its exact sequence.
#[derive(Debug)]
pub struct SeededRng {
    state: Cell<u64>,
}

impl SeededRng {
    /// An rng whose stream is fully determined by `seed`.
    pub fn new(seed: u64) -> Self {
        SeededRng {
            state: Cell::new(seed),
        }
    }

    fn next_u64(&self) -> u64 {
        // SplitMix64 (Steele/Lea/Flood): tiny but well-mixed.
        let z = self.state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.state.set(z);
        let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&self, len: usize) -> usize {
        (self.next_u64() % len as u64) as usize
    }
}

impl RandomLike for SeededRng {
    fn choice(&self, seq: &[PathBuf]) -> PathBuf {
        assert!(!seq.is_empty(), "Cannot choose from an empty sequence");
        seq[self.below(seq.len())].clone()
    }

    fn shuffle(&self, x: &mut [String]) {
        // Fisher-Yates, matching random.shuffle's pass over range(1, len).
        for i in (1..x.len()).rev() {
            let j = self.below(i + 1);
            x.swap(i, j);
        }
    }
}

thread_local! {
    /// Python falls back to the process-wide `random` module when a call site
    /// passes no rng; this is the Rust equivalent (seeded once per thread
    /// from the clock, so production picks vary per run).
    static DEFAULT_RNG: SeededRng = SeededRng::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
            ^ u64::from(std::process::id()),
    );
}

struct ProcessRng;

impl RandomLike for ProcessRng {
    fn choice(&self, seq: &[PathBuf]) -> PathBuf {
        DEFAULT_RNG.with(|rng| rng.choice(seq))
    }

    fn shuffle(&self, x: &mut [String]) {
        DEFAULT_RNG.with(|rng| rng.shuffle(x))
    }
}

static PROCESS_RNG: ProcessRng = ProcessRng;

/// `rng = rng or random` - the injected rng if a call site supplied one,
/// otherwise the process-wide default.
fn resolve_rng(rng: Option<&dyn RandomLike>) -> &dyn RandomLike {
    match rng {
        Some(rng) => rng,
        None => &PROCESS_RNG,
    }
}

/// WP-19's own mapping table, used only as the fallback when sounds.json
/// is missing - the real, editable source of truth is the file itself
/// (same "data file, not code" spirit as reactions.json), this just means
/// a fresh checkout isn't silently mute before anyone's written the file.
pub fn default_tag_to_kind() -> SoundMap {
    SoundMap::from([
        ("sleepy/yawn".to_owned(), SoundKind::One("yawn".to_owned())),
        (
            "emotion/laughing".to_owned(),
            SoundKind::Many(vec!["laugh".to_owned(), "giggle".to_owned()]),
        ),
        (
            "emotion/sighing".to_owned(),
            SoundKind::One("sigh".to_owned()),
        ),
        ("air/land_hard".to_owned(), SoundKind::One("ugh".to_owned())),
        (
            "emotion/surprised".to_owned(),
            SoundKind::One("gasp".to_owned()),
        ),
        (
            "reaction/turn_surprised".to_owned(),
            SoundKind::One("gasp".to_owned()),
        ),
    ])
}

/// The editable source of truth's location: Python resolves
/// `Path(__file__).resolve().parents[2] / "sounds.json"` - the repo root two
/// levels above the package (`src/felt_animation/sounds.py`) - so the default
/// here is the workspace root two levels above this crate.
pub fn default_sounds_map_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("sounds.json")
}

/// tag -> sound kind, or a list of kinds to pick between at random.
/// Missing/malformed file falls back to [`default_tag_to_kind`] rather than
/// going silent outright - matches reaction_table.load_reactions' "degrade
/// gracefully" precedent, but with a real fallback here since this table is
/// short and stable enough to be worth defaulting rather than just returning
/// `{}`. Entries whose key starts with `_` are comment lines and are skipped.
pub fn load_sound_map(path: &Path) -> SoundMap {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_err) => {
            // Python logs via logger.exception and falls back on OSError
            // (missing file included); a non-UTF-8 file raises there instead
            // - this port degrades to the default table either way.
            return default_tag_to_kind();
        }
    };
    let data: serde_json::Value = match serde_json::from_str(&text) {
        Ok(data) => data,
        Err(_err) => return default_tag_to_kind(),
    };
    let Some(map) = data.as_object() else {
        return default_tag_to_kind();
    };
    map.iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .map(|(k, v)| {
            let kind = match v {
                serde_json::Value::Array(items) => {
                    SoundKind::Many(items.iter().map(py_str).collect())
                }
                other => SoundKind::One(py_str(other)),
            };
            (k.clone(), kind)
        })
        .collect()
}

/// A random file from sounds_dir/kind/, or None if that folder is
/// missing/empty - the same "not curated yet, no sound" degrade as
/// everywhere else in this project that reads an on-disk content
/// library. `prefix` narrows it to files whose name starts with it (the
/// presence lines share one folder: back_*.wav, zip_it_*.wav, rude_*.wav).
/// Pure given a directory listing; the real filesystem read is
/// the only non-pure part, same split as signals.games'
/// running_game_exes vs. diff_running_games.
pub fn pick_sound_path(
    kind: &str,
    sounds_dir: &Path,
    rng: Option<&dyn RandomLike>,
    prefix: Option<&str>,
) -> Option<PathBuf> {
    let kind_dir = sounds_dir.join(kind);
    if !kind_dir.is_dir() {
        return None;
    }
    let candidates = wav_files(&kind_dir, |name| prefix.is_none_or(|p| name.starts_with(p)));
    if candidates.is_empty() {
        return None;
    }
    Some(resolve_rng(rng).choice(&candidates))
}

/// sounds.json values are one kind or a list of kinds
/// ("emotion/laughing": ["laugh", "giggle"]). For a list, one kind is
/// chosen at random among those that actually have curated files, then a
/// random take within it.
pub fn pick_kind_sound(
    kinds: &SoundKind,
    sounds_dir: &Path,
    rng: Option<&dyn RandomLike>,
) -> Option<PathBuf> {
    let rng = resolve_rng(rng);
    let mut options = match kinds {
        SoundKind::One(kind) => vec![kind.clone()],
        SoundKind::Many(kinds) => kinds.clone(),
    };
    rng.shuffle(&mut options);
    for kind in &options {
        if let Some(path) = pick_sound_path(kind, sounds_dir, Some(rng), None) {
            return Some(path);
        }
    }
    None
}

// WP-19: presence lines are their own kind (sounds_dir/presence/), never
// slow-request fillers. Files are named after the line: back_*.wav ("back!",
// the away timer bringing her back), zip_it_*.wav ("okay okay, I'll zip it",
// on a hush), rude_*.wav ("rude!", used by WP-23 below).
// WP-23 slice 4: a small pool for "her platform vanished under her"
// (minimised/closed/moved away) - window_events.py rotates between these
// (no immediate repeat) rather than always playing "rude". Uncurated (no
// wav yet) degrades the same as every other line here: silently nothing.
/// The folder presence lines live in.
pub const PRESENCE_KIND: &str = "presence";

/// The curated presence lines; files are named `<line>_<take>.wav`.
pub const PRESENCE_LINES: [&str; 5] = ["back", "zip_it", "rude", "standing_there", "wow_okay"];

/// The random take for one presence line (`presence/<line>_*.wav`), or None
/// when that line is uncurated.
pub fn pick_presence_path(
    line: &str,
    sounds_dir: &Path,
    rng: Option<&dyn RandomLike>,
) -> Option<PathBuf> {
    let prefix = format!("{line}_");
    pick_sound_path(PRESENCE_KIND, sounds_dir, rng, Some(&prefix))
}

/// A random filler line from sounds_dir/filler/ only - flat, not split
/// into sub-kinds (the fillers are all interchangeable "still here,
/// thinking" noises). Never a presence line: a stray back_/zip_it_/rude_
/// file left in filler/ (the first WP-19 batch mixed them in) is ignored,
/// so "rude!" can't play because a request was slow.
pub fn pick_filler_path(sounds_dir: &Path, rng: Option<&dyn RandomLike>) -> Option<PathBuf> {
    let kind_dir = sounds_dir.join("filler");
    if !kind_dir.is_dir() {
        return None;
    }
    let candidates = wav_files(&kind_dir, |name| {
        // name starts with `line + "_"` without a per-line String alloc.
        !PRESENCE_LINES.iter().any(|line| {
            name.strip_prefix(*line)
                .is_some_and(|rest| rest.starts_with('_'))
        })
    });
    if candidates.is_empty() {
        return None;
    }
    Some(resolve_rng(rng).choice(&candidates))
}

/// The `.wav` files directly inside `dir` whose name passes `accept` - the
/// shared "any file inside counts" scan of a kind folder.
fn wav_files(dir: &Path, accept: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("wav"))
                && path
                    .file_name()
                    .is_some_and(|name| accept(&name.to_string_lossy()))
        })
        .collect()
}
