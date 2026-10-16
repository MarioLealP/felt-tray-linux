//! What she wears (WP-27): the outfit VRMs in data/outfits.json. Mario's rules (felt-outfits decides): Oct 31
//! halloween, Dec 23-26 xmas/reindeer, pyjamas 01:00-05:00 or when sleepy, else the day's daily-pool pick
//! (never yesterday's); now and then an ambient swap (85 % daily / 15 % themed, held 2 h); the bikini only as a
//! 1 % reaction to a sunny weather answer. A scheduled change is "proper" (she walks off, changes, comes back);
//! an ambient one is "quick" (a spin, a puff). Only when she's free: visible, idle, standing, not talking.
//! State in wardrobe.json (restored from Windows' felt-animation/wardrobe.json).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_outfits::{
    APPLY_TIMEOUT_S, DEFAULT_ID, FREE_WAIT_S, OVERRIDE_SECONDS, Outfit, OutfitRng, PRESENCE_WAIT_S,
    PyRandom, QUICK_SWAP_DELAY_S, SLEEPY_ENERGY,
};
use rand::seq::IndexedRandom;
use serde_json::{Map, Value, json};

use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::overlay::wait_for;
use super::{clock, presence, tees};

/// wardrobe.json: the outfit fields plus the tee fields, as one JSON object.
pub struct Wardrobe(pub Map<String, Value>);

impl Wardrobe {
    pub fn load(path: &Path) -> Self {
        let m = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok())
            .unwrap_or_default();
        Self(m)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(Value::as_str).map(str::to_owned)
    }

    pub fn set(&mut self, key: &str, v: Value) {
        self.0.insert(key.to_owned(), v);
    }

    /// Write-then-rename, so a reader never sees half a file.
    pub fn save(&self, path: &Path) {
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&self.0).unwrap_or_default();
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)) {
            tracing::warn!("outfits: can't save {}: {e}", path.display());
        }
    }
}

pub struct Outfits {
    catalogue: Vec<Outfit>,
    rng: RefCell<PyRandom>,
    changing: Cell<bool>,
    pub current: RefCell<String>,
    current_vrm: RefCell<Option<String>>,
    /// Ambient swap chance per ambient tick.
    ambient_chance: f64,
}

impl Outfits {
    pub fn load(data_dir: &Path, wardrobe: &Path, ambient_chance: f64) -> Self {
        let catalogue = felt_outfits::load_outfits(&data_dir.join("outfits.json"));
        let ws = Wardrobe::load(wardrobe);
        let current = ws
            .get_str("outfit_current")
            .filter(|c| catalogue.iter().any(|o| &o.id == c))
            .unwrap_or_else(|| DEFAULT_ID.into());
        tracing::info!(
            "outfits: {} in the catalogue, wearing {current}",
            catalogue.len()
        );
        Self {
            catalogue,
            rng: RefCell::new(PyRandom::from_entropy()),
            changing: Cell::new(false),
            current: RefCell::new(current),
            current_vrm: RefCell::new(ws.get_str("outfit_current_vrm")),
            ambient_chance,
        }
    }

    fn get(&self, id: &str) -> Option<&Outfit> {
        self.catalogue.iter().find(|o| o.id == id)
    }

    fn enabled(&self) -> bool {
        self.get(DEFAULT_ID).is_some()
    }

    fn pool(&self, pool: &str) -> Vec<String> {
        felt_outfits::pool_ids(&self.catalogue, pool)
    }
}

fn wardrobe_save(ctx: &Ctx, fields: &[(&str, Value)]) {
    let mut ws = Wardrobe::load(&ctx.wardrobe_path);
    for (k, v) in fields {
        ws.set(k, v.clone());
    }
    ws.save(&ctx.wardrobe_path);
}

fn now() -> felt_outfits::DateTime {
    let z = clock::local();
    felt_outfits::DateTime::new(
        i32::from(z.year()),
        z.month().unsigned_abs().into(),
        z.day().unsigned_abs().into(),
        z.hour().unsigned_abs().into(),
        z.minute().unsigned_abs().into(),
        z.second().unsigned_abs().into(),
    )
}

fn sleepy(ctx: &Ctx) -> bool {
    ctx.mood
        .borrow()
        .values
        .get("energy")
        .copied()
        .unwrap_or(1.0)
        < SLEEPY_ENERGY
}

/// The day's daily-pool pick, once per wardrobe day (it rolls at 05:00), never yesterday's.
fn daily_pick(ctx: &Ctx, at: &felt_outfits::DateTime) -> String {
    let o = &ctx.outfits;
    let ws = Wardrobe::load(&ctx.wardrobe_path);
    let d = felt_outfits::effective_date(at);
    let today = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
    if ws.get_str("outfit_daily_date").as_deref() == Some(today.as_str())
        && let Some(id) = ws.get_str("outfit_daily").filter(|id| o.get(id).is_some())
    {
        return id;
    }
    let prev = ws.get_str("outfit_daily");
    let chosen =
        felt_outfits::pick_daily(&o.pool("daily"), prev.as_deref(), &mut *o.rng.borrow_mut());
    tracing::info!("outfits: daily pick {chosen} (yesterday {prev:?})");
    wardrobe_save(
        ctx,
        &[
            ("outfit_daily", json!(chosen)),
            ("outfit_daily_date", json!(today)),
            ("outfit_daily_prev", json!(prev)),
        ],
    );
    chosen
}

