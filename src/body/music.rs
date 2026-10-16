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

use felt_beat::{TempoLock, beat_phase, estimate_tempo_from_audio};
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
/// Below this RMS it's silence, not music; this RMS counts as full volume.
const ENERGY_FLOOR: f64 = 0.01;
const ENERGY_REFERENCE: f64 = 0.05;

type Buffer = Rc<RefCell<VecDeque<f32>>>;

pub async fn run(ctx: Rc<Ctx>, capture: Vec<String>) {
    let buf: Buffer = Rc::new(RefCell::new(VecDeque::with_capacity(
        RATE as usize * BUFFER_S,
    )));
    tokio::task::spawn_local(listen(capture, buf.clone()));
    let mut lock = TempoLock::new();
    let mut events = MusicEventTracker::new(clock::monotonic_s as fn() -> f64);
    let mut tick = tokio::time::interval(TICK);
    let (mut anchor, mut sent_at) = (Instant::now(), None::<Instant>);
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
            .flatten();
        let was = lock.locked;
        let locked = lock.update(estimate, None);
        ctx.tempo_locked.set(locked);
        if locked {
            if !was {
                anchor = Instant::now();
                sent_at = None;
                tracing::info!("music: tempo lock at {:.1} BPM", lock.bpm);
            }
            if sent_at.is_none_or(|t| t.elapsed() >= BEAT_RESEND) {
                sent_at = Some(Instant::now());
                let now = clock::monotonic_s();
                let reference = now - anchor.elapsed().as_secs_f64();
                let phase = beat_phase(now, reference, lock.bpm);
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
