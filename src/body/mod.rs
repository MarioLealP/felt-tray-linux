//! Her body on the desktop: felt-tray drives the Godot overlay that draws her (one program since 2026-10-16 -
//! the logic is the `felt-*` crates under `crates/`). This file only wires the parts together.

#[expect(
    dead_code,
    reason = "the waits, layers and geometry are for the chat hooks and loops (port steps 3-4)"
)]
pub mod overlay;

use std::sync::Arc;

use crate::config;
use overlay::{Event, Overlay};

/// Starts the overlay and keeps it connected; on every (re)connect she starts from idle.
pub fn start(cfg: &config::Body) -> Arc<Overlay> {
    let o = Arc::new(Overlay::new());
    tokio::spawn(overlay::supervisor::run(
        o.clone(),
        cfg.command.clone(),
        cfg.port,
    ));
    let (oc, mut rx) = (o.clone(), o.subscribe());
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(Event::Connected) => {
                    oc.play_tag("idle").await;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    });
    o
}
