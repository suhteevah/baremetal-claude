//! ACPI table discovery and initialization.
//!
//! Parses ACPI tables from the RSDP address provided by the UEFI bootloader.
//! Discovers CPU cores (MADT), power management (FADT), precision timer (HPET),
//! and PCIe enhanced config space (MCFG). Results are stored in a global
//! `AcpiInfo` struct for use by other kernel subsystems.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use claudio_acpi::{
    AcpiError, AcpiTables, Fadt, Hpet, Madt, MadtEntry, Mcfg, McfgEntry,
    IoApic, LocalApic, InterruptSourceOverride, PowerManager,
};

/// Global flag indicating ACPI was successfully initialized.
static ACPI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Global ACPI information, populated during `init()`.
static ACPI_INFO: Mutex<Option<AcpiInfo>> = Mutex::new(None);

/// Global power manager for shutdown/reboot operations.
static POWER_MANAGER: Mutex<Option<PowerManager>> = Mutex::new(None);

/// Aggregated ACPI hardware discovery results.
#[derive(Debug)]
pub struct AcpiInfo {
    /// Number of enabled CPU cores discovered via MADT.
    pub cpu_count: usize,
    /// Local APIC base address from MADT.
    pub local_apic_address: u64,
    /// All enabled Local APIC entries (one per CPU core).
    pub local_apics: Vec<LocalApic>,
    /// All I/O APIC entries.
    pub io_apics: Vec<IoApic>,
    /// Interrupt source overrides (ISA IRQ remappings).
    pub interrupt_overrides: Vec<InterruptSourceOverride>,
    /// Whether legacy 8259 PICs are present (PCAT_COMPAT flag).
    pub has_legacy_pics: bool,
    /// FADT power management info (SCI interrupt, PM ports, etc.).
    pub fadt_info: Option<FadtInfo>,
    /// HPET precision timer info.
    pub hpet_info: Option<HpetInfo>,
    /// PCIe ECAM entries from MCFG.
    pub mcfg_entries: Vec<McfgEntry>,
}

/// Subset of FADT fields relevant to kernel subsystems.
#[derive(Debug, Clone, Copy)]
pub struct FadtInfo {
    /// SCI interrupt number.
    pub sci_interrupt: u16,
    /// PM1a control block I/O port.
    pub pm1a_cnt_port: u16,
    /// PM1b control block I/O port (0 if not present).
    pub pm1b_cnt_port: u16,
    /// PM timer I/O port.
    pub pm_timer_port: u16,
    /// Whether the PM timer is 32-bit (vs 24-bit).
    pub pm_timer_32bit: bool,
    /// Whether the reset register is supported.
    pub reset_supported: bool,
    /// DSDT physical address.
    pub dsdt_address: Option<u64>,
}

/// HPET timer discovery results.
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    /// MMIO base address for HPET registers.
    pub mmio_base: u64,
    /// Number of comparators (timers).
    pub num_comparators: u8,
    /// Whether the counter is 64-bit.
    pub counter_64bit: bool,
    /// Whether legacy replacement routing is supported.
    pub legacy_replacement: bool,
    /// HPET period in femtoseconds.
    pub period_fs: u32,
}

