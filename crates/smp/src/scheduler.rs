//! Multi-core task scheduler for ClaudioOS.
//!
//! Each CPU core has its own run queue. Idle cores steal tasks from busy cores.
//! Timer interrupts drive preemptive scheduling decisions.
//!
//! ## Work Stealing Algorithm
//!
//! When a core's run queue is empty, it attempts to steal half the tasks from
//! the busiest other core. This naturally balances load across cores without
//! a central dispatcher. The steal threshold is > 1 task (don't steal from a
//! core that only has one task).
//!
//! ## Context Switch Mechanism
//!
//! The `context_switch()` function saves callee-saved registers (rsp, rbp, rbx,
//! r12-r15, rflags) into the old task's `CpuContext` and restores them from
//! the new task's context. The RIP is saved as a label address within the asm
//! block -- when we switch back to a task, execution resumes right after the
//! context switch point. For new tasks, RIP is the entry function and RDI holds
//! the first argument (System V ABI).
//!
//! Context switches must occur with interrupts disabled to prevent re-entrant
//! scheduling during the register save/restore window.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::percpu::{self, MAX_CPUS};
use crate::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Task state
// ---------------------------------------------------------------------------

/// Unique task identifier.
pub type TaskId = u64;

/// Task execution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    /// Task is ready to run and sitting in a run queue.
    Ready = 0,
    /// Task is currently executing on a core.
    Running = 1,
    /// Task is blocked waiting for an event.
    Blocked = 2,
    /// Task has finished execution.
    Dead = 3,
}

impl From<u8> for TaskState {
    fn from(v: u8) -> Self {
        match v {
            0 => TaskState::Ready,
            1 => TaskState::Running,
            2 => TaskState::Blocked,
            _ => TaskState::Dead,
        }
    }
}

// ---------------------------------------------------------------------------
// Saved CPU context for context switch
// ---------------------------------------------------------------------------

/// Full x86_64 register context saved during context switch.
///
/// This matches the layout expected by the context switch assembly stub.
/// System V AMD64 ABI callee-saved registers: rbx, rbp, r12-r15, rsp, rip.
/// We save all GPRs for completeness (interrupt context).
///
/// The struct layout matters: the inline asm in `context_switch()` accesses
/// fields by fixed byte offsets (e.g., `[ctx + 0x00]` = rsp, `[ctx + 0x38]` = rip).
/// Any reordering would break the context switch.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct CpuContext {
    // Callee-saved (always preserved across context switch)
    pub rsp: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    // Instruction pointer (return address)
    pub rip: u64,
    // RFLAGS
    pub rflags: u64,
    // Caller-saved (saved for interrupt-driven switches)
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    // Segment selectors (usually constant, but saved for completeness)
    pub cs: u64,
    pub ss: u64,
}

impl CpuContext {
    /// Create a zeroed context.
    pub const fn zeroed() -> Self {
        Self {
            rsp: 0,
            rbp: 0,
            rbx: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0,
            rax: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            cs: 0,
            ss: 0,
        }
    }

