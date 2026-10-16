//! Her face between and during replies (WP-12): every 5-20 s while she's idle a small micro-expression (the
//! reaction table's mood-weighted `idle_micro`, else a soft smile / curious brow / pout, never twice in a row);
//! while she talks, brief brow raises on the voice's loud peaks, and a closing soft smile after a happy reply.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_face_life::{
    BROW_RAISE_HOLD_S, BROW_RAISE_INTENSITY, MICRO_EXPRESSION_MAX_INTERVAL_S,
    MICRO_EXPRESSION_MIN_INTERVAL_S,
};
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::{Value, json};

use super::ctx::Ctx;
use super::presence;

/// Poses after which she finishes a reply with a soft smile.
const HAPPY_FINISH: [&str; 3] = ["soft_smile", "grin", "smug"];

struct Dice(StdRng);

impl felt_face_life::Rng for Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0.random_range(0..n)
    }
}

pub async fn micro_expressions(ctx: Rc<Ctx>) {
    let mut last: Option<String> = None;
    loop {
        let wait = rand::rng()
            .random_range(MICRO_EXPRESSION_MIN_INTERVAL_S..MICRO_EXPRESSION_MAX_INTERVAL_S);
        tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        if !presence::settled_visible(&ctx)
            || ctx.behavior.busy()
            || ctx.face_held_until.get() > Instant::now()
        {
            continue;
        }
        let pose = table_pick(&ctx, last.as_deref()).unwrap_or_else(|| {
            felt_face_life::pick_micro_expression(
                &mut Dice(StdRng::from_rng(&mut rand::rng())),
                last.as_deref(),
            )
            .to_owned()
        });
        let hold = rand::rng().random_range(1.5..3.0);
        ctx.apply_pose(&pose, hold).await;
        last = Some(pose);
    }
}

fn table_pick(ctx: &Ctx, last: Option<&str>) -> Option<String> {
    let entries = ctx.assets.reactions.get("idle_micro")?;
    let mood: HashMap<String, f64> = {
        let mut m = ctx.mood.borrow_mut();
        m.tick();
        m.as_dict().into_iter().collect()
    };
    let mut rng = StdRng::from_rng(&mut rand::rng());
    let picked = felt_reaction_table::pick_reaction(entries, &mood, last, &mut rng)?;
    picked
        .get("pose")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// While a sentence plays (from its real start): brow raises on the loud peaks, then - after a happy reply's
/// last sentence - a soft smile.
pub async fn accents(ctx: Rc<Ctx>, wav: Vec<u8>, pose: String) {
    let Some((mono, rate)) = decode_wav(&wav) else {
        return;
    };
    let started = Instant::now();
    for peak in felt_face_life::detect_envelope_peaks(&mono, 1, rate) {
        let at = started + Duration::from_secs_f64(peak);
        tokio::time::sleep_until(at.into()).await;
        let brows = json!({ "brows": { "Fcl_BRW_Surprised": BROW_RAISE_INTENSITY } });
        ctx.overlay.face(&brows, BROW_RAISE_HOLD_S).await;
    }
    if HAPPY_FINISH.contains(&pose.as_str()) {
        let end = started + Duration::from_secs_f64(mono.len() as f64 / f64::from(rate));
        tokio::time::sleep_until(end.into()).await;
        ctx.apply_pose("soft_smile", 1.5).await;
    }
}

/// 16-bit PCM WAV -> mono samples and the rate (her voice is always 16-bit); None for anything else.
fn decode_wav(b: &[u8]) -> Option<(Vec<i16>, u32)> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut channels, mut rate, mut bits) = (12usize, 0u16, 0u32, 0u16);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = b.get(pos + 8..(pos + 8 + len).min(b.len()))?;
        if id == b"fmt " && body.len() >= 16 {
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 || channels == 0 {
                return None;
            }
            let (samples, _) = body.as_chunks::<2>();
            let mono = samples
                .chunks(channels as usize)
                .map(|f| i16::from_le_bytes(f[0]))
                .collect();
            return Some((mono, rate));
        }
        pos += 8 + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data.len() as u32).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * 2).to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(data);
        v
    }

    #[test]
    fn decodes_a_16_bit_wav() {
        let (s, r) = decode_wav(&wav(&[0, 100, -100, 32000], 24000)).unwrap();
        assert_eq!((s, r), (vec![0, 100, -100, 32000], 24000));
        assert!(decode_wav(b"nope").is_none());
    }

    #[test]
    fn her_recorded_clips_decode() {
        let p = std::path::Path::new(env!("HOME"))
            .join("Drives/Storage/Felt/Felt/Sounds/presence/unhush_1.wav");
        if let Ok(b) = std::fs::read(p) {
            let (s, r) = decode_wav(&b).expect("a 16-bit wav");
            assert!(r > 8000 && !s.is_empty());
        }
    }
}