/// What she should be wearing now: a date/night rule, else an unexpired override, else the daily pick.
fn target(ctx: &Ctx, at: &felt_outfits::DateTime) -> String {
    let o = &ctx.outfits;
    if let Some(s) = felt_outfits::scheduled_outfit(at, sleepy(ctx)).filter(|s| o.get(s).is_some())
    {
        if Wardrobe::load(&ctx.wardrobe_path)
            .0
            .get("outfit_override")
            .is_some_and(|v| !v.is_null())
        {
            wardrobe_save(ctx, &[("outfit_override", Value::Null)]); // a scheduled change ends it
        }
        return s.to_owned();
    }
    let ov = Wardrobe::load(&ctx.wardrobe_path)
        .0
        .get("outfit_override")
        .cloned();
    if let Some(ov) = ov
        && let (Some(id), Some(until)) = (ov["id"].as_str(), ov["until"].as_f64())
        && o.get(id).is_some()
        && until > at.timestamp()
    {
        return id.to_owned();
    }
    daily_pick(ctx, at)
}

/// Visible, idle, standing, not talking - else a change waits.
fn can_change(ctx: &Ctx) -> bool {
    !ctx.outfits.changing.get()
        && presence::settled_visible(ctx)
        && !ctx.behavior.busy()
        && !ctx.behavior.sitting()
        && ctx.world.borrow().seated_on.is_none()
        && super::nav::on_floor(ctx)
        && ctx.speaking_until.get() <= Instant::now()
}

/// From the ambient loop: the scheduled / daily outfit, or now and then an ambient swap. True if she changed.
pub async fn tick(ctx: &Rc<Ctx>) -> bool {
    let o = &ctx.outfits;
    if !o.enabled() || !can_change(ctx) {
        return false;
    }
    let at = now();
    let mut style = "proper";
    if felt_outfits::scheduled_outfit(&at, sleepy(ctx)).is_none()
        && o.rng.borrow_mut().random() < o.ambient_chance
    {
        let pick = felt_outfits::ambient_pick(
            &o.pool("daily"),
            &o.pool("themed"),
            &mut *o.rng.borrow_mut(),
        );
        if pick != *o.current.borrow() {
            hold(ctx, &pick, &at);
            style = "quick";
        }
    }
    let want = target(ctx, &at);
    if want == *o.current.borrow() {
        return false;
    }
    change_to(ctx, &want, style).await
}

/// Keeps `id` on for two hours (unless a date/night rule outranks it).
fn hold(ctx: &Ctx, id: &str, at: &felt_outfits::DateTime) {
    let until = at.timestamp() + OVERRIDE_SECONDS;
    wardrobe_save(
        ctx,
        &[("outfit_override", json!({ "id": id, "until": until }))],
    );
}

