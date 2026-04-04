//! High-level SMP controller API for ClaudioOS.
//!
//! Orchestrates APIC configuration, AP boot, per-CPU setup, and exposes
//! a simple interface for the kernel to manage multiple cores.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::apic::{self, LocalApic};
use crate::ioapic::{IoApic, IoApicManager};
use crate::percpu::{self, PerCpu};
use crate::scheduler::{Scheduler, Task, TaskId, DEFAULT_STACK_SIZE};
use crate::spinlock::SpinLock;
use crate::trampoline::{ApTrampoline, TRAMPOLINE_PAGE, TRAMPOLINE_PHYS};

// ---------------------------------------------------------------------------
// ACPI MADT structures (minimal — just what we need to parse)
// ---------------------------------------------------------------------------

/// Information about a local APIC discovered from the ACPI MADT.
#[derive(Debug, Clone)]
pub struct MadtLocalApic {
    /// ACPI processor ID.
    pub processor_id: u8,
    /// Local APIC ID.
    pub apic_id: u8,
    /// Whether this processor is enabled.
    pub enabled: bool,
}

/// Information about an I/O APIC discovered from the ACPI MADT.
#[derive(Debug, Clone)]
pub struct MadtIoApic {
    /// I/O APIC ID.
    pub id: u8,
    /// Physical base address of the I/O APIC registers.
    pub address: u64,
    /// Global System Interrupt base.
    pub gsi_base: u32,
}

/// Parsed MADT information passed to the SMP controller.
#[derive(Debug, Clone)]
pub struct MadtInfo {
    /// Local APIC physical base address.
    pub local_apic_addr: u64,
    /// All local APICs (including BSP).
    pub local_apics: Vec<MadtLocalApic>,
    /// All I/O APICs.
    pub io_apics: Vec<MadtIoApic>,
}

// ---------------------------------------------------------------------------
// AP entry point
// ---------------------------------------------------------------------------

/// Flag indicating all APs should halt (used during shutdown).
static AP_HALT: AtomicBool = AtomicBool::new(false);

/// Number of APs that have successfully booted.
static APS_BOOTED: AtomicU32 = AtomicU32::new(0);

/// AP entry point called by the trampoline after switching to long mode.
///
/// This function is called with interrupts disabled. It must:
/// 1. Initialize the local APIC on this core
/// 2. Set up per-CPU data
/// 3. Enter the scheduler idle loop
///
/// # Safety
/// Called from the trampoline in a bare environment. `cpu_index` must be valid.
pub extern "C" fn ap_entry(cpu_index: u64) {
    let cpu_index = cpu_index as u32;
    log::info!("smp: AP {} has entered long mode", cpu_index);

    // Initialize local APIC on this AP
    let apic = LocalApic::new(0xFEE0_0000);
    let apic_id = apic.id();
    log::info!("smp: AP {} local APIC ID = {}", cpu_index, apic_id);

    apic.enable();
    log::info!("smp: AP {} local APIC enabled", cpu_index);

    // Set TPR to 0 (accept all interrupts)
    apic.write_tpr(0);

    // Create and register per-CPU data
    let percpu = PerCpu::new(cpu_index, apic_id, false);
    unsafe {
        percpu::register_percpu(percpu);
    }
    log::info!("smp: AP {} per-CPU data registered", cpu_index);

    // Signal that this AP is ready
    APS_BOOTED.fetch_add(1, Ordering::Release);
    log::info!("smp: AP {} boot complete, entering idle loop", cpu_index);

    // AP idle loop — wait for tasks from the scheduler
    loop {
        if AP_HALT.load(Ordering::Acquire) {
            log::info!("smp: AP {} halting", cpu_index);
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack));
            }
        }
        // Wait for interrupt (scheduler IPI will wake us)
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
        }
    }
}

// ---------------------------------------------------------------------------
// SMP Controller
// ---------------------------------------------------------------------------

/// Default stack size for AP kernel stacks (256 KiB).
const AP_STACK_SIZE: usize = 256 * 1024;

/// High-level SMP controller.
///
/// Manages discovery, boot, and coordination of all CPU cores.
pub struct SmpController {
    /// BSP's local APIC ID.
    bsp_apic_id: u32,
    /// Local APIC driver (BSP instance).
    local_apic: LocalApic,
    /// I/O APIC manager.
    ioapic_manager: IoApicManager,
    /// AP trampoline manager.
    trampoline: ApTrampoline,
    /// Physical-to-virtual address offset (0 for identity-mapped).
    phys_offset: u64,
    /// Number of APs (not including BSP).
    ap_count: u32,
    /// Total number of active cores (BSP + APs).
    total_cores: AtomicU32,
    /// MADT info for reference.
    madt_info: Option<MadtInfo>,
    /// Scheduler instance.
    scheduler: Option<Scheduler>,
    /// Whether SMP has been fully initialized.
    initialized: AtomicBool,
}

