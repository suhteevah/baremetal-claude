//! SMP initialization for ClaudioOS.
//!
//! Boots all application processors (APs) discovered via the ACPI MADT table.
//! After init, all CPU cores are running and idle, waiting for work via IPI.
//!
//! Call sequence:
//!   1. `init()` — read MADT from acpi_init, configure BSP APIC, boot APs
//!   2. `spawn_agent_on_core()` — dispatch agent work to a specific core
//!   3. `spawn_agent()` — dispatch to least-loaded core

use claudio_smp::driver::SmpController;
use claudio_smp::SpinLock;

/// Global SMP controller, initialized by `init()`.
static SMP_CONTROLLER: SpinLock<Option<SmpController>> = SpinLock::new(None);

/// Initialize SMP: read ACPI MADT data, configure APIC, boot all APs.
///
/// After this returns, all discovered CPU cores are running and in an
/// idle halt loop, ready to receive work via IPI.
///
/// Must be called after `acpi_init::init()` has completed.
pub fn init() {
    log::info!("[smp] === SMP INITIALIZATION BEGIN ===");

    // ── Step 1: Get MADT data from the ACPI subsystem ───────────────────

    if !crate::acpi_init::is_initialized() {
        log::error!("[smp] ACPI not initialized — SMP disabled");
        return;
    }

    let madt_info = match crate::acpi_init::madt_for_smp() {
        Some(info) => info,
        None => {
            log::error!("[smp] no MADT data available from ACPI — SMP disabled");
            return;
        }
    };

    log::info!(
        "[smp] MADT: {} local APICs, {} I/O APICs, LAPIC addr={:#X}",
        madt_info.local_apics.len(),
        madt_info.io_apics.len(),
        madt_info.local_apic_addr,
    );

    for la in &madt_info.local_apics {
        log::info!(
            "[smp]   CPU: processor_id={} apic_id={} enabled={}",
            la.processor_id,
            la.apic_id,
            la.enabled,
        );
    }

    for ia in &madt_info.io_apics {
        log::info!(
            "[smp]   I/O APIC: id={} addr={:#X} gsi_base={}",
            ia.id,
            ia.address,
            ia.gsi_base,
        );
    }

    // ── Step 2: Ensure trampoline memory is accessible ──────────────────
    //
    // The AP trampoline must reside at physical address 0x8000 (below 1 MiB).
    // With the bootloader's identity mapping of low memory, this should
    // already be accessible. Verify with a test write.
    log::info!("[smp] verifying trampoline page at phys {:#X}",
        claudio_smp::trampoline::TRAMPOLINE_PHYS);
    unsafe {
        let trampoline_ptr = claudio_smp::trampoline::TRAMPOLINE_PHYS as *mut u8;
        // Verify the page is writable by doing a test write
        core::ptr::write_volatile(trampoline_ptr, 0xAA);
        let readback = core::ptr::read_volatile(trampoline_ptr);
        if readback != 0xAA {
            log::error!(
                "[smp] trampoline page at {:#X} not writable (read {:#X}) — SMP disabled",
                claudio_smp::trampoline::TRAMPOLINE_PHYS,
                readback,
            );
            return;
        }
        // Clear the test byte
        core::ptr::write_volatile(trampoline_ptr, 0x00);
        log::info!("[smp] trampoline page verified writable");
    }

    // ── Step 3: Disable legacy 8259 PIC ─────────────────────────────────
    //
    // The PIC was initialized in interrupts::init() for single-core boot.
    // Now that we're switching to APIC mode, mask all PIC IRQs to prevent
    // conflicts. The I/O APIC will take over interrupt routing.
    disable_legacy_pic();

    // ── Step 4: Create SMP controller and boot all APs ──────────────────

    let apic_base = madt_info.local_apic_addr;
    let mut controller = SmpController::new(apic_base);

    // Run the full SMP init: BSP APIC setup, I/O APIC config, AP boot
    controller.init(madt_info);

    let total_cores = controller.num_cores();
    let aps_booted = controller.ap_count();

    log::info!(
        "[smp] Booted {} application processors ({} total cores)",
        aps_booted,
        total_cores,
    );

    // ── Step 5: Store controller globally ───────────────────────────────

    *SMP_CONTROLLER.lock() = Some(controller);

    log::info!("[smp] === SMP INITIALIZATION COMPLETE ===");
    log::info!(
        "[smp] {} cores running and ready for work",
        total_cores,
    );
}

