//! Minimal single-threaded future plumbing for this crate's async
//! orchestrators.
//!
//! The Python module runs on `asyncio` (an event loop, tasks,
//! `asyncio.sleep`). This crate keeps that shape but stays pure logic:
//! futures are driven by [`block_on`] and [`spawn`], time comes from an
//! injectable [`Clock`](crate::shared::Clock), and a spawned timer is the
//! [`SitTask`](crate::shared::SitTask) stored on the app state. The
//! executor re-polls ready-to-be-woken work round-robin, so futures here
//! never need real waker bookkeeping.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use crate::shared::SitTask;

/// Boxed future used by the crate's dyn-safe traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A spawned task - the cancel/done surface `asyncio.Task` exposes to
/// `ledge_sit_task`.
pub struct TaskHandle {
    id: u64,
    cancelled: Cell<bool>,
    done: Cell<bool>,
}

impl SitTask for TaskHandle {
    fn is_done(&self) -> bool {
        self.done.get()
    }

    fn cancel(&self) {
        self.cancelled.set(true);
    }

    /// `task is asyncio.current_task()` - true only while the executor is
    /// polling this very task (stand_up_from_ledge must not cancel itself).
    fn is_current(&self) -> bool {
        CURRENT_TASK.with(|current| current.get()) == Some(self.id)
    }

    /// `await task` - resolves once the task is done (a cancelled task
    /// resolves too; asyncio's CancelledError has no direct counterpart).
    fn wait(&self) -> BoxFuture<'_, ()> {
        Box::pin(poll_fn(move |_cx| {
            if self.done.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }))
    }
}

struct Task {
    handle: Rc<TaskHandle>,
    future: RefCell<Option<BoxFuture<'static, ()>>>,
}

thread_local! {
    static TASKS: RefCell<VecDeque<Rc<Task>>> = const { RefCell::new(VecDeque::new()) };
    static NEXT_TASK_ID: Cell<u64> = const { Cell::new(1) };
    static CURRENT_TASK: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Spawns `future` onto the thread-local queue - this crate's
/// `asyncio.create_task`.
pub fn spawn<F>(future: F) -> Rc<TaskHandle>
where
    F: Future<Output = ()> + 'static,
{
    let id = NEXT_TASK_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    let handle = Rc::new(TaskHandle {
        id,
        cancelled: Cell::new(false),
        done: Cell::new(false),
    });
    let boxed: BoxFuture<'static, ()> = Box::pin(future);
    let task = Rc::new(Task {
        handle: Rc::clone(&handle),
        future: RefCell::new(Some(boxed)),
    });
    TASKS.with(|tasks| tasks.borrow_mut().push_back(task));
    handle
}

/// Drives `future` to completion on this thread, polling spawned tasks
/// round-robin between its polls (a deliberately tiny event loop).
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut main = Box::pin(future);
    loop {
        if let Poll::Ready(output) = main.as_mut().poll(&mut cx) {
            TASKS.with(|tasks| tasks.borrow_mut().clear());
            return output;
        }
        let queued = TASKS.with(|tasks| tasks.borrow().len());
        for _ in 0..queued {
            let task = TASKS.with(|tasks| tasks.borrow_mut().pop_front());
            let Some(task) = task else { break };
            poll_task(&task, &mut cx);
        }
    }
}

fn poll_task(task: &Rc<Task>, cx: &mut Context<'_>) {
    if task.handle.done.get() {
        return;
    }
    if task.handle.cancelled.get() {
        // cancel(): drop the future at the next poll - the task is finished.
        *task.future.borrow_mut() = None;
        task.handle.done.set(true);
        return;
    }
    CURRENT_TASK.with(|current| current.set(Some(task.handle.id)));
    let polled = {
        let mut slot = task.future.borrow_mut();
        match slot.as_mut() {
            Some(future) => future.as_mut().poll(cx),
            None => Poll::Ready(()),
        }
    };
    CURRENT_TASK.with(|current| current.set(None));
    match polled {
        Poll::Ready(()) => {
            *task.future.borrow_mut() = None;
            task.handle.done.set(true);
        }
        Poll::Pending => {
            TASKS.with(|tasks| tasks.borrow_mut().push_back(Rc::clone(task)));
        }
    }
}

/// A minimal async mutex - `state.behavior`'s base lock (`asyncio.Lock`
/// in Python). A contended lock just returns Pending; [`block_on`]'s
/// round-robin polling is the wake-up.
#[derive(Default)]
pub struct AsyncMutex {
    locked: Cell<bool>,
}

impl AsyncMutex {
    pub fn new() -> Self {
        Self::default()
    }

    /// `BehaviorState.base_busy` - is the lock currently held?
    pub fn is_locked(&self) -> bool {
        self.locked.get()
    }

    /// `async with behavior.hold_base():` - resolves to the guard.
    pub fn lock(&self) -> BoxFuture<'_, AsyncMutexGuard<'_>> {
        Box::pin(poll_fn(move |_cx| match self.try_lock() {
            Some(guard) => Poll::Ready(guard),
            None => Poll::Pending,
        }))
    }

    fn try_lock(&self) -> Option<AsyncMutexGuard<'_>> {
        if self.locked.replace(true) {
            None
        } else {
            Some(AsyncMutexGuard { mutex: self })
        }
    }
}

/// RAII guard for [`AsyncMutex::lock`]; releases on drop.
pub struct AsyncMutexGuard<'a> {
    mutex: &'a AsyncMutex,
}

impl Drop for AsyncMutexGuard<'_> {
    fn drop(&mut self) {
        self.mutex.locked.set(false);
    }
}
