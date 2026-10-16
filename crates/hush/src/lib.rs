//! WP-09: "shut up" hush - Mario-initiated, so it doesn't break the never-
//! quiet rule (unprompted reactions still show a bubble during hush, they just
//! don't speak). Pure state machine, no clock coupling baked in - tests inject
//! a `now_fn` so extend/cap/expire can be verified without a real 10-minute
//! sleep.

use std::sync::OnceLock;
use std::time::Instant;

/// How long a plain hush lasts when the caller doesn't pick a duration.
pub const DEFAULT_MINUTES: f64 = 10.0;

/// Hard ceiling on the remaining hush time: `set` never lets the deadline
/// run further than this into the future, however often it's called.
pub const MAX_MINUTES: f64 = 60.0;

/// Lifted with less than this left = she "served the full hush" (the tray
/// toggle can't be pressed at the exact expiry second).
pub const EARLY_MARGIN_S: f64 = 30.0;

/// What an active hush looked like when /unhush lifted it (or when it ran
/// out on its own) - the details felt-core's `unhushed` event line is
/// phrased from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HushEnded {
    /// How long the hush actually ran, from `set` to lift/expiry.
    pub hushed_for_s: f64,
    /// How long it was planned to run (includes any extensions).
    pub planned_s: f64,
    /// Lifted with more than [`EARLY_MARGIN_S`] still on the clock.
    pub early: bool,
}

/// Pure hush state machine. The clock is a constructor-injected `Fn() -> f64`
/// (monotonic-ish seconds; only differences and comparisons matter) so tests
/// drive time by hand.
pub struct HushState<F: Fn() -> f64> {
    now_fn: F,
    until: Option<f64>,
    started: Option<f64>,
}

fn monotonic_seconds() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

impl HushState<fn() -> f64> {
    /// The default clock: a monotonic seconds counter, like Python's
    /// `time.monotonic`.
    pub fn monotonic() -> Self {
        Self::new(monotonic_seconds)
    }
}

impl<F: Fn() -> f64> HushState<F> {
    /// Builds an inactive hush driven by `now_fn`.
    pub fn new(now_fn: F) -> Self {
        Self {
            now_fn,
            until: None,
            started: None,
        }
    }

    /// Extends hush by `minutes` from whatever's left (or from now, if
    /// hush wasn't already active), capped so the total remaining time
    /// from now never exceeds [`MAX_MINUTES`]. Returns the new deadline.
    pub fn set(&mut self, minutes: f64) -> f64 {
        let now = (self.now_fn)();
        if !matches!(self.until, Some(until) if now < until) {
            self.started = Some(now);
        }
        let base = match self.until {
            Some(until) if until > now => until,
            _ => now,
        };
        let until = (base + minutes * 60.0).min(now + MAX_MINUTES * 60.0);
        self.until = Some(until);
        until
    }

    /// Lifts the hush. Returns how long it ran vs. was planned (the
    /// planned length includes any extensions), or `None` if no hush was
    /// actually active. Always resets.
    pub fn clear(&mut self) -> Option<HushEnded> {
        let now = (self.now_fn)();
        let ended = match (self.until, self.started) {
            (Some(until), Some(started)) if now < until => Some(HushEnded {
                hushed_for_s: now - started,
                planned_s: until - started,
                early: until - now > EARLY_MARGIN_S,
            }),
            _ => None,
        };
        self.until = None;
        self.started = None;
        ended
    }

    /// A hush that ran out on its own, nobody lifted it (Mario,
    /// 2026-09-26: she comments on that too). Returns it once, served in
    /// full (`early = false`), and resets, so the expiry loop announces it
    /// exactly once. `None` while a hush is still running or none was set.
    pub fn consume_expiry(&mut self) -> Option<HushEnded> {
        let until = self.until?;
        let started = self.started?;
        if (self.now_fn)() < until {
            return None;
        }
        let served = until - started;
        self.until = None;
        self.started = None;
        Some(HushEnded {
            hushed_for_s: served,
            planned_s: served,
            early: false,
        })
    }

    /// Whether a hush is running right now (false once the deadline passes).
    pub fn active(&self) -> bool {
        self.until.is_some_and(|until| (self.now_fn)() < until)
    }

    /// Seconds of hush left; `0.0` when expired or never set.
    pub fn remaining_seconds(&self) -> f64 {
        let now = (self.now_fn)();
        match self.until {
            Some(until) if now < until => until - now,
            _ => 0.0,
        }
    }
}
