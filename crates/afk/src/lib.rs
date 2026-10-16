//! WP-17: AFK detection. The idle reading itself is a real Windows API call
//! (`GetLastInputInfo`, system-wide regardless of focused window) - exposed
//! here as the [`SystemIdle`] trait with no implementation, since there is no
//! real OS input-state to inject in unit tests. [`AfkTracker`] is the pure
//! decision logic (threshold crossing + a real elapsed duration on return),
//! fully tested against synthetic idle readings.

/// The WP's own number: "> 10 min" before she counts it as AFK.
pub const AFK_THRESHOLD_S: f64 = 600.0;

/// OS input-state provider: seconds since the last system-wide keyboard/mouse
/// input. The Python original calls `GetLastInputInfo` / `GetTickCount` via
/// ctypes (Windows-only); platform access is left unimplemented on purpose.
pub trait SystemIdle {
    /// Seconds since the last system-wide keyboard/mouse input. The Python
    /// original computes `GetTickCount() - dwTime` in milliseconds and divides
    /// by 1000.0.
    fn idle_seconds(&self) -> f64;
}

/// Clock-injectable (same style as presence.PresenceState). Feed it
/// real idle readings; it returns `Some(("afk_start", 0.0))` the poll
/// idle time first crosses the threshold, or `Some(("afk_end", duration_s))`
/// the poll AFTER that where input has resumed (idle time dropped back
/// below threshold, meaning a keypress/click just happened) - `None` on
/// every other poll.
pub struct AfkTracker<F> {
    threshold_s: f64,
    now_fn: F,
    /// True while an AFK period is in progress.
    pub afk: bool,
    afk_started_at: Option<f64>,
}

impl<F: Fn() -> f64> AfkTracker<F> {
    /// `threshold_s` defaults to [`AFK_THRESHOLD_S`] and `now_fn` to
    /// `time.monotonic` in the Python original; both are explicit here so the
    /// tracker stays pure and deterministic under test.
    pub fn new(threshold_s: f64, now_fn: F) -> Self {
        Self {
            threshold_s,
            now_fn,
            afk: false,
            afk_started_at: None,
        }
    }

    /// Feed one idle-time reading; returns the AFK event it causes, if any.
    pub fn update(&mut self, idle_s: f64) -> Option<(&'static str, f64)> {
        let now = (self.now_fn)();
        if !self.afk {
            if idle_s < self.threshold_s {
                return None;
            }
            self.afk = true;
            // Back-dated to when idle time actually started accruing,
            // not "now" (which would undercount every AFK duration by a
            // full threshold's worth of time - she went idle 10 minutes
            // before this poll noticed, not 10 minutes ago from right now).
            self.afk_started_at = Some(now - idle_s);
            return Some(("afk_start", 0.0));
        }

        if idle_s >= self.threshold_s {
            return None;
        }
        let duration = now - self.afk_started_at.unwrap_or(now);
        self.afk = false;
        self.afk_started_at = None;
        Some(("afk_end", duration))
    }
}