/// Disable the legacy 8259 PIC by masking all IRQs.
///
/// This is necessary before enabling the Local APIC and I/O APIC, as
/// both interrupt controllers must not be active simultaneously.
fn disable_legacy_pic() {
    log::info!("[smp] disabling legacy 8259 PIC");
    unsafe {
        // Mask all IRQs on PIC1 (master) and PIC2 (slave)
        let mut pic1_data = x86_64::instructions::port::Port::<u8>::new(0x21);
        let mut pic2_data = x86_64::instructions::port::Port::<u8>::new(0xA1);
        pic1_data.write(0xFF);
        pic2_data.write(0xFF);
    }
    log::info!("[smp] legacy PIC disabled (all IRQs masked)");
}

// ---------------------------------------------------------------------------
// Public API for the rest of the kernel
// ---------------------------------------------------------------------------

/// Return the total number of active CPU cores (BSP + APs).
///
/// Returns 1 if SMP was not initialized.
pub fn num_cores() -> u32 {
    match SMP_CONTROLLER.lock().as_ref() {
        Some(ctrl) => ctrl.num_cores(),
        None => 1,
    }
}

/// Spawn an agent task on a specific core.
///
/// `core_id` — target CPU core (0 = BSP, 1..N = APs).
/// `name` — human-readable label for logging.
/// `entry` — function pointer: `extern "C" fn(arg: u64)`.
/// `arg` — argument passed to the entry function.
///
/// Returns the task ID if successful.
pub fn spawn_agent_on_core(
    core_id: u32,
    name: &'static str,
    entry: u64,
    arg: u64,
) -> Option<claudio_smp::TaskId> {
    let guard = SMP_CONTROLLER.lock();
    match guard.as_ref() {
        Some(ctrl) => {
            log::info!(
                "[smp] spawning agent '{}' on core {} (entry={:#X})",
                name,
                core_id,
                entry,
            );
            ctrl.spawn_on_core(core_id, name, entry, arg)
        }
        None => {
            log::error!("[smp] cannot spawn agent — SMP not initialized");
            None
        }
    }
}

/// Spawn a task on the least-loaded core.
///
/// `name` — human-readable label for logging.
/// `entry` — function pointer: `extern "C" fn(arg: u64)`.
/// `arg` — argument passed to the entry function.
///
/// Returns the task ID if successful.
pub fn spawn_agent(
    name: &'static str,
    entry: u64,
    arg: u64,
) -> Option<claudio_smp::TaskId> {
    let guard = SMP_CONTROLLER.lock();
    match guard.as_ref() {
        Some(ctrl) => {
            log::info!(
                "[smp] spawning agent '{}' on least-loaded core (entry={:#X})",
                name,
                entry,
            );
            ctrl.spawn(name, entry, arg)
        }
        None => {
            log::error!("[smp] cannot spawn agent — SMP not initialized");
            None
        }
    }
}

/// Check if SMP has been initialized.
pub fn is_initialized() -> bool {
    match SMP_CONTROLLER.lock().as_ref() {
        Some(ctrl) => ctrl.is_initialized(),
        None => false,
    }
}

/// Send EOI (End-Of-Interrupt) via the local APIC.
///
/// Call this from interrupt handlers when running in APIC mode.
pub fn apic_eoi() {
    let guard = SMP_CONTROLLER.lock();
    if let Some(ctrl) = guard.as_ref() {
        ctrl.eoi();
    }
}
