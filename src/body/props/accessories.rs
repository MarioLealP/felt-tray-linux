//! What she wears (WP-32): props held on a bone for minutes to hours - hats and ear headbands, glasses, bags...
//! One item per slot (props.json "accessories"."slots": hat | hair | face | neck | bag). Two sources: the outfit's
//! own pieces (outfits.json "accessories", on while it's on, they win their slot) and now and then an ambient one
//! in a free slot, picked by mood / weather / hour from each item's tags, worn 20-90 min.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rand::RngExt;
use rand::seq::IndexedRandom;
use serde_json::json;

use super::super::clock;
use super::super::ctx::Ctx;
use super::director;

const EVERY: Duration = Duration::from_secs(180);
const CHANCE: f64 = 0.12;
const WORN_MIN_S: f64 = 20.0 * 60.0;
const WORN_MAX_S: f64 = 90.0 * 60.0;

struct Worn {
    item: String,
    id: String,
    from_outfit: bool,
    until: Option<Instant>,
}

#[derive(Default)]
pub struct Accessories {
    worn: RefCell<HashMap<String, Worn>>,
    last_check: Cell<Option<Instant>>,
}

/// Which item tags fit right now.
fn context_tags(
    hour: i8,
    mood: &BTreeMap<String, f64>,
    weather: Option<&str>,
) -> BTreeSet<&'static str> {
    let g = |k: &str| mood.get(k).copied().unwrap_or(0.5);
    let mut t = BTreeSet::from(["casual"]);
    if g("happiness") >= 0.6 || g("affection") >= 0.6 {
        t.insert("cute");
    }
    if matches!(weather, Some("sun" | "clear")) && (9..19).contains(&hour) {
        t.insert("summer");
    }
    if !(2..18).contains(&hour) {
        t.insert("evening");
    }
    t
}

fn slot_of(ctx: &Ctx, item: &str) -> Option<String> {
    let slots = ctx.props.manifest.0["accessories"]["slots"].as_object()?;
    slots
        .iter()
        .find(|(_, items)| {
            items
                .as_array()
                .is_some_and(|a| a.iter().any(|i| i == item))
        })
        .map(|(s, _)| s.clone())
}

fn bone_of(ctx: &Ctx, item: &str) -> Option<String> {
    ctx.props.manifest.item(item)?["grip"]
        .as_object()?
        .keys()
        .next()
        .cloned()
}

/// Puts `item` on (`seconds`: how long, ambient only). An outfit's piece keeps its slot against ambient ones.
async fn wear(ctx: &Rc<Ctx>, item: &str, from_outfit: bool, seconds: Option<f64>) -> bool {
    let (Some(slot), Some(bone)) = (slot_of(ctx, item), bone_of(ctx, item)) else {
        return false;
    };
    if !ctx.props.enabled() {
        return false;
    }
    {
        let mut worn = ctx.props.accessories.worn.borrow_mut();
        if let Some(cur) = worn.get_mut(&slot) {
            if cur.item == item {
                cur.from_outfit |= from_outfit;
                if cur.from_outfit {
                    cur.until = None;
                }
                return true;
            }
            if cur.from_outfit && !from_outfit {
                return false;
            }
        }
    }
    take_off(ctx, &slot).await;
    let id = {
        let _legs = ctx.behavior.hold_base().await;
        director::conjure_held(ctx, item, &bone).await
    };
    let until = seconds.map(|s| Instant::now() + Duration::from_secs_f64(s));
    ctx.props.accessories.worn.borrow_mut().insert(
        slot.clone(),
        Worn {
            item: item.into(),
            id,
            from_outfit,
            until,
        },
    );
    tracing::info!(
        "accessories: wearing {item} ({slot}, {})",
        if from_outfit { "outfit" } else { "ambient" }
    );
    ctx.events.report(
        "accessory",
        json!({ "item": item, "source": if from_outfit { "outfit" } else { "ambient" } }),
    );
    true
}

async fn take_off(ctx: &Ctx, slot: &str) {
    let Some(cur) = ctx.props.accessories.worn.borrow_mut().remove(slot) else {
        return;
    };
    let _legs = ctx.behavior.hold_base().await;
    director::dismiss(ctx, &cur.id).await;
}

/// After an outfit change: its accessories on, the previous outfit's off.
pub async fn set_outfit(ctx: &Rc<Ctx>, items: &[String]) {
    let wanted: HashMap<String, String> = items
        .iter()
        .filter_map(|i| Some((slot_of(ctx, i)?, i.clone())))
        .collect();
    let stale: Vec<String> = ctx
        .props
        .accessories
        .worn
        .borrow()
        .iter()
        .filter(|(slot, w)| w.from_outfit && wanted.get(*slot) != Some(&w.item))
        .map(|(slot, _)| slot.clone())
        .collect();
    for slot in stale {
        take_off(ctx, &slot).await;
    }
    for item in wanted.values() {
        wear(ctx, item, true, None).await;
    }
}

/// The overlay restarted: whatever she wore went with it.
pub fn reset(ctx: &Ctx) {
    ctx.props.accessories.worn.borrow_mut().clear();
}

/// From the ambient loop: ambient pieces worn long enough come off; now and then one more goes on.
pub async fn tick(ctx: &Rc<Ctx>) {
    let expired: Vec<String> = ctx
        .props
        .accessories
        .worn
        .borrow()
        .iter()
        .filter(|(_, w)| w.until.is_some_and(|u| Instant::now() >= u))
        .map(|(s, _)| s.clone())
        .collect();
    for slot in expired {
        tracing::info!("accessories: off ({slot}, worn long enough)");
        take_off(ctx, &slot).await;
    }
    let a = &ctx.props.accessories;
    if a.last_check.get().is_some_and(|t| t.elapsed() < EVERY) {
        return;
    }
    a.last_check.set(Some(Instant::now()));
    if !rand::rng().random_bool(CHANCE) {
        return;
    }
    let mood = ctx.mood.borrow().as_dict();
    let weather = ctx.weather.borrow().clone();
    let fits = context_tags(clock::local().hour(), &mood, weather.as_deref());
    let options: Vec<String> = {
        let spec = &ctx.props.manifest.0["accessories"];
        let worn = a.worn.borrow();
        spec["slots"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(slot, _)| !worn.contains_key(*slot))
            .flat_map(|(_, items)| {
                items
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| i.as_str())
            })
            .filter(|item| {
                spec["tags"][*item].as_array().is_some_and(|t| {
                    t.iter()
                        .any(|x| x.as_str().is_some_and(|x| fits.contains(x)))
                })
            })
            .map(str::to_owned)
            .collect()
    };
    if let Some(item) = options.choose(&mut rand::rng()).cloned() {
        let secs = rand::rng().random_range(WORN_MIN_S..WORN_MAX_S);
        wear(ctx, &item, false, Some(secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_follow_mood_weather_and_hour() {
        let happy: BTreeMap<String, f64> = [("happiness".into(), 0.8)].into();
        let t = context_tags(13, &happy, Some("sun"));
        assert!(t.contains("cute") && t.contains("summer") && !t.contains("evening"));
        let t = context_tags(21, &BTreeMap::new(), Some("rain"));
        assert_eq!(t, BTreeSet::from(["casual", "evening"]));
    }
}
