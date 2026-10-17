//! Her props (WP-21) - Shimeji-style: she brings things out now and then and they stay as clutter. This file
//! holds her prop state and the glue: event episodes (game start -> a controller, music -> headphones, evening ->
//! a lamp, late night -> a blanket...) with per-kind cooldowns, playing a clip that needs a prop, the extra
//! ambient actions (tidy, kick, time-of-day props), and main.gd's `prop_moved` reports.

pub mod clutter;
pub mod director;
pub mod manifest;
pub mod store;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value, json};

use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use manifest::Manifest;
use store::{Store, now_s};

pub struct Props {
    pub manifest: Manifest,
    pub store: RefCell<Store>,
    /// The chair she's sitting on, and the sit clips picked for this sit.
    pub seated_on: RefCell<Option<String>>,
    pub sit_clips: RefCell<HashMap<String, String>>,
    /// Event key -> when it last ran (unix s).
    last_event: RefCell<HashMap<String, f64>>,
    /// The episode in flight (one at a time).
    task: RefCell<Option<tokio::task::JoinHandle<()>>>,
}

impl Props {
    pub fn load(data_dir: &Path, state_dir: &Path) -> Self {
        Self {
            manifest: Manifest::load(&data_dir.join("props.json")),
            store: RefCell::new(Store::load(state_dir.join("placed_props.json"))),
            seated_on: RefCell::new(None),
            sit_clips: RefCell::new(HashMap::new()),
            last_event: RefCell::new(HashMap::new()),
            task: RefCell::new(None),
        }
    }

    pub fn enabled(&self) -> bool {
        self.manifest.enabled()
    }
}

fn cooldown_s(key: &str) -> f64 {
    match key.split(':').next().unwrap_or_default() {
        "weather" => 3600.0,
        "time" => 5400.0,
        "game_start" => 600.0,
        "music_start" => 900.0,
        _ => 600.0,
    }
}

fn due(ctx: &Ctx, key: &str) -> bool {
    ctx.props
        .last_event
        .borrow()
        .get(key)
        .is_none_or(|t| now_s() - t >= cooldown_s(key))
}

/// An event episode in the background (one at a time, per-kind cooldown). True if it started.
pub fn start_event(ctx: &Rc<Ctx>, key: &str) -> bool {
    let p = &ctx.props;
    let Some(spec) = p.manifest.event(key) else {
        return false;
    };
    if !super::presence::settled_visible(ctx)
        || !due(ctx, key)
        || p.task.borrow().as_ref().is_some_and(|t| !t.is_finished())
    {
        return false;
    }
    p.last_event.borrow_mut().insert(key.to_owned(), now_s());
    tracing::info!("props: event {key} -> {}", spec["item"]);
    let c = ctx.clone();
    p.task.replace(Some(tokio::task::spawn_local(async move {
        run_spec(&c, &spec).await
    })));
    true
}

async fn run_spec(ctx: &Rc<Ctx>, spec: &Value) {
    if director::use_prop(ctx, spec, None, None).await {
        ctx.events
            .report("prop_used", json!({ "item": spec["item"] }));
    }
    if spec["also"].is_object() && director::use_prop(ctx, &spec["also"], None, None).await {
        ctx.events
            .report("prop_used", json!({ "item": spec["also"]["item"] }));
    }
}

/// Plays `tag` (picking its clip here): a clip the catalogue pairs with a prop (drink_water -> the bottle) runs
/// as a prop episode instead.
pub async fn play_with_props(ctx: &Rc<Ctx>, tag: &str, animations: &Path) {
    use rand::seq::IndexedRandom;
    let clips = felt_library::clips_in(animations, tag);
    let which = clips.choose(&mut rand::rng()).cloned();
    if let Some(spec) = ctx.props.manifest.clip_prop(tag, which.as_deref())
        && ctx.props.enabled()
        && director::use_prop(ctx, &spec, Some(tag), which.as_deref()).await
    {
        ctx.events
            .report("prop_used", json!({ "item": spec["item"] }));
        return;
    }
    let clips_map: HashMap<String, String> =
        which.iter().map(|c| (tag.to_owned(), c.clone())).collect();
    let pinned = (!clips_map.is_empty()).then_some(&clips_map);
    ctx.behavior
        .run_sequence(&[tag], CLIP_TIMEOUT_S, pinned)
        .await;
}

/// The props' extra ambient actions: tidying (urgent over the cap), kicking when annoyed, the evening lamp and
/// the late-night blanket.
pub fn ambient_weights(
    ctx: &Ctx,
    hour: i8,
    mood: &std::collections::BTreeMap<String, f64>,
) -> Vec<(&'static str, f64)> {
    let mut w = Vec::new();
    if !ctx.props.enabled() {
        return w;
    }
    if director::over_capacity(ctx) {
        w.push(("tidy", 40.0));
    } else if !director::tidy_list(ctx).is_empty() {
        w.push(("tidy", 8.0));
    }
    if mood.get("annoyance").copied().unwrap_or(0.0) >= 0.45 && !director::kickable(ctx).is_empty()
    {
        w.push(("kick", 10.0));
    }
    let placed: Vec<String> = ctx
        .props
        .store
        .borrow()
        .placed
        .values()
        .map(|p| p.item.clone())
        .collect();
    if (18..23).contains(&hour) && !placed.iter().any(|i| i == "desk_lamp") {
        w.push(("prop_event:time:evening", 8.0));
    }
    if !(6..23).contains(&hour) && !placed.iter().any(|i| i == "blanket") {
        w.push(("prop_event:time:late_night", 8.0));
    }
    w
}

/// A kicked or dropped prop settled somewhere: keep the record in step.
pub fn on_message(ctx: &Ctx, m: &Value) {
    let moved = &m["prop_moved"];
    let (Some(id), Some(x), Some(z)) = (
        moved["id"].as_str(),
        moved["pos"][0].as_f64(),
        moved["pos"][2].as_f64(),
    ) else {
        return;
    };
    if ctx.props.store.borrow().placed.contains_key(id) {
        ctx.props.store.borrow_mut().moved(id, x, Some(z));
    }
}

/// `{"prop_floors": [[x0, x1, y], ...]}`: the floors (desktop px) main.gd gives the props.
pub async fn sync_floors(ctx: &Ctx) {
    let floors: Vec<Value> = ctx
        .world
        .borrow()
        .platforms
        .iter()
        .filter(|p| p.kind == "floor")
        .map(|p| json!([p.x0, p.x1, p.y]))
        .collect();
    if !floors.is_empty() {
        ctx.overlay.send(&json!({ "prop_floors": floors })).await;
    }
}
