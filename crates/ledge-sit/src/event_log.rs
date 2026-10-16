//! `event_log.report_event` reduced to its guard clause. The actual
//! fire-and-forget POST is network I/O behind [`EventSink`](crate::shared::EventSink)
//! (no implementation in this crate); the `data: dict | None` payload is
//! dropped here because ledge_sit only ever reports the empty dict.

use crate::runtime::AppState;

/// `event_log.report_event` - fire-and-forget; a no-op when reporting is
/// disabled or `felt_core_url` isn't configured (the same "empty =
/// skipped" convention as behavior.react_event).
pub fn report_event(state: &AppState, kind: &str) {
    if !state.config.felt_event_reporting_enabled || state.config.felt_core_url.is_empty() {
        return;
    }
    state.event_sink.post_event(kind);
}
