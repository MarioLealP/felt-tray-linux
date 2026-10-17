//! Mouthing along (WP-16 part 3): while a song plays she sometimes mouths along to its chorus for a few seconds -
//! no sound, just her mouth, in subdued visemes (she isn't really singing). felt-core's /v1/music/now every 15 s
//! while music plays (30/60 s when nothing does), the lyrics once per track; inside the chorus a 35 % chance,
//! once per track. The real lyric lines time it (one viseme per word); without timed lines, a plausible pattern.

use std::rc::Rc;
use std::time::{Duration, Instant};

use rand::RngExt;
use rand::seq::IndexedRandom;
use serde_json::{Value, json};

use super::ctx::Ctx;
use super::presence;

const CHANCE: f64 = 0.35;
const SECONDS: f64 = 4.0;
const PLAYING_POLL: Duration = Duration::from_secs(15);
const IDLE_POLLS: [Duration; 2] = [Duration::from_secs(30), Duration::from_secs(60)];
const FINAL_LINE_S: f64 = 4.0;
const WORD_FILL: f64 = 0.85;

pub async fn run(ctx: Rc<Ctx>) {
    let mut idle = 0usize;
    let mut lyrics: Option<(String, Value)> = None;
    let mut mouthed: Option<String> = None;
    loop {
        let wait = if idle == 0 {
            PLAYING_POLL
        } else {
            IDLE_POLLS[(idle - 1).min(1)]
        };
        tokio::time::sleep(wait).await;
        let Ok(now) = ctx.core.music_now().await else {
            continue;
        };
        let polled = Instant::now();
        let track = now["track_id"]
            .as_str()
            .filter(|_| now["is_playing"].as_bool() == Some(true))
            .map(str::to_owned);
        idle = if track.is_some() { 0 } else { idle + 1 };
        let Some(track) = track else {
            continue;
        };
        if !presence::settled_visible(&ctx)
            || ctx.quiet.get()
            || mouthed.as_deref() == Some(track.as_str())
        {
            continue;
        }
        if lyrics.as_ref().is_none_or(|(t, _)| *t != track) {
            let Ok(l) = ctx.core.music_lyrics().await else {
                continue;
            };
            lyrics = Some((track.clone(), l));
        }
        let Some((_, l)) = &lyrics else { continue };
        let progress = now["progress_ms"].as_i64().unwrap_or(0);
        let chorus = &l["chorus"];
        let in_chorus = matches!((chorus["start_ms"].as_i64(), chorus["end_ms"].as_i64()), (Some(a), Some(b)) if (a..=b).contains(&progress));
        if l["found"].as_bool() != Some(true) || !in_chorus || !rand::rng().random_bool(CHANCE) {
            continue;
        }
        mouthed = Some(track);
        let at_ms = progress + polled.elapsed().as_millis() as i64;
        let mut cues = lyric_cues(&lines(l), at_ms, SECONDS);
        if cues.is_empty() {
            cues = synthetic_cues(SECONDS);
        }
        tracing::info!("sing: mouthing along ({} cues)", cues.len());
        ctx.overlay.lipsync(&cues).await;
    }
}

/// felt-core's timed lines -> (start_ms, end_ms?, text), time-sorted (it may name the list lines / synced_lines /
/// synced, the start start_ms / time_ms).
fn lines(payload: &Value) -> Vec<(i64, Option<i64>, String)> {
    let raw = ["lines", "synced_lines", "synced"]
        .iter()
        .find_map(|k| payload[*k].as_array());
    let mut out: Vec<(i64, Option<i64>, String)> = raw
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let start = i["start_ms"].as_i64().or_else(|| i["time_ms"].as_i64())?;
            Some((
                start,
                i["end_ms"].as_i64(),
                i["text"].as_str().unwrap_or_default().to_owned(),
            ))
        })
        .collect();
    out.sort_by_key(|l| l.0);
    out
}

/// A subdued viseme for a word's first vowel (never the full-weight D/E/F).
fn word_viseme(word: &str) -> &'static str {
    for c in word.to_lowercase().chars() {
        match c {
            'a' | 'o' => return "C",
            'e' | 'i' | 'y' => return "B",
            'u' => return "H",
            _ => {}
        }
    }
    "G"
}

/// The next `duration_s` of the song (t=0 = `position_ms`): one viseme per lyric word over its line's span,
/// closed between words and in gaps. Empty if no worded line overlaps the window.
fn lyric_cues(
    lines: &[(i64, Option<i64>, String)],
    position_ms: i64,
    duration_s: f64,
) -> Vec<Value> {
    let (w0, w1) = (
        position_ms as f64 / 1000.0,
        position_ms as f64 / 1000.0 + duration_s,
    );
    let mut cues = Vec::new();
    for (i, (start_ms, end_ms, text)) in lines.iter().enumerate() {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let start = *start_ms as f64 / 1000.0;
        let end = match end_ms {
            Some(e) => *e as f64 / 1000.0,
            None => lines
                .get(i + 1)
                .map_or(start + FINAL_LINE_S, |n| n.0 as f64 / 1000.0),
        };
        if end <= w0 || start >= w1 || end <= start {
            continue;
        }
        let slot = (end - start) / words.len() as f64;
        for (j, word) in words.iter().enumerate() {
            let ws = start + j as f64 * slot;
            for (a, b, v) in [
                (ws, ws + slot * WORD_FILL, word_viseme(word)),
                (ws + slot * WORD_FILL, ws + slot, "A"),
            ] {
                let (a, b) = (a.max(w0), b.min(w1));
                if b > a {
                    let r = |x: f64| ((x - w0) * 1000.0).round() / 1000.0;
                    cues.push(json!({ "start": r(a), "end": r(b), "value": v }));
                }
            }
        }
    }
    if cues.iter().any(|c| c["value"] != "A") {
        cues
    } else {
        Vec::new()
    }
}

/// Without timed lyrics: subdued visemes in 0.35 s pulses with brief closed gaps.
fn synthetic_cues(duration_s: f64) -> Vec<Value> {
    let mut cues = Vec::new();
    let mut t = 0.0;
    while t < duration_s {
        let end = (t + 0.35).min(duration_s);
        let v = ["B", "C", "G", "H"]
            .choose(&mut rand::rng())
            .copied()
            .unwrap_or("B");
        cues.push(json!({ "start": t, "end": end, "value": v }));
        t = end;
        if t >= duration_s {
            break;
        }
        let gap = (t + 0.15).min(duration_s);
        cues.push(json!({ "start": t, "end": gap, "value": "A" }));
        t = gap;
    }
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyric_lines_become_word_visemes_in_the_window() {
        let l = lines(
            &json!({"lines": [{"time_ms": 1000, "text": "oh baby"}, {"start_ms": 3000, "text": "uh"}]}),
        );
        let cues = lyric_cues(&l, 1000, 2.0);
        assert_eq!(cues[0]["value"], "C");
        assert_eq!(cues[2]["value"], "C"); // "baby": its first vowel is the a
        assert!(cues.iter().all(|c| c["end"].as_f64().unwrap() <= 2.0));
        assert!(lyric_cues(&l, 60_000, 2.0).is_empty());
    }

    #[test]
    fn synthetic_cues_fill_the_time_with_gaps() {
        let c = synthetic_cues(1.0);
        assert_eq!(c.last().unwrap()["end"], 1.0);
        assert!(c.iter().any(|x| x["value"] == "A"));
    }
}
