//! `BehaviorState` - the "never interrupt, always finish the current thing
//! first" rule, ported from felt_animation/state.py. See the crate docs for
//! the full story (module docstring there).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::exec::{self, AsyncEvent, AsyncLock, JoinHandle, LockGuard};
use crate::overlay::{LAYER_UPPER, OverlayLike, layer_for};

// A thinking/listening iteration shorter than this means no clip actually
// played (see the loop in begin_thinking).
const MIN_THINKING_ITERATION_S: f64 = 0.2;
const IDLE_THINKING_RETRY_S: f64 = 1.0;

/// Injectable monotonic clock - the Python code reads
/// `asyncio.get_running_loop().time()` for its pacing checks; tests can pass
/// their own to keep those checks deterministic.
pub trait Clock {
    /// Monotonic seconds from an arbitrary origin (like `loop.time()`).
    fn now_s(&self) -> f64;
}

/// The real monotonic clock.
pub struct MonotonicClock {
    origin: Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now_s(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// The class main.gd reported for `tag`, via the overlay's optional
/// `tag_layer` (overlay.py's `BehaviorState.layer_of` - the getattr-based,
/// deliberately-optional lookup: an overlay without it behaves like an old
/// overlay reporting nothing).
#[must_use]
pub fn layer_of(overlay: &impl OverlayLike, tag: &str) -> String {
    overlay
        .tag_layer(tag)
        .unwrap_or_else(|| layer_for(tag, &HashMap::new()))
}

fn is_upper(overlay: &impl OverlayLike, tag: &str) -> bool {
    layer_of(overlay, tag) == LAYER_UPPER
}

/// The locks `tags` need, held until dropped: the upper lock if every tag is
/// upper-body, otherwise base then upper (see the module docstring).
/// The guards are stored only so dropping this releases them (RAII) - they
/// are never read after construction.
#[allow(dead_code)]
struct HeldLocks {
    base: Option<LockGuard>,
    upper: Option<LockGuard>,
}

async fn hold_for(
    overlay: &impl OverlayLike,
    base_lock: &AsyncLock,
    upper_lock: &AsyncLock,
    tags: &[&str],
) -> HeldLocks {
    if !tags.is_empty() && tags.iter().all(|tag| is_upper(overlay, tag)) {
        HeldLocks {
            base: None,
            upper: Some(upper_lock.lock().await),
        }
    } else {
        let base = base_lock.lock().await;
        let upper = upper_lock.lock().await;
        HeldLocks {
            base: Some(base),
            upper: Some(upper),
        }
    }
}

/// Keeps walk_body's arrived waiter from outliving a cancelled walk -
/// Python's `finally: if not arrived.done(): arrived.cancel()` (also on
/// cancellation, e.g. a superseded presence walk).
struct ArrivedGuard(JoinHandle<()>);

impl ArrivedGuard {
    async fn join(&self) {
        let _ = self.0.join().await;
    }
}

impl Drop for ArrivedGuard {
    fn drop(&mut self) {
        if !self.0.done() {
            self.0.cancel();
        }
    }
}

/// Clears `_thinking_task` in `end_thinking_and_wait`'s `finally` - also when
/// end_thinking itself is cancelled mid-await.
struct ClearThinkingOnDrop<'a>(&'a RefCell<Option<JoinHandle<()>>>);

impl Drop for ClearThinkingOnDrop<'_> {
    fn drop(&mut self) {
        *self.0.borrow_mut() = None;
    }
}

pub struct BehaviorState<O: OverlayLike> {
    overlay: Rc<O>,
    clock: Rc<dyn Clock>,
    /// "standing" (or "sitting" - set by ambient behavior).
    pub posture: String,
    base_lock: AsyncLock,
    upper_lock: AsyncLock,
    thinking_task: RefCell<Option<JoinHandle<()>>>,
    stop_thinking: AsyncEvent,
}

