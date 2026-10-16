//! Her hide / quiet switches (WP-42.5), set from the Felt app anywhere (they live in felt-core). Polled every
//! 5 s. Quiet: she stops mid-sentence and says nothing on her own (screen reactions, her own lines) - an
//! answer you asked for still speaks. Hide: her body walks off screen, and nothing unprompted either. Urgent lines
//! (calendar heads-up) always get through. A heartbeat every 15 s tells felt-core the PC side is alive.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::core::FeltCore;
use crate::voice::Voice;

const POLL: Duration = Duration::from_secs(5);
const HEARTBEAT_EVERY: u32 = 3; // polls

#[derive(Default)]
pub struct Switches {
    quiet: AtomicBool,
    hidden: AtomicBool,
}

impl Switches {
    /// May she say something nobody asked for?
    pub fn unprompted_ok(&self) -> bool {
        !self.quiet.load(Ordering::Relaxed) && !self.hidden.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Deserialize)]
pub struct State {
    #[serde(default)]
    pub version: u64,
    pub hide: Switch,
    pub quiet: Switch,
}

#[derive(Debug, Deserialize)]
pub struct Switch {
    #[serde(default)]
    pub on: bool,
}

pub async fn run(core: Arc<FeltCore>, switches: Arc<Switches>, voice: Voice) {
    let mut applied: Option<u64> = None;
    let mut tick = 0u32;
    loop {
        match core.controls().await {
            Ok(s) => {
                let was_quiet = switches.quiet.swap(s.quiet.on, Ordering::Relaxed);
                let was_hidden = switches.hidden.swap(s.hide.on, Ordering::Relaxed);
                if s.quiet.on && !was_quiet {
                    voice.player().stop(); // cut off mid-sentence
                }
                if s.hide.on != was_hidden || applied.is_none() {
                    voice.set_hidden(s.hide.on);
                }
                if applied != Some(s.version) {
                    tracing::info!(
                        "controls: v{} quiet={} hide={}",
                        s.version,
                        s.quiet.on,
                        s.hide.on
                    );
                    applied = Some(s.version);
                }
            }
            Err(e) => tracing::debug!("controls: {e:#}"),
        }
        if tick.is_multiple_of(HEARTBEAT_EVERY) {
            let report = json!({
                "animation": voice.has_body(),
                "applied": applied,
                "presence": if switches.hidden.load(Ordering::Relaxed) { "hidden" } else { "visible" },
                "hushed": switches.quiet.load(Ordering::Relaxed),
            });
            if let Err(e) = core.report_desktop(&report).await {
                tracing::debug!("controls: heartbeat: {e:#}");
            }
        }
        tick = tick.wrapping_add(1);
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_felt_core_state() {
        let s: State = serde_json::from_str(
            r#"{"version": 4, "hide": {"on": false, "until": null}, "quiet": {"on": true, "until": "2026-10-15T23:00:00Z"}}"#,
        )
        .unwrap();
        assert!(s.quiet.on && !s.hide.on && s.version == 4);
    }

    #[test]
    fn quiet_or_hide_stops_unprompted_speech() {
        let sw = Switches::default();
        assert!(sw.unprompted_ok());
        sw.quiet.store(true, Ordering::Relaxed);
        assert!(!sw.unprompted_ok());
        sw.quiet.store(false, Ordering::Relaxed);
        sw.hidden.store(true, Ordering::Relaxed);
        assert!(!sw.unprompted_ok());
    }
}