/// A reply's animation tag: a sunny weather answer rolls the 1 % bikini (when she's free to change).
pub fn on_reply_tag(ctx: &Rc<Ctx>, tag: Option<&str>) {
    let o = &ctx.outfits;
    if !o.enabled() || o.get("bikini").is_none() {
        return;
    }
    if !felt_outfits::bikini_roll(tag, &mut *o.rng.borrow_mut(), 0.01) {
        return;
    }
    if felt_outfits::scheduled_outfit(&now(), sleepy(ctx)).is_some() {
        return; // a date/night outfit outranks it
    }
    let c = ctx.clone();
    tokio::task::spawn_local(async move {
        let deadline = Instant::now() + Duration::from_secs_f64(FREE_WAIT_S);
        while !can_change(&c) {
            if Instant::now() > deadline {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        hold(&c, "bikini", &now());
        change_to(&c, "bikini", "quick").await;
    });
}

/// One change at a time. True once the overlay took it.
pub async fn change_to(ctx: &Rc<Ctx>, id: &str, style: &str) -> bool {
    let o = &ctx.outfits;
    if o.get(id).is_none() || o.changing.replace(true) {
        return false;
    }
    tracing::info!("outfits: changing to {id} ({style})");
    let ok = if style == "proper" {
        proper(ctx, id).await
    } else {
        quick(ctx, id).await
    };
    o.changing.set(false);
    ok
}

/// A spin clip, and the swap with a puff part-way into it.
async fn quick(ctx: &Rc<Ctx>, id: &str) -> bool {
    let spins: Vec<String> = ctx
        .assets
        .tag_list
        .iter()
        .filter(|t| t.to_lowercase().contains("spin") || t.to_lowercase().contains("twirl"))
        .cloned()
        .collect();
    let Some(spin) = spins.choose(&mut rand::rng()).cloned() else {
        let _legs = ctx.behavior.hold_base().await;
        return swap(ctx, id, "puff").await;
    };
    let c = ctx.clone();
    let clip = tokio::task::spawn_local(async move {
        c.behavior
            .run_sequence(&[spin.as_str()], CLIP_TIMEOUT_S, None)
            .await;
    });
    tokio::time::sleep(Duration::from_secs_f64(QUICK_SWAP_DELAY_S)).await;
    let ok = swap(ctx, id, "puff").await;
    let _ = clip.await;
    ok
}

/// She walks off, changes out of sight, walks back on.
async fn proper(ctx: &Rc<Ctx>, id: &str) -> bool {
    presence::hide(ctx);
    if !wait_presence(ctx, felt_presence::HIDDEN).await {
        tracing::info!("outfits: the walk-off didn't finish - change deferred");
        presence::show(ctx);
        return false;
    }
    let ok = swap(ctx, id, "none").await;
    presence::show(ctx);
    wait_presence(ctx, felt_presence::VISIBLE).await;
    ok
}

async fn wait_presence(ctx: &Ctx, want: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(PRESENCE_WAIT_S);
    while ctx.presence.borrow().state != want {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    true
}

/// `{outfit: {id, vrm, fx}}` (the original VRM or one of its approved variants), waiting for `outfit_applied`.
async fn swap(ctx: &Ctx, id: &str, fx: &str) -> bool {
    let o = &ctx.outfits;
    let Some(outfit) = o.get(id) else {
        return false;
    };
    let vrms: Vec<&String> = std::iter::once(&outfit.vrm)
        .chain(&outfit.variants)
        .collect();
    let vrm = (*vrms
        .choose(&mut rand::rng())
        .expect("at least the original"))
    .clone();
    let mut rx = ctx.overlay.subscribe();
    if !ctx
        .overlay
        .send(&json!({ "outfit": { "id": id, "vrm": vrm, "fx": fx } }))
        .await
    {
        return false;
    }
    o.current.replace(id.to_owned());
    o.current_vrm.replace(Some(vrm.clone()));
    wardrobe_save(
        ctx,
        &[
            ("outfit_current", json!(id)),
            ("outfit_current_vrm", json!(vrm)),
        ],
    );
    let done = |m: &Value| {
        ["outfit_applied", "outfit_failed"]
            .iter()
            .any(|k| m[*k]["id"].as_str() == Some(id))
    };
    match wait_for(&mut rx, Duration::from_secs_f64(APPLY_TIMEOUT_S), done).await {
        Some(m) if m.get("outfit_failed").is_some() => false,
        Some(_) => true,
        None => {
            tracing::warn!("outfits: no outfit_applied for {id} in time");
            true
        }
    }
}

/// A (re)started overlay wears felt.vrm: put her outfit back (and today's tee on the default).
pub async fn on_connected(ctx: &Ctx) {
    let o = &ctx.outfits;
    if !o.enabled() {
        return;
    }
    let id = o.current.borrow().clone();
    if id == DEFAULT_ID {
        if let Some((tee, _)) = tees::todays(ctx) {
            tees::wear(ctx, &tee, false).await;
        }
        return;
    }
    let vrm = o
        .current_vrm
        .borrow()
        .clone()
        .or_else(|| o.get(&id).map(|e| e.vrm.clone()));
    ctx.overlay
        .send(&json!({ "outfit": { "id": id, "vrm": vrm, "fx": "none" } }))
        .await;
}

/// main.gd's answers: back to the default after a failed swap; the tee back on the default outfit.
pub async fn on_message(ctx: &Ctx, m: &Value) {
    if m["outfit_applied"]["id"].as_str() == Some(DEFAULT_ID)
        && let Some((tee, _)) = tees::todays(ctx)
    {
        tees::wear(ctx, &tee, false).await;
    }
    if let Some(id) = m["outfit_failed"]["id"].as_str() {
        tracing::error!(
            "outfits: the overlay couldn't wear {id}: {}",
            m["outfit_failed"]["error"]
        );
        if id != DEFAULT_ID && ctx.outfits.enabled() {
            ctx.outfits.current.replace(DEFAULT_ID.into());
            wardrobe_save(
                ctx,
                &[
                    ("outfit_current", json!(DEFAULT_ID)),
                    ("outfit_override", Value::Null),
                ],
            );
            let vrm = ctx.outfits.get(DEFAULT_ID).map(|e| e.vrm.clone());
            ctx.overlay
                .send(&json!({ "outfit": { "id": DEFAULT_ID, "vrm": vrm, "fx": "none" } }))
                .await;
        }
    }
}

/// From the ambient loop on the default outfit: a new day's tee, worn with a little show-off.
pub async fn daily_tee(ctx: &Ctx) {
    if *ctx.outfits.current.borrow() != DEFAULT_ID {
        return;
    }
    if let Some((tee, true)) = tees::todays(ctx) {
        tees::wear(ctx, &tee, true).await;
    }
}

/// `$XDG_DATA_HOME/felt-tray/wardrobe.json`.
pub fn wardrobe_path(state_dir: &Path) -> PathBuf {
    state_dir.join("wardrobe.json")
}
