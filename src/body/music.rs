//! Music (WP-16): she bobs to whatever's playing. The speakers' output (PipeWire: the default output's monitor,
//! 16 kHz mono floats from `pw-record`) feeds a rolling 12 s buffer in memory - never stored or sent anywhere.
//! Every second the last 8 s give a tempo estimate (felt-beat); once it holds steady, main.gd gets the beat
//! (bpm, phase, how hard - louder and happier = more) every 2 s, and an explicit stop when it's lost. Music
//! starting / stopping / getting intense are reaction-table events (a face, sometimes a clip).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::process::Stdio;
use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_beat::{TempoLock, estimate_tempo_from_audio};
use felt_music_events::MusicEventTracker;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::clock;
use super::ctx::Ctx;
use super::{presence, react};

pub const RATE: u32 = 16_000;
const BUFFER_S: usize = 12;
const WINDOW_S: usize = 8;
const TICK: Duration = Duration::from_secs(1);
const BEAT_RESEND: Duration = Duration::from_secs(2);
/// Below this RMS it's silence, not music; this RMS counts as full volume. Calibrated on the Linux desktop
/// (2026-10-16: Spotify at normal volume through PipeWire's monitor ~0.01 RMS, peaks ~0.05 - Windows' WASAPI
/// loopback ran hotter, its 0.01 floor called that silence). Only the floor cares about level; the tempo
/// estimate itself works on ratios.
const ENERGY_FLOOR: f64 = 0.002;
const ENERGY_REFERENCE: f64 = 0.02;

type Buffer = Rc<RefCell<VecDeque<f32>>>;

pub async fn run(ctx: Rc<Ctx>, capture: Vec<String>) {
    let buf: Buffer = Rc::new(RefCell::new(VecDeque::with_capacity(
        RATE as usize * BUFFER_S,
    )));
    tokio::task::spawn_local(listen(capture, buf.clone()));
    let mut lock = TempoLock::new();
    let mut events = MusicEventTracker::new(clock::monotonic_s as fn() -> f64);
    let mut tick = tokio::time::interval(TICK);
    // one continuous beat clock: advanced by each tick's own bpm, so a drifting estimate never jumps the phase
    let (mut phase, mut ticked, mut sent_at) = (0.0f64, Instant::now(), None::<Instant>);
    loop {
        tick.tick().await;
        let frames: Vec<f64> = {
            let b = buf.borrow();
            let take = b.len().min(RATE as usize * WINDOW_S);
            b.iter()
                .skip(b.len() - take)
                .map(|&x| f64::from(x))
                .collect()
        };
        let energy = rms(&frames);
        let estimate = (energy >= ENERGY_FLOOR)
            .then(|| estimate_tempo_from_audio(&frames, RATE))
            .flatten()
            .map(|(bpm, confidence)| (fold_octave(bpm), confidence));
        tracing::debug!(
            "music: energy {energy:.4}, estimate {estimate:?}, {} s buffered",
            frames.len() as u32 / RATE
        );
        let was = lock.locked;
        let locked = lock.update(estimate, None);
        ctx.tempo_locked.set(locked);
        let dt = ticked.elapsed().as_secs_f64();
        ticked = Instant::now();
        if locked {
            if was {
                phase = (phase + dt * lock.bpm / 60.0).rem_euclid(1.0);
            } else {
                phase = 0.0;
                sent_at = None;
                tracing::info!("music: tempo lock at {:.1} BPM", lock.bpm);
            }
            if sent_at.is_none_or(|t| t.elapsed() >= BEAT_RESEND) {
                sent_at = Some(Instant::now());
                let happiness = ctx
                    .mood
                    .borrow()
                    .values
                    .get("happiness")
                    .copied()
                    .unwrap_or(0.5);
                let amplitude =
                    (energy / ENERGY_REFERENCE).clamp(0.3, 1.0) * (0.5 + 0.5 * happiness);
                let beat = json!({ "active": true, "bpm": lock.bpm, "phase": phase, "amplitude": amplitude });
                ctx.overlay.send(&json!({ "beat": beat })).await;
            }
        } else if was {
            tracing::info!("music: tempo lock lost");
            ctx.overlay
                .send(&json!({ "beat": { "active": false } }))
                .await;
        }
        for event in events.update(locked, energy) {
            tracing::info!("music: {event}");
            match event {
                "music_start" => {
                    super::props::start_event(&ctx, "music_start"); // headphones on
                }
                "music_stop" if ctx.props.store.borrow().held_id("headphones").is_some() => {
                    let c = ctx.clone();
                    tokio::task::spawn_local(async move {
                        super::props::director::take_off(&c, "headphones").await;
                    });
                }
                _ => {}
            }
            if presence::visible(&ctx) && !ctx.behavior.busy() {
                react::reaction(&ctx, event).await;
            }
        }
    }
}

