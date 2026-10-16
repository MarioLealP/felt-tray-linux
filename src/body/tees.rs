//! Her daily tee (WP-21/WP-27): once a day, one of the approved graphic tees (`tees_approved.json` in the tees
//! folder; 21 days before the same one again unless she's worn them all), as the default outfit's shirt
//! texture. Only felt.vrm (the default outfit) wears it. State in wardrobe.json with the outfit fields.

use std::path::Path;

use rand::seq::IndexedRandom;
use serde_json::{Value, json};

use super::clock;
use super::ctx::Ctx;
use super::outfits::Wardrobe;

const COOLDOWN_DAYS: i32 = 21;
const SHIRT_MATERIAL: &str = "N00_004_01_Tops_01_CLOTH";

/// Today's tee: a new roll on a new day, else the one already on. None without a tees folder or list.
pub fn todays(ctx: &Ctx) -> Option<(String, bool)> {
    let dir = ctx.tees.as_deref()?;
    let today = clock::local().date();
    let mut ws = Wardrobe::load(&ctx.wardrobe_path);
    let current = ws.get_str("current_tee");
    if ws.get_str("last_worn_date").as_deref() == Some(today.to_string().as_str())
        && let Some(t) = current
    {
        return Some((t, false));
    }
    let approved = approved(dir);
    let history = ws.0.get("history").cloned().unwrap_or(json!({}));
    let fresh: Vec<&String> = approved
        .iter()
        .filter(|t| {
            let last = history.get(t.as_str()).and_then(Value::as_str);
            !last
                .and_then(|d| d.parse::<jiff::civil::Date>().ok())
                .is_some_and(|d| (today - d).get_days() < COOLDOWN_DAYS)
        })
        .collect();
    let pool: Vec<&String> = if fresh.is_empty() {
        approved.iter().collect()
    } else {
        fresh
    };
    let chosen = (*pool.choose(&mut rand::rng())?).clone();
    tracing::info!("tee: today's is {chosen}");
    ws.set("current_tee", json!(chosen));
    ws.set("last_worn_date", json!(today.to_string()));
    let mut h = history.as_object().cloned().unwrap_or_default();
    h.insert(chosen.clone(), json!(today.to_string()));
    ws.set("history", Value::Object(h));
    ws.save(&ctx.wardrobe_path);
    Some((chosen, true))
}

/// Puts `tee` on the default outfit's shirt (`show_off`: a little show-off clip first).
pub async fn wear(ctx: &Ctx, tee: &str, show_off: bool) {
    let Some(dir) = ctx.tees.as_deref() else {
        return;
    };
    if show_off && ctx.assets.tags.contains("ambient/show_off") {
        ctx.overlay.play_tag("ambient/show_off", false, None).await;
    }
    let albedo = dir.join(format!("{tee}.png"));
    let garment = json!({ "material": SHIRT_MATERIAL, "albedo": albedo.to_string_lossy() });
    ctx.overlay.send(&json!({ "garment": garment })).await;
}

fn approved(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("tees_approved.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}