/// Initialize ACPI tables from the RSDP address provided by the bootloader.
///
/// This function:
/// 1. Finds the RSDP (from UEFI boot info or BIOS memory search)
/// 2. Parses RSDT/XSDT to discover all ACPI tables
/// 3. Parses MADT for CPU cores and I/O APICs
/// 4. Parses FADT for power management registers
/// 5. Parses HPET for precision timing
/// 6. Parses MCFG for PCIe ECAM base addresses
/// 7. Initializes the power manager for shutdown/reboot
///
/// Call this after heap init but before networking.
pub fn init(rsdp_addr: Option<u64>) {
    log::info!("[acpi] ============================================");
    log::info!("[acpi]   ACPI Hardware Discovery");
    log::info!("[acpi] ============================================");

    // Step 1: Find and parse RSDP -> RSDT/XSDT -> table list
    let tables = match find_acpi_tables(rsdp_addr) {
        Ok(t) => t,
        Err(e) => {
            log::error!("[acpi] failed to parse ACPI tables: {:?}", e);
            log::warn!("[acpi] continuing boot without ACPI — hardware discovery limited");
            return;
        }
    };

    let mut info = AcpiInfo {
        cpu_count: 0,
        local_apic_address: 0,
        local_apics: Vec::new(),
        io_apics: Vec::new(),
        interrupt_overrides: Vec::new(),
        has_legacy_pics: false,
        fadt_info: None,
        hpet_info: None,
        mcfg_entries: Vec::new(),
    };

    // Step 2: Parse MADT (Multiple APIC Description Table)
    parse_madt(&tables, &mut info);

    // Step 3: Parse FADT (Fixed ACPI Description Table) + power management
    parse_fadt(&tables, &mut info);

    // Step 4: Parse HPET (High Precision Event Timer)
    parse_hpet(&tables, &mut info);

    // Step 5: Parse MCFG (PCIe Enhanced Configuration)
    parse_mcfg(&tables, &mut info);

    // Summary
    log::info!("[acpi] ============================================");
    log::info!("[acpi]   Discovery Summary");
    log::info!("[acpi] ============================================");
    log::info!("[acpi] CPU cores:       {} enabled", info.cpu_count);
    log::info!("[acpi] Local APIC addr: {:#X}", info.local_apic_address);
    log::info!("[acpi] I/O APICs:       {}", info.io_apics.len());
    for (i, ioapic) in info.io_apics.iter().enumerate() {
        log::info!(
            "[acpi]   I/O APIC #{}: id={} addr={:#X} GSI base={}",
            i, ioapic.io_apic_id, ioapic.io_apic_address, ioapic.global_system_interrupt_base,
        );
    }
    log::info!("[acpi] IRQ overrides:   {}", info.interrupt_overrides.len());
    for iso in &info.interrupt_overrides {
        log::info!(
            "[acpi]   IRQ {} -> GSI {} (flags={:#X})",
            iso.irq_source, iso.global_system_interrupt, iso.flags,
        );
    }
    log::info!("[acpi] Legacy PICs:     {}", info.has_legacy_pics);

    if let Some(ref fadt) = info.fadt_info {
        log::info!("[acpi] FADT: SCI_INT={} PM1a_CNT={:#X} PM_TMR={:#X} reset={}",
            fadt.sci_interrupt, fadt.pm1a_cnt_port, fadt.pm_timer_port, fadt.reset_supported);
    } else {
        log::warn!("[acpi] FADT: not found");
    }

    if let Some(ref hpet) = info.hpet_info {
        log::info!("[acpi] HPET: base={:#X} comparators={} 64bit={} period={}fs",
            hpet.mmio_base, hpet.num_comparators, hpet.counter_64bit, hpet.period_fs);
    } else {
        log::info!("[acpi] HPET: not found");
    }

    log::info!("[acpi] PCIe ECAM entries: {}", info.mcfg_entries.len());
    for (i, entry) in info.mcfg_entries.iter().enumerate() {
        log::info!(
            "[acpi]   ECAM #{}: base={:#X} segment={} bus={}-{}",
            i, entry.base_address, entry.segment_group, entry.start_bus, entry.end_bus,
        );
    }

    log::info!("[acpi] ============================================");

    // Store results globally
    *ACPI_INFO.lock() = Some(info);
    ACPI_INITIALIZED.store(true, Ordering::Release);
    log::info!("[acpi] initialization complete");
}

