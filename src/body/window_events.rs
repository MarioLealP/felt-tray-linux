//! Window events (WP-23 slice 4): riding a window you drag while she stands on it, and her reaction when the
//! ledge under her goes - annoyed when it closes or moves away (sometimes a recorded "rude!", at most every 45 s,
//! never the same line twice running), surprised when it's maximised. felt-window-events decides; its types
//! differ from felt-world's only in integer widths.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_window_events::{WINDOW_ANNOYED_LINES, WINDOW_EVENT_SPEECH_COOLDOWN_S};
use felt_world::{Monitor, Platform, Win};
use serde_json::json;

use super::ctx::{CLIP_TIMEOUT_S, Ctx};
use super::lines;

#[derive(Default)]
pub struct Grumbles {
    last_line: RefCell<Option<String>>,
    last_at: Cell<Option<Instant>>,
}

/// (dx_px, new_top_y) when the window she stands on moved and she's still on it.
pub fn ride(
    current: Option<&Platform>,
    previous: &[Win],
    now: &[Win],
    her_x_px: f64,
) -> Option<(f64, f64)> {
    let c = current.map(platform);
    felt_window_events::detect_ride(c.as_ref(), &wins(previous), &wins(now), her_x_px)
}

/// Why her ledge went: "vanished" (closed/minimised), "maximised" or "moved_away".
pub fn fall_cause(last: &Platform, now: &[Win], monitors: &[Monitor]) -> &'static str {
    let mons: Vec<felt_window_events::Monitor> = monitors.iter().map(monitor).collect();
    felt_window_events::classify_fall_cause(&platform(last), &wins(now), &mons)
}

/// Once she has landed after her ledge went.
pub async fn react_to_landing(ctx: Rc<Ctx>, cause: &'static str) {
    ctx.events.report("window_fall", json!({ "cause": cause }));
    let want = if cause == "maximised" {
        "emotion/surprised"
    } else {
        "emotion/annoyed"
    };
    if let Some(tag) = felt_library::resolve_known_tag(Some(want), &ctx.assets.tags) {
        let tag = tag.to_owned();
        ctx.behavior
            .run_sequence(&[tag.as_str()], CLIP_TIMEOUT_S, None)
            .await;
    }
    if cause == "maximised" {
        return; // a surprise, not rudeness
    }
    let g = &ctx.grumbles;
    if g.last_at
        .get()
        .is_some_and(|t| t.elapsed() < Duration::from_secs_f64(WINDOW_EVENT_SPEECH_COOLDOWN_S))
    {
        return;
    }
    let last = g.last_line.borrow().clone();
    let pool: Vec<&str> = WINDOW_ANNOYED_LINES
        .iter()
        .copied()
        .filter(|l| Some(*l) != last.as_deref())
        .collect();
    use rand::seq::IndexedRandom;
    let Some(line) = pool.choose(&mut rand::rng()) else {
        return;
    };
    // only the takes Mario recorded exist; an unrecorded line is silently nothing
    if let Some(wav) = lines::presence_line(&ctx, line) {
        ctx.player.play(wav);
        g.last_line.replace(Some((*line).to_owned()));
        g.last_at.set(Some(Instant::now()));
    }
}

fn wins(w: &[Win]) -> Vec<felt_window_events::Win> {
    w.iter()
        .map(|w| felt_window_events::Win {
            hwnd: w.hwnd as isize,
            rect: w.rect,
            title: w.title.clone(),
        })
        .collect()
}

fn platform(p: &Platform) -> felt_window_events::Platform {
    felt_window_events::Platform {
        id: p.id.clone(),
        hwnd: p.hwnd.map(|h| h as isize),
        x0: p.x0,
        x1: p.x1,
        y: p.y,
        kind: p.kind.clone(),
    }
}

fn monitor(m: &Monitor) -> felt_window_events::Monitor {
    felt_window_events::Monitor {
        index: m.index as i32,
        left: m.left,
        top: m.top,
        right: m.right,
        bottom: m.bottom,
        work_bottom: m.work_bottom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: i64, l: f64, t: f64) -> Win {
        Win {
            hwnd,
            rect: (l, t, l + 800.0, t + 600.0),
            title: String::new(),
        }
    }

    #[test]
    fn she_rides_a_dragged_window_and_falls_when_it_goes() {
        let ledge = Platform {
            id: "win:3:0".into(),
            hwnd: Some(3),
            x0: 100.0,
            x1: 900.0,
            y: 900.0,
            kind: "window".into(),
        };
        let before = [win(3, 100.0, 900.0)];
        assert_eq!(
            ride(Some(&ledge), &before, &[win(3, 150.0, 880.0)], 500.0),
            Some((50.0, 880.0))
        );
        assert_eq!(ride(Some(&ledge), &before, &before, 500.0), None);
        let mon = Monitor {
            index: 0,
            left: 0.0,
            top: 0.0,
            right: 2560.0,
            bottom: 1600.0,
            work_bottom: Some(1600.0),
        };
        assert_eq!(
            fall_cause(&ledge, &[], std::slice::from_ref(&mon)),
            "vanished"
        );
        let maxed = Win {
            hwnd: 3,
            rect: (0.0, 0.0, 2560.0, 1600.0),
            title: String::new(),
        };
        assert_eq!(fall_cause(&ledge, &[maxed], &[mon]), "maximised");
    }
}
