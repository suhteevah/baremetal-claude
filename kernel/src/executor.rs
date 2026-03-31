//! Cooperative async executor with interrupt-driven wakers.
//!
//! Tasks are spawned and driven by polling. When no tasks are ready,
//! the executor calls HLT to save power until the next interrupt.
//!
//! Hardware interrupts (timer, keyboard, NIC rx) wake specific futures
//! by calling `wake_task(id)` from the ISR, which pushes the task ID
//! onto a lock-free ready queue without touching the executor state.

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::task::Wake;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use spin::Mutex;

// ── Public types ────────────────────────────────────────────────────

/// Unique identifier for an async task. Exported so interrupt handlers
/// can reference specific tasks by ID (e.g., "the keyboard consumer task").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

// ── Ready queue (separate from executor for ISR safety) ─────────────

/// The ready queue lives in its own static so that interrupt handlers
/// can push task IDs without locking the entire executor. ISR context
/// only ever calls `wake_task()` which locks just this vec.
static READY_QUEUE: Mutex<Vec<TaskId>> = Mutex::new(Vec::new());

/// Push a task ID onto the ready queue. Safe to call from ISRs,
/// wakers, or any other context. If the task ID is already queued,
/// this is a no-op (dedup avoids redundant polls).
pub fn wake_task(id: TaskId) {
    let mut queue = READY_QUEUE.lock();
    if !queue.contains(&id) {
        queue.push(id);
    }
}

// ── Task ────────────────────────────────────────────────────────────

/// A pinned, heap-allocated, type-erased future. Each spawned async
/// function becomes one of these.
struct Task {
    future: Pin<Box<dyn Future<Output = ()> + Send>>,
}

// ── TaskWaker ───────────────────────────────────────────────────────

/// Waker implementation that re-enqueues a task into the ready queue.
/// Created via `Arc<TaskWaker>` and converted to a `Waker` through the
/// `alloc::task::Wake` trait. The waker only touches `READY_QUEUE`,
/// never the executor's task map, so it is safe to invoke from ISRs
/// or from within a task's poll function.
struct TaskWaker {
    task_id: TaskId,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        wake_task(self.task_id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        wake_task(self.task_id);
    }
}

// ── Executor state (not touched by ISRs) ────────────────────────────

/// The executor's mutable state. Only accessed from the `run()` loop
/// and from `spawn()`. ISRs never touch this — they only push to
/// `READY_QUEUE`.
struct Executor {
    /// All live tasks, keyed by ID. A task is removed when it returns
    /// `Poll::Ready(())`.
    tasks: BTreeMap<TaskId, Task>,

    /// Cached wakers, one per task. Avoids re-allocating an Arc on
    /// every poll. Removed when the corresponding task completes.
    waker_cache: BTreeMap<TaskId, Waker>,

    /// Monotonically increasing counter for assigning task IDs.
    next_id: u64,
}

/// Global executor state. Protected by a spin mutex. `spawn()` locks
/// this to insert new tasks; the `run()` loop locks it to extract
/// tasks for polling. ISRs never lock this.
static EXECUTOR: Mutex<Option<Executor>> = Mutex::new(None);

// ── Public API ──────────────────────────────────────────────────────

