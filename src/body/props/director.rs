//! The prop sequences (WP-21), run under her body's locks: the legs (base) for a whole episode, walks included,
//! and the upper body only around each full-body clip - always legs before upper body. Small things come out of
//! her chest (walk over, rummage, come out holding it); big things - or anything when she's far from the chest -
//! she conjures with a dissolve, never a plain pop. What she puts down stays as clutter; she tidies the old stuff
//! back into the chest (or throws it in), kicks things when annoyed, and clean house dissolves the lot.

use std::rc::Rc;
use std::time::Duration;

use rand::RngExt;
use serde_json::{Value, json};

use super::super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::super::moves::{self, SITTING_IDLE, WALK_TAG};
use super::clutter::{self, Spot};
use super::store::now_s;

const CHEST_ID: &str = "chest";

async fn sleep(s: f64) {
    tokio::time::sleep(Duration::from_secs_f64(s.max(0.0))).await;
}

/// `{"prop": {id, action, ...}}`
pub async fn prop(ctx: &Ctx, action: &str, id: &str, fields: Value) {
    let mut m = json!({ "id": id, "action": action });
    if let (Some(m), Some(f)) = (m.as_object_mut(), fields.as_object()) {
        for (k, v) in f {
            if !v.is_null() {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    ctx.overlay.send(&json!({ "prop": m })).await;
}

fn hand_seq(name: &str, bone: &str) -> String {
    if bone == "LeftHand" {
        format!("{name}_left")
    } else {
        name.to_owned()
    }
}

/// The chest's home (world x): its `desktop_x` through the overlay's pixel map when it has one, else `x`;
/// kept just inside the floor (on one monitor it ends up at that floor's end).
pub fn chest_x(ctx: &Ctx) -> f64 {
    let chest = ctx.props.manifest.chest();
    let g = moves::geometry(ctx);
    let mut x = chest["x"].as_f64().unwrap_or(0.0);
    if let Some(px) = chest["desktop_x"].as_f64()
        && g.has_pixel_map()
    {
        x = g.px_to_world_x(px);
    }
    let (lo, hi) = g.span();
    let inset = chest["edge_inset_m"].as_f64().unwrap_or(0.3);
    x.clamp(lo.min(hi) + inset, lo.max(hi) - inset)
}

/// A walk inside an episode (the legs are already held). Past her roam range it goes out as `offscreen`, or
/// main.gd would clamp it short of the chest.
async fn walk(ctx: &Ctx, x: f64) {
    let g = moves::geometry(ctx);
    let (lo, hi) = g.span();
    let x = x.clamp(lo.min(hi), lo.max(hi));
    if (x - ctx.current_x.get()).abs() < 0.05 {
        return;
    }
    stand_up_held(ctx).await;
    let (rlo, rhi) = g.roam();
    let looped = ctx.assets.tags.contains(WALK_TAG).then_some(WALK_TAG);
    let offscreen = !(rlo.min(rhi)..=rlo.max(rhi)).contains(&x);
    ctx.behavior
        .walk_body(x, looped, false, None, None, offscreen, CLIP_TIMEOUT_S)
        .await;
    ctx.current_x.set(x);
}

type Cue = (
    f64,
    Box<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()>>>>,
);

/// One clip with timed cues (the hand closing on the item, the throw's release) - the cues run even if the
/// clip is missing, so a prop never gets stuck half-done. `start`/`end`: only that segment (clip time).
async fn clip(
    ctx: &Rc<Ctx>,
    tag: &str,
    which: Option<&str>,
    cues: Vec<Cue>,
    start: f64,
    end: Option<f64>,
) {
    if !ctx.assets.tags.contains(tag) {
        tracing::warn!("props: no '{tag}' clip - running its cues without it");
        for (_, f) in cues {
            f().await;
        }
        return;
    }
    let mut tasks = Vec::new();
    for (at, f) in cues {
        tasks.push(tokio::task::spawn_local(async move {
            sleep(at - start).await;
            f().await;
        }));
    }
    if let Some(end) = end {
        let c = ctx.clone();
        tasks.push(tokio::task::spawn_local(async move {
            sleep(end - start).await;
            let idle = if c.behavior.sitting() {
                SITTING_IDLE
            } else {
                "idle"
            };
            c.overlay.play_tag(idle, false, None).await; // the segment ends; her idle posture takes over
        }));
    }
    ctx.overlay
        .play_segment(tag, Duration::from_secs_f64(CLIP_TIMEOUT_S), which, start)
        .await;
    for t in tasks {
        let _ = t.await;
    }
}

type Job<'a> = std::pin::Pin<Box<dyn Future<Output = ()> + 'a>>;

/// Runs a manifest sequence (the legs are held): `{tag, clip?, cue: {at, do}}`, `{chest}`, `{do}`, `{wait}`.
/// `job` is the one named cue this run provides (the hand closing on the item...): it runs at its moment inside
/// its clip, or at the end if the sequence never reached it - so a prop never gets stuck half-done.
async fn steps<'a>(ctx: &Rc<Ctx>, name: &str, cue_name: &str, job: Job<'a>) {
    let mut job = Some(job);
    for step in ctx.props.manifest.sequence(name).to_vec() {
        if let Some(tag) = step["tag"].as_str() {
            let cue = &step["cue"];
            let at = cue["at"].as_f64().unwrap_or(0.0);
            let doing = cue["do"].as_str().unwrap_or_default().to_owned();
            let mine = if doing == cue_name { job.take() } else { None };
            let _upper = ctx.behavior.hold_upper().await;
            let cue_fut = async {
                if let Some(j) = mine {
                    sleep(at).await;
                    j.await;
                } else if doing == "chest_open" || doing == "chest_close" {
                    sleep(at).await;
                    let what = if doing == "chest_open" {
                        "open"
                    } else {
                        "close"
                    };
                    ctx.overlay.send(&json!({ "chest": what })).await;
                }
            };
            tokio::join!(
                clip(ctx, tag, step["clip"].as_str(), Vec::new(), 0.0, None),
                cue_fut
            );
        } else if let Some(what) = step["chest"].as_str() {
            ctx.overlay.send(&json!({ "chest": what })).await;
        } else if step["do"].as_str() == Some(cue_name) {
            if let Some(j) = job.take() {
                j.await;
            }
        } else if let Some(w) = step["wait"].as_f64() {
            sleep(w).await;
        }
    }
    if let Some(j) = job {
        j.await;
    }
}

// -- getting an item into her hand --------------------------------------------------------------------------

/// Already holding one -> that; one on the floor -> pick it up; near the chest -> fetch it; far away (or big)
/// -> conjure it into her hand. The legs are held.
async fn obtain(ctx: &Rc<Ctx>, item: &str, bone: &str) -> String {
    let p = &ctx.props;
    if let Some(id) = p.store.borrow().held_id(item) {
        return id;
    }
    let here = ctx.current_x.get();
    let mut lying = p.store.borrow().placed_of(item);
    lying.sort_by(|a, b| (a.x - here).abs().total_cmp(&(b.x - here).abs()));
    if let Some(near) = lying.first()
        && !p.manifest.is_static(item)
    {
        pick_up(ctx, &near.id, bone).await;
        return near.id.clone();
    }
    if p.manifest.is_big(item)
        || (here - chest_x(ctx)).abs() > p.manifest.f("conjure_distance_m", 3.0)
    {
        return conjure_held(ctx, item, bone).await;
    }
    fetch(ctx, item, bone).await
}

/// To the chest, open it, rummage, out comes the item (it fades in inside the open chest, then up into her hand).
async fn fetch(ctx: &Rc<Ctx>, item: &str, bone: &str) -> String {
    let p = &ctx.props;
    let seq = hand_seq("fetch", bone);
    walk(ctx, chest_x(ctx) - p.manifest.hand_dx(&seq)).await;
    let id = p.store.borrow_mut().new_id(item);
    let chest = p.manifest.chest().clone();
    let take: Job = Box::pin(async {
        let inside = json!([
            chest_x(ctx),
            chest["inside_y"].as_f64().unwrap_or(0.03),
            chest["z"].as_f64().unwrap_or(0.0)
        ]);
        prop(
            ctx,
            "spawn",
            &id,
            json!({ "item": item, "pos": inside, "fx": "dissolve", "fx_s": 0.3, "puff": false }),
        )
        .await;
        sleep(0.3).await;
        prop(ctx, "attach", &id, json!({ "bone": bone })).await;
        ctx.props.store.borrow_mut().hold(&id, item, bone);
    });
    steps(ctx, &seq, "take", take).await;
    tracing::info!("props: fetched {item} ({id}) from the chest");
    id
}

/// A placed item faded in right where her hand lands in the pickup clip (so the pickup needs no walk).
async fn spawn_at_feet(ctx: &Ctx, item: &str, hand: &str) -> String {
    let p = &ctx.props;
    let id = p.store.borrow_mut().new_id(item);
    let x = ctx.current_x.get() + p.manifest.hand_dx(&hand_seq("pickup", hand));
    let z = p.manifest.z_for(item);
    prop(
        ctx,
        "spawn",
        &id,
        json!({ "item": item, "pos": [x, 0.0, z], "fx": "dissolve" }),
    )
    .await;
    p.store.borrow_mut().add_placed(&id, item, x, z);
    sleep(0.8).await;
    id
}

async fn pick_up(ctx: &Rc<Ctx>, id: &str, bone: &str) {
    let Some(placed) = ctx.props.store.borrow().placed.get(id).cloned() else {
        return;
    };
    let seq = hand_seq("pickup", bone);
    walk(ctx, placed.x - ctx.props.manifest.hand_dx(&seq)).await;
    let grab: Job = Box::pin(async {
        prop(ctx, "attach", id, json!({ "bone": bone })).await;
        ctx.props.store.borrow_mut().hold(id, &placed.item, bone);
    });
    steps(ctx, &seq, "grab", grab).await;
}

async fn conjure_held(ctx: &Rc<Ctx>, item: &str, bone: &str) -> String {
    let id = ctx.props.store.borrow_mut().new_id(item);
    let conjure: Job = Box::pin(async {
        prop(
            ctx,
            "spawn",
            &id,
            json!({ "item": item, "bone": bone, "fx": "dissolve" }),
        )
        .await;
        ctx.props.store.borrow_mut().hold(&id, item, bone);
    });
    steps(ctx, "conjure", "conjure", conjure).await;
    tracing::info!("props: conjured {item} ({id}) into her {bone}");
    id
}

/// Big items: look around, reach, and it dissolves in on the floor.
async fn conjure_placed(ctx: &Rc<Ctx>, item: &str, x: Option<f64>, z: Option<f64>) -> String {
    let x = x.unwrap_or_else(|| spot_for(ctx, item, None));
    let z = z.unwrap_or_else(|| ctx.props.manifest.z_for(item));
    let id = ctx.props.store.borrow_mut().new_id(item);
    let conjure: Job = Box::pin(async {
        prop(
            ctx,
            "spawn",
            &id,
            json!({ "item": item, "pos": [x, 0.0, z], "fx": "dissolve" }),
        )
        .await;
        ctx.props.store.borrow_mut().add_placed(&id, item, x, z);
    });
    steps(ctx, "conjure", "conjure", conjure).await;
    tracing::info!("props: conjured {item} ({id}) at x={x:.2}");
    id
}

// -- letting go ---------------------------------------------------------------------------------------------

fn spot_for(ctx: &Ctx, item: &str, near: Option<f64>) -> f64 {
    let p = &ctx.props;
    let g = moves::geometry(ctx);
    // the top-most window on her floor stands in for the focused one
    let focus = ctx
        .world
        .borrow()
        .top_window_px()
        .filter(|_| g.has_pixel_map())
        .map(|(l, r)| {
            let (fx, hw) = g.window_rect_to_world(l, r);
            (fx, (hw * 0.35).max(0.3))
        });
    let z = p.manifest.z_for(item);
    let mut occupied: Vec<f64> = p
        .store
        .borrow()
        .placed
        .values()
        .filter(|q| (q.z - z).abs() < 0.3)
        .map(|q| q.x)
        .collect();
    if p.manifest.chest().is_object() {
        occupied.push(chest_x(ctx));
    }
    if p.manifest.is_big(item) && near.is_none() {
        occupied.push(ctx.current_x.get()); // beside her, not hidden behind her
    }
    let (lo, hi) = g.roam();
    let spacing = p
        .manifest
        .item(item)
        .and_then(|e| e["spacing"].as_f64())
        .unwrap_or(0.45);
    clutter::pick_spot(&Spot {
        near_x: near.unwrap_or(ctx.current_x.get()),
        roam: (lo.min(hi), lo.max(hi)),
        occupied: &occupied,
        focus,
        spacing,
    })
}

/// Crouch and set it on the floor: it stays there as clutter.
async fn put_down(ctx: &Rc<Ctx>, id: &str) {
    let p = &ctx.props;
    let Some((item, mut bone)) = p.store.borrow().held.get(id).cloned() else {
        return;
    };
    if bone != "RightHand" && bone != "LeftHand" {
        // off her head (headphones) into her hand first
        bone = "RightHand".into();
        prop(ctx, "attach", id, json!({ "bone": bone })).await;
        p.store.borrow_mut().hold(id, &item, &bone);
    }
    let seq = hand_seq("put_down", &bone);
    let x = spot_for(ctx, &item, None);
    walk(ctx, x - p.manifest.hand_dx(&seq)).await;
    let z = p.manifest.z_for(&item);
    let place: Job = Box::pin(async {
        prop(ctx, "place", id, json!({ "pos": [x, 0.0, z] })).await;
        ctx.props.store.borrow_mut().add_placed(id, &item, x, z);
    });
    steps(ctx, &seq, "place", place).await;
}

async fn dismiss(ctx: &Ctx, id: &str) {
    prop(ctx, "despawn", id, json!({ "fx": "dissolve" })).await;
    ctx.props.store.borrow_mut().remove(id);
    let mut seat = ctx.props.seated_on.borrow_mut();
    if seat.as_deref() == Some(id) {
        *seat = None;
    }
}

// -- using props --------------------------------------------------------------------------------------------

/// An episode: get the item (or place it), play the use clip, then put it down / dissolve it / keep it. Holds
/// her legs throughout. False if it couldn't run (props off, unknown item).
pub async fn use_prop(ctx: &Rc<Ctx>, spec: &Value, tag: Option<&str>, which: Option<&str>) -> bool {
    let p = &ctx.props;
    let item = spec["item"].as_str().unwrap_or_default().to_owned();
    if !p.manifest.enabled() || p.manifest.item(&item).is_none() {
        return false;
    }
    let tag = spec["tag"].as_str().or(tag).map(str::to_owned);
    let which = spec["clip"].as_str().or(which).map(str::to_owned);
    let _legs = ctx.behavior.hold_base().await;
    if spec["place"].as_bool() == Some(true) {
        if p.store.borrow().placed_of(&item).is_empty() {
            conjure_placed(ctx, &item, None, None).await;
        }
        if let Some(t) = tag.as_deref().filter(|t| ctx.assets.tags.contains(*t)) {
            let _upper = ctx.behavior.hold_upper().await;
            clip(ctx, t, which.as_deref(), Vec::new(), 0.0, None).await;
        }
        return true;
    }
    let bone = spec["bone"].as_str().unwrap_or("RightHand").to_owned();
    let hand = if bone == "LeftHand" {
        "LeftHand"
    } else {
        "RightHand"
    };
    let none_around = || {
        p.store.borrow().held_id(&item).is_none() && p.store.borrow().placed_of(&item).is_empty()
    };
    if spec["from_floor"].as_bool() == Some(true) && none_around() {
        spawn_at_feet(ctx, &item, hand).await; // picked up off the floor: one fades in by her feet first
    }
    let id = if spec["conjure"].as_bool() == Some(true) || (bone == "Head" && none_around()) {
        conjure_held(ctx, &item, &bone).await // straight onto her head / into her hand
    } else {
        let id = obtain(ctx, &item, hand).await;
        if bone != hand {
            prop(ctx, "attach", &id, json!({ "bone": bone })).await;
            p.store.borrow_mut().hold(&id, &item, &bone);
        }
        id
    };
    if let Some(t) = tag.as_deref().filter(|t| ctx.assets.tags.contains(*t)) {
        // "switch": [[t, bone], ...] - hand the item over inside the clip
        let mut cues: Vec<Cue> = Vec::new();
        for sw in spec["switch"].as_array().into_iter().flatten() {
            let (Some(at), Some(to)) = (sw[0].as_f64(), sw[1].as_str()) else {
                continue;
            };
            let (c, id, item, to) = (ctx.clone(), id.clone(), item.clone(), to.to_owned());
            cues.push((
                at,
                Box::new(move || {
                    let (c, id, item, to) = (c.clone(), id.clone(), item.clone(), to.clone());
                    Box::pin(async move {
                        prop(&c, "attach", &id, json!({ "bone": to })).await;
                        c.props.store.borrow_mut().hold(&id, &item, &to);
                    })
                }),
            ));
        }
        let start = spec["start"].as_f64().unwrap_or(0.0);
        let _upper = ctx.behavior.hold_upper().await;
        clip(ctx, t, which.as_deref(), cues, start, spec["end"].as_f64()).await;
    }
    sleep(spec["hold_s"].as_f64().unwrap_or(0.0)).await;
    match spec["after"].as_str().unwrap_or("put_down") {
        "dismiss" => dismiss(ctx, &id).await,
        "put_down" => put_down(ctx, &id).await,
        _ => {} // keep: stays in her hand
    }
    true
}

/// Off with it (headphones when the music stops): into her hand, then down.
pub async fn take_off(ctx: &Rc<Ctx>, item: &str) -> bool {
    let Some(id) = ctx.props.store.borrow().held_id(item) else {
        return false;
    };
    let _legs = ctx.behavior.hold_base().await;
    prop(ctx, "attach", &id, json!({ "bone": "RightHand" })).await;
    ctx.props.store.borrow_mut().hold(&id, item, "RightHand");
    put_down(ctx, &id).await;
    true
}

// -- the chair ---------------------------------------------------------------------------------------------

/// The chair-compatible clip for a sit tag (its hips match the seat); None = no chair rule for it.
pub fn sit_clip(ctx: &Ctx, tag: &str) -> Option<String> {
    use rand::seq::IndexedRandom;
    let choices: Vec<String> = ctx.props.manifest.0["sit"]["clips"][tag]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if choices.is_empty() {
        return None;
    }
    if tag == SITTING_IDLE {
        return choices.choose(&mut rand::rng()).cloned(); // vary the seated idle within the fitting ones
    }
    ctx.props
        .sit_clips
        .borrow()
        .get(tag)
        .cloned()
        .or_else(|| choices.choose(&mut rand::rng()).cloned())
}

/// Sit on a real chair: the nearest placed one (walk there), else one conjured right behind her. False without
/// a chair setup. Takes her legs itself.
pub async fn sit_down(ctx: &Rc<Ctx>) -> bool {
    let p = &ctx.props;
    let sit = p.manifest.0["sit"].clone();
    let chair_item = sit["item"].as_str().unwrap_or("chair").to_owned();
    if !p.manifest.enabled() || p.manifest.item(&chair_item).is_none() {
        return false;
    }
    let dx = sit["seat_offset"][0].as_f64().unwrap_or(0.0);
    let dz = sit["seat_offset"][1].as_f64().unwrap_or(0.25);
    let _legs = ctx.behavior.hold_base().await;
    let here = ctx.current_x.get();
    let mut chairs = p.store.borrow().placed_of(&chair_item);
    chairs.sort_by(|a, b| (a.x - here).abs().total_cmp(&(b.x - here).abs()));
    let chair = if let Some(c) = chairs.first().cloned() {
        if (c.z - dz).abs() > 0.005 {
            prop(
                ctx,
                "place",
                &c.id,
                json!({ "pos": [c.x, 0.0, dz], "instant": true }),
            )
            .await;
            p.store.borrow_mut().moved(&c.id, c.x, Some(dz));
        }
        walk(ctx, c.x - dx).await;
        c.id
    } else {
        conjure_placed(ctx, &chair_item, Some(here + dx), Some(dz)).await
    };
    p.seated_on.replace(Some(chair.clone()));
    {
        use rand::seq::IndexedRandom;
        let mut picks = p.sit_clips.borrow_mut();
        picks.clear();
        for (tag, c) in sit["clips"].as_object().into_iter().flatten() {
            let opts: Vec<&str> = c
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(pick) = opts.choose(&mut rand::rng()) {
                picks.insert(tag.clone(), (*pick).to_owned());
            }
        }
    }
    let enter = p.sit_clips.borrow().get("transition/stand_to_sit").cloned();
    {
        let _upper = ctx.behavior.hold_upper().await;
        clip(
            ctx,
            "transition/stand_to_sit",
            enter.as_deref(),
            Vec::new(),
            0.0,
            None,
        )
        .await;
    }
    ctx.behavior.set_posture("sitting");
    let idle = sit_clip(ctx, SITTING_IDLE);
    ctx.overlay
        .play_tag(SITTING_IDLE, false, idle.as_deref())
        .await;
    tracing::info!("props: sitting on {chair}");
    true
}

/// Up off the chair with its matching clip (the legs are held). Never slides her across the floor seated.
async fn stand_up_held(ctx: &Ctx) {
    if !ctx.behavior.sitting() {
        return;
    }
    let exit = stand_up_clip(ctx);
    {
        let _upper = ctx.behavior.hold_upper().await;
        ctx.overlay
            .play_and_wait(
                "transition/sit_to_stand",
                Duration::from_secs_f64(CLIP_TIMEOUT_S),
                exit.as_deref(),
            )
            .await;
    }
    ctx.behavior.set_posture("standing");
    ctx.overlay.play_tag("idle", false, None).await;
}

/// The stand-up clip that fits the chair she's on (and she's off it).
pub fn stand_up_clip(ctx: &Ctx) -> Option<String> {
    let on_chair = ctx.props.seated_on.replace(None).is_some();
    on_chair
        .then(|| sit_clip(ctx, "transition/sit_to_stand"))
        .flatten()
}

// -- clutter upkeep ----------------------------------------------------------------------------------------

pub fn tidy_list(ctx: &Ctx) -> Vec<super::store::Placed> {
    let p = &ctx.props;
    let keep: Vec<String> = p.seated_on.borrow().iter().cloned().collect();
    let placed: Vec<_> = p.store.borrow().placed.values().cloned().collect();
    let max = p.manifest.f("max_placed", 6.0) as usize;
    clutter::tidy_candidates(
        &placed,
        now_s(),
        p.manifest.f("tidy_after_hours", 3.0) * 3600.0,
        max,
        &keep,
    )
}

pub fn over_capacity(ctx: &Ctx) -> bool {
    ctx.props.store.borrow().placed.len() > ctx.props.manifest.f("max_placed", 6.0) as usize
}

/// The oldest clutter back into the chest: pick it up, then carry it over and put it in, or throw it in from a
/// couple of metres. A chair or beanbag isn't carried: it's un-conjured.
pub async fn tidy_one(ctx: &Rc<Ctx>) -> bool {
    let Some(old) = tidy_list(ctx).into_iter().next() else {
        return false;
    };
    let p = &ctx.props;
    let chest = p.manifest.chest().clone();
    let cx = chest_x(ctx);
    let _legs = ctx.behavior.hold_base().await;
    if p.manifest.is_static(&old.item) {
        dismiss(ctx, &old.id).await;
        return true;
    }
    pick_up(ctx, &old.id, "LeftHand").await; // the throw toward the chest is the left-handed clip
    let id = old.id.clone();
    if rand::rng().random_bool(0.5) {
        walk(ctx, cx + p.manifest.f("throw_distance_m", 1.6)).await;
        let release: Job = Box::pin(async {
            prop(
                ctx,
                "throw",
                &id,
                json!({ "to": [cx, 0.25, chest["z"].as_f64().unwrap_or(0.0)] }),
            )
            .await;
            ctx.props.store.borrow_mut().remove(&id);
        });
        steps(ctx, "throw", "release", release).await;
    } else {
        walk(ctx, cx - p.manifest.hand_dx("fetch_left")).await;
        let stash: Job = Box::pin(async {
            prop(
                ctx,
                "despawn",
                &id,
                json!({ "fx": "dissolve", "fx_s": 0.3 }),
            )
            .await;
            ctx.props.store.borrow_mut().remove(&id);
        });
        steps(ctx, "stash_left", "stash", stash).await;
    }
    tracing::info!("props: tidied {} into the chest", old.id);
    true
}

pub fn kickable(ctx: &Ctx) -> Vec<super::store::Placed> {
    let p = &ctx.props;
    p.store
        .borrow()
        .placed
        .values()
        .filter(|q| !p.manifest.is_static(&q.item))
        .cloned()
        .collect()
}

/// Annoyed: walk up to a dropped item and boot it, on in the direction she came from.
pub async fn kick_one(ctx: &Rc<Ctx>) -> bool {
    let here = ctx.current_x.get();
    let mut items = kickable(ctx);
    items.sort_by(|a, b| (a.x - here).abs().total_cmp(&(b.x - here).abs()));
    let Some(target) = items.first().cloned() else {
        return false;
    };
    let mut side = if here >= target.x { 1.0 } else { -1.0 };
    if (here - target.x).abs() < 0.1 {
        side = if target.x > 0.0 { 1.0 } else { -1.0 };
    }
    let _legs = ctx.behavior.hold_base().await;
    walk(ctx, target.x - ctx.props.manifest.hand_dx("kick")).await;
    let push: Job = Box::pin(async {
        let power = ctx.props.manifest.f("kick_impulse", 2.2);
        prop(
            ctx,
            "push",
            &target.id,
            json!({ "impulse": [-side * power, power * 0.45, 0.0] }),
        )
        .await;
    });
    steps(ctx, "kick", "push", push).await;
    tracing::info!("props: kicked {}", target.id);
    true
}

/// Clean house: everything but the chest dissolves out at once (no lock - it's your key, it happens now).
pub async fn clear(ctx: &Ctx) -> usize {
    let ids = ctx.props.store.borrow_mut().clear();
    if ctx
        .props
        .seated_on
        .borrow()
        .as_ref()
        .is_some_and(|s| ids.contains(s))
    {
        ctx.props.seated_on.replace(None);
    }
    for id in &ids {
        prop(ctx, "despawn", id, json!({ "fx": "dissolve" })).await;
    }
    tracing::info!("props: clean house - {} dissolved", ids.len());
    ids.len()
}

/// A fresh overlay knows nothing: the catalogue, the floors, the chest, the clutter.
pub async fn on_connected(ctx: &Ctx) {
    let p = &ctx.props;
    if !p.manifest.enabled() {
        return;
    }
    ctx.overlay.send(&p.manifest.catalogue_message()).await;
    super::sync_floors(ctx).await;
    let chest = p.manifest.chest().clone();
    if chest.is_object() {
        let item = chest["item"].as_str().unwrap_or("chest");
        let pos = json!([chest_x(ctx), 0.0, chest["z"].as_f64().unwrap_or(0.0)]);
        prop(
            ctx,
            "spawn",
            CHEST_ID,
            json!({ "item": item, "pos": pos, "yaw": chest["yaw"], "fx": "none" }),
        )
        .await;
    }
    p.store.borrow_mut().held.clear();
    let placed: Vec<_> = p.store.borrow().placed.values().cloned().collect();
    let seat = p.seated_on.borrow().clone();
    if seat.is_some_and(|s| !placed.iter().any(|q| q.id == s)) {
        p.seated_on.replace(None);
    }
    for q in placed.iter().filter(|q| p.manifest.item(&q.item).is_some()) {
        prop(
            ctx,
            "spawn",
            &q.id,
            json!({ "item": q.item, "pos": [q.x, 0.0, q.z], "fx": "none" }),
        )
        .await;
    }
    tracing::info!(
        "props: catalogue, chest and {} placed item(s) sent",
        placed.len()
    );
}