/// Find ACPI tables from the RSDP. Tries UEFI-provided address first,
/// then falls back to BIOS memory region search.
fn find_acpi_tables(rsdp_addr: Option<u64>) -> Result<AcpiTables, AcpiError> {
    if let Some(addr) = rsdp_addr {
        log::info!("[acpi] RSDP address from bootloader: {:#X}", addr);
        // The bootloader_api provides the physical address of the RSDP.
        // With physical memory mapping enabled, we can read it directly
        // by adding the physical memory offset.
        let phys_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
        let mapped_addr = addr + phys_offset;
        log::info!("[acpi] RSDP mapped address: {:#X} (phys={:#X} + offset={:#X})",
            mapped_addr, addr, phys_offset);
        unsafe { claudio_acpi::init_from_rsdp_addr(mapped_addr) }
    } else {
        log::warn!("[acpi] no RSDP address from bootloader, searching BIOS memory regions");
        // On BIOS systems or if the bootloader didn't provide RSDP,
        // search the standard EBDA and BIOS ROM areas.
        let phys_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
        log::info!("[acpi] physical memory offset: {:#X}", phys_offset);
        // The BIOS memory regions need the physical offset applied too.
        // However, the ACPI crate's search_bios() reads from raw physical addresses.
        // Since we have a physical memory mapping, the addresses 0xE0000-0xFFFFF
        // are mapped at phys_offset + 0xE0000.
        // We'll try the direct BIOS search — the memory is identity-mapped by the
        // bootloader's physical memory mapping.
        unsafe { claudio_acpi::init_from_bios_search() }
    }
}

/// Parse the MADT to discover CPU cores and I/O APICs.
fn parse_madt(tables: &AcpiTables, info: &mut AcpiInfo) {
    let madt_addr = match tables.find_table("APIC") {
        Some(addr) => addr,
        None => {
            log::warn!("[acpi] MADT (APIC) table not found — cannot enumerate CPUs");
            return;
        }
    };

    log::info!("[acpi] parsing MADT at {:#X}", madt_addr);
    let madt = match unsafe { Madt::from_address(madt_addr) } {
        Ok(m) => m,
        Err(e) => {
            log::error!("[acpi] failed to parse MADT: {:?}", e);
            return;
        }
    };

    // Extract Local APIC base address (may be overridden by a 64-bit entry)
    let mut lapic_addr = madt.local_apic_address as u64;
    for entry in &madt.entries {
        if let MadtEntry::LocalApicAddressOverride(ovr) = entry {
            log::info!("[acpi] Local APIC address override: {:#X} -> {:#X}",
                lapic_addr, ovr.local_apic_address);
            lapic_addr = ovr.local_apic_address;
        }
    }

    info.local_apic_address = lapic_addr;
    info.local_apics = madt.local_apics();
    info.io_apics = madt.io_apics();
    info.interrupt_overrides = madt.interrupt_overrides();
    info.has_legacy_pics = madt.has_legacy_pics();
    info.cpu_count = info.local_apics.len();

    log::info!("[acpi] MADT: {} enabled CPUs, {} I/O APICs, LAPIC at {:#X}",
        info.cpu_count, info.io_apics.len(), info.local_apic_address);
}

/// Parse the FADT for power management registers and initialize the PowerManager.
fn parse_fadt(tables: &AcpiTables, info: &mut AcpiInfo) {
    let fadt_addr = match tables.find_table("FACP") {
        Some(addr) => addr,
        None => {
            log::warn!("[acpi] FADT (FACP) table not found — no power management");
            return;
        }
    };

    log::info!("[acpi] parsing FADT at {:#X}", fadt_addr);
    let fadt = match unsafe { Fadt::from_address(fadt_addr) } {
        Ok(f) => f,
        Err(e) => {
            log::error!("[acpi] failed to parse FADT: {:?}", e);
            return;
        }
    };

    let dsdt_address = fadt.dsdt_address();

    info.fadt_info = Some(FadtInfo {
        sci_interrupt: fadt.sci_interrupt,
        pm1a_cnt_port: fadt.pm1a_cnt_port(),
        pm1b_cnt_port: fadt.pm1b_cnt_port(),
        pm_timer_port: fadt.pm_timer_port(),
        pm_timer_32bit: fadt.pm_timer_is_32bit(),
        reset_supported: fadt.reset_supported(),
        dsdt_address,
    });

    // Initialize the power manager for shutdown/reboot
    let mut pm = PowerManager::new(fadt);

    // Try to parse S5 sleep type from DSDT for proper ACPI shutdown
    if let Some(dsdt_addr) = dsdt_address {
        log::info!("[acpi] parsing DSDT at {:#X} for S5 shutdown object", dsdt_addr);
        match unsafe { pm.parse_s5_from_dsdt(dsdt_addr) } {
            Ok(()) => {
                log::info!("[acpi] S5 shutdown sleep types found");
            }
            Err(e) => {
                log::warn!("[acpi] S5 object not found in DSDT: {:?}", e);
                log::warn!("[acpi] ACPI shutdown may not work — will fall back to keyboard controller reset");
            }
        }
    } else {
        log::warn!("[acpi] no DSDT address in FADT — S5 shutdown unavailable");
    }

    // Enable ACPI mode if not already enabled
    log::info!("[acpi] enabling ACPI mode...");
    match unsafe { pm.enable_acpi() } {
        Ok(()) => log::info!("[acpi] ACPI mode enabled"),
        Err(e) => log::warn!("[acpi] failed to enable ACPI mode: {:?}", e),
    }

    // Store the power manager globally
    *POWER_MANAGER.lock() = Some(pm);
}

