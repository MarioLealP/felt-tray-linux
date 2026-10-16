//! `routers.debug`'s GET /world - the cached desktop world model's
//! debug snapshot. WP-23 slice 5 added `seated_on_ledge` (the platform
//! id she's sitting on, or None). Debug-only; nothing here sends
//! anything to the overlay.

use crate::shared::{Monitor, Platform};

/// The world fields the /world snapshot reads off the app state.
pub struct WorldState {
    pub platforms: Vec<Platform>,
    pub monitors: Vec<Monitor>,
    pub updated_at: f64,
    pub current_platform_id: Option<String>,
    pub seated_on_ledge: Option<String>,
}

/// The GET /world response body.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldReport {
    pub platforms: Vec<Platform>,
    pub monitors: Vec<Monitor>,
    pub updated_at: f64,
    pub current_platform_id: Option<String>,
    pub seated_on_ledge: Option<String>,
}

/// `debug.world_endpoint`.
pub fn world_endpoint(state: &WorldState) -> WorldReport {
    WorldReport {
        platforms: state.platforms.clone(),
        monitors: state.monitors.clone(),
        updated_at: state.updated_at,
        current_platform_id: state.current_platform_id.clone(),
        seated_on_ledge: state.seated_on_ledge.clone(),
    }
}