    /// Create an initial context for a new task.
    ///
    /// `entry` — function pointer the task will begin executing.
    /// `stack_top` — top of the task's stack (highest address).
    /// `arg` — argument passed in rdi (System V ABI first parameter).
    pub fn new_task(entry: u64, stack_top: u64, arg: u64) -> Self {
        Self {
            rsp: stack_top,
            rbp: stack_top,
            rbx: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry,
            rflags: 0x200, // IF=1 (interrupts enabled)
            rax: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: arg,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            cs: 0x08, // kernel code segment
            ss: 0x10, // kernel data segment
        }
    }
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// Default task stack size: 64 KiB.
pub const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Next task ID counter.
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

/// A schedulable task.
pub struct Task {
    /// Unique task identifier.
    pub id: TaskId,
    /// Human-readable name (for logging).
    pub name: &'static str,
    /// Current state.
    pub state: AtomicU8,
    /// Saved CPU context.
    pub context: CpuContext,
    /// Stack allocation (owned by the task).
    pub stack: Box<[u8]>,
    /// CPU affinity: if Some, this task can only run on the specified core.
    pub affinity: Option<u32>,
    /// The core this task is currently assigned to.
    pub assigned_core: Option<u32>,
}

impl Task {
    /// Create a new task.
    ///
    /// `name` — human-readable label for logging.
    /// `entry` — function pointer: `extern "C" fn(arg: u64)`.
    /// `arg` — argument passed to the entry function.
    /// `stack_size` — size of the task's stack in bytes.
    pub fn new(name: &'static str, entry: u64, arg: u64, stack_size: usize) -> Self {
        let id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
        log::info!(
            "scheduler: creating task id={} name='{}' entry={:#X} stack_size={:#X}",
            id,
            name,
            entry,
            stack_size
        );

        // Allocate stack (zeroed)
        let stack = alloc::vec![0u8; stack_size].into_boxed_slice();
        // Stack grows downward; top is at the end of the allocation
        let stack_top = stack.as_ptr() as u64 + stack_size as u64;
        // Align stack top to 16 bytes (System V ABI requirement)
        let stack_top_aligned = stack_top & !0xF;

        let context = CpuContext::new_task(entry, stack_top_aligned, arg);

        log::debug!(
            "scheduler: task {} stack: base={:#X} top={:#X}",
            id,
            stack.as_ptr() as u64,
            stack_top_aligned
        );

        Self {
            id,
            name,
            state: AtomicU8::new(TaskState::Ready as u8),
            context,
            stack,
            affinity: None,
            assigned_core: None,
        }
    }

    /// Create a task with CPU affinity.
    pub fn with_affinity(mut self, core_id: u32) -> Self {
        log::debug!(
            "scheduler: task {} pinned to core {}",
            self.id,
            core_id
        );
        self.affinity = Some(core_id);
        self
    }

    /// Get the current task state.
    pub fn get_state(&self) -> TaskState {
        TaskState::from(self.state.load(Ordering::Acquire))
    }

