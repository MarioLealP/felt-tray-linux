//! What she does on her own (WP-08/WP-15/WP-22): a fresh idle every 10-18 s so she never freezes on one pose,
//! and every 20-45 s an ambient action - idle variety, an ambient clip, something sleepy, a walk (sometimes to
//! another monitor), sitting down / standing up, a surprise. Weighted by the hour (sleepier late at night) and
//! her mood (bored -> wanders more, low energy -> sits and dozes). Nothing while she's busy.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use rand::RngExt;
use rand::seq::IndexedRandom;

use super::clock;
use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::moves::{self, RUN_TAG, SITTING_IDLE, WALK_TAG};
use super::presence;
use super::{ledge, nav, outfits, props};

/// How far from a monitor's middle an ambient walk goes (world m), and how often it crosses monitors.
const ROAM_REACH_M: f64 = 2.0;
const CROSS_MONITOR_CHANCE: f64 = 0.3;

pub async fn idle_refresh(ctx: Rc<Ctx>) {
    loop {
        sleep_between(10.0, 18.0).await;
        if ctx.behavior.busy() || !presence::visible(&ctx) || ledge::seated(&ctx) {
            continue; // a ledge sit holds its own pose
        }
        let tag = if ctx.behavior.sitting() {
            SITTING_IDLE
        } else {
            "idle"
        };
        // on a real chair: a seated idle whose hips fit its seat
        let fit = (tag == SITTING_IDLE && ctx.props.seated_on.borrow().is_some())
            .then(|| props::director::sit_clip(&ctx, SITTING_IDLE))
            .flatten();
        if ctx.assets.tags.contains(tag) {
            ctx.overlay.play_tag(tag, false, fit.as_deref()).await;
        }
    }
}

pub async fn run(ctx: Rc<Ctx>) {
    loop {
        sleep_between(20.0, 45.0).await;
        if ctx.behavior.busy() || !presence::visible(&ctx) {
            continue;
        }
        outfits::daily_tee(&ctx).await;
        if outfits::tick(&ctx).await {
            continue; // a change was this tick's action
        }
        let hour = clock::local().hour();
        let mood = {
            let mut m = ctx.mood.borrow_mut();
            m.tick();
            m.as_dict()
        };
        let (boredom, energy) = (mood["boredom"], mood["energy"]);
        if ledge::seated(&ctx) {
            // sitting on a window's edge: mostly stay, sometimes get up (the stay timer also ends it)
            if rand::rng().random_bool(0.3) {
                ledge::stand_up(&ctx).await;
            }
        } else if ctx.behavior.sitting() {
            let w = scale(sitting_weights(hour), boredom, energy);
            sitting(&ctx, pick(&w)).await;
        } else {
            let mut w = scale(standing_weights(hour), boredom, energy);
            w.extend(props::ambient_weights(&ctx, hour, &mood));
            // WP-16: a dance now and then while real music plays and she's up for it
            if ctx.tempo_locked.get()
                && ctx.assets.tags.contains("dance")
                && energy >= 0.6
                && mood["happiness"] >= 0.6
            {
                w.push(("dance", 15.0));
            }
            standing(&ctx, pick(&w), &mood).await;
        }
    }
}

/// Usually a floor wander; sometimes (WP-23) up onto a window's ledge, where she may sit a while. Up on a
/// ledge: a stroll along it, or the way back down - never a floor walk at ledge height off its end.
async fn walk(ctx: &Rc<Ctx>, mood: &BTreeMap<String, f64>) {
    if !nav::on_floor(ctx) {
        let plan = if rand::rng().random_bool(0.5) {
            nav::plan_stroll(ctx)
        } else {
            nav::plan_down(ctx)
        };
        if let Some(plan) = plan {
            nav::run_plan(ctx, &plan).await;
        }
        return;
    }
    if rand::rng().random_bool(nav::PLATFORM_ROAM_CHANCE)
        && let Some((plan, ledge)) = nav::plan_ledge_walk(ctx, mood)
    {
        tracing::info!("ambient: to the ledge {} ({} steps)", ledge.id, plan.len());
        if nav::run_plan(ctx, &plan).await {
            ledge::maybe_sit(ctx, &ledge, mood, false).await;
        }
        return;
    }
    let run = ctx.assets.tags.contains(RUN_TAG) && rand::rng().random_bool(0.3);
    let x = wander_target(ctx);
    moves::walk_to(ctx, x, if run { RUN_TAG } else { WALK_TAG }, false).await;
}

