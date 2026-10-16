//! WP-16 reaction-table events for music (audit 2026-09-24: the spec's
//! music_start / music_high_energy / music_stop were never emitted).
//!
//! Pure and clock-injectable. Fed once per music_loop tick with the tempo
//! lock state and the RMS energy of the analysis window:
//!
//! - music_start: the first lock of a listening session.
//! - music_stop: no lock for [STOP_GRACE_S] while a session is running (short
//!   lock drops - a quiet bridge, speech over the music - don't end it).
//! - music_high_energy: the energy is >= [HIGH_ENERGY_RATIO] x the session's
//!   own running average for [HIGH_ENERGY_SUSTAIN_S] (a chorus/drop), at most
//!   once per [HIGH_ENERGY_COOLDOWN_S]. Relative to the session's own level,
//!   so it doesn't depend on the system volume.

/// No lock for this long while a session is running ends the session.
pub const STOP_GRACE_S: f64 = 15.0;

/// Energy must reach this multiple of the session's running average to count
/// as high energy.
pub const HIGH_ENERGY_RATIO: f64 = 1.25;

/// High energy must be sustained this long before the event fires.
pub const HIGH_ENERGY_SUSTAIN_S: f64 = 6.0;

/// Minimum gap between two `music_high_energy` events.
pub const HIGH_ENERGY_COOLDOWN_S: f64 = 180.0;

/// Let the running average settle this long after a session starts before
/// firing high energy.
pub const HIGH_ENERGY_WARMUP_S: f64 = 20.0;

/// Time constant of the exponential running-average of the energy.
const AVERAGE_TAU_S: f64 = 60.0;

/// Emits `music_start` / `music_high_energy` / `music_stop` from the tempo
/// lock state and the RMS energy of each analysis window.
///
/// Pure and clock-injectable: `now_fn` returns monotonic seconds (in Python
/// this defaulted to `time.monotonic`).
pub struct MusicEventTracker<F> {
    now_fn: F,
    /// Whether a listening session is currently running.
    pub playing: bool,
    started_at: f64,
    last_locked_at: f64,
    average: Option<f64>,
    last_update: Option<f64>,
    high_since: Option<f64>,
    last_high_fired: Option<f64>,
}

impl<F: FnMut() -> f64> MusicEventTracker<F> {
    /// Creates a tracker whose clock is `now_fn` (monotonic seconds).
    pub fn new(now_fn: F) -> Self {
        Self {
            now_fn,
            playing: false,
            started_at: 0.0,
            last_locked_at: 0.0,
            average: None,
            last_update: None,
            high_since: None,
            last_high_fired: None,
        }
    }

    /// Feeds one tick of tempo-lock state and window energy; returns the
    /// events fired by this tick (possibly several, in order).
    pub fn update(&mut self, locked: bool, energy: f64) -> Vec<&'static str> {
        let now = (self.now_fn)();
        let dt = self.last_update.map_or(0.0, |last| (now - last).max(0.0));
        self.last_update = Some(now);
        let mut events = Vec::new();

        if locked {
            self.last_locked_at = now;
            if !self.playing {
                self.playing = true;
                self.started_at = now;
                self.average = Some(energy);
                self.high_since = None;
                events.push("music_start");
            }
        } else if self.playing && now - self.last_locked_at >= STOP_GRACE_S {
            self.playing = false;
            self.average = None;
            self.high_since = None;
            events.push("music_stop");
            return events;
        }

        if !self.playing || !locked {
            self.high_since = None;
            return events;
        }

        let average = self.average.unwrap_or(energy);
        let high = average > 0.0 && energy >= HIGH_ENERGY_RATIO * average;
        let alpha = if dt != 0.0 {
            (dt / AVERAGE_TAU_S).min(1.0)
        } else {
            0.0
        };
        self.average = Some(average + alpha * (energy - average));

        if !high || now - self.started_at < HIGH_ENERGY_WARMUP_S {
            self.high_since = None;
            return events;
        }
        let high_since = *self.high_since.get_or_insert(now);
        if now - high_since < HIGH_ENERGY_SUSTAIN_S {
            return events;
        }
        let cooling = self
            .last_high_fired
            .is_some_and(|fired| now - fired < HIGH_ENERGY_COOLDOWN_S);
        if cooling {
            return events;
        }
        self.last_high_fired = Some(now);
        events.push("music_high_energy");
        events
    }
}
