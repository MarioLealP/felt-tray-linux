//! WP-17: GPU heat. [`read_gpu_temp_c`] asks the GPU itself: on this
//! desktop's AMD card, AMD's own driver library (atiadlxx.dll, ADL), user
//! mode, nothing extra installed (Mario 2026-10-01: option A - no
//! LibreHardwareMonitor, its kernel driver is the kind anti-cheats flag).
//! LibreHardwareMonitor's WMI namespace stays as a fallback for other
//! machines. [`GpuHeatTracker`] below is the pure sustained-threshold
//! decision logic, tested against synthetic temperature readings.
//!
//! Live (2026-10-01): RX 6700, ADL PMLog sensor 8 (edge) read 82 C at 97 %
//! load - so 80 C fired every gaming session; hot now means >= 85 C edge.
//!
//! The Windows driver side (ctypes against `atiadlxx.dll`, the `wmi`
//! package) is not ported here; host code implements [`GpuTempSource`]
//! and feeds its readings to [`GpuHeatTracker`].

// The WP said "> ~80 C sustained", but the RX 6700's edge sits at ~82 C in
// any game; 85 C is properly hot for it.
/// Edge temperature in Celsius at/above which the GPU counts as hot.
pub const GPU_HOT_THRESHOLD_C: f64 = 85.0;
/// How long the edge must stay at/above the threshold before "gpu_hot" fires.
pub const GPU_HOT_SUSTAIN_S: f64 = 10.0;
/// One cool-off drink per half hour of a hot game, not every poll.
pub const GPU_HOT_COOLDOWN_S: f64 = 30.0 * 60.0;

/// How often the signals loop polls the GPU temperature.
pub const POLL_INTERVAL_S: f64 = 5.0;

/// One GPU temperature source - AMD's ADL on this desktop, or
/// LibreHardwareMonitor's WMI provider on other machines.
///
/// The Windows side is not ported (ctypes/WinDLL against `atiadlxx.dll`,
/// the `wmi` package): a host that can talk to the driver implements this
/// trait, mirroring the Python `_Adl.read_edge_c` / `_read_lhm_c` pair.
pub trait GpuTempSource {
    /// The GPU temperature in Celsius, or `None` if this source can't answer.
    fn read_temp_c(&self) -> Option<f64>;
}

/// The GPU temperature in Celsius, or `None` if no source answers.
/// AMD's ADL first (this desktop), then LibreHardwareMonitor's WMI -
/// the fallback order of the Python `read_gpu_temp_c()`.
pub fn read_gpu_temp_c(adl: &impl GpuTempSource, lhm: &impl GpuTempSource) -> Option<f64> {
    adl.read_temp_c().or_else(|| lhm.read_temp_c())
}

/// Clock-injectable (same style as `call_monitor.TalkingDetector`).
/// Feed it real [`read_gpu_temp_c`] readings; returns `true` the instant a
/// "gpu_hot" event should fire (temp at/above threshold, sustained for
/// `sustain_s`), at most once per `cooldown_s`.
pub struct GpuHeatTracker<F> {
    threshold_c: f64,
    sustain_s: f64,
    cooldown_s: f64,
    now_fn: F,
    hot_since: Option<f64>,
    last_fired_at: Option<f64>,
    /// Public, same convention as `CallMonitor.last_peak`/`AfkTracker.afk` -
    /// just status visibility, not decision state.
    pub last_temp_c: Option<f64>,
}

impl<F: FnMut() -> f64> GpuHeatTracker<F> {
    /// Thresholds in Celsius/seconds; `now_fn` is the injectable monotonic
    /// clock (the Python default was `time.monotonic`).
    pub fn new(threshold_c: f64, sustain_s: f64, cooldown_s: f64, now_fn: F) -> Self {
        Self {
            threshold_c,
            sustain_s,
            cooldown_s,
            now_fn,
            hot_since: None,
            last_fired_at: None,
            last_temp_c: None,
        }
    }

    /// Module defaults ([`GPU_HOT_THRESHOLD_C`], [`GPU_HOT_SUSTAIN_S`],
    /// [`GPU_HOT_COOLDOWN_S`]) with the given clock.
    pub fn with_defaults(now_fn: F) -> Self {
        Self::new(
            GPU_HOT_THRESHOLD_C,
            GPU_HOT_SUSTAIN_S,
            GPU_HOT_COOLDOWN_S,
            now_fn,
        )
    }

    /// Feed one temperature reading (`None` = no source answered).
    /// Returns `true` when a "gpu_hot" event should fire now.
    pub fn update(&mut self, temp_c: Option<f64>) -> bool {
        self.last_temp_c = temp_c;
        // A None reading is treated as not hot, same as a dip below the
        // threshold: it resets the sustain timer.
        if temp_c.is_none_or(|t| t < self.threshold_c) {
            self.hot_since = None;
            return false;
        }

        let now = (self.now_fn)();
        let hot_since = *self.hot_since.get_or_insert(now);

        if now - hot_since < self.sustain_s {
            return false;
        }
        if self
            .last_fired_at
            .is_some_and(|last_fired_at| now - last_fired_at < self.cooldown_s)
        {
            return false;
        }
        self.last_fired_at = Some(now);
        true
    }
}
