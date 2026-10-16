//! A minimal single-threaded cooperative async runtime - this crate's stand-in
//! for Python's `asyncio`, on which `felt_animation.state` is built (Lock, Event,
//! Task, create_task, sleep, wait_for).
//!
//! Everything runs cooperatively on one thread: tasks are futures polled from a
//! shared run queue, and a pending task only ever resumes because another task
//! woke it (an event was set, a lock was released, a join completed) or because
//! a timer expired. That makes the interleaving deterministic - which is what
//! the Python tests (spawn a task, `await asyncio.sleep(0)`, assert on the
//! mid-sequence state) rely on.
//!
//! Clock/timer note: [sleep]/[wait_for] use the real monotonic clock (the
//! Python tests' `asyncio.sleep`/`wait_for` timeouts are real-time too), while
//! application-level pacing takes an injectable [Clock] (see `crate::Clock`).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

type TaskFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

struct TaskSlot {
    future: Option<TaskFuture>,
    /// In the run queue right now (or about to be) - a wake while already
    /// scheduled must not enqueue the task twice.
    scheduled: bool,
    done: bool,
    cancelled: bool,
    /// Wakers of tasks awaiting `JoinHandle::join`.
    joiners: Vec<Waker>,
}

struct ExecutorState {
    tasks: HashMap<usize, TaskSlot>,
    queue: VecDeque<usize>,
    /// (deadline, waker) for every [sleep]/[wait_for] currently pending.
    timers: Vec<(Instant, Waker)>,
    next_id: usize,
}

impl ExecutorState {
    fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            queue: VecDeque::new(),
            timers: Vec::new(),
            next_id: 0,
        }
    }
}

thread_local! {
    static EXEC: RefCell<ExecutorState> = RefCell::new(ExecutorState::new());
}

fn with_exec<R>(f: impl FnOnce(&mut ExecutorState) -> R) -> R {
    EXEC.with(|e| f(&mut e.borrow_mut()))
}

/// Enqueue `id` if it isn't scheduled already. Called from wakers and spawn.
fn schedule(id: usize) {
    with_exec(|e| {
        if let Some(slot) = e.tasks.get_mut(&id)
            && !slot.scheduled
            && !slot.done
            && !slot.cancelled
        {
            slot.scheduled = true;
            e.queue.push_back(id);
        }
    });
}

/// Waker half of a task. Separate from the future slot on purpose: `Waker`
/// requires the waked type to be `Send + Sync`, while the futures themselves
/// are single-threaded. Identification is by task id.
struct TaskWaker {
    id: usize,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        schedule(self.id);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        schedule(self.id);
    }
}

fn waker_for(id: usize) -> Waker {
    Waker::from(Arc::new(TaskWaker { id }))
}

fn store_future(id: usize, future: TaskFuture) {
    with_exec(|e| {
        if let Some(slot) = e.tasks.get_mut(&id)
            && !slot.done
            && !slot.cancelled
        {
            slot.future = Some(future);
        }
    });
}

/// Poll a task once. The future is taken out of its slot while polling so the
/// poll itself may spawn tasks, wake others or join - all of which touch the
/// executor state. A finished or cancelled future is dropped *outside* the
/// executor borrow, because dropping it releases any lock guards it holds,
/// which wakes other tasks.
fn poll_task(id: usize) {
    let taken = with_exec(|e| {
        let slot = match e.tasks.get_mut(&id) {
            Some(slot) => slot,
            None => return None,
        };
        if slot.done || slot.cancelled {
            return None;
        }
        slot.scheduled = false;
        slot.future.take()
    });
    let Some(mut future) = taken else {
        return;
    };
    let ready = {
        let waker = waker_for(id);
        let mut cx = Context::from_waker(&waker);
        future.as_mut().poll(&mut cx).is_ready()
    };
    let (keep, joiners) = with_exec(|e| {
        let slot = match e.tasks.get_mut(&id) {
            Some(slot) => slot,
            None => return (false, Vec::new()),
        };
        if ready {
            slot.done = true;
            (false, std::mem::take(&mut slot.joiners))
        } else if slot.cancelled {
            (false, std::mem::take(&mut slot.joiners))
        } else {
            (true, Vec::new())
        }
    });
    if keep {
        store_future(id, future);
    } else {
        drop(future);
        for waker in joiners {
            waker.wake();
        }
    }
}

/// Spawn `fut` as a new task on the running executor; the returned
/// [JoinHandle] awaits (or cancels) it. Mirrors `asyncio.create_task`.
pub fn spawn<F, T>(fut: F) -> JoinHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let id = with_exec(|e| {
        let id = e.next_id;
        e.next_id += 1;
        e.tasks.insert(
            id,
            TaskSlot {
                future: None,
                scheduled: false,
                done: false,
                cancelled: false,
                joiners: Vec::new(),
            },
        );
        id
    });
    let output: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
    let out = Arc::clone(&output);
    let wrapped: TaskFuture = Box::pin(async move {
        let value = fut.await;
        *out.lock().unwrap() = Some(value);
    });
    store_future(id, wrapped);
    schedule(id);
    JoinHandle { id, output }
}

/// Handle to a spawned task. Dropping the handle does *not* cancel the task
/// (like dropping an `asyncio.Task` reference); call [JoinHandle::cancel] for
/// that - this is what `walk_body`'s `finally` does to its arrived waiter.
pub struct JoinHandle<T> {
    id: usize,
    output: Arc<Mutex<Option<T>>>,
}

