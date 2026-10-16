//! The `asyncio` pieces `felt_animation.state` is built on (Lock, Event, Task, create_task, sleep, wait_for),
//! on tokio's single-threaded runtime: her body runs as one `LocalSet` (felt-tray's body thread), so tasks
//! interleave cooperatively like the Python event loop and may hold `Rc`s.
//!
//! [spawn] needs a running `LocalSet` (felt-tray's body thread, or [block_on] in tests). Lock, Event and
//! [wait_for] are runtime-agnostic. Clock note: [sleep]/[wait_for] use tokio's timer (real time), while
//! application-level pacing takes an injectable [Clock](crate::Clock).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

struct TaskShared<T> {
    output: Option<T>,
    /// Finished normally or cancelled.
    done: bool,
    /// Wakers of tasks awaiting [JoinHandle::join].
    joiners: Vec<Waker>,
}

/// Spawn `fut` as a new task on the current `LocalSet`; the returned [JoinHandle] awaits (or cancels) it.
/// Mirrors `asyncio.create_task`.
pub fn spawn<F, T>(fut: F) -> JoinHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let shared = Rc::new(RefCell::new(TaskShared {
        output: None,
        done: false,
        joiners: Vec::new(),
    }));
    let s = Rc::clone(&shared);
    let task = tokio::task::spawn_local(async move {
        let value = fut.await;
        let joiners = {
            let mut st = s.borrow_mut();
            st.output = Some(value);
            st.done = true;
            std::mem::take(&mut st.joiners)
        };
        joiners.into_iter().for_each(Waker::wake);
    });
    JoinHandle {
        shared,
        abort: task.abort_handle(),
    }
}

/// Handle to a spawned task. Dropping the handle does *not* cancel the task (like dropping an `asyncio.Task`
/// reference); call [JoinHandle::cancel] for that - this is what `walk_body`'s `finally` does to its arrived
/// waiter.
pub struct JoinHandle<T> {
    shared: Rc<RefCell<TaskShared<T>>>,
    abort: tokio::task::AbortHandle,
}

impl<T> Clone for JoinHandle<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Rc::clone(&self.shared),
            abort: self.abort.clone(),
        }
    }
}

impl<T> JoinHandle<T> {
    /// True once the task finished (normally or cancelled) - like `asyncio.Task.done()`.
    #[must_use]
    pub fn done(&self) -> bool {
        self.shared.borrow().done
    }

    /// Request cancellation: the task's future is dropped at its current suspension point (releasing any held
    /// locks) and `join` yields `None`.
    pub fn cancel(&self) {
        let joiners = {
            let mut st = self.shared.borrow_mut();
            if st.done {
                return;
            }
            st.done = true;
            std::mem::take(&mut st.joiners)
        };
        self.abort.abort();
        joiners.into_iter().for_each(Waker::wake);
    }

