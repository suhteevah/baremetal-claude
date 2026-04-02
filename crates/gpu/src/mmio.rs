//! GPU MMIO register access.
//!
//! NVIDIA GPUs expose their control registers through BAR0, which is a large
//! MMIO region (typically 16-32 MiB). The register space is divided into
//! functional blocks, each at a fixed offset from BAR0.
//!
//! Register offsets sourced from envytools:
//! https://envytools.readthedocs.io/en/latest/hw/bus/bars.html
//!
//! All accesses are volatile to prevent the compiler from reordering or
//! eliding them.

use core::ptr;

// ===========================================================================
// NV_PMC — Master Control (0x000000)
// Top-level GPU control: chip ID, master enable, interrupt routing.
// ===========================================================================

/// NV_PMC base offset from BAR0.
pub const NV_PMC: u32 = 0x000000;

/// Boot device identification register. Contains chipset ID.
/// Bits [31:20] = chipset ID (e.g., 0x174 for GA104).
pub const NV_PMC_BOOT_0: u32 = NV_PMC + 0x000;

/// Master interrupt status. Each bit corresponds to a GPU engine.
/// Read to determine which engine raised an interrupt.
pub const NV_PMC_INTR_0: u32 = NV_PMC + 0x100;

/// Master interrupt enable. Write 1 to bits to enable interrupts per engine.
pub const NV_PMC_INTR_EN_0: u32 = NV_PMC + 0x140;

/// Master enable register. Bit 0: enable entire GPU.
/// Writing 0 will disable all GPU engines (used during init).
pub const NV_PMC_ENABLE: u32 = NV_PMC + 0x200;

/// Interrupt bitmask: PFIFO engine
pub const NV_PMC_INTR_PFIFO: u32 = 1 << 8;
/// Interrupt bitmask: PGRAPH engine
pub const NV_PMC_INTR_PGRAPH: u32 = 1 << 12;
/// Interrupt bitmask: PTIMER
pub const NV_PMC_INTR_PTIMER: u32 = 1 << 20;

// ===========================================================================
// NV_PBUS — Bus Interface (0x001000)
// PCI/PCIe interface configuration, power management.
// ===========================================================================

/// NV_PBUS base offset from BAR0.
pub const NV_PBUS: u32 = 0x001000;

/// PCI-side interrupt status (mirrors PCI interrupt line).
pub const NV_PBUS_INTR_0: u32 = NV_PBUS + 0x100;

/// PCI-side interrupt enable.
pub const NV_PBUS_INTR_EN_0: u32 = NV_PBUS + 0x140;

// ===========================================================================
// NV_PFIFO — Command Submission FIFO (0x002000)
// Controls how GPU commands are submitted and scheduled.
// ===========================================================================

/// NV_PFIFO base offset from BAR0.
pub const NV_PFIFO: u32 = 0x002000;

/// PFIFO interrupt status.
pub const NV_PFIFO_INTR_0: u32 = NV_PFIFO + 0x100;

/// PFIFO interrupt enable.
pub const NV_PFIFO_INTR_EN_0: u32 = NV_PFIFO + 0x140;

/// PFIFO engine — contains channel table pointers, runlist configuration.
/// On Ampere+, runlist management is at 0x002000 + engine-specific offsets.
pub const NV_PFIFO_RUNLIST_BASE: u32 = NV_PFIFO + 0x2270;

// ===========================================================================
// NV_PTIMER — Timer (0x009000)
// GPU-side timer for timestamps, watchdogs, and synchronization.
// ===========================================================================

/// NV_PTIMER base offset from BAR0.
pub const NV_PTIMER: u32 = 0x009000;

/// Timer low 32 bits (nanoseconds). Read NV_PTIMER_TIME_1 first for atomic 64-bit read.
pub const NV_PTIMER_TIME_0: u32 = NV_PTIMER + 0x400;

/// Timer high 32 bits (nanoseconds).
pub const NV_PTIMER_TIME_1: u32 = NV_PTIMER + 0x404;

/// Timer interrupt status.
pub const NV_PTIMER_INTR_0: u32 = NV_PTIMER + 0x100;

/// Timer interrupt enable.
pub const NV_PTIMER_INTR_EN_0: u32 = NV_PTIMER + 0x140;

// ===========================================================================
// NV_PFB — Framebuffer / Memory Controller (0x100000)
// Controls VRAM, memory timings, partition configuration.
// ===========================================================================

/// NV_PFB base offset from BAR0.
pub const NV_PFB: u32 = 0x100000;