async fn standing(ctx: &Rc<Ctx>, action: &str, mood: &BTreeMap<String, f64>) {
    tracing::debug!("ambient: {action}");
    match action {
        "idle_variety" => {
            ctx.overlay.play_tag("idle", false, None).await;
        }
        "ambient_action" => play_with_props(ctx, &tags_under(ctx, "ambient")).await,
        "sleepy" => play_one_of(ctx, &tags_under(ctx, "sleepy")).await,
        "walk" if ctx.assets.tags.contains(WALK_TAG) => walk(ctx, mood).await,
        "sit_down" => {
            // on a real chair when there's one to sit on (walked to, or conjured behind her)
            if !(ctx.props.enabled() && props::director::sit_down(ctx).await) {
                moves::sit_down(ctx).await;
            }
        }
        "tidy" => {
            props::director::tidy_one(ctx).await;
        }
        "kick" => {
            props::director::kick_one(ctx).await;
        }
        e if e.starts_with("prop_event:") => {
            props::start_event(ctx, &e["prop_event:".len()..]);
        }
        "dance" => play_one_of(ctx, &["dance".to_owned()]).await,
        "surprise" => {
            let picks: Vec<String> = ctx
                .assets
                .tags
                .iter()
                .filter(|t| *t == "dance" || *t == "greeting" || t.starts_with("reaction/"))
                .cloned()
                .collect();
            play_with_props(ctx, &picks).await;
        }
        _ => {}
    }
}

async fn sitting(ctx: &Ctx, action: &str) {
    tracing::debug!("ambient (sitting): {action}");
    match action {
        "stay_sitting" if ctx.assets.tags.contains(SITTING_IDLE) => {
            ctx.overlay.play_tag(SITTING_IDLE, false, None).await;
        }
        "stand_up" => moves::stand_up_if_sitting(ctx).await,
        "sleepy" => {
            let sleepy = tags_under(ctx, "sleepy");
            if !sleepy.is_empty() {
                moves::stand_up_if_sitting(ctx).await;
                play_one_of(ctx, &sleepy).await;
            }
        }
        _ => {}
    }
}

/// One of `tags`, with its prop when its clip needs one.
async fn play_with_props(ctx: &Rc<Ctx>, tags: &[String]) {
    let Some(tag) = tags.choose(&mut rand::rng()).cloned() else {
        return;
    };
    props::play_with_props(ctx, &tag, &ctx.animations).await;
}

async fn play_one_of(ctx: &Ctx, tags: &[String]) {
    let Some(tag) = tags.choose(&mut rand::rng()).cloned() else {
        return;
    };
    ctx.behavior
        .run_sequence(&[tag.as_str()], CLIP_TIMEOUT_S, None)
        .await;
}

/// Somewhere around the middle of the monitor she's on - sometimes another monitor's.
fn wander_target(ctx: &Ctx) -> f64 {
    let g = moves::geometry(ctx);
    let here = g.view_at_world_x(ctx.current_x.get()).cloned();
    let mut rng = rand::rng();
    let Some(here) = here else {
        return rng.random_range(-ROAM_REACH_M..ROAM_REACH_M);
    };
    let others: Vec<_> = g.views.iter().filter(|v| **v != here).collect();
    let view = match others.choose(&mut rng) {
        Some(o) if rng.random_bool(CROSS_MONITOR_CHANCE) => (*o).clone(),
        _ => here,
    };
    let (lo, hi) = g.view_span(&view);
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let mid = (lo + hi) / 2.0;
    let reach = ROAM_REACH_M.min(((hi - lo) / 2.0).max(0.0));
    if reach <= 0.0 {
        return g.clamp_roam(mid);
    }
    g.clamp_roam(rng.random_range(mid - reach..mid + reach))
}