    /// Set the task state.
    pub fn set_state(&self, state: TaskState) {
        log::trace!(
            "scheduler: task {} state -> {:?}",
            self.id,
            state
        );
        self.state.store(state as u8, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Per-core run queue
// ---------------------------------------------------------------------------

/// Run queue for a single CPU core.
pub struct RunQueue {
    /// Core index this queue belongs to.
    core_id: u32,
    /// Queue of ready tasks (FIFO scheduling).
    queue: SpinLock<VecDeque<Box<Task>>>,
    /// Number of tasks in the queue (for quick steal decisions).
    len: AtomicU64,
}

impl RunQueue {
    /// Create a new run queue for the given core.
    pub fn new(core_id: u32) -> Self {
        log::info!("scheduler: creating run queue for core {}", core_id);
        Self {
            core_id,
            queue: SpinLock::new(VecDeque::new()),
            len: AtomicU64::new(0),
        }
    }

    /// Push a task onto the back of the queue.
    pub fn push(&self, task: Box<Task>) {
        log::debug!(
            "scheduler: core {} enqueue task {} '{}'",
            self.core_id,
            task.id,
            task.name
        );
        let mut q = self.queue.lock();
        q.push_back(task);
        self.len.fetch_add(1, Ordering::Release);
    }

    /// Pop a task from the front of the queue.
    pub fn pop(&self) -> Option<Box<Task>> {
        let mut q = self.queue.lock();
        if let Some(task) = q.pop_front() {
            self.len.fetch_sub(1, Ordering::Release);
            log::debug!(
                "scheduler: core {} dequeue task {} '{}'",
                self.core_id,
                task.id,
                task.name
            );
            Some(task)
        } else {
            None
        }
    }

    /// Steal half the tasks from this queue (for work stealing).
    pub fn steal_half(&self) -> Vec<Box<Task>> {
        let mut q = self.queue.lock();
        let steal_count = q.len() / 2;
        if steal_count == 0 {
            return Vec::new();
        }
        log::debug!(
            "scheduler: stealing {} tasks from core {} (had {})",
            steal_count,
            self.core_id,
            q.len()
        );
        let mut stolen = Vec::with_capacity(steal_count);
        for _ in 0..steal_count {
            if let Some(task) = q.pop_back() {
                stolen.push(task);
            }
        }
        self.len
            .fetch_sub(stolen.len() as u64, Ordering::Release);
        stolen
    }

    /// Return the number of tasks in the queue.
    pub fn len(&self) -> u64 {
        self.len.load(Ordering::Acquire)
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// IPI vector used to wake idle cores when tasks are available.
pub const IPI_SCHEDULE_VECTOR: u8 = 0xFE;

/// Multi-core scheduler with per-core run queues and work stealing.
pub struct Scheduler {
    /// Per-core run queues, indexed by core ID.
    run_queues: Vec<RunQueue>,
    /// Currently running task per core (indexed by core ID).
    current_tasks: Vec<SpinLock<Option<Box<Task>>>>,
    /// Number of active cores.
    num_cores: u32,
}

impl Scheduler {
    /// Create a new scheduler for `num_cores` cores.
    pub fn new(num_cores: u32) -> Self {
        log::info!("scheduler: initializing for {} cores", num_cores);

        let mut run_queues = Vec::with_capacity(num_cores as usize);
        let mut current_tasks = Vec::with_capacity(num_cores as usize);

        for i in 0..num_cores {
            run_queues.push(RunQueue::new(i));
            current_tasks.push(SpinLock::new(None));
        }

        log::info!("scheduler: {} run queues created", num_cores);
        Self {
            run_queues,
            current_tasks,
            num_cores,
        }
    }

    /// Spawn a new task. If it has CPU affinity, enqueue it there;
    /// otherwise, enqueue on the least-loaded core.
    pub fn spawn(&self, task: Box<Task>) {
        let core = if let Some(affinity) = task.affinity {
            log::debug!(
                "scheduler: task {} has affinity to core {}",
                task.id,
                affinity
            );
            affinity
        } else {
            self.least_loaded_core()
        };

        log::info!(
            "scheduler: spawning task {} '{}' on core {}",
            task.id,
            task.name,
            core
        );
        self.run_queues[core as usize].push(task);
    }

    /// Spawn a task on a specific core, ignoring affinity.
    pub fn spawn_on_core(&self, core_id: u32, task: Box<Task>) {
        assert!(
            core_id < self.num_cores,
            "scheduler: core_id {} out of range (max {})",
            core_id,
            self.num_cores
        );
        log::info!(
            "scheduler: spawning task {} '{}' on core {} (explicit)",
            task.id,
            task.name,
            core_id
        );
        self.run_queues[core_id as usize].push(task);
    }

    /// Find the core with the fewest queued tasks.
    fn least_loaded_core(&self) -> u32 {
        let mut min_load = u64::MAX;
        let mut min_core = 0u32;
        for (i, rq) in self.run_queues.iter().enumerate() {
            let load = rq.len();
            if load < min_load {
                min_load = load;
                min_core = i as u32;
            }
        }
        log::trace!("scheduler: least loaded core = {} (load={})", min_core, min_load);
        min_core
    }

    /// Called from the timer interrupt handler on each core.
    ///
    /// Picks the next task to run. If the current core's queue is empty,
    /// attempts to steal work from the busiest core.
    ///
    /// Returns the next task to switch to, and the previously running task
    /// (if any) to be re-enqueued.
    pub fn schedule(&self, core_id: u32) -> Option<Box<Task>> {
        let rq = &self.run_queues[core_id as usize];

        // Try our own queue first
        if let Some(task) = rq.pop() {
            log::trace!(
                "scheduler: core {} picked task {} from own queue",
                core_id,
                task.id
            );
            return Some(task);
        }

        // Work stealing: find the busiest core and steal half its queue
        let busiest = self.busiest_core(core_id);
        if let Some(victim) = busiest {
            let stolen = self.run_queues[victim as usize].steal_half();
            if !stolen.is_empty() {
                log::info!(
                    "scheduler: core {} stole {} tasks from core {}",
                    core_id,
                    stolen.len(),
                    victim
                );
                let mut iter = stolen.into_iter();
                let first = iter.next();
                // Push remaining stolen tasks to our queue
                for task in iter {
                    rq.push(task);
                }
                return first;
            }
        }

        log::trace!("scheduler: core {} has no work", core_id);
        None
    }

    /// Find the busiest core (excluding `exclude_core`).
    fn busiest_core(&self, exclude_core: u32) -> Option<u32> {
        let mut max_load = 1u64; // Don't steal from cores with only 1 task
        let mut max_core = None;
        for (i, rq) in self.run_queues.iter().enumerate() {
            if i as u32 == exclude_core {
                continue;
            }
            let load = rq.len();
            if load > max_load {
                max_load = load;
                max_core = Some(i as u32);
            }
        }
        max_core
    }

    /// Park the current task (mark as blocked) and switch to the next.
    ///
    /// Returns the next task to run (or None if idle).
    pub fn block_current(&self, core_id: u32) -> Option<Box<Task>> {
        log::debug!("scheduler: core {} blocking current task", core_id);
        // The caller is responsible for saving the current task's context
        // and actually removing it from the current_tasks slot.
        self.schedule(core_id)
    }

    /// Wake a blocked task by ID, placing it on the specified core's queue.
    pub fn wake(&self, task: Box<Task>, target_core: u32) {
        log::info!(
            "scheduler: waking task {} '{}' -> core {}",
            task.id,
            task.name,
            target_core
        );
        task.set_state(TaskState::Ready);
        self.run_queues[target_core as usize].push(task);
    }

    /// Save the currently running task back to a queue (e.g., on preemption).
    pub fn preempt_current(&self, core_id: u32, task: Box<Task>) {
        log::trace!(
            "scheduler: core {} preempting task {} '{}'",
            core_id,
            task.id,
            task.name
        );
        task.set_state(TaskState::Ready);
        self.run_queues[core_id as usize].push(task);
    }

    /// Return total number of queued tasks across all cores.
    pub fn total_queued(&self) -> u64 {
        self.run_queues.iter().map(|rq| rq.len()).sum()
    }

    /// Return per-core queue lengths (for diagnostics).
    pub fn queue_lengths(&self) -> Vec<u64> {
        self.run_queues.iter().map(|rq| rq.len()).collect()
    }
}

// ---------------------------------------------------------------------------
// Context switch (architecture-specific)
// ---------------------------------------------------------------------------

/// Perform a context switch from `old` to `new`.
///
/// Saves the current CPU state into `old` and restores from `new`.
///
/// # Safety
/// Both contexts must be valid. The new context's stack must be properly set up.
/// Must be called with interrupts disabled.
#[inline(never)]
pub unsafe fn context_switch(old: &mut CpuContext, new: &CpuContext) {
    log::trace!(
        "scheduler: context_switch old_rip={:#X} -> new_rip={:#X}",
        old.rip,
        new.rip
    );

    // Save callee-saved registers into old context, load from new context.
    // This is the core context switch: we save the current state and restore
    // the target state. The `ret` at the end of this block will return to
    // new.rip because we set rsp to new.rsp which has new.rip on top.
    core::arch::asm!(
        // Save callee-saved registers
        "mov [{old} + 0x00], rsp",
        "mov [{old} + 0x08], rbp",
        "mov [{old} + 0x10], rbx",
        "mov [{old} + 0x18], r12",
        "mov [{old} + 0x20], r13",
        "mov [{old} + 0x28], r14",
        "mov [{old} + 0x30], r15",
        // Save return address (rip) — the address after this asm block
        "lea rax, [rip + 2f]",
        "mov [{old} + 0x38], rax",
        // Save rflags
        "pushfq",
        "pop rax",
        "mov [{old} + 0x40], rax",

        // Restore callee-saved registers from new context
        "mov rsp, [{new} + 0x00]",
        "mov rbp, [{new} + 0x08]",
        "mov rbx, [{new} + 0x10]",
        "mov r12, [{new} + 0x18]",
        "mov r13, [{new} + 0x20]",
        "mov r14, [{new} + 0x28]",
        "mov r15, [{new} + 0x30]",
        // Restore rflags
        "mov rax, [{new} + 0x40]",
        "push rax",
        "popfq",
        // Restore rdi (first argument for new tasks)
        "mov rdi, [{new} + 0x68]",
        // Jump to new context's rip
        "mov rax, [{new} + 0x38]",
        "jmp rax",

        // Label for the saved rip — when we switch back to old, we resume here
        "2:",

        old = in(reg) old as *mut CpuContext,
        new = in(reg) new as *const CpuContext,
        // Clobber everything the asm touches
        out("rax") _,
        out("rdi") _,
        options(nostack),
    );
}
