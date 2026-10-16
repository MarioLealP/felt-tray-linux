//! Calls (WP-09/WP-17): while someone on your call is talking she ducks her voice (~9 dB) instead of staying
//! silent - she may still talk, just quieter. A call = a call app (Equibop/Discord, Teams, Zoom) with its mic
//! stream open in PipeWire; then each of its playback streams is captured (16 kHz mono, in memory only, levels
//! only - never content) and its peaks decide "someone's talking" (a threshold and 1 s hang time). The same
//! peaks make the call lively/quiet events and a guess that a friend joined (after 2 min of silence).

use std::collections::BTreeSet;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use felt_call_energy::{
    CallEnergyTracker, CallJoinGuessTracker, ENERGY_SMOOTHING, JOIN_GUESS_SILENCE_S,
    LIVELY_PEAK_THRESHOLD, LIVELY_SUSTAIN_S, QUIET_HANG_S,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use super::ctx::Ctx;
use super::{clock, presence, react};

const POLL: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(50);
const HANG: Duration = Duration::from_secs(1);
const RATE: u32 = 16_000;

pub struct Settings {
    pub apps: Vec<String>,
    pub peak_threshold: f64,
    pub duck_db: f64,
}

/// The newest peak (f32 bits) from the call app's streams, reset each tick.
type Peak = Arc<AtomicU32>;

pub async fn run(ctx: Rc<Ctx>, s: Settings) {
    let duck = 10f64.powf(-s.duck_db / 20.0) as f32;
    let peak: Peak = Arc::new(AtomicU32::new(0));
    let mut captures: Vec<Child> = Vec::new();
    let mut streams: BTreeSet<u64> = BTreeSet::new();
    let mut checked = Instant::now() - POLL;
    let mut last_loud: Option<Instant> = None;
    let mut talking = false;
    let now = clock::monotonic_s as fn() -> f64;
    let mut energy = CallEnergyTracker::new(
        LIVELY_PEAK_THRESHOLD,
        LIVELY_SUSTAIN_S,
        QUIET_HANG_S,
        ENERGY_SMOOTHING,
        now,
    );
    let mut join = CallJoinGuessTracker::new(JOIN_GUESS_SILENCE_S, now);
    let (mut last_second, mut second_peak) = (Instant::now(), 0.0f64);
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        if checked.elapsed() >= POLL {
            checked = Instant::now();
            let now_streams = call_streams(&s.apps).await;
            if now_streams != streams {
                captures.clear(); // kill_on_drop
                for id in &now_streams {
                    if let Some(c) = capture(*id, peak.clone()) {
                        captures.push(c);
                    }
                }
                if now_streams.is_empty() {
                    tracing::info!("call: ended");
                } else if streams.is_empty() {
                    tracing::info!("call: on ({} stream(s))", now_streams.len());
                }
                streams = now_streams;
            }
        }
        if streams.is_empty() {
            if talking {
                talking = false;
                ctx.player.set_gain(1.0);
            }
            continue;
        }
        let p = f64::from(f32::from_bits(peak.swap(0, Ordering::Relaxed)));
        second_peak = second_peak.max(p);
        if p > s.peak_threshold {
            last_loud = Some(Instant::now());
        }
        let now_talking = last_loud.is_some_and(|t| t.elapsed() <= HANG);
        if now_talking != talking {
            talking = now_talking;
            ctx.player.set_gain(if talking { duck } else { 1.0 });
            tracing::debug!(
                "call: {}",
                if talking {
                    "someone's talking - ducked"
                } else {
                    "quiet - unducked"
                }
            );
        }
        if last_second.elapsed() >= Duration::from_secs(1) {
            last_second = Instant::now();
            let visible = presence::visible(&ctx) && !ctx.behavior.busy();
            match energy.update(second_peak) {
                Some(e) if visible => {
                    tracing::info!("signals: call_{e}");
                    react::signal_event(&ctx, &format!("call_{e}"), Value::Null).await;
                }
                _ => {}
            }
            if join.update(talking) && visible {
                tracing::info!("signals: call_join_guess");
                react::signal_event(&ctx, "call_join_guess", Value::Null).await;
            }
            second_peak = 0.0;
        }
    }
}