/// Parse the HPET table and enable the precision timer.
fn parse_hpet(tables: &AcpiTables, info: &mut AcpiInfo) {
    let hpet_addr = match tables.find_table("HPET") {
        Some(addr) => addr,
        None => {
            log::info!("[acpi] HPET table not found — using PIT for timing");
            return;
        }
    };

    log::info!("[acpi] parsing HPET at {:#X}", hpet_addr);
    let hpet = match unsafe { Hpet::from_address(hpet_addr) } {
        Ok(h) => h,
        Err(e) => {
            log::error!("[acpi] failed to parse HPET: {:?}", e);
            return;
        }
    };

    // Read the HPET period from hardware registers
    let period_fs = unsafe { hpet.read_period_fs() };
    if period_fs == 0 {
        log::warn!("[acpi] HPET period is 0 — timer hardware may not be functional");
        return;
    }

    let freq_hz = 1_000_000_000_000_000u64 / period_fs as u64;
    log::info!("[acpi] HPET frequency: {} Hz (period={}fs)", freq_hz, period_fs);

    // Enable the HPET main counter
    unsafe { hpet.enable(); }
    let counter_val = unsafe { hpet.read_counter() };
    log::info!("[acpi] HPET counter running: initial value={}", counter_val);

    info.hpet_info = Some(HpetInfo {
        mmio_base: hpet.mmio_base(),
        num_comparators: hpet.num_comparators(),
        counter_64bit: hpet.counter_64bit,
        legacy_replacement: hpet.legacy_replacement,
        period_fs,
    });
}

/// Parse the MCFG table for PCIe ECAM base addresses.
fn parse_mcfg(tables: &AcpiTables, info: &mut AcpiInfo) {
    let mcfg_addr = match tables.find_table("MCFG") {
        Some(addr) => addr,
        None => {
            log::info!("[acpi] MCFG table not found — using legacy PCI I/O for config space");
            return;
        }
    };

    log::info!("[acpi] parsing MCFG at {:#X}", mcfg_addr);
    let mcfg = match unsafe { Mcfg::from_address(mcfg_addr) } {
        Ok(m) => m,
        Err(e) => {
            log::error!("[acpi] failed to parse MCFG: {:?}", e);
            return;
        }
    };

    info.mcfg_entries = mcfg.entries;
    log::info!("[acpi] MCFG: {} PCIe ECAM entries", info.mcfg_entries.len());
}

// ── Public API for other kernel subsystems ──────────────────────────

/// Returns true if ACPI was successfully initialized.
pub fn is_initialized() -> bool {
    ACPI_INITIALIZED.load(Ordering::Acquire)
}

/// Get a copy of the ACPI discovery info (CPU count, I/O APICs, etc.).
/// Returns `None` if ACPI was not initialized.
pub fn info() -> Option<AcpiInfoSnapshot> {
    let guard = ACPI_INFO.lock();
    let info = guard.as_ref()?;
    Some(AcpiInfoSnapshot {
        cpu_count: info.cpu_count,
        local_apic_address: info.local_apic_address,
        io_apic_count: info.io_apics.len(),
        has_legacy_pics: info.has_legacy_pics,
        fadt_info: info.fadt_info,
        hpet_info: info.hpet_info,
        mcfg_entry_count: info.mcfg_entries.len(),
    })
}