/// Memory configuration register. Contains VRAM type, bus width info.
pub const NV_PFB_CFG0: u32 = NV_PFB + 0x200;

/// VRAM size register (varies by generation).
/// On Ampere: bits [7:0] = log2(size in 128MB units) or similar encoding.
pub const NV_PFB_VRAM_SIZE: u32 = NV_PFB + 0x20C;

/// Number of memory partitions (determines memory bandwidth).
pub const NV_PFB_NPARTS: u32 = NV_PFB + 0x22C;

// ===========================================================================
// NV_PGRAPH — Graphics/Compute Engine (0x400000)
// The main compute and graphics engine. Runs shader programs.
// ===========================================================================

/// NV_PGRAPH base offset from BAR0.
pub const NV_PGRAPH: u32 = 0x400000;

/// PGRAPH interrupt status.
pub const NV_PGRAPH_INTR: u32 = NV_PGRAPH + 0x1100;

/// PGRAPH interrupt enable.
pub const NV_PGRAPH_INTR_EN: u32 = NV_PGRAPH + 0x1140;

/// GPC (Graphics Processing Cluster) count register.
/// RTX 3070 Ti (GA104) has 6 GPCs.
pub const NV_PGRAPH_GPC_COUNT: u32 = NV_PGRAPH + 0x2608;

/// TPC (Texture Processing Cluster) per GPC.
/// Determines the number of CUDA cores per GPC.
pub const NV_PGRAPH_TPC_PER_GPC: u32 = NV_PGRAPH + 0x2614;

/// SM (Streaming Multiprocessor) count — derived from GPC * TPC * SM_PER_TPC.
/// On Ampere, each TPC has 2 SMs. RTX 3070 Ti: 6 GPCs * 8 TPCs * 2 = 48 SMs.

// ===========================================================================
// NV_PDISP — Display Engine (0x610000)
// Controls display output. Not needed for compute but useful for debugging.
// ===========================================================================

/// NV_PDISP base offset from BAR0.
pub const NV_PDISP: u32 = 0x610000;

/// Display interrupt status.
pub const NV_PDISP_INTR_0: u32 = NV_PDISP + 0x020;

// ===========================================================================
// NV_PMU / NV_PPWR — Power Management Unit (0x10a000)
// The PMU is a Falcon microcontroller that manages power states, clocks,
// thermal throttling, and fan control.
// ===========================================================================

/// NV_PMU base offset from BAR0.
pub const NV_PMU: u32 = 0x10a000;

/// PMU Falcon registers start here. See falcon.rs for Falcon register layout.
/// These are relative offsets from NV_PMU.
pub const NV_PMU_FALCON_IRQSRD: u32 = NV_PMU + 0x020;
pub const NV_PMU_FALCON_IRQMSET: u32 = NV_PMU + 0x024;
pub const NV_PMU_FALCON_MAILBOX0: u32 = NV_PMU + 0x040;
pub const NV_PMU_FALCON_MAILBOX1: u32 = NV_PMU + 0x044;

// ===========================================================================
// NV_SEC2 — Security Engine 2 (0x840000, Turing+)
// Another Falcon used for secure boot and firmware authentication.
// ===========================================================================

/// NV_SEC2 base offset from BAR0 (Turing and later).
pub const NV_SEC2: u32 = 0x840000;

// ===========================================================================
// NV_GSP — GPU System Processor (Turing+)
// On Turing and later, NVIDIA moved most driver logic into a proprietary
// firmware blob (GSP-RM) that runs on a RISC-V core inside the GPU.
// Without GSP-RM firmware, Turing+ GPUs cannot be fully initialized.
// ===========================================================================

/// NV_GSP base offset from BAR0.
pub const NV_GSP: u32 = 0x110000;

// ===========================================================================
// Volatile MMIO access helpers
// ===========================================================================

/// GPU MMIO register accessor. Wraps a BAR0 base address and provides
/// volatile read/write access to GPU registers.
#[derive(Debug, Clone, Copy)]
pub struct GpuRegs {
    /// Virtual address of BAR0 mapping.
    bar0: *mut u8,
}

unsafe impl Send for GpuRegs {}
unsafe impl Sync for GpuRegs {}

impl GpuRegs {
    /// Create a new GPU register accessor from a BAR0 virtual address.
    ///
    /// # Safety
    /// The caller must ensure `bar0_vaddr` is a valid mapping of the GPU's BAR0
    /// MMIO region and remains valid for the lifetime of this struct.
    pub unsafe fn new(bar0_vaddr: *mut u8) -> Self {
        log::debug!("gpu/mmio: created register accessor at {:p}", bar0_vaddr);
        Self { bar0: bar0_vaddr }
    }

