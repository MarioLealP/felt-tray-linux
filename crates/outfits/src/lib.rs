//! WP-27 Tier-C VRM outfits: what she wears, when, and the change choreography.
//!
//! felt-animation decides; main.gd only swaps the model. The relay contract:
//!   -> {"outfit": {"id", "vrm", "fx": "puff"|"none"}}
//!   <- {"outfit_applied": {"id"}} | {"outfit_failed": {"id", "error"}}
//!
//! The decision half is pure (scheduled_outfit / pick_daily / ambient_pick /
//! bikini_roll take the clock and rng as arguments); [`OutfitDirector`] holds
//! the persisted state (in wardrobe.json next to the WP-33 tee fields) and
//! drives the overlay.
//!
//! Mario's rules (2026-09-30): Oct 31 halloween; Dec 23-26 xmas/reindeer on
//! alternate days; pyjama 01:00-05:00 or when sleepy; otherwise the day's
//! random daily-pool pick (default felt.vrm is the only one that wears the
//! tee); ambient swaps 85% daily / 15% themed, never special; bikini only as
//! a 1% reaction to a sunny weather answer.

pub mod datetime;
pub mod director;
pub mod rng;
pub mod state;
pub mod wardrobe;

pub use datetime::{Date, DateTime};
pub use director::OutfitDirector;
pub use rng::{OutfitRng, PyRandom};

use std::path::Path;

use serde_json::Value;

/// The fallback outfit id; `default` in the catalogue enables the feature.
pub const DEFAULT_ID: &str = "default";
pub const NIGHT_START_HOUR: u32 = 1;
/// First daily pick / back to the day outfit.
pub const DAY_START_HOUR: u32 = 5;
/// Same threshold as the ambient "sleepy" weighting.
pub const SLEEPY_ENERGY: f64 = 0.35;
pub const OVERRIDE_SECONDS: f64 = 2.0 * 3600.0;
pub const THEMED_SHARE: f64 = 0.15;
/// Into the spin clip.
pub const QUICK_SWAP_DELAY_S: f64 = 0.7;
pub const APPLY_TIMEOUT_S: f64 = 8.0;
pub const PRESENCE_WAIT_S: f64 = 90.0;
pub const FREE_WAIT_S: f64 = 90.0;

/// One catalogue entry (id/vrm/pool/label, plus optional Tier-A variants and
/// WP-32 accessories).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outfit {
    pub id: String,
    pub vrm: String,
    pub pool: String,
    pub label: Option<String>,
    /// Mario's keepers: approved Tier-A recolour/restyle VRMs of this outfit.
    pub variants: Vec<String>,
    /// WP-32 props worn while this outfit is on.
    pub accessories: Vec<String>,
}

/// The catalogue entries (id/vrm/pool/label). Missing/bad file -> [].
pub fn load_outfits(path: &Path) -> Vec<Outfit> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => {
            return Vec::new();
        }
    };
    let data: Value = match serde_json::from_str(&text) {
        Ok(data) => data,
        Err(_) => {
            return Vec::new();
        }
    };
    let entries: Vec<&Value> = match &data {
        Value::Object(map) => map
            .get("outfits")
            .and_then(Value::as_array)
            .map(|items| items.as_slice())
            .unwrap_or_default()
            .iter()
            .collect(),
        Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|e| {
            let e = e.as_object()?;
            let id = e.get("id")?.as_str()?;
            let vrm = e.get("vrm")?.as_str()?;
            let pool = e.get("pool")?.as_str()?;
            if id.is_empty() || vrm.is_empty() || pool.is_empty() {
                return None;
            }
            Some(Outfit {
                id: id.to_string(),
                vrm: vrm.to_string(),
                pool: pool.to_string(),
                label: e.get("label").and_then(Value::as_str).map(str::to_string),
                variants: str_list(e.get("variants")),
                accessories: str_list(e.get("accessories")),
            })
        })
        .collect()
}

/// The non-empty strings in a JSON list (variants/accessories).
fn str_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of one pool, in catalogue order.
pub fn pool_ids(outfits: &[Outfit], pool: &str) -> Vec<String> {
    outfits
        .iter()
        .filter(|e| e.pool == pool)
        .map(|e| e.id.clone())
        .collect()
}

/// The wardrobe "day" rolls at 05:00, not midnight.
pub fn effective_date(now: &DateTime) -> Date {
    now.minus_hours(DAY_START_HOUR).date()
}

/// A date/night rule's outfit, or None when the day outfit applies.
/// Date rules win over night (listed first in the spec).
pub fn scheduled_outfit(now: &DateTime, sleepy: bool) -> Option<&'static str> {
    if now.month == 10 && now.day == 31 {
        return Some("halloween");
    }
    if now.month == 12 && (23..=26).contains(&now.day) {
        return Some(if now.day % 2 == 1 { "xmas" } else { "reindeer" });
    }
    if (NIGHT_START_HOUR..DAY_START_HOUR).contains(&now.hour) || sleepy {
        return Some("pyjama");
    }
    None
}

/// The day's random daily-pool pick, never repeating yesterday's.
pub fn pick_daily(daily: &[String], yesterday: Option<&str>, rng: &mut dyn OutfitRng) -> String {
    let mut choices: Vec<String> = daily
        .iter()
        .filter(|i| Some(i.as_str()) != yesterday)
        .cloned()
        .collect();
    if choices.is_empty() {
        choices = daily.to_vec();
    }
    choices[rng.choice_index(choices.len())].clone()
}

/// 85% daily pool, 15% themed pool, never special.
pub fn ambient_pick(daily: &[String], themed: &[String], rng: &mut dyn OutfitRng) -> String {
    if !themed.is_empty() && (daily.is_empty() || rng.random() < THEMED_SHARE) {
        themed[rng.choice_index(themed.len())].clone()
    } else {
        daily[rng.choice_index(daily.len())].clone()
    }
}

/// A reply tag from a sunny weather answer.
pub fn is_sunny_weather_tag(tag: Option<&str>) -> bool {
    tag.is_some_and(|t| !t.is_empty() && t.starts_with("reaction/weather/") && t.contains("sun"))
}

/// The 1%-style bikini roll on a sunny weather answer.
pub fn bikini_roll(tag: Option<&str>, rng: &mut dyn OutfitRng, chance: f64) -> bool {
    is_sunny_weather_tag(tag) && rng.random() < chance
}
