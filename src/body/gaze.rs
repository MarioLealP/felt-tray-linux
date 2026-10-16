//! Where she looks (WP-14): felt-gaze decides - off during clips that move her head themselves, at you while
//! she talks, up-left while she thinks, at you once the cursor has rested 4 s, else at the cursor. On Wayland
//! the overlay can't see the cursor, so "cursor" goes out as a `desktop` point from our KWin cursor feed.
//! Sent only when it changes (every cursor move, while following it), and again after a reconnect.

use std::rc::Rc;
use std::time::{Duration, Instant};

use felt_gaze::{DEFAULT_OPT_OUT_TAGS, GazeMode, decide_gaze, is_opted_out};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use super::Cursor;
use super::ctx::Ctx;
use super::overlay::Event;

const TICK: Duration = Duration::from_millis(200);

pub async fn run(ctx: Rc<Ctx>, mut cursor: Cursor) {
    let mut events = ctx.overlay.subscribe();
    let mut tick = tokio::time::interval(TICK);
    let mut cursor_open = true;
    let mut moved_at = Instant::now();
    let mut sent: Option<Value> = None;
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            r = cursor.changed(), if cursor_open => match r {
                Ok(()) => moved_at = Instant::now(),
                Err(_) => cursor_open = false,
            },
            e = events.recv() => match e {
                Ok(Event::Connected) => sent = None, // a fresh overlay knows nothing
                Ok(Event::Message(_)) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
        }
        let now = Instant::now();
        let tag = ctx.overlay.current_tag();
        let (mode, x, y) = decide_gaze(
            is_opted_out(tag.as_deref(), DEFAULT_OPT_OUT_TAGS),
            ctx.speaking_until.get() > now,
            ctx.thinking.get(),
            (now - moved_at).as_secs_f64(),
        );
        let msg = message(mode, x, y, *cursor.borrow());
        if sent.as_ref() != Some(&msg) && ctx.overlay.send(&json!({ "gaze": msg })).await {
            sent = Some(msg);
        }
    }
}

/// The relay's gaze payload. Following the cursor = a `desktop` pixel point when the KWin feed knows where it
/// is (main.gd's own "cursor" mode only works where Godot can read the mouse, i.e. not on Wayland).
fn message(mode: GazeMode, x: Option<f64>, y: Option<f64>, cursor: Option<(f64, f64)>) -> Value {
    match (mode, x, y, cursor) {
        (GazeMode::Point, Some(x), Some(y), _) => json!({ "mode": "point", "x": x, "y": y }),
        (GazeMode::Cursor, _, _, Some((cx, cy))) => json!({ "mode": "desktop", "x": cx, "y": cy }),
        (m, ..) => json!({ "mode": m.as_str() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_the_cursor_is_a_desktop_point() {
        let m = message(GazeMode::Cursor, None, None, Some((6400.0, 700.0)));
        assert_eq!(m, json!({"mode": "desktop", "x": 6400.0, "y": 700.0}));
        assert_eq!(
            message(GazeMode::Cursor, None, None, None),
            json!({"mode": "cursor"})
        );
    }

    #[test]
    fn thinking_and_camera_pass_through() {
        let m = message(GazeMode::Point, Some(0.35), Some(0.3), Some((1.0, 1.0)));
        assert_eq!(m, json!({"mode": "point", "x": 0.35, "y": 0.3}));
        assert_eq!(
            message(GazeMode::Camera, None, None, Some((1.0, 1.0))),
            json!({"mode": "camera"})
        );
        assert_eq!(
            message(GazeMode::Off, None, None, None),
            json!({"mode": "off"})
        );
    }
}
