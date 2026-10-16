//! Her world (WP-23): the tops of your windows are ledges she can stand on. Ten times a second KWin's window
//! list (top-most first) and the monitors' work areas become platforms - each window's top edge minus whatever
//! covers it, one floor per monitor (felt-world) - on the monitors she has a view on. "Never floating": if the
//! ledge under her goes (a window closed, minimised, covered or moved away) she drops to the highest platform
//! below her, else that monitor's floor.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use felt_world::{Monitor, Platform, Win};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use super::ctx::Ctx;
use super::overlay::Event;
use super::{moves, window_events};
use crate::screen::kwin::Snapshot;

// KWin sends at most every 100 ms; matching it keeps riding a dragged window smooth
const TICK: Duration = Duration::from_millis(100);
/// Her height before the overlay reports its own numbers (her size at 100 %).
const DEFAULT_HEIGHT_PX: f64 = 540.0;

#[derive(Default)]
pub struct World {
    pub platforms: Vec<Platform>,
    /// The platform she's standing on, once known.
    pub current: Option<String>,
    /// Where a fall / hop / climb in flight will land her.
    pub pending: Option<String>,
    /// The ledge she's sitting on, and the timer that stands her up again.
    pub seated_on: Option<String>,
    pub sit_timer: Option<tokio::task::JoinHandle<()>>,
    /// Last tick's windows (top-most first) and monitors, for riding and fall causes.
    windows: Vec<Win>,
    monitors: Vec<Monitor>,
    /// Why the fall in flight started (her ledge went), reacted to on landing.
    fall_cause: Option<&'static str>,
    /// KWin window id -> the number felt-world calls it.
    ids: HashMap<String, i64>,
}

impl World {
    pub fn platform(&self, id: &str) -> Option<&Platform> {
        self.platforms.iter().find(|p| p.id == id)
    }

    fn hwnd(&mut self, kwin_id: &str) -> i64 {
        let next = self.ids.len() as i64 + 1;
        *self.ids.entry(kwin_id.to_owned()).or_insert(next)
    }
}

pub async fn run(ctx: Rc<Ctx>, windows: super::Windows) {
    let mut tick = tokio::time::interval(TICK);
    let mut events = ctx.overlay.subscribe();
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let snap = windows.borrow().clone();
                update(&ctx, &snap).await;
            }
            e = events.recv() => match e {
                Ok(Event::Message(m)) => settled(&ctx, &m),
                Ok(Event::Connected) => {
                    // a fresh overlay stands her on the floor
                    let mut w = ctx.world.borrow_mut();
                    w.current = None;
                    w.pending = None;
                    w.seated_on = None;
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
        }
    }
}

async fn update(ctx: &Rc<Ctx>, snap: &Snapshot) {
    let g = moves::geometry(ctx);
    // only the monitors she has a view on (her own overlay windows aren't in the list: they skip the taskbar)
    let monitors: Vec<Monitor> = snap
        .screens
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            g.views.is_empty()
                || g.views
                    .iter()
                    .any(|v| v.x < s.x + s.w as i32 && s.x < v.x + v.w)
        })
        .map(|(i, s)| Monitor {
            index: i as i64,
            left: f64::from(s.x),
            top: f64::from(s.y),
            right: f64::from(s.x) + f64::from(s.w),
            bottom: f64::from(s.y) + f64::from(s.h),
            work_bottom: Some(f64::from(s.work_bottom)),
        })
        .collect();
    if monitors.is_empty() {
        return;
    }
    let platforms = {
        let mut w = ctx.world.borrow_mut();
        let wins: Vec<Win> = snap
            .windows
            .iter()
            .map(|win| Win {
                hwnd: w.hwnd(&win.id),
                rect: (
                    f64::from(win.x),
                    f64::from(win.y),
                    f64::from(win.x) + f64::from(win.w),
                    f64::from(win.y) + f64::from(win.h),
                ),
                title: String::new(),
            })
            .collect();
        let platforms = felt_world::compute_platforms(&wins, &monitors, her_height_px(ctx), &[]);
        (platforms, wins)
    };
    let (platforms, wins) = platforms;
    if ride(ctx, &wins).await {
        let mut w = ctx.world.borrow_mut();
        w.platforms = platforms;
        w.windows = wins;
        w.monitors = monitors;
        return; // riding is not a fall
    }
    let previous = {
        let mut w = ctx.world.borrow_mut();
        w.windows = wins;
        w.monitors = monitors;
        if platforms != w.platforms {
            let ids: Vec<String> = platforms
                .iter()
                .map(|p| format!("{} {:.0}-{:.0} @{:.0}", p.id, p.x0, p.x1, p.y))
                .collect();
            tracing::debug!("world: {} platform(s) - {ids:?}", platforms.len());
        }
        std::mem::replace(&mut w.platforms, platforms)
    };
    keep_supported(ctx, &previous).await;
}