impl<T> Clone for JoinHandle<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            output: Arc::clone(&self.output),
        }
    }
}

impl<T> JoinHandle<T> {
    /// True once the task finished (normally or cancelled) - like
    /// `asyncio.Task.done()`.
    #[must_use]
    pub fn done(&self) -> bool {
        with_exec(|e| match e.tasks.get(&self.id) {
            Some(slot) => slot.done || slot.cancelled,
            None => true,
        })
    }

    /// Request cancellation: the task's future is dropped at its current
    /// suspension point (releasing any held locks) and `join` yields `None`.
    pub fn cancel(&self) {
        let (future, joiners) = with_exec(|e| match e.tasks.get_mut(&self.id) {
            Some(slot) if !slot.done && !slot.cancelled => {
                slot.cancelled = true;
                (slot.future.take(), std::mem::take(&mut slot.joiners))
            }
            _ => (None, Vec::new()),
        });
        // Dropping the future releases its lock guards, which wakes other
        // tasks - do it outside the executor borrow.
        drop(future);
        for waker in joiners {
            waker.wake();
        }
    }

    /// Wait for the task to finish, like `await task`. Resolves to `None` if
    /// the task was cancelled.
    pub fn join(&self) -> impl Future<Output = Option<T>> + '_ {
        enum JoinPoll<T> {
            Ready(Option<T>),
            Wait,
        }
        poll_fn(move |cx| {
            let polled = with_exec(|e| match e.tasks.get_mut(&self.id) {
                Some(slot) if slot.cancelled => JoinPoll::Ready(None),
                Some(slot) if slot.done => {
                    JoinPoll::Ready(self.output.lock().expect("join output lock").take())
                }
                Some(slot) => {
                    slot.joiners.push(cx.waker().clone());
                    JoinPoll::Wait
                }
                None => JoinPoll::Ready(None),
            });
            match polled {
                JoinPoll::Ready(value) => Poll::Ready(value),
                JoinPoll::Wait => Poll::Pending,
            }
        })
    }
}

/// Run `fut` to completion on the single-threaded executor, driving spawned
/// tasks and timers alongside it. Like `asyncio.run`.
pub fn block_on<F: Future + 'static>(fut: F) -> F::Output
where
    F::Output: 'static,
{
    let output: Arc<Mutex<Option<F::Output>>> = Arc::new(Mutex::new(None));
    let out = Arc::clone(&output);
    let main = spawn(async move {
        let value = fut.await;
        *out.lock().unwrap() = Some(value);
    });
    let main_id = main.id;

    loop {
        while let Some(id) = with_exec(|e| e.queue.pop_front()) {
            poll_task(id);
        }
        if output.lock().unwrap().is_some() {
            break;
        }
        // Collect expired timers, then wake them - waking touches the
        // executor state, so it must happen outside the borrow.
        let now = Instant::now();
        let expired: Vec<Waker> = with_exec(|e| {
            let mut expired = Vec::new();
            e.timers.retain(|(deadline, waker)| {
                if *deadline <= now {
                    expired.push(waker.clone());
                    false
                } else {
                    true
                }
            });
            expired
        });
        if !expired.is_empty() {
            for waker in expired {
                waker.wake();
            }
            continue;
        }
        let nearest = with_exec(|e| e.timers.iter().map(|(deadline, _)| *deadline).min());
        match nearest {
            Some(deadline) => {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
            None => {
                let live = with_exec(|e| {
                    e.tasks
                        .values()
                        .filter(|slot| !slot.done && !slot.cancelled)
                        .count()
                });
                if live > 0 {
                    // Nothing runnable, nothing to wake: the futures left are
                    // waiting on events/joins that will never resolve.
                    panic!(
                        "exec::block_on: deadlock - {live} task(s) pending with \
                         no runnable work and no timers (main task id {main_id})"
                    );
                }
                panic!("exec::block_on: deadlock - block_on future pending with nothing to run");
            }
        }
    }

    // Drop leftover spawned tasks outside the borrow: their futures may hold
    // lock guards whose release wakes other tasks.
    let leftover: Vec<TaskFuture> = with_exec(|e| {
        let leftover: Vec<TaskFuture> = e
            .tasks
            .values_mut()
            .filter_map(|slot| slot.future.take())
            .collect();
        e.tasks.clear();
        e.queue.clear();
        e.timers.clear();
        leftover
    });
    drop(leftover);

    output
        .lock()
        .expect("block_on output lock")
        .take()
        .expect("block_on output missing")
}

/// Yield once to the executor, like `asyncio.sleep(0)`: other ready tasks get
/// to run before this one resumes.
pub fn yield_now() -> impl Future<Output = ()> {
    let mut yielded = false;
    poll_fn(move |cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
}

/// Suspend for `duration` of real time, like `asyncio.sleep`.
pub fn sleep(duration: Duration) -> impl Future<Output = ()> {
    sleep_until(Instant::now() + duration)
}

/// Suspend until the real-time `deadline`.
pub fn sleep_until(deadline: Instant) -> impl Future<Output = ()> {
    let mut registered = false;
    poll_fn(move |cx| {
        if Instant::now() >= deadline {
            return Poll::Ready(());
        }
        if !registered {
            registered = true;
            with_exec(|e| e.timers.push((deadline, cx.waker().clone())));
        }
        Poll::Pending
    })
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
    let mut timer = Box::pin(sleep_until(Instant::now() + timeout));
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