/// The playback stream node ids of a call app whose mic is open (= in a call); empty otherwise.
async fn call_streams(apps: &[String]) -> BTreeSet<u64> {
    // nosemgrep: clockmost-shell-out  PipeWire's own graph dump (no shell, no input)
    let Ok(out) = Command::new("pw-dump")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
    else {
        return BTreeSet::new();
    };
    let Ok(nodes) = serde_json::from_slice::<Vec<Value>>(&out.stdout) else {
        return BTreeSet::new();
    };
    streams_of(&nodes, apps)
}

fn streams_of(nodes: &[Value], apps: &[String]) -> BTreeSet<u64> {
    let app_of = |n: &Value| {
        let p = &n["info"]["props"];
        let app = p["application.name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        apps.contains(&app).then_some((
            app,
            p["media.class"].as_str().unwrap_or_default().to_owned(),
        ))
    };
    let on_call: BTreeSet<String> = nodes
        .iter()
        .filter_map(app_of)
        .filter(|(_, class)| class == "Stream/Input/Audio")
        .map(|(app, _)| app)
        .collect();
    nodes
        .iter()
        .filter(|n| {
            app_of(n).is_some_and(|(app, class)| {
                class == "Stream/Output/Audio" && on_call.contains(&app)
            })
        })
        .filter_map(|n| n["id"].as_u64())
        .collect()
}

/// Records one call-app playback stream; each 50 ms block's peak goes into `peak` (the loudest wins).
fn capture(node: u64, peak: Peak) -> Option<Child> {
    // nosemgrep: clockmost-shell-out  PipeWire's recorder on the call app's own stream; levels only, in memory
    let mut child = Command::new("pw-record")
        .args([
            "--raw",
            "--target",
            &node.to_string(),
            "--rate",
            &RATE.to_string(),
            "--channels",
            "1",
        ])
        .args([
            "--format",
            "f32",
            "-P",
            "{ media.name=\"Felt listening for call voices\" }",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .inspect_err(|e| tracing::warn!("call: can't record stream {node}: {e}"))
        .ok()?;
    let mut out = child.stdout.take()?;
    tokio::spawn(async move {
        let mut buf = vec![0u8; (RATE / 20 * 4) as usize];
        while let Ok(n) = out.read(&mut buf).await {
            if n == 0 {
                return;
            }
            let (samples, _) = buf[..n / 4 * 4].as_chunks::<4>();
            let p = samples
                .iter()
                .map(|s| f32::from_le_bytes(*s).abs())
                .fold(0.0f32, f32::max);
            let _ = peak.fetch_max(p.to_bits(), Ordering::Relaxed); // positive f32 bits order like the values
        }
    });
    Some(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: u64, app: &str, class: &str) -> Value {
        json!({"id": id, "info": {"props": {"application.name": app, "media.class": class}}})
    }

    #[test]
    fn only_an_app_with_its_mic_open_is_on_a_call() {
        let apps = vec!["equibop".to_owned()];
        let mut nodes = vec![
            node(174, "equibop", "Stream/Output/Audio"),
            node(134, "equibop", "Stream/Output/Audio"),
            node(173, "FINAL FANTASY XIV", "Stream/Output/Audio"),
        ];
        assert!(streams_of(&nodes, &apps).is_empty());
        nodes.push(node(151, "equibop", "Stream/Input/Audio"));
        assert_eq!(streams_of(&nodes, &apps), BTreeSet::from([134, 174]));
    }

    #[test]
    fn duck_db_is_a_gain() {
        let g = 10f64.powf(-9.0 / 20.0);
        assert!((g - 0.3548).abs() < 1e-3);
    }
}
