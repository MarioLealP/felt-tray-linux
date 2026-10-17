//! Screen reactions (WP-07 on Linux): when the active window changes (after a 3 s settle) and every
//! `screen_sample_s` while you stay put, felt-tray decides over KWin's window LIST what may be seen
//! ([`policy`]), captures only that one window ([`kwin`]), drops near-duplicates, and sends it to felt-core,
//! whose reply she speaks. Nothing is captured while away (10 min idle) or when the blocklist can't be read.

pub mod kwin;
pub mod policy;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};

use crate::controls::Switches;
use crate::core::FeltCore;
use crate::voice::Voice;
use kwin::Snapshot;
use policy::Blocklist;

const SETTLE: Duration = Duration::from_secs(3);
const AFK_SKIP_S: u64 = 600;
const MAX_SIDE: u32 = 768;
const JPEG_QUALITY: u8 = 80;
/// dHash bits that may differ before a frame counts as "changed" (a starting guess - tune by the log).
const DEDUPE_DISTANCE: u32 = 6;

pub struct Screen {
    pub core: Arc<FeltCore>,
    pub voice: Voice,
    pub conn: zbus::Connection,
    pub blocklist: PathBuf,
    pub sample: Duration,
    pub session_id: String,
    pub switches: Arc<Switches>,
}

impl Screen {
    pub async fn run(self, mut windows: watch::Receiver<Snapshot>) {
        tracing::info!("screen reactions on (KWin), sample every {:?}", self.sample);
        let mut last_active = String::new();
        let mut settle: Option<Instant> = None;
        let mut next_sample = Instant::now() + self.sample;
        let mut seen: HashMap<String, u64> = HashMap::new();
        loop {
            let wake = settle.map_or(next_sample, |s| s.min(next_sample));
            tokio::select! {
                changed = windows.changed() => {
                    if changed.is_err() { break; }
                    let active = windows.borrow().active.clone();
                    if active != last_active {
                        last_active = active;
                        settle = Some(Instant::now() + SETTLE);
                    }
                }
                () = sleep_until(wake) => {
                    let trigger = if settle.is_some_and(|s| s <= Instant::now()) {
                        settle = None;
                        "focus"
                    } else {
                        next_sample = Instant::now() + self.sample;
                        if kwin::idle_seconds(&self.conn).await.is_some_and(|s| s > AFK_SKIP_S) {
                            continue;
                        }
                        "periodic"
                    };
                    let snap = windows.borrow().clone();
                    self.capture_and_send(&snap, trigger, &mut seen).await;
                }
            }
        }
    }

    /// Decide (window list only) -> capture that one window -> dedupe -> send. The order IS the privacy
    /// guarantee: nothing is captured before `policy::decide` picked an allowed window.
    async fn capture_and_send(
        &self,
        snap: &Snapshot,
        trigger: &str,
        seen: &mut HashMap<String, u64>,
    ) {
        if !self.switches.unprompted_ok() {
            return; // quiet / hidden: she does not look either
        }
        let blocklist = match Blocklist::load(&self.blocklist) {
            Ok(b) => b,
            Err(e) => return tracing::warn!("no capture (fail closed): {e:#}"),
        };
        let decision = policy::decide(
            &snap.windows,
            &blocklist,
            Some(&snap.active),
            std::process::id(),
        );
        tracing::info!(
            "screen: trigger={trigger} {}",
            policy::format_decision(&decision)
        );
        let Some(target) = decision.target.cloned() else {
            return;
        };

        let shot = match kwin::capture_window(&self.conn, &target.id).await {
            Ok(img) => img,
            Err(e) => return tracing::warn!("screen: capture of {} failed: {e:#}", target.label()),
        };
        let (jpeg, hash) = match tokio::task::spawn_blocking(move || encode(shot)).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                return tracing::info!("screen: {} is (near) black, skipped", target.label());
            }
            Err(e) => return tracing::warn!("screen: encode failed: {e}"),
        };
        if let Some(&prev) = seen.get(&target.id) {
            let dist = (prev ^ hash).count_ones();
            let dup = dist <= DEDUPE_DISTANCE;
            tracing::info!(
                "screen: dhash app={} dist={dist} thr={DEDUPE_DISTANCE} -> {}",
                target.label(),
                if dup { "skip" } else { "send" }
            );
            if dup {
                return;
            }
        }
        seen.insert(target.id.clone(), hash);

        match self
            .core
            .react_screen(
                jpeg,
                &target.app,
                &target.title,
                trigger,
                &self.session_id,
                self.voice.tone(),
            )
            .await
        {
            Ok(Some(reply)) => {
                tracing::info!(
                    "screen: reaction to {} ({} chars)",
                    target.label(),
                    reply.text.len()
                );
                self.voice
                    .say(&reply.text, reply.audio, reply.tag, reply.emotion, false);
            }
            Ok(None) => tracing::info!("screen: no reaction this time (503)"),
            Err(e) => tracing::warn!("screen: sending to felt-core failed: {e:#}"),
        }
    }
}

/// Downscale + JPEG, and a 64-bit difference hash for dedupe. None = (near) black (minimized, protected).
fn encode(img: RgbImage) -> Option<(Vec<u8>, u64)> {
    let mean = img
        .pixels()
        .map(|p| u64::from(p.0.iter().map(|&c| u32::from(c)).sum::<u32>()))
        .sum::<u64>()
        / (u64::from(img.width()) * u64::from(img.height()) * 3).max(1);
    if mean < 3 {
        return None;
    }
    let mut img = DynamicImage::ImageRgb8(img);
    let long = img.width().max(img.height());
    if long > MAX_SIDE {
        let scale = f64::from(MAX_SIDE) / f64::from(long);
        let (w, h) = (
            (f64::from(img.width()) * scale) as u32,
            (f64::from(img.height()) * scale) as u32,
        );
        img = img.resize_exact(w.max(1), h.max(1), FilterType::Triangle);
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&img)
        .ok()?;
    Some((jpeg, dhash(&img)))
}

fn dhash(img: &DynamicImage) -> u64 {
    let small = img.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            bits = (bits << 1)
                | u64::from(small.get_pixel(x, y).0[0] > small.get_pixel(x + 1, y).0[0]);
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, shift: u8) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x as u8).wrapping_add(shift), (y as u8), 128])
        })
    }

    #[test]
    fn black_frames_are_skipped() {
        assert!(encode(RgbImage::new(64, 64)).is_none());
    }

    #[test]
    fn big_frames_are_downscaled_to_the_long_side() {
        let (jpeg, _) = encode(gradient(2000, 1000, 0)).unwrap();
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (768, 384));
    }

    #[test]
    fn same_frame_hashes_equal_different_frame_differs() {
        let a = dhash(&DynamicImage::ImageRgb8(gradient(200, 100, 0)));
        let b = dhash(&DynamicImage::ImageRgb8(gradient(200, 100, 0)));
        let c = dhash(&DynamicImage::ImageRgb8(RgbImage::from_fn(
            200,
            100,
            |x, _| image::Rgb([if (x / 10) % 2 == 0 { 255 } else { 0 }, 0, 0]),
        )));
        assert_eq!(a, b);
        assert!((a ^ c).count_ones() > DEDUPE_DISTANCE);
    }
}
