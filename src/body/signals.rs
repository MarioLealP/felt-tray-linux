//! What she notices about your desktop (WP-17), every 5 s: games starting and stopping, going AFK and coming
//! back, the GPU running hot. Each is a reaction (react.rs) and a diary event. Linux sources: games from KWin's
//! window list (Steam/Proton games are `steam_app_<id>` windows titled with the game; the `games` list in
//! games.toml adds others by app id), idle time from the session's ScreenSaver, GPU heat from the kernel's
//! amdgpu sensor (the edge temperature, like ADL on Windows).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use felt_afk::{AFK_THRESHOLD_S, AfkTracker};
use felt_games::detect::GameTracker;
use felt_gpu::GpuHeatTracker;
use serde_json::json;
use tokio::sync::watch;

use super::clock;
use super::ctx::Ctx;
use super::presence;
use super::react::signal_event;
use crate::screen::kwin::{self, Snapshot};

const POLL: Duration = Duration::from_secs(5);

pub async fn run(ctx: Rc<Ctx>, windows: watch::Receiver<Snapshot>, data_dir: std::path::PathBuf) {
    let conn = zbus::Connection::session().await.ok();
    let manual = felt_games::config::load_games_config(&data_dir.join("games.toml")).games;
    let now = clock::monotonic_s as fn() -> f64;
    let mut games = GameTracker::new(now);
    let mut afk = AfkTracker::new(AFK_THRESHOLD_S, now);
    let mut gpu = GpuHeatTracker::with_defaults(now);
    // games already open when she starts are known, not news (no "you started X" after every restart)
    tokio::time::sleep(POLL).await;
    let (already, _) = games.update(&running_games(&windows.borrow(), &manual));
    if !already.is_empty() {
        tracing::info!("signals: already running: {}", already.join(", "));
    }
    loop {
        tokio::time::sleep(POLL).await;
        if presence::visible(&ctx) {
            let running = running_games(&windows.borrow(), &manual);
            let (started, stopped) = games.update(&running);
            for name in started {
                tracing::info!("signals: game_start({name})");
                event(&ctx, "game_start", json!({ "name": name })).await;
            }
            for (name, secs) in stopped {
                tracing::info!("signals: game_stop({name}, {secs:.0}s)");
                let details = json!({ "name": name, "duration_s": secs.round() });
                event(&ctx, "game_stop", details).await;
            }
        }
        if let Some(conn) = &conn
            && let Some(idle) = kwin::idle_seconds(conn).await
            && let Some((kind, secs)) = afk.update(idle as f64)
        {
            tracing::info!("signals: {kind}({secs:.0}s)");
            let details = if kind == "afk_end" {
                json!({ "duration_s": secs.round() })
            } else {
                json!({})
            };
            event(&ctx, kind, details).await;
        }
        let temp = gpu_temp_c();
        if gpu.update(temp)
            && let Some(t) = temp
        {
            tracing::info!("signals: gpu_hot({t:.1}C)");
            let t = (t * 10.0).round() / 10.0;
            signal_event(&ctx, "gpu_hot", json!({ "temp_c": t })).await;
        }
    }
}

async fn event(ctx: &Rc<Ctx>, kind: &str, details: serde_json::Value) {
    ctx.events.report(kind, details.clone());
    signal_event(ctx, kind, details).await;
}

/// {app id: the game's name} for every game window open now.
fn running_games(snap: &Snapshot, manual: &HashSet<String>) -> HashMap<String, String> {
    snap.windows
        .iter()
        .filter(|w| is_game(&w.app, manual))
        .map(|w| {
            let name = felt_games::title::clean_title(Some(&w.title), Some(&w.app));
            (w.app.to_lowercase(), name)
        })
        .collect()
}

fn is_game(app: &str, manual: &HashSet<String>) -> bool {
    let app = app.to_lowercase();
    if app.is_empty() {
        return false;
    }
    let steam = app
        .strip_prefix("steam_app_")
        .is_some_and(|id| id != "0" && id.bytes().all(|b| b.is_ascii_digit()));
    steam || manual.contains(&app) || manual.contains(&format!("{app}.exe"))
}

/// The AMD card's edge temperature (`/sys/class/hwmon/*` named amdgpu, the `edge` input), if there is one.
fn gpu_temp_c() -> Option<f64> {
    let hwmon = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for dir in hwmon.flatten().map(|e| e.path()) {
        if read(&dir.join("name")).as_deref() != Some("amdgpu") {
            continue;
        }
        for i in 1..=4 {
            if read(&dir.join(format!("temp{i}_label"))).as_deref() == Some("edge") {
                let milli: f64 = read(&dir.join(format!("temp{i}_input")))?.parse().ok()?;
                return Some(milli / 1000.0);
            }
        }
    }
    None
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::policy::WindowInfo;

    fn win(app: &str, title: &str) -> WindowInfo {
        serde_json::from_value(json!({"id": app, "app": app, "title": title})).unwrap()
    }

    #[test]
    fn steam_and_listed_windows_are_games() {
        let manual: HashSet<String> = ["factorio.exe".to_owned()].into();
        let snap = Snapshot {
            active: String::new(),
            screens: Vec::new(),
            windows: vec![
                win("steam_app_4732690", "Endfield"),
                win("factorio", "Factorio 2.0.55"),
                win("steam", "Steam"),
                win("steam_app_0", "Steam Big Picture"),
                win("kitty", "~"),
            ],
        };
        let g = running_games(&snap, &manual);
        assert_eq!(g.len(), 2);
        assert_eq!(g["steam_app_4732690"], "Endfield");
        assert_eq!(g["factorio"], "Factorio");
    }

    #[test]
    fn this_desktop_reads_a_gpu_temperature_or_none() {
        if let Some(t) = gpu_temp_c() {
            assert!((0.0..130.0).contains(&t));
        }
    }
}