fn tags_under(ctx: &Ctx, prefix: &str) -> Vec<String> {
    let nested = format!("{prefix}/");
    ctx.assets
        .tags
        .iter()
        .filter(|t| *t == prefix || t.starts_with(&nested))
        .cloned()
        .collect()
}

type Weights = Vec<(&'static str, f64)>;

fn standing_weights(hour: i8) -> Weights {
    if !(6..23).contains(&hour) {
        // late night: sleepier
        return vec![
            ("idle_variety", 25.0),
            ("ambient_action", 25.0),
            ("sleepy", 35.0),
            ("walk", 5.0),
            ("sit_down", 10.0),
        ];
    }
    vec![
        ("idle_variety", 20.0),
        ("ambient_action", 30.0),
        ("walk", 22.0),
        ("sleepy", 8.0),
        ("sit_down", 15.0),
        ("surprise", 5.0),
    ]
}

fn sitting_weights(hour: i8) -> Weights {
    let late = !(6..23).contains(&hour);
    vec![
        ("stay_sitting", 50.0),
        ("stand_up", 35.0),
        ("sleepy", if late { 15.0 } else { 5.0 }),
    ]
}

/// WP-15: bored -> walks and ambient clips more; low energy -> sleepy, sits, stays sitting.
fn scale(mut w: Weights, boredom: f64, energy: f64) -> Weights {
    for (action, weight) in &mut w {
        let mult = match *action {
            "walk" => 1.0 + boredom,
            "ambient_action" => 1.0 + boredom * 0.6,
            "sleepy" if energy < 0.35 => 2.0,
            "sit_down" if energy < 0.35 => 1.6,
            "stay_sitting" if energy < 0.35 => 1.4,
            _ => 1.0,
        };
        *weight = (*weight * mult).round().max(1.0);
    }
    w
}

fn pick(w: &Weights) -> &'static str {
    let total: f64 = w.iter().map(|(_, x)| x).sum();
    let mut r = rand::rng().random_range(0.0..total);
    for (action, weight) in w {
        if r < *weight {
            return action;
        }
        r -= weight;
    }
    w.last().map_or("idle_variety", |(a, _)| a)
}

async fn sleep_between(lo: f64, hi: f64) {
    let s = rand::rng().random_range(lo..hi);
    tokio::time::sleep(Duration::from_secs_f64(s)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_night_is_sleepier() {
        let day: f64 = standing_weights(14)
            .iter()
            .find(|(a, _)| *a == "sleepy")
            .unwrap()
            .1;
        let night: f64 = standing_weights(2)
            .iter()
            .find(|(a, _)| *a == "sleepy")
            .unwrap()
            .1;
        assert!(night > day);
        assert!(standing_weights(23).iter().all(|(a, _)| *a != "surprise"));
    }

    #[test]
    fn boredom_and_tiredness_scale_the_weights() {
        let w = scale(standing_weights(14), 1.0, 0.2);
        let get = |a: &str| w.iter().find(|(x, _)| *x == a).unwrap().1;
        assert_eq!(get("walk"), 44.0);
        assert_eq!(get("ambient_action"), 48.0);
        assert_eq!(get("sleepy"), 16.0);
        assert_eq!(get("sit_down"), 24.0);
        assert_eq!(get("idle_variety"), 20.0);
    }

    #[test]
    fn pick_only_returns_listed_actions() {
        let w = standing_weights(14);
        for _ in 0..200 {
            let a = pick(&w);
            assert!(w.iter().any(|(x, _)| *x == a));
        }
    }
}