/// Get the MADT data needed for SMP initialization.
///
/// Returns the local APIC address, list of local APICs, and list of I/O APICs
/// in the format expected by `claudio_smp::driver::SmpController`.
///
/// Returns `None` if ACPI was not initialized or MADT was not found.
pub fn madt_for_smp() -> Option<claudio_smp::driver::MadtInfo> {
    let guard = ACPI_INFO.lock();
    let info = guard.as_ref()?;
    if info.local_apics.is_empty() {
        return None;
    }

    let local_apics = info.local_apics.iter().map(|la| {
        claudio_smp::driver::MadtLocalApic {
            processor_id: la.acpi_processor_id,
            apic_id: la.apic_id,
            enabled: la.is_enabled() || la.is_online_capable(),
        }
    }).collect();

    let io_apics = info.io_apics.iter().map(|ia| {
        claudio_smp::driver::MadtIoApic {
            id: ia.io_apic_id,
            address: ia.io_apic_address as u64,
            gsi_base: ia.global_system_interrupt_base,
        }
    }).collect();

    Some(claudio_smp::driver::MadtInfo {
        local_apic_addr: info.local_apic_address,
        local_apics,
        io_apics,
    })
}

/// Lightweight snapshot of ACPI info (no heap allocation needed to read).
#[derive(Debug, Clone, Copy)]
pub struct AcpiInfoSnapshot {
    pub cpu_count: usize,
    pub local_apic_address: u64,
    pub io_apic_count: usize,
    pub has_legacy_pics: bool,
    pub fadt_info: Option<FadtInfo>,
    pub hpet_info: Option<HpetInfo>,
    pub mcfg_entry_count: usize,
}

/// Perform an ACPI shutdown (S5 sleep state). Powers off the machine.
///
/// Falls back to QEMU-specific shutdown port (0x604) if ACPI shutdown
/// is not available.
pub fn shutdown() -> ! {
    log::info!("[acpi] shutdown requested");

    // Try ACPI S5 shutdown first
    {
        let guard = POWER_MANAGER.lock();
        if let Some(ref pm) = *guard {
            if pm.s5_slp_typ_a.is_some() {
                log::info!("[acpi] performing ACPI S5 shutdown...");
                let _ = unsafe { pm.shutdown() };
                // shutdown() is divergent on success; if we get here, it failed
            } else {
                log::warn!("[acpi] S5 sleep type not available, using fallback shutdown");
            }
        } else {
            log::warn!("[acpi] power manager not initialized, using fallback shutdown");
        }
    }

    // Fallback: QEMU shutdown port
    log::info!("[acpi] fallback: writing 0x2000 to port 0x604 (QEMU shutdown)");
    unsafe {
        x86_64::instructions::port::Port::<u16>::new(0x604).write(0x2000);
    }

    // If that didn't work either, halt
    log::error!("[acpi] shutdown failed — halting");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Perform an ACPI reboot via the FADT reset register.
///
/// Falls back to keyboard controller reset (0xFE to port 0x64) if
/// ACPI reboot is not available.
pub fn reboot() -> ! {
    log::info!("[acpi] reboot requested");

    // Try ACPI reset register first
    {
        let guard = POWER_MANAGER.lock();
        if let Some(ref pm) = *guard {
            if pm.fadt.reset_supported() {
                log::info!("[acpi] performing ACPI reboot via reset register...");
                let _ = unsafe { pm.reboot() };
                // reboot() is divergent on success; if we get here, it failed
            } else {
                log::warn!("[acpi] ACPI reset register not supported, using fallback");
            }
        } else {
            log::warn!("[acpi] power manager not initialized, using fallback reboot");
        }
    }

    // Fallback: keyboard controller reset
    log::info!("[acpi] fallback: keyboard controller reset (0xFE to port 0x64)");
    unsafe {
        x86_64::instructions::port::Port::<u8>::new(0x64).write(0xFE);
    }

    // If that didn't work either, halt
    log::error!("[acpi] reboot failed — halting");
    loop {
        x86_64::instructions::hlt();
    }
}
