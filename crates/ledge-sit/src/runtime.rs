//! `runtime.AppState` bookkeeping and the `{"seated": ...}` branch of
//! its `_on_world_settled` on_message callback. The landed/climbed
//! platform bookkeeping and the fall-reaction branch are WP-23 slice
//! 3/4 (runtime.py / loops.py) and are not ported here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::shared::{
    BehaviorState, Clock, EventConfig, EventSink, Mood, Overlay, RandomSource, SitTask,
};

/// A notice from the overlay that `_on_world_settled` reacts to. Other
/// message kinds are ignored, like Python's dict `.get` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldMsg {
    /// `{"seated": true|false}` from main.gd.
    Seated(bool),
    /// Anything else - ignored.
    Other,
}

/// The slice of `runtime.AppState` this module needs: the overlay, the
/// behavior lock, the mood (for the stay timer), the ledge-sit
/// bookkeeping, and the injectables.
pub struct AppState {
    pub overlay: Rc<dyn Overlay>,
    pub behavior: BehaviorState,
    pub mood: Mood,
    pub config: EventConfig,
    pub clock: Rc<dyn Clock>,
    pub rng: RefCell<Box<dyn RandomSource>>,
    pub event_sink: Rc<dyn EventSink>,
    /// `seated_on_ledge` - platform id she's sitting on, or None.
    pub seated_on_ledge: RefCell<Option<String>>,
    /// `ledge_sit_task` - the pending stay-then-stand timer.
    pub ledge_sit_task: RefCell<Option<Rc<dyn SitTask>>>,
    /// `current_x` - "where she thinks she is" bookkeeping.
    pub current_x: Cell<f64>,
}

impl AppState {
    pub fn new(
        overlay: Rc<dyn Overlay>,
        mood: Mood,
        config: EventConfig,
        clock: Rc<dyn Clock>,
        rng: Box<dyn RandomSource>,
        event_sink: Rc<dyn EventSink>,
    ) -> Self {
        Self {
            overlay,
            behavior: BehaviorState::default(),
            mood,
            config,
            clock,
            rng: rng.into(),
            event_sink,
            seated_on_ledge: RefCell::new(None),
            ledge_sit_task: RefCell::new(None),
            current_x: Cell::new(0.0),
        }
    }

    /// `runtime.AppState._on_world_settled`, WP-23 slice 5 branch only:
    /// an unprompted `{"seated": false}` (main.gd unseated her instantly
    /// on a support drop - no chair_end) syncs OUR bookkeeping: clear the
    /// ledge and cancel the stay timer. Our own deliberate
    /// stand_up_from_ledge already clears `seated_on_ledge` before its
    /// ledge_stand ack arrives, so this never fires a second time for it.
    pub fn on_world_settled(&self, msg: WorldMsg) {
        if msg == WorldMsg::Seated(false) && self.seated_on_ledge.borrow().is_some() {
            *self.seated_on_ledge.borrow_mut() = None;
            let task = self.ledge_sit_task.borrow_mut().take();
            if let Some(task) = &task
                && !task.is_done()
            {
                task.cancel();
            }
        }
    }
}
