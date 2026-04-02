//! SMP-safe synchronization primitives for ClaudioOS.
//!
//! Provides ticket locks (fair), spin locks with interrupt disable,
//! reader-writer locks, and one-time initialization.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Interrupt enable/disable helpers
// ---------------------------------------------------------------------------

/// Read the RFLAGS register.
#[inline]
fn read_rflags() -> u64 {
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags),
        );
    }
    rflags
}

/// Check if interrupts are enabled (IF flag, bit 9).
#[inline]
pub fn interrupts_enabled() -> bool {
    (read_rflags() & (1 << 9)) != 0
}

/// Disable interrupts and return whether they were previously enabled.
#[inline]
pub fn disable_interrupts() -> bool {
    let was_enabled = interrupts_enabled();
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    was_enabled
}

/// Enable interrupts.
#[inline]
pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Restore interrupts to a previous state.
#[inline]
pub fn restore_interrupts(was_enabled: bool) {
    if was_enabled {
        enable_interrupts();
    }
}

// ===========================================================================
// TicketLock — fair FIFO spinlock
// ===========================================================================

/// A fair (FIFO) ticket-based spinlock.
///
/// Guarantees that waiters acquire the lock in the order they requested it.
/// Uses two counters: `next_ticket` (incremented on lock attempt) and
/// `now_serving` (incremented on unlock).
pub struct TicketLock<T: ?Sized> {
    next_ticket: AtomicU64,
    now_serving: AtomicU64,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for TicketLock<T> {}
unsafe impl<T: ?Sized + Send> Send for TicketLock<T> {}

impl<T> TicketLock<T> {
    /// Create a new ticket lock wrapping `data`.
    pub const fn new(data: T) -> Self {
        Self {
            next_ticket: AtomicU64::new(0),
            now_serving: AtomicU64::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock, spinning until it is our turn.
    pub fn lock(&self) -> TicketLockGuard<'_, T> {
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        log::trace!("ticket_lock: acquired ticket {}", ticket);

        let mut spins = 0u64;
        while self.now_serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
            spins += 1;
            if spins % 10_000_000 == 0 {
                log::warn!(
                    "ticket_lock: long wait on ticket {} (serving {}), spins={}",
                    ticket,
                    self.now_serving.load(Ordering::Relaxed),
                    spins
                );
            }
        }

        log::trace!("ticket_lock: ticket {} now serving", ticket);
        TicketLockGuard { lock: self }
    }

    /// Try to acquire the lock without spinning. Returns `None` if busy.
    pub fn try_lock(&self) -> Option<TicketLockGuard<'_, T>> {
        let current = self.now_serving.load(Ordering::Acquire);
        let next = self.next_ticket.load(Ordering::Relaxed);
        if current == next {
            // Attempt to take the next ticket
            if self
                .next_ticket
                .compare_exchange(next, next + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                log::trace!("ticket_lock: try_lock succeeded (ticket {})", next);
                return Some(TicketLockGuard { lock: self });
            }
        }
        log::trace!("ticket_lock: try_lock failed");
        None
    }

    /// Check if the lock is currently held.
    pub fn is_locked(&self) -> bool {
        self.now_serving.load(Ordering::Relaxed) != self.next_ticket.load(Ordering::Relaxed)
    }
}

/// RAII guard for `TicketLock`.
pub struct TicketLockGuard<'a, T: ?Sized> {
    lock: &'a TicketLock<T>,
}

impl<T: ?Sized> Deref for TicketLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for TicketLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for TicketLockGuard<'_, T> {
    fn drop(&mut self) {
        log::trace!("ticket_lock: releasing");
        self.lock.now_serving.fetch_add(1, Ordering::Release);
    }
}

// ===========================================================================
// SpinLock — spinlock with interrupt disable
// ===========================================================================