    /// Wait for the task to finish, like `await task`. Resolves to `None` if the task was cancelled.
    pub fn join(&self) -> impl Future<Output = Option<T>> + '_ {
        poll_fn(move |cx| {
            let mut st = self.shared.borrow_mut();
            if st.done {
                Poll::Ready(st.output.take())
            } else {
                st.joiners.push(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

/// Run `fut` to completion on a fresh single-threaded runtime + `LocalSet`, driving spawned tasks and timers
/// alongside it. Like `asyncio.run` (tests; felt-tray's body thread builds its own).
pub fn block_on<F: Future + 'static>(fut: F) -> F::Output
where
    F::Output: 'static,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a single-threaded tokio runtime");
    tokio::task::LocalSet::new().block_on(&rt, fut)
}

/// Yield once to the executor, like `asyncio.sleep(0)`: other ready tasks get to run before this one resumes.
pub fn yield_now() -> impl Future<Output = ()> {
    tokio::task::yield_now()
}

/// Suspend for `duration` of real time, like `asyncio.sleep`.
pub fn sleep(duration: Duration) -> impl Future<Output = ()> {
    tokio::time::sleep(duration)
}

/// Suspend until the real-time `deadline`.
pub fn sleep_until(deadline: Instant) -> impl Future<Output = ()> {
    tokio::time::sleep_until(deadline.into())
}

/// [wait_for] gave up before its future completed - the Python raises
/// `TimeoutError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout;

/// Await `fut`, giving up after `timeout` - like `asyncio.wait_for`. Resolves
/// to `Err(Timeout)` on timeout.
pub fn wait_for<F: Future>(
    fut: F,
    timeout: Duration,
) -> impl Future<Output = Result<F::Output, Timeout>> {
    let mut fut = Box::pin(fut);
    let mut timer = Box::pin(tokio::time::sleep(timeout));
    poll_fn(move |cx| {
        if let Poll::Ready(value) = fut.as_mut().poll(cx) {
            return Poll::Ready(Ok(value));
        }
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Timeout));
        }
        Poll::Pending
    })
}

// -- asyncio.Lock / asyncio.Event -------------------------------------------

struct LockState {
    locked: bool,
    waiters: VecDeque<Waker>,
}

/// Mutual-exclusion lock awaited in async code - `asyncio.Lock`. The guard
/// releases the lock when dropped (including when the holding future is
/// cancelled mid-sequence).
#[derive(Clone)]
pub struct AsyncLock {
    shared: Arc<Mutex<LockState>>,
}

impl Default for AsyncLock {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncLock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(LockState {
                locked: false,
                waiters: VecDeque::new(),
            })),
        }
    }

    /// True while someone holds the lock - `Lock.locked()`.
    #[must_use]
    pub fn locked(&self) -> bool {
        self.shared.lock().expect("lock state").locked
    }

    /// Wait for the lock, resolving to a guard that releases it on drop.
    pub fn lock(&self) -> impl Future<Output = LockGuard> + '_ {
        let shared = Arc::clone(&self.shared);
        poll_fn(move |cx| {
            let mut state = shared.lock().expect("lock state");
            if state.locked {
                state.waiters.push_back(cx.waker().clone());
                Poll::Pending
            } else {
                state.locked = true;
                Poll::Ready(LockGuard {
                    shared: Arc::clone(&shared),
                })
            }
        })
    }
}

/// Held lock; releases on drop.
pub struct LockGuard {
    shared: Arc<Mutex<LockState>>,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Wake every waiter: each re-checks the lock on its next poll, so a
        // wake stolen by a fresh acquirer can never strand the others (and
        // schedule order keeps the queue roughly FIFO).
        let waiters = {
            let mut state = self.shared.lock().expect("lock state");
            state.locked = false;
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }
}

struct EventState {
    set: bool,
    waiters: Vec<Waker>,
}

/// One-shot event flag - `asyncio.Event`.
#[derive(Clone)]
pub struct AsyncEvent {
    shared: Arc<Mutex<EventState>>,
}

impl Default for AsyncEvent {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncEvent {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(EventState {
                set: false,
                waiters: Vec::new(),
            })),
        }
    }

    /// Flag the event, waking every waiter.
    pub fn set(&self) {
        let waiters = {
            let mut state = self.shared.lock().expect("event state");
            state.set = true;
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters {
            waker.wake();
        }
    }

    /// Unset the flag again - `Event.clear()`.
    pub fn clear(&self) {
        self.shared.lock().expect("event state").set = false;
    }

    #[must_use]
    pub fn is_set(&self) -> bool {
        self.shared.lock().expect("event state").set
    }

    /// Wait until the event is set - `Event.wait()`.
    pub fn wait(&self) -> impl Future<Output = ()> + '_ {
        let shared = Arc::clone(&self.shared);
        poll_fn(move |cx| {
            let mut state = shared.lock().expect("event state");
            if state.set {
                Poll::Ready(())
            } else {
                state.waiters.push(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}