impl SmpController {
    /// Create a new SMP controller.
    ///
    /// `apic_base_vaddr` — virtual address of the local APIC MMIO registers.
    /// `phys_offset` — offset added to physical addresses to get virtual addresses.
    ///   Pass 0 for identity-mapped memory.
    pub fn new(apic_base_vaddr: u64, phys_offset: u64) -> Self {
        log::info!("smp: creating SmpController, APIC base={:#X}, phys_offset={:#X}",
            apic_base_vaddr, phys_offset);

        let local_apic = LocalApic::new(apic_base_vaddr);
        let bsp_apic_id = local_apic.id();
        log::info!("smp: BSP APIC ID = {}", bsp_apic_id);

        // Trampoline code lives at a fixed physical address; translate to virtual
        let trampoline_vaddr = TRAMPOLINE_PHYS + phys_offset;
        log::info!("smp: trampoline phys={:#X} virt={:#X}", TRAMPOLINE_PHYS, trampoline_vaddr);

        Self {
            bsp_apic_id,
            local_apic,
            ioapic_manager: IoApicManager::new(),
            trampoline: ApTrampoline::new(trampoline_vaddr),
            phys_offset,
            ap_count: 0,
            total_cores: AtomicU32::new(1), // BSP counts as 1
            madt_info: None,
            scheduler: None,
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialize SMP from parsed ACPI MADT information.
    ///
    /// This is the main entry point: it configures the local APIC, sets up
    /// I/O APICs, boots all APs, and initializes the scheduler.
    pub fn init(&mut self, madt_info: MadtInfo) {
        log::info!("smp: === SMP INITIALIZATION BEGIN ===");
        log::info!(
            "smp: MADT reports {} local APICs, {} I/O APICs",
            madt_info.local_apics.len(),
            madt_info.io_apics.len()
        );

        // Count enabled APs
        let enabled_aps: Vec<&MadtLocalApic> = madt_info
            .local_apics
            .iter()
            .filter(|la| la.enabled && la.apic_id as u32 != self.bsp_apic_id)
            .collect();
        self.ap_count = enabled_aps.len() as u32;
        log::info!(
            "smp: BSP APIC ID = {}, found {} enabled APs",
            self.bsp_apic_id,
            self.ap_count
        );

        // Step 1: Configure BSP local APIC
        self.init_bsp_apic();

        // Step 2: Configure I/O APICs
        self.init_io_apics(&madt_info);

        // Step 3: Set up BSP per-CPU data
        self.init_bsp_percpu();

        // Step 4: Boot APs
        self.boot_aps(&enabled_aps);

        // Step 5: Initialize scheduler
        let total = self.num_cores();
        self.scheduler = Some(Scheduler::new(total));
        log::info!("smp: scheduler initialized for {} cores", total);

        self.madt_info = Some(madt_info);
        self.initialized.store(true, Ordering::Release);
        log::info!("smp: === SMP INITIALIZATION COMPLETE ===");
        log::info!(
            "smp: {} total cores active ({} APs booted)",
            self.num_cores(),
            APS_BOOTED.load(Ordering::Acquire)
        );
    }

    /// Configure the BSP's local APIC.
    fn init_bsp_apic(&self) {
        log::info!("smp: configuring BSP local APIC");

        // Enable the local APIC
        self.local_apic.enable();

        // Set TPR to 0 (accept all interrupts)
        self.local_apic.write_tpr(0);

        // Configure flat model for logical destination
        self.local_apic.write_dfr(0xFFFF_FFFF); // flat model
        let ldr = (self.local_apic.read_ldr() & 0x00FF_FFFF) | ((1u32) << 24);
        self.local_apic.write_ldr(ldr);

        // Mask all LVT entries initially
        self.local_apic.write_lvt_lint0(apic::LVT_MASKED);
        self.local_apic.write_lvt_lint1(apic::LVT_MASKED);
        self.local_apic.write_lvt_error(apic::LVT_MASKED);
        self.local_apic.write_lvt_timer(apic::LVT_MASKED);
        self.local_apic.write_lvt_perf(apic::LVT_MASKED);
        self.local_apic.write_lvt_thermal(apic::LVT_MASKED);

        let version = self.local_apic.version();
        log::info!(
            "smp: BSP APIC configured — version={:#X}, max_lvt={}",
            version & 0xFF,
            ((version >> 16) & 0xFF) + 1
        );
    }

    /// Configure all I/O APICs from MADT data.
    fn init_io_apics(&mut self, madt_info: &MadtInfo) {
        log::info!("smp: configuring {} I/O APICs", madt_info.io_apics.len());

        for io_apic_info in &madt_info.io_apics {
            // Translate I/O APIC MMIO address from physical to virtual
            let ioapic_vaddr = io_apic_info.address + self.phys_offset;
            log::info!(
                "smp: I/O APIC id={}, phys={:#X}, virt={:#X}, gsi_base={}",
                io_apic_info.id,
                io_apic_info.address,
                ioapic_vaddr,
                io_apic_info.gsi_base
            );
            let ioapic = IoApic::new(ioapic_vaddr, io_apic_info.gsi_base);
            // Mask all entries initially
            ioapic.mask_all();
            self.ioapic_manager.add(ioapic);
        }

        log::info!(
            "smp: {} I/O APICs configured, all IRQs masked",
            self.ioapic_manager.count()
        );
    }

    /// Set up BSP per-CPU data.
    fn init_bsp_percpu(&self) {
        log::info!("smp: setting up BSP per-CPU data");
        let percpu = PerCpu::new(0, self.bsp_apic_id, true);
        unsafe {
            percpu::register_percpu(percpu);
        }
        log::info!("smp: BSP per-CPU data registered (cpu_index=0)");
    }

    /// Boot all APs using the INIT-SIPI-SIPI sequence.
    fn boot_aps(&mut self, aps: &[&MadtLocalApic]) {
        if aps.is_empty() {
            log::info!("smp: no APs to boot");
            return;
        }

        log::info!("smp: booting {} APs", aps.len());

        // Install trampoline code
        unsafe {
            self.trampoline.install();
        }
        log::info!("smp: trampoline installed at phys {:#X}", TRAMPOLINE_PHYS);

        // Read CR3 for the current page table
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        }
        log::info!("smp: current CR3 (PML4) = {:#X}", cr3);

        for (i, ap) in aps.iter().enumerate() {
            let cpu_index = (i + 1) as u32; // BSP is 0
            log::info!(
                "smp: booting AP {} (APIC ID {}, cpu_index {})",
                i,
                ap.apic_id,
                cpu_index
            );

            // Allocate a stack for this AP
            let stack = alloc::vec![0u8; AP_STACK_SIZE].into_boxed_slice();
            let stack_top = stack.as_ptr() as u64 + AP_STACK_SIZE as u64;
            let stack_top_aligned = stack_top & !0xF;
            log::debug!(
                "smp: AP {} stack: base={:#X}, top={:#X}, size={:#X}",
                cpu_index,
                stack.as_ptr() as u64,
                stack_top_aligned,
                AP_STACK_SIZE
            );
            // Leak the stack so it lives forever
            core::mem::forget(stack);

            // Set up trampoline data for this AP
            unsafe {
                self.trampoline.set_ap_data(
                    cr3,
                    stack_top_aligned,
                    ap_entry as u64,
                    cpu_index,
                );
            }

            // INIT-SIPI-SIPI sequence (Intel SDM Vol 3, 8.4.4.1)
            log::info!("smp: sending INIT IPI to APIC ID {}", ap.apic_id);
            self.local_apic.send_init_ipi(ap.apic_id as u32);

            // Wait ~10ms (spin loop — no proper timer yet, so approximate)
            log::debug!("smp: waiting ~10ms after INIT");
            spin_delay(10_000_000);

            // Send first SIPI
            log::info!(
                "smp: sending first SIPI to APIC ID {}, page {:#04X}",
                ap.apic_id,
                TRAMPOLINE_PAGE
            );
            self.local_apic
                .send_startup_ipi(ap.apic_id as u32, TRAMPOLINE_PAGE);

            // Wait ~200us
            spin_delay(200_000);

            // If AP hasn't responded, send second SIPI (per Intel spec)
            if !self.trampoline.is_ap_ready() {
                log::warn!(
                    "smp: AP {} not ready after first SIPI, sending second",
                    cpu_index
                );
                self.local_apic
                    .send_startup_ipi(ap.apic_id as u32, TRAMPOLINE_PAGE);
                spin_delay(200_000);
            }

            // Wait for AP to signal ready (with timeout)
            if self.trampoline.wait_ap_ready(100_000_000) {
                self.total_cores.fetch_add(1, Ordering::Release);
                log::info!("smp: AP {} (APIC ID {}) booted successfully", cpu_index, ap.apic_id);
            } else {
                log::error!(
                    "smp: AP {} (APIC ID {}) FAILED TO BOOT — timed out waiting for ready signal",
                    cpu_index,
                    ap.apic_id
                );
            }
        }

        log::info!(
            "smp: AP boot complete. {} of {} APs booted.",
            APS_BOOTED.load(Ordering::Acquire),
            aps.len()
        );
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Return the total number of active cores (BSP + booted APs).
    pub fn num_cores(&self) -> u32 {
        self.total_cores.load(Ordering::Acquire)
    }

    /// Return the BSP's APIC ID.
    pub fn bsp_apic_id(&self) -> u32 {
        self.bsp_apic_id
    }

    /// Return the current core's CPU index.
    pub fn current_core(&self) -> u32 {
        percpu::get_current_cpu().cpu_index
    }

    /// Return the number of APs that were booted.
    pub fn ap_count(&self) -> u32 {
        APS_BOOTED.load(Ordering::Acquire)
    }

    /// Send an IPI to a specific core.
    pub fn ipi_send(&self, target_core: u32, vector: u8) {
        if let Some(cpu) = percpu::get_cpu(target_core as usize) {
            log::debug!(
                "smp: sending IPI vector {:#04X} to core {} (APIC ID {})",
                vector,
                target_core,
                cpu.apic_id
            );
            self.local_apic.send_ipi(
                cpu.apic_id,
                apic::ICR_FIXED | apic::ICR_DEST_PHYSICAL | apic::ICR_NO_SHORTHAND | (vector as u32),
            );
        } else {
            log::error!("smp: cannot send IPI to core {} — not registered", target_core);
        }
    }

    /// Spawn a task on a specific core.
    pub fn spawn_on_core(
        &self,
        core_id: u32,
        name: &'static str,
        entry: u64,
        arg: u64,
    ) -> Option<TaskId> {
        if let Some(ref scheduler) = self.scheduler {
            let task = Box::new(Task::new(name, entry, arg, DEFAULT_STACK_SIZE));
            let id = task.id;
            scheduler.spawn_on_core(core_id, task);
            // Send scheduling IPI to wake the target core
            self.ipi_send(core_id, crate::scheduler::IPI_SCHEDULE_VECTOR);
            log::info!(
                "smp: spawned task {} '{}' on core {}, IPI sent",
                id,
                name,
                core_id
            );
            Some(id)
        } else {
            log::error!("smp: cannot spawn task — scheduler not initialized");
            None
        }
    }

    /// Spawn a task on the least-loaded core.
    pub fn spawn(&self, name: &'static str, entry: u64, arg: u64) -> Option<TaskId> {
        if let Some(ref scheduler) = self.scheduler {
            let task = Box::new(Task::new(name, entry, arg, DEFAULT_STACK_SIZE));
            let id = task.id;
            scheduler.spawn(task);
            log::info!("smp: spawned task {} '{}'", id, name);
            Some(id)
        } else {
            log::error!("smp: cannot spawn task — scheduler not initialized");
            None
        }
    }

    /// Route an IRQ (GSI) to a specific core through the I/O APIC.
    pub fn route_irq(&self, gsi: u32, vector: u8, core_id: u32) {
        if let Some(cpu) = percpu::get_cpu(core_id as usize) {
            log::info!(
                "smp: routing GSI {} -> vector {:#04X} -> core {} (APIC {})",
                gsi,
                vector,
                core_id,
                cpu.apic_id
            );
            self.ioapic_manager
                .route_gsi(gsi, vector, cpu.apic_id as u8);
        } else {
            log::error!(
                "smp: cannot route GSI {} to core {} — core not registered",
                gsi,
                core_id
            );
        }
    }

    /// Acknowledge the current interrupt (send EOI).
    pub fn eoi(&self) {
        self.local_apic.eoi();
    }

    /// Return a reference to the local APIC driver.
    pub fn local_apic(&self) -> &LocalApic {
        &self.local_apic
    }

    /// Return a reference to the I/O APIC manager.
    pub fn ioapic_manager(&self) -> &IoApicManager {
        &self.ioapic_manager
    }

    /// Return a reference to the scheduler (if initialized).
    pub fn scheduler(&self) -> Option<&Scheduler> {
        self.scheduler.as_ref()
    }

    /// Check if SMP is fully initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Request all APs to halt.
    pub fn halt_aps(&self) {
        log::warn!("smp: requesting all APs to halt");
        AP_HALT.store(true, Ordering::Release);
        // Send NMI to all APs to wake them from HLT
        for i in 1..self.num_cores() {
            self.ipi_send(i, 0x02); // NMI
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Approximate spin delay. Not calibrated — just burns cycles.
/// Each iteration is roughly a few nanoseconds on modern x86.
#[inline(never)]
fn spin_delay(iterations: u64) {
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}
