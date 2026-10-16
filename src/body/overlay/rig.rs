//! felt-state's view of the overlay (its `OverlayLike`): what her behaviour state machine drives.

use std::sync::Arc;
use std::time::Duration;

use felt_state::OverlayLike;

use super::Overlay;

pub struct Rig(pub Arc<Overlay>);

impl OverlayLike for Rig {
    async fn play_and_wait(&self, tag: &str, timeout: f64, clip: Option<&str>) {
        self.0
            .play_and_wait(tag, Duration::from_secs_f64(timeout), clip)
            .await;
    }

    async fn play_tag(&self, tag: &str, looped: bool) {
        self.0.play_tag(tag, looped, None).await;
    }

    async fn move_to(&self, x: f64, offscreen: bool, running: bool) {
        self.0.move_to(x, offscreen, running).await;
    }

    async fn wait_for_arrived(&self, timeout: f64) {
        self.0
            .wait_for_arrived(Duration::from_secs_f64(timeout))
            .await;
    }

    fn tag_layer(&self, tag: &str) -> Option<String> {
        Some(self.0.tag_layer(tag))
    }
}