impl<O: OverlayLike + 'static> BehaviorState<O> {
    #[must_use]
    pub fn new(overlay: Rc<O>, clock: Rc<dyn Clock>) -> Self {
        Self {
            overlay,
            clock,
            posture: "standing".to_string(),
            base_lock: AsyncLock::new(),
            upper_lock: AsyncLock::new(),
            thinking_task: RefCell::new(None),
            stop_thinking: AsyncEvent::new(),
        }
    }

    /// Anything at all is playing (either layer) - what the ambient,
    /// idle-refresh and micro-expression loops wait out, as before.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.base_lock.locked() || self.upper_lock.locked()
    }

    #[must_use]
    pub fn base_busy(&self) -> bool {
        self.base_lock.locked()
    }

    #[must_use]
    pub fn upper_busy(&self) -> bool {
        self.upper_lock.locked()
    }

    #[must_use]
    pub fn layer_of(&self, tag: &str) -> String {
        layer_of(&*self.overlay, tag)
    }

    #[must_use]
    pub fn is_upper(&self, tag: &str) -> bool {
        self.layer_of(tag) == LAYER_UPPER
    }

    /// WP-21: a multi-step prop sequence (walk to the chest, crouch, pick
    /// up, walk back) holds the base lock throughout and takes the upper
    /// lock only around its full-body clips (hold_upper) - so a gesture can
    /// still ride its walks. Always base before upper, like hold_for.
    /// Guard releases the lock when dropped.
    pub async fn hold_base(&self) -> LockGuard {
        self.base_lock.lock().await
    }

    /// Guard releases the lock when dropped.
    pub async fn hold_upper(&self) -> LockGuard {
        self.upper_lock.lock().await
    }

    /// Fire-and-forget: starts a background task that (once it's her
    /// turn - see the lock-wait below) keeps replaying `tag` until
    /// end_thinking_and_wait() is called. Never plays a second thinking
    /// clip on top of a running one.
    ///
    /// Special-cases tag == "idle" (the honest fallback when no dedicated
    /// "thinking" clip exists yet): main.gd's own loop_mode assignment
    /// makes "idle" LOOP_LINEAR specifically, and _on_animation_finished's
    /// own comment is explicit that a looping clip never reaches
    /// "finished" - so a wait-loop against it would never get a real
    /// completion notice and would sit on play_and_wait's full safety
    /// timeout every cycle, stalling every reply by up to that timeout for
    /// no reason. There's nothing to actually wait on here - idle already
    /// plays (or keeps playing) on its own, so this is a no-op and
    /// end_thinking_and_wait() returns immediately.
    pub fn begin_thinking(&self, tag: &str, clip_timeout: f64) {
        if tag == "idle" {
            return;
        }
        let still_running = self
            .thinking_task
            .borrow()
            .as_ref()
            .is_some_and(|task| !task.done());
        if still_running {
            return;
        }

        // Cleared HERE, synchronously, before the task is even created - not
        // inside the loop. A real bug (found live, 2026-09-21, "she was
        // repeating the think animation" after the reply had already come
        // in): creating a task only schedules the loop, it doesn't run it
        // immediately. If end_thinking_and_wait() (a separate task) called
        // .set() during that gap - entirely possible, especially once the
        // reply comes back fast - the loop would then start, immediately
        // clear() the very stop signal that was just set, and loop forever
        // with nothing left to ever set it again.
        self.stop_thinking.clear();

        let overlay = Rc::clone(&self.overlay);
        let stop = self.stop_thinking.clone();
        let base_lock = self.base_lock.clone();
        let upper_lock = self.upper_lock.clone();
        let clock = Rc::clone(&self.clock);
        let tag = tag.to_string();
        let handle = exec::spawn(async move {
            // WP-20: gesture/thinking and reaction/listening are upper-body -
            // she can think while walking or sitting.
            let tag_str: &str = &tag;
            let _held = hold_for(&*overlay, &base_lock, &upper_lock, &[tag_str]).await;
            while !stop.is_set() {
                let started = clock.now_s();
                overlay.play_and_wait(tag_str, clip_timeout, None).await;
                if clock.now_s() - started < MIN_THINKING_ITERATION_S {
                    // play_and_wait came straight back (overlay not
                    // connected, or the tag has no clips) - pace the loop
                    // instead of spinning, while still stopping the moment
                    // end_thinking_and_wait() asks.
                    let _ =
                        exec::wait_for(stop.wait(), Duration::from_secs_f64(IDLE_THINKING_RETRY_S))
                            .await;
                }
            }
        });
        *self.thinking_task.borrow_mut() = Some(handle);
    }

    /// Signals the thinking loop to stop after its current iteration,
    /// and waits for that to actually happen - this is the "reply arrived
    /// but let thinking finish its current cycle first" half of the rule.
    pub async fn end_thinking_and_wait(&self) {
        if self.thinking_task.borrow().is_none() {
            return;
        }
        let _clear = ClearThinkingOnDrop(&self.thinking_task);
        self.stop_thinking.set();
        // Clone the handle so the task stays visible to begin_thinking
        // (which must not start a second loop) while we await it.
        let task = self.thinking_task.borrow().as_ref().cloned();
        if let Some(task) = task {
            task.join().await;
        }
        // `_clear` restores _thinking_task = None here (Python's `finally`).
    }

    /// WP-18: start -> loop `cycle_tag` until overlay reports the real
    /// {"arrived": true} notice (main.gd owns the actual turn/step
    /// physics) -> stop. Holds the BASE lock for the whole thing - an
    /// ambient wander/presence walk is a single atomic sequence for anything
    /// else that needs her legs. WP-20: the upper lock stays free, so a
    /// gesture can play while she walks.
    ///
    /// The cycle tag is sent ONCE with loop=True: main.gd loops that one
    /// clip for the whole walk and swaps to idle on arrival. It used to be
    /// a one-shot re-sent on every "finished" - a restart blend, a network
    /// round trip and the clip's frozen tail pose at every ~1 s cycle
    /// boundary (Mario, 2026-09-26: "she walks but isn't smooth at all").
    /// The arrived waiter is armed before the move goes out, so an instant
    /// reply (a move to where she already stands) can't be missed.
    #[allow(clippy::too_many_arguments, reason = "mirrors the Python signature")]
    pub async fn walk_to(
        &self,
        target_x: f64,
        cycle_tag: Option<&str>,
        running: bool,
        start_tag: Option<&str>,
        stop_tag: Option<&str>,
        offscreen: bool,
        clip_timeout: f64,
    ) {
        let _held = self.base_lock.lock().await;
        self.walk_body(
            target_x,
            cycle_tag,
            running,
            start_tag,
            stop_tag,
            offscreen,
            clip_timeout,
        )
        .await;
    }

    /// walk_to without taking the lock - for a caller already holding
    /// the base lock (WP-21 prop sequences, see hold_base).
    #[allow(clippy::too_many_arguments, reason = "mirrors the Python signature")]
    pub async fn walk_body(
        &self,
        target_x: f64,
        cycle_tag: Option<&str>,
        running: bool,
        start_tag: Option<&str>,
        stop_tag: Option<&str>,
        offscreen: bool,
        clip_timeout: f64,
    ) {
        if let Some(start) = start_tag {
            self.overlay.play_and_wait(start, clip_timeout, None).await;
        }

        let overlay = Rc::clone(&self.overlay);
        let arrived = ArrivedGuard(exec::spawn(async move {
            overlay.wait_for_arrived(clip_timeout).await;
        }));
        exec::yield_now().await; // let the waiter register before the move is sent
        self.overlay.move_to(target_x, offscreen, running).await;
        if let Some(cycle) = cycle_tag {
            self.overlay.play_tag(cycle, true).await;
        }
        arrived.join().await;
        // `arrived` drops here: Python's finally cancels the waiter if it is
        // somehow still pending.
        drop(arrived);

        if let Some(stop) = stop_tag {
            self.overlay.play_and_wait(stop, clip_timeout, None).await;
        }
    }

    /// Runs a one-shot sequence of tags back to back, holding the
    /// lock(s) its tags need for the whole thing - so anything else
    /// (ambient wandering, a new thinking loop) waiting on them genuinely
    /// waits for this to finish, not just "some guessed number of
    /// seconds". WP-20: an all-upper-body sequence (a reply gesture, an
    /// emotion) takes only the upper lock and plays over a walk or a sit;
    /// anything with a base/full-body tag in it takes both. Doesn't need
    /// to explicitly return to idle afterwards - main.gd does that whenever
    /// a one-shot clip completes (back to its last idle posture, sitting
    /// or standing).
    pub async fn run_sequence(
        &self,
        tags: &[&str],
        clip_timeout: f64,
        clips: Option<&HashMap<String, String>>,
    ) {
        let _held = hold_for(&*self.overlay, &self.base_lock, &self.upper_lock, tags).await;
        for tag in tags {
            let clip = clips
                .and_then(|clips| clips.get(*tag))
                .filter(|clip| !clip.is_empty());
            match clip {
                Some(clip) => {
                    // WP-21: an exact clip (a sit clip that fits the chair).
                    self.overlay
                        .play_and_wait(tag, clip_timeout, Some(clip.as_str()))
                        .await;
                }
                None => {
                    self.overlay.play_and_wait(tag, clip_timeout, None).await;
                }
            }
        }
    }
}