/// The window she stands on moved (dragged): she moves with it (`shift`). True if she rode this tick.
async fn ride(ctx: &Ctx, now: &[Win]) -> bool {
    if ctx.behavior.base_busy() {
        return false;
    }
    let g = moves::geometry(ctx);
    let her_x = g.world_to_px_x(ctx.current_x.get());
    let r = {
        let w = ctx.world.borrow();
        let current = w.current.as_deref().and_then(|id| w.platform(id));
        window_events::ride(current, &w.windows, now, her_x)
    };
    let Some((dx, top)) = r else {
        return false;
    };
    if !ctx
        .overlay
        .send(&serde_json::json!({ "shift": { "dx_px": dx, "y_px": top } }))
        .await
    {
        return false;
    }
    ctx.current_x.set(g.px_to_world_x(her_x + dx));
    tracing::debug!("world: riding a window (dx {dx:.0} px)");
    true
}

/// "Never floating": her platform vanished, or moved out from under her.
async fn keep_supported(ctx: &Ctx, previous: &[Platform]) {
    if ctx.behavior.base_busy() {
        return; // a walk / hop / climb already owns her feet
    }
    let x_px = moves::geometry(ctx).world_to_px_x(ctx.current_x.get());
    let target_y = {
        let mut w = ctx.world.borrow_mut();
        if w.platforms.is_empty() {
            return;
        }
        let Some(current) = w.current.clone() else {
            // first look: she starts on the floor under her
            let floor = w
                .platforms
                .iter()
                .find(|p| p.kind == "floor" && p.x0 <= x_px && x_px <= p.x1)
                .map(|p| p.id.clone());
            w.current = floor;
            return;
        };
        let here = w.platform(&current).cloned();
        if here.as_ref().is_some_and(|p| p.x0 <= x_px && x_px <= p.x1) {
            return; // still standing where she should be
        }
        let last_y = here
            .or_else(|| previous.iter().find(|p| p.id == current).cloned())
            .map_or(0.0, |p| p.y);
        let Some(target) = felt_world::pick_landing_platform(&w.platforms, x_px, last_y).cloned()
        else {
            return;
        };
        if (target.y - last_y).abs() < 1.0 {
            // same height (a segment renumbered): no fall, just re-anchor
            w.current = Some(target.id);
            w.pending = None;
            return;
        }
        if w.pending.as_deref() == Some(target.id.as_str()) {
            return; // already falling there
        }
        tracing::info!("world: her ledge went - falling to {}", target.id);
        if let Some(last) = previous.iter().find(|p| p.id == current) {
            w.fall_cause = Some(window_events::fall_cause(last, &w.windows, &w.monitors));
        }
        w.pending = Some(target.id);
        target.y
    };
    ctx.overlay.support(target_y).await;
}

/// main.gd's `landed` / `climbed` commit the pending platform; an unprompted `seated: false` means the ledge
/// she sat on went (main.gd already stood her up).
fn settled(ctx: &Rc<Ctx>, m: &Value) {
    let mut w = ctx.world.borrow_mut();
    if (m.get("landed").is_some() || m.get("climbed").is_some()) && w.pending.is_some() {
        w.current = w.pending.take();
    }
    if m.get("landed").is_some()
        && let Some(cause) = w.fall_cause.take()
    {
        tokio::task::spawn_local(window_events::react_to_landing(ctx.clone(), cause));
    }
    if m.get("seated").and_then(Value::as_bool) == Some(false) && w.seated_on.is_some() {
        tracing::info!("ledge: her seat went - unseated");
        w.seated_on = None;
        if let Some(t) = w.sit_timer.take() {
            t.abort();
        }
    }
}

/// main.gd's ppm x her height, from its geometry report.
pub fn her_height_px(ctx: &Ctx) -> f64 {
    let Some(g) = ctx.overlay.geometry() else {
        return DEFAULT_HEIGHT_PX;
    };
    let (ppm, h) = (g["ppm"].as_f64(), g["character_height"].as_f64());
    match (ppm, h) {
        (Some(p), Some(h)) if p > 0.0 && h > 0.0 => p * h,
        _ => DEFAULT_HEIGHT_PX,
    }
}

/// felt-navigation's view of the platforms.
pub fn nav_platforms(platforms: &[Platform]) -> Vec<felt_navigation::Platform> {
    use felt_navigation::PlatformKind;
    platforms
        .iter()
        .map(|p| felt_navigation::Platform {
            id: p.id.clone(),
            hwnd: p.hwnd,
            x0: p.x0,
            x1: p.x1,
            y: p.y,
            kind: match p.kind.as_str() {
                "floor" => PlatformKind::Floor,
                "seam" => PlatformKind::Seam,
                _ => PlatformKind::Window,
            },
        })
        .collect()
}
