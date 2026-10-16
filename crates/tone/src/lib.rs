//! WP-15: builds the one-line tone instruction felt-core appends to its
//! system prompt, from mood x time-of-day (RX-5's own default table,
//! docs/plans/felt-review-2026-09-22.md in the E:\Felt umbrella repo) plus
//! streak awareness over the last few reaction emotions. Pure functions -
//! the HTTP layer's `GET /tone` just calls [`build_tone_line`] with the live
//! mood and the current hour.

use std::cmp::min;
use std::collections::BTreeMap;

pub const MORNING: &str = "morning";
pub const DAY: &str = "day";
pub const EVENING: &str = "evening";
pub const LATE_NIGHT: &str = "late_night";

/// Map of mood-axis name to value in `0.0..=1.0`; missing keys fall back to
/// the same defaults the Python `dict.get` calls used (see [`mood_modifiers`]).
pub type Mood = BTreeMap<String, f64>;

pub fn time_band(hour: i32) -> &'static str {
    if !(4..23).contains(&hour) {
        LATE_NIGHT
    } else if hour < 11 {
        MORNING
    } else if hour < 18 {
        DAY
    } else {
        EVENING
    }
}

/// RX-5's own default table.
fn baseline_tone(band: &str) -> &'static str {
    match band {
        MORNING => "Soft and a bit sleepy, encouraging.",
        DAY => "Hype-friend energy - upbeat and energetic.",
        EVENING => "Cozy, relaxed, playful teasing.",
        LATE_NIGHT => "Tired and cheeky - needle him a little about still being up this late.",
        _ => unreachable!("time_band only returns the four known bands"),
    }
}

// Thresholds above which a mood axis is considered "active" enough to
// color the tone line - matches WP-15's own bulleted mood -> tone list.
const ANNOYED_THRESHOLD: f64 = 0.55;
const SLEEPY_ENERGY_THRESHOLD: f64 = 0.30;
const BORED_THRESHOLD: f64 = 0.60;
const HAPPY_THRESHOLD: f64 = 0.70;
const AFFECTION_THRESHOLD: f64 = 0.70;

fn mood_value(mood: &Mood, key: &str, default: f64) -> f64 {
    mood.get(key).copied().unwrap_or(default)
}

fn mood_modifiers(mood: &Mood) -> Vec<&'static str> {
    let mut mods: Vec<&'static str> = Vec::new();
    // Checked first/strongest: WP-15 says annoyed lines should be short
    // and sarcastic with more of the reaction carried by face/body than
    // words - that's a stronger override of the baseline band than the
    // others below, so it's listed first.
    if mood_value(mood, "annoyance", 0.0) >= ANNOYED_THRESHOLD {
        mods.push(
            "She's a little annoyed right now - keep it short and a bit \
             sarcastic, let her face and body carry more of the reaction \
             than words do.",
        );
    }
    if mood_value(mood, "energy", 1.0) <= SLEEPY_ENERGY_THRESHOLD {
        mods.push("She's low-energy and sleepy - fewer, shorter lines, the odd yawn is fine.");
    }
    if mood_value(mood, "boredom", 0.0) >= BORED_THRESHOLD {
        mods.push("She's bored - more heckling and unprompted commentary, to stir things up.");
    }
    if mood_value(mood, "happiness", 0.0) >= HAPPY_THRESHOLD
        || mood_value(mood, "affection", 0.0) >= AFFECTION_THRESHOLD
    {
        // Was "more cheering and forgiving" - cheering fed B-1's hype loop.
        mods.push("She's in a genuinely good mood - warmer and more forgiving than usual.");
    }
    mods
}

// Streak escalation - WP-15: "the reaction prompt carries the last few
// reactions and events, so the 3rd death in a row doesn't get the same
// line as the 1st... sympathy -> teasing -> 'want me to look away?'."
// Grounded in the emotion values felt-core's screen reactions actually
// produce (X-Emotion: happy/excited/amused/sad/annoyed/surprised/
// thinking/sleepy/neutral) - there's no real "death detector" anywhere in
// the system, so a losing streak is read off consecutive sad/annoyed
// reactions and a winning one off consecutive happy/excited ones, which is
// the honest signal that actually exists rather than an invented one.
//
// 2026-09-27 (B-1 root cause, personality plan): the streak is HER OWN
// recent reaction emotions, not anything that happened in the game. The old
// lines ("A 3rd win in a row - go big, she's genuinely hyped now") read her
// own excitement as wins and told the model to escalate. That produced more
// excitement and extended the "streak": a feedback loop. It is literally where
// "Three in a row! Another win!" came from in an ARPG with no wins. The
// lines now DAMP instead. A run of same-register remarks means she's in a
// rut, so the prompt asks her to switch register, and it never claims an
// event happened.
const NEGATIVE_EMOTIONS: &[&str] = &["sad", "annoyed"];
const POSITIVE_EMOTIONS: &[&str] = &["happy", "excited"];

const LOSS_STREAK_LINES: [&str; 2] = [
    "Your last couple of remarks were grumpy - switch it up: curious, dry, or kind this time.",
    "Your last few remarks were all grumpy - drop it entirely, say something light or ask him \
     something.",
];
const WIN_STREAK_LINES: [&str; 2] = [
    "Your last couple of remarks were upbeat - switch register: calm, curious, dry or teasing, \
     not more hype.",
    "Your last few remarks were all hype - no cheering this time. Make an observation, ask a \
     question, or say something about yourself.",
];

fn current_streak_run(streak: &[&str], emotions: &[&str]) -> usize {
    match streak.last() {
        Some(last) if emotions.contains(last) => {}
        _ => return 0,
    }
    streak
        .iter()
        .rev()
        .take_while(|emotion| emotions.contains(emotion))
        .count()
}

fn streak_line(streak: &[&str]) -> Option<&'static str> {
    let loss_run = current_streak_run(streak, NEGATIVE_EMOTIONS);
    if loss_run >= 2 {
        return Some(LOSS_STREAK_LINES[min(loss_run, 3) - 2]);
    }
    let win_run = current_streak_run(streak, POSITIVE_EMOTIONS);
    if win_run >= 2 {
        return Some(WIN_STREAK_LINES[min(win_run, 3) - 2]);
    }
    None
}

/// `streak` is the last few reaction emotions, oldest first (the server
/// keeps a short rolling window) - a first/only reaction in any direction
/// gets no escalation text, just the baseline band + mood modifiers.
pub fn build_tone_line(mood: &Mood, hour: i32, streak: Option<&[&str]>) -> String {
    let band = time_band(hour);
    let mut parts = vec![baseline_tone(band)];
    parts.extend(mood_modifiers(mood));
    if let Some(streak_line) = streak_line(streak.unwrap_or_default()) {
        parts.push(streak_line);
    }
    parts.join(" ")
}