/// Keeps `pw-record` running (restarted after a pause if it stops) and the buffer filled.
async fn listen(capture: Vec<String>, buf: Buffer) {
    let Some((program, args)) = capture.split_first() else {
        return;
    };
    loop {
        // nosemgrep: clockmost-shell-out  the configured capture command (no shell); audio stays in memory
        let child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        match child {
            Ok(mut c) => {
                if let Some(mut out) = c.stdout.take() {
                    read_samples(&mut out, &buf).await;
                }
                let _ = c.kill().await;
                tracing::info!("music: capture stopped - restarting in 10 s");
            }
            Err(e) => tracing::warn!("music: can't start {program}: {e} - retrying in 10 s"),
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn read_samples(out: &mut tokio::process::ChildStdout, buf: &Buffer) {
    let cap = RATE as usize * BUFFER_S;
    let mut chunk = vec![0u8; 4096];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match out.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        carry.extend_from_slice(&chunk[..n]);
        let whole = carry.len() / 4 * 4;
        let mut b = buf.borrow_mut();
        let (samples, _) = carry[..whole].as_chunks::<4>();
        b.extend(samples.iter().map(|s| f32::from_le_bytes(*s)));
        let extra = b.len().saturating_sub(cap);
        b.drain(..extra);
        drop(b);
        carry.drain(..whole);
    }
}

/// The estimator flips between a tempo and its double/half (live: 86 <-> 171 BPM second to second), which a
/// 4 % lock never holds through: fold every estimate into one octave. Half-time bobbing to fast music looks right.
fn fold_octave(mut bpm: f64) -> f64 {
    const LO: f64 = 75.0;
    if bpm <= 0.0 {
        return bpm;
    }
    while bpm >= LO * 2.0 {
        bpm /= 2.0;
    }
    while bpm < LO {
        bpm *= 2.0;
    }
    bpm
}

fn rms(frames: &[f64]) -> f64 {
    if frames.is_empty() {
        return 0.0;
    }
    (frames.iter().map(|x| x * x).sum::<f64>() / frames.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_and_a_square() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 10]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn octave_flips_fold_together() {
        assert!((fold_octave(171.0) - 85.5).abs() < 1e-9);
        assert!((fold_octave(86.0) - 86.0).abs() < 1e-9);
        assert!((fold_octave(60.0) - 120.0).abs() < 1e-9);
        assert!((fold_octave(149.0) - 149.0).abs() < 1e-9);
        assert!((fold_octave(150.0) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn a_click_track_locks_a_tempo() {
        // 120 BPM clicks for 8 s
        let mut frames = vec![0.0f64; RATE as usize * 8];
        let beat = RATE as usize / 2;
        for start in (0..frames.len()).step_by(beat) {
            for s in frames.iter_mut().skip(start).take(400) {
                *s = 0.8;
            }
        }
        let (bpm, _conf) = estimate_tempo_from_audio(&frames, RATE).expect("a tempo");
        assert!(
            (bpm - 120.0).abs() < 3.0 || (bpm - 60.0).abs() < 2.0,
            "got {bpm}"
        );
    }
}