/// A spinlock that disables interrupts while held.
///
/// This prevents deadlocks from interrupt handlers trying to acquire the
/// same lock. Interrupts are restored to their previous state on drop.
pub struct SpinLock<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Create a new spinlock wrapping `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock, disabling interrupts and spinning until available.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let was_enabled = disable_interrupts();
        log::trace!("spinlock: acquiring (interrupts were {})", was_enabled);

        let mut spins = 0u64;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Spin on read to reduce cache-line bouncing
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
                spins += 1;
            }
            if spins % 10_000_000 == 0 && spins > 0 {
                log::warn!("spinlock: long contention, spins={}", spins);
            }
        }

        log::trace!("spinlock: acquired after {} spins", spins);
        SpinLockGuard {
            lock: self,
            interrupts_were_enabled: was_enabled,
        }
    }

    /// Try to acquire the lock without spinning. Returns `None` if busy.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let was_enabled = disable_interrupts();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            log::trace!("spinlock: try_lock succeeded");
            Some(SpinLockGuard {
                lock: self,
                interrupts_were_enabled: was_enabled,
            })
        } else {
            restore_interrupts(was_enabled);
            log::trace!("spinlock: try_lock failed");
            None
        }
    }

    /// Check if the lock is currently held.
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

/// RAII guard for `SpinLock`.
pub struct SpinLockGuard<'a, T: ?Sized> {
    lock: &'a SpinLock<T>,
    interrupts_were_enabled: bool,
}

impl<T: ?Sized> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        log::trace!("spinlock: releasing");
        self.lock.locked.store(false, Ordering::Release);
        restore_interrupts(self.interrupts_were_enabled);
    }
}

// ===========================================================================
// RwLock — reader-writer lock
// ===========================================================================

/// A reader-writer spinlock.
///
/// Multiple readers can hold the lock simultaneously, but a writer gets
/// exclusive access. Writers are NOT prioritized (simple implementation).
///
/// State encoding in `state`:
///   - 0 = unlocked
///   - positive = number of active readers
///   - u32::MAX (0xFFFF_FFFF) = writer holds the lock
const WRITER_BIT: u32 = u32::MAX;

pub struct RwLock<T: ?Sized> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for RwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Send for RwLock<T> {}

impl<T> RwLock<T> {
    /// Create a new reader-writer lock wrapping `data`.
    pub const fn new(data: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire a read lock. Spins until no writer holds the lock.
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        log::trace!("rwlock: acquiring read lock");
        loop {
            let current = self.state.load(Ordering::Relaxed);
            if current == WRITER_BIT {
                // Writer active, spin
                core::hint::spin_loop();
                continue;
            }
            // Try to increment reader count
            if self
                .state
                .compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                log::trace!("rwlock: read lock acquired (readers={})", current + 1);
                return RwLockReadGuard { lock: self };
            }
        }
    }

    /// Try to acquire a read lock without spinning.
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let current = self.state.load(Ordering::Relaxed);
        if current == WRITER_BIT {
            return None;
        }
        if self
            .state
            .compare_exchange(current, current + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(RwLockReadGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquire a write lock. Spins until all readers and any writer release.
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        log::trace!("rwlock: acquiring write lock");
        let mut spins = 0u64;
        loop {
            if self
                .state
                .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                log::trace!("rwlock: write lock acquired after {} spins", spins);
                return RwLockWriteGuard { lock: self };
            }
            core::hint::spin_loop();
            spins += 1;
            if spins % 10_000_000 == 0 {
                log::warn!("rwlock: write lock contention, spins={}", spins);
            }
        }
    }

    /// Try to acquire a write lock without spinning.
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        if self
            .state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(RwLockWriteGuard { lock: self })
        } else {
            None
        }
    }
}

/// RAII guard for read access.
pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        log::trace!("rwlock: releasing read lock");
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}

/// RAII guard for write access.
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        log::trace!("rwlock: releasing write lock");
        self.lock.state.store(0, Ordering::Release);
    }
}

// ===========================================================================
// Once — one-time initialization
// ===========================================================================

/// States for `Once`.
const ONCE_UNINIT: u32 = 0;
const ONCE_RUNNING: u32 = 1;
const ONCE_DONE: u32 = 2;

/// A one-time initialization primitive.
///
/// Ensures a closure runs exactly once, even under concurrent access.
pub struct Once {
    state: AtomicU32,
}

impl Once {
    /// Create a new uninitialized `Once`.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(ONCE_UNINIT),
        }
    }

    /// Run the closure exactly once. If another thread is currently running
    /// it, this will spin until completion.
    pub fn call_once(&self, f: impl FnOnce()) {
        if self.state.load(Ordering::Acquire) == ONCE_DONE {
            return;
        }

        log::trace!("once: attempting initialization");
        if self
            .state
            .compare_exchange(ONCE_UNINIT, ONCE_RUNNING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            log::debug!("once: running initializer");
            f();
            self.state.store(ONCE_DONE, Ordering::Release);
            log::debug!("once: initialization complete");
        } else {
            // Another thread is running the initializer, spin until done
            log::trace!("once: waiting for another thread to complete init");
            while self.state.load(Ordering::Acquire) != ONCE_DONE {
                core::hint::spin_loop();
            }
        }
    }

    /// Check if initialization has completed.
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == ONCE_DONE
    }
}

// ===========================================================================
// Atomic wrappers with explicit ordering
// ===========================================================================

/// Atomic fence with sequential consistency.
#[inline]
pub fn fence_seq_cst() {
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// Atomic fence with acquire ordering.
#[inline]
pub fn fence_acquire() {
    core::sync::atomic::fence(Ordering::Acquire);
}

/// Atomic fence with release ordering.
#[inline]
pub fn fence_release() {
    core::sync::atomic::fence(Ordering::Release);
}

/// Memory fence via `mfence` instruction (full barrier).
#[inline]
pub fn mfence() {
    unsafe {
        core::arch::asm!("mfence", options(nomem, nostack, preserves_flags));
    }
}

/// Store fence via `sfence` instruction (store barrier).
#[inline]
pub fn sfence() {
    unsafe {
        core::arch::asm!("sfence", options(nomem, nostack, preserves_flags));
    }
}

/// Load fence via `lfence` instruction (load barrier).
#[inline]
pub fn lfence() {
    unsafe {
        core::arch::asm!("lfence", options(nomem, nostack, preserves_flags));
    }
}