    /// Read a 32-bit GPU register at the given offset from BAR0.
    #[inline]
    pub fn read32(&self, offset: u32) -> u32 {
        unsafe {
            let addr = self.bar0.add(offset as usize) as *const u32;
            ptr::read_volatile(addr)
        }
    }

    /// Write a 32-bit value to a GPU register at the given offset from BAR0.
    #[inline]
    pub fn write32(&self, offset: u32, value: u32) {
        unsafe {
            let addr = self.bar0.add(offset as usize) as *mut u32;
            ptr::write_volatile(addr, value);
        }
    }

    /// Read a 64-bit GPU register (two consecutive 32-bit reads, low then high).
    #[inline]
    pub fn read64(&self, offset: u32) -> u64 {
        let lo = self.read32(offset) as u64;
        let hi = self.read32(offset + 4) as u64;
        (hi << 32) | lo
    }

    /// Write a 64-bit value to a GPU register (two consecutive 32-bit writes).
    #[inline]
    pub fn write64(&self, offset: u32, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }

    /// Read the GPU chip ID from NV_PMC_BOOT_0.
    /// Returns the chipset ID in bits [31:20].
    pub fn read_chip_id(&self) -> u32 {
        let boot0 = self.read32(NV_PMC_BOOT_0);
        log::debug!("gpu/mmio: NV_PMC_BOOT_0 = 0x{:08X}", boot0);
        (boot0 >> 20) & 0xFFF
    }

    /// Read the GPU timer (64-bit nanosecond counter).
    pub fn read_timer_ns(&self) -> u64 {
        // Read high first, then low, then high again for consistency.
        loop {
            let hi1 = self.read32(NV_PTIMER_TIME_1);
            let lo = self.read32(NV_PTIMER_TIME_0);
            let hi2 = self.read32(NV_PTIMER_TIME_1);
            if hi1 == hi2 {
                return ((hi1 as u64) << 32) | (lo as u64);
            }
        }
    }

    /// Perform a GPU master reset by toggling NV_PMC_ENABLE.
    ///
    /// This is a very disruptive operation — it resets all GPU engines.
    pub fn master_reset(&self) {
        log::warn!("gpu/mmio: performing GPU master reset via NV_PMC_ENABLE");
        let enable = self.read32(NV_PMC_ENABLE);
        log::debug!("gpu/mmio: NV_PMC_ENABLE before reset = 0x{:08X}", enable);

        // Disable all engines
        self.write32(NV_PMC_ENABLE, 0);
        // Small delay — read back to ensure write is flushed
        let _ = self.read32(NV_PMC_ENABLE);

        // Re-enable all engines
        self.write32(NV_PMC_ENABLE, enable);
        let _ = self.read32(NV_PMC_ENABLE);

        log::info!("gpu/mmio: GPU master reset complete");
    }

    /// Disable all GPU interrupts.
    pub fn disable_interrupts(&self) {
        log::debug!("gpu/mmio: disabling all GPU interrupts");
        self.write32(NV_PMC_INTR_EN_0, 0x0000_0000);
    }

    /// Enable interrupts for PFIFO, PGRAPH, and PTIMER.
    pub fn enable_interrupts(&self) {
        let mask = NV_PMC_INTR_PFIFO | NV_PMC_INTR_PGRAPH | NV_PMC_INTR_PTIMER;
        log::debug!("gpu/mmio: enabling interrupts, mask = 0x{:08X}", mask);
        self.write32(NV_PMC_INTR_EN_0, mask);
    }

    /// Read and return pending interrupt status.
    pub fn read_interrupts(&self) -> u32 {
        self.read32(NV_PMC_INTR_0)
    }

    /// Read GPC (Graphics Processing Cluster) count from PGRAPH.
    pub fn gpc_count(&self) -> u32 {
        let count = self.read32(NV_PGRAPH_GPC_COUNT);
        log::debug!("gpu/mmio: GPC count = {}", count);
        count
    }

    /// Read TPC (Texture Processing Cluster) per GPC from PGRAPH.
    pub fn tpc_per_gpc(&self) -> u32 {
        let count = self.read32(NV_PGRAPH_TPC_PER_GPC);
        log::debug!("gpu/mmio: TPC per GPC = {}", count);
        count
    }
}
