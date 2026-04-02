//! Per-CPU data structures for ClaudioOS.
//!
//! Each CPU core has its own `PerCpu` struct holding core-local state.
//! The current CPU's `PerCpu` is accessible via the GS segment base register
//! (IA32_GS_BASE MSR, 0xC0000101) or by indexing with the local APIC ID.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// MSR for GS base address.
const IA32_GS_BASE: u32 = 0xC000_0101;
/// MSR for kernel GS base (swapped on SWAPGS).
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 256;

/// Global array of per-CPU data pointers, indexed by CPU index (0..num_cpus).
static mut PERCPU_ARRAY: [*mut PerCpu; MAX_CPUS] = [core::ptr::null_mut(); MAX_CPUS];

/// Number of initialized per-CPU structures.
static PERCPU_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Per-CPU data structure.
///
/// Each CPU core has exactly one of these, allocated during SMP init.
/// Fields are laid out for cache-line friendliness.
#[repr(C, align(64))]
pub struct PerCpu {
    /// Self-pointer (for validation when accessed via GS base).
    pub self_ptr: *const PerCpu,
    /// Logical CPU index (0 = BSP, 1..N = APs).
    pub cpu_index: u32,
    /// Local APIC ID for this core.
    pub apic_id: u32,
    /// Whether this core is the BSP.
    pub is_bsp: bool,
    /// Top of the kernel stack for this core.
    pub kernel_stack_top: u64,
    /// Size of the kernel stack in bytes.
    pub kernel_stack_size: u64,
    /// Currently running task ID (0 = idle).
    pub current_task_id: AtomicU64,
    /// Number of interrupts disabled (nesting counter for cli/sti).
    pub interrupt_disable_count: AtomicUsize,
    /// Per-CPU local data pointer (for extensibility).
    pub local_data: AtomicU64,
    /// Padding to fill a cache line.
    _pad: [u8; 8],
}

impl PerCpu {
    /// Allocate and initialize a new `PerCpu` for the given CPU.
    pub fn new(cpu_index: u32, apic_id: u32, is_bsp: bool) -> Box<Self> {
        log::info!(
            "percpu: allocating PerCpu for cpu_index={}, apic_id={}, bsp={}",
            cpu_index,
            apic_id,
            is_bsp
        );
        let mut percpu = Box::new(Self {
            self_ptr: core::ptr::null(),
            cpu_index,
            apic_id,
            is_bsp,
            kernel_stack_top: 0,
            kernel_stack_size: 0,
            current_task_id: AtomicU64::new(0),
            interrupt_disable_count: AtomicUsize::new(0),
            local_data: AtomicU64::new(0),
            _pad: [0u8; 8],
        });
        // Set self-pointer for validation
        percpu.self_ptr = &*percpu as *const PerCpu;
        percpu
    }

    /// Set the kernel stack for this CPU.
    pub fn set_kernel_stack(&mut self, stack_top: u64, stack_size: u64) {
        log::debug!(
            "percpu: cpu {} kernel stack: top={:#X}, size={:#X}",
            self.cpu_index,
            stack_top,
            stack_size
        );
        self.kernel_stack_top = stack_top;
        self.kernel_stack_size = stack_size;
    }
}

// ---------------------------------------------------------------------------
// MSR helpers
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags),
    );
    ((high as u64) << 32) | (low as u64)
}

#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags),
    );
}

// ---------------------------------------------------------------------------
// Per-CPU registration and lookup
// ---------------------------------------------------------------------------

/// Register a `PerCpu` structure in the global array and set it as the
/// current CPU's GS base.
///
/// # Safety
/// Must be called exactly once per CPU during boot. The `PerCpu` must be
/// heap-allocated and must outlive the CPU (i.e., leaked / never freed).
pub unsafe fn register_percpu(percpu: Box<PerCpu>) {
    let index = percpu.cpu_index as usize;
    assert!(
        index < MAX_CPUS,
        "percpu: cpu_index {} exceeds MAX_CPUS {}",
        index,
        MAX_CPUS
    );

    // Leak the Box to get a 'static pointer
    let ptr = Box::into_raw(percpu);
    log::info!(
        "percpu: registering cpu_index={} at ptr={:p}",
        index,
        ptr
    );

    PERCPU_ARRAY[index] = ptr;
    PERCPU_COUNT.fetch_add(1, Ordering::Release);

    // Set GS base to point to this PerCpu
    wrmsr(IA32_GS_BASE, ptr as u64);
    log::debug!(
        "percpu: IA32_GS_BASE set to {:#X} for cpu {}",
        ptr as u64,
        index
    );
}

/// Get the `PerCpu` for the currently executing CPU via GS base.
///
/// # Safety
/// GS base must have been set via `register_percpu()` on this core.
/// This function is safe to call from interrupt handlers.
pub fn get_current_cpu() -> &'static PerCpu {
    unsafe {
        let gs_base = rdmsr(IA32_GS_BASE);
        let percpu = &*(gs_base as *const PerCpu);
        // Validate self-pointer
        debug_assert_eq!(
            percpu.self_ptr as u64, gs_base,
            "percpu: GS base self-pointer mismatch!"
        );
        percpu
    }
}

/// Get a mutable reference to the current CPU's `PerCpu`.
///
/// # Safety
/// Caller must ensure exclusive access (e.g., interrupts disabled).
pub unsafe fn get_current_cpu_mut() -> &'static mut PerCpu {
    let gs_base = rdmsr(IA32_GS_BASE);
    &mut *(gs_base as *mut PerCpu)
}

/// Get the `PerCpu` for a specific CPU index.
///
/// Returns `None` if the CPU has not been registered.
pub fn get_cpu(index: usize) -> Option<&'static PerCpu> {
    if index >= MAX_CPUS {
        return None;
    }
    unsafe {
        let ptr = PERCPU_ARRAY[index];
        if ptr.is_null() {
            None
        } else {
            Some(&*ptr)
        }
    }
}

/// Return the number of registered CPUs.
pub fn cpu_count() -> usize {
    PERCPU_COUNT.load(Ordering::Acquire)
}

/// Return all registered PerCpu references.
pub fn all_cpus() -> Vec<&'static PerCpu> {
    let count = cpu_count();
    let mut cpus = Vec::with_capacity(count);
    for i in 0..MAX_CPUS {
        if let Some(cpu) = get_cpu(i) {
            cpus.push(cpu);
            if cpus.len() == count {
                break;
            }
        }
    }
    cpus
}