/// Run the executor with an initial future. **Does not return.**
///
/// This is the kernel's main scheduling loop. It:
/// 1. Seeds the executor with `main` as task 0
/// 2. Drains the ready queue each iteration
/// 3. Polls each ready task exactly once
/// 4. Removes completed tasks
/// 5. Halts the CPU (via `hlt`) when no work is pending, waking on
///    the next hardware interrupt (timer, keyboard, NIC, etc.)
pub fn run(main: impl Future<Output = ()> + Send + 'static) -> ! {
    // Build initial executor state with the main task pre-queued.
    let mut executor = Executor {
        tasks: BTreeMap::new(),
        waker_cache: BTreeMap::new(),
        next_id: 0,
    };

    let main_id = TaskId(executor.next_id);
    executor.next_id += 1;
    executor.tasks.insert(
        main_id,
        Task {
            future: Box::pin(main),
        },
    );

    // Install executor state into the global, then seed the ready queue.
    *EXECUTOR.lock() = Some(executor);
    READY_QUEUE.lock().push(main_id);

    log::trace!("[exec] executor started, main task id={:?}", main_id);

    // ── Main scheduling loop ────────────────────────────────────────
    loop {
        // 1. Drain the ready queue into a local vec. This is a brief
        //    lock that ISRs also contend for, so keep it short.
        let ready: Vec<TaskId> = core::mem::take(&mut *READY_QUEUE.lock());

        // 2. Poll each ready task.
        for task_id in ready {
            // Extract the task and its cached waker from the executor.
            // We remove the task from the map so the executor lock is
            // NOT held during poll(). This prevents deadlocks when
            // poll() calls spawn() (which also locks EXECUTOR).
            let (task_opt, waker) = {
                let mut guard = EXECUTOR.lock();
                let exec = guard.as_mut().unwrap();

                let task = exec.tasks.remove(&task_id);
                if task.is_none() {
                    // Task was removed between being queued and now
                    // (e.g., it completed in a previous iteration but
                    // was re-woken by a stale waker). Skip it.
                    continue;
                }

                // Get or create the waker for this task.
                let waker = exec
                    .waker_cache
                    .entry(task_id)
                    .or_insert_with(|| {
                        Waker::from(Arc::new(TaskWaker { task_id }))
                    })
                    .clone();

                (task, waker)
            };
            // EXECUTOR lock is released here.

            // Poll the future with the task's waker.
            let mut task = task_opt.unwrap();
            let mut cx = Context::from_waker(&waker);

            match task.future.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    // Task completed. Clean up its waker cache entry.
                    log::trace!("[exec] task {:?} completed", task_id);
                    if let Some(ref mut exec) = *EXECUTOR.lock() {
                        exec.waker_cache.remove(&task_id);
                    }
                    // Task is already removed from `tasks` (we took it
                    // out above) so nothing more to do. The Arc<TaskWaker>
                    // will be dropped when the Waker is dropped.
                }
                Poll::Pending => {
                    // Task is not done yet. Put it back in the map.
                    // It will be re-polled when its waker fires.
                    if let Some(ref mut exec) = *EXECUTOR.lock() {
                        exec.tasks.insert(task_id, task);
                    }
                }
            }
        }

        // 3. Check whether new work arrived while we were polling.
        //    We must disable interrupts between checking the queue and
        //    calling `hlt` to avoid this race condition:
        //
        //      check queue -> empty
        //                          <--- interrupt fires, pushes to queue
        //      hlt                 <--- CPU sleeps, misses the new work
        //
        //    With interrupts disabled, the check-then-hlt is atomic
        //    from the perspective of hardware interrupts. `enable_and_hlt`
        //    atomically enables interrupts and halts in one instruction.
        x86_64::instructions::interrupts::disable();
        if READY_QUEUE.lock().is_empty() {
            // Nothing to do — sleep until the next interrupt.
            x86_64::instructions::interrupts::enable_and_hlt();
        } else {
            // More work arrived while we were polling. Re-enable
            // interrupts and loop immediately.
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// Spawn a new async task from any context — including from within
/// a running task's poll function, from an ISR callback, or from
/// initialization code after the executor is started.
///
/// Returns the `TaskId` of the spawned task so callers can reference
/// it (e.g., to set up waker relationships with interrupt handlers).
///
/// # Panics
/// Panics if called before `run()` has initialized the executor.
pub fn spawn(future: impl Future<Output = ()> + Send + 'static) -> TaskId {
    let id = {
        let mut guard = EXECUTOR.lock();
        let exec = guard
            .as_mut()
            .expect("spawn() called before executor::run()");

        let id = TaskId(exec.next_id);
        exec.next_id += 1;
        exec.tasks.insert(
            id,
            Task {
                future: Box::pin(future),
            },
        );
        log::trace!("[exec] spawned task {:?}", id);
        id
    };
    // EXECUTOR lock released before touching READY_QUEUE to avoid
    // nested lock ordering issues.
    READY_QUEUE.lock().push(id);
    id
}

// ── Yielding ────────────────────────────────────────────────────────

/// A future that yields once to the executor, then completes.
/// Useful for cooperative multitasking within a long-running task.
///
/// On first poll: wakes itself (so it re-enters the ready queue),
/// then returns `Pending`. On second poll: returns `Ready(())`.
pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Yield control back to the executor, allowing other tasks to run.
/// The current task is immediately re-queued and will resume after
/// other ready tasks have had a chance to poll.
pub async fn yield_now() {
    YieldNow(false).await;
}
