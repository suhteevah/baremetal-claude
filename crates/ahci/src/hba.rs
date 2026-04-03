//! HBA (Host Bus Adapter) memory-mapped register definitions.
//!
//! The AHCI spec defines a set of registers at the ABAR (AHCI Base Address
//! Register, PCI BAR5). This module maps those registers as volatile
//! memory-mapped structs with safe read/write accessors.
//!
//! ## Register Space Layout
//!
//! ```text
//! ABAR + 0x000..0x02B  Generic Host Control (CAP, GHC, IS, PI, VS, etc.)
//! ABAR + 0x02C..0x09F  Reserved / vendor-specific
//! ABAR + 0x100..0x17F  Port 0 registers (CLB, FB, IS, IE, CMD, TFD, SIG, SSTS, etc.)
//! ABAR + 0x180..0x1FF  Port 1 registers
//! ...                  (each port is 0x80 bytes)
//! ABAR + 0x100+N*0x80  Port N registers (N = 0..31)
//! ```
//!
//! All reads/writes use `ptr::read_volatile` / `ptr::write_volatile` to prevent
//! the compiler from reordering or eliding MMIO accesses.
//!
//! Reference: AHCI 1.3.1 specification, Section 3 (HBA Memory Registers).

use core::ptr;

// ---------------------------------------------------------------------------
// Generic Host Control register offsets from ABAR
// ---------------------------------------------------------------------------

/// Host Capabilities (read-only). Reports number of ports, command slots, etc.
const CAP_OFFSET: usize = 0x00;
/// Global HBA Control. Bits for reset, interrupt enable, AHCI enable.
const GHC_OFFSET: usize = 0x04;
/// Interrupt Status. One bit per port; set when that port has a pending interrupt.
const IS_OFFSET: usize = 0x08;
/// Ports Implemented. Bitmask of which ports are physically wired.
const PI_OFFSET: usize = 0x0C;
/// AHCI Version (e.g. 0x00010301 = 1.3.1).
const VS_OFFSET: usize = 0x10;
/// Command Completion Coalescing Control.
const CCC_CTL_OFFSET: usize = 0x14;
/// Command Completion Coalescing Ports.
const CCC_PORTS_OFFSET: usize = 0x18;
/// Enclosure Management Location.
const EM_LOC_OFFSET: usize = 0x1C;
/// Enclosure Management Control.
const EM_CTL_OFFSET: usize = 0x20;
/// Host Capabilities Extended.
const CAP2_OFFSET: usize = 0x24;
/// BIOS/OS Handoff Control and Status.
const BOHC_OFFSET: usize = 0x28;

// ---------------------------------------------------------------------------
// GHC (Global HBA Control) bits
// ---------------------------------------------------------------------------

/// HBA Reset. Setting this causes a full controller reset.
pub const GHC_HR: u32 = 1 << 0;
/// Interrupt Enable. Master interrupt enable for all ports.
pub const GHC_IE: u32 = 1 << 1;
/// AHCI Enable. When set, the HBA operates in AHCI mode (vs legacy IDE).
pub const GHC_AE: u32 = 1 << 31;

// ---------------------------------------------------------------------------
// CAP bits of interest
// ---------------------------------------------------------------------------

/// Number of ports (bits 4:0), zero-based. Actual count = field + 1.
pub const CAP_NP_MASK: u32 = 0x1F;
/// Number of command slots (bits 12:8), zero-based.
pub const CAP_NCS_MASK: u32 = 0x1F00;
pub const CAP_NCS_SHIFT: u32 = 8;
/// Supports 64-bit addressing.
pub const CAP_S64A: u32 = 1 << 31;

// ---------------------------------------------------------------------------
// Port register offsets (relative to port base: ABAR + 0x100 + port*0x80)
// ---------------------------------------------------------------------------

/// Command List Base Address (lower 32 bits, 1024-byte aligned).
const PORT_CLB: usize = 0x00;
/// Command List Base Address Upper 32 bits.
const PORT_CLBU: usize = 0x04;
/// FIS Base Address (lower 32 bits, 256-byte aligned).
const PORT_FB: usize = 0x08;
/// FIS Base Address Upper 32 bits.
const PORT_FBU: usize = 0x0C;
/// Port Interrupt Status.
const PORT_IS: usize = 0x10;
/// Port Interrupt Enable.
const PORT_IE: usize = 0x14;
/// Port Command and Status.
const PORT_CMD: usize = 0x18;
/// _Reserved_
const _PORT_RSV: usize = 0x1C;
/// Port Task File Data.
const PORT_TFD: usize = 0x20;
/// Port Signature.
const PORT_SIG: usize = 0x24;
/// Port SATA Status (SCR0: SStatus).
const PORT_SSTS: usize = 0x28;
/// Port SATA Control (SCR2: SControl).
const PORT_SCTL: usize = 0x2C;
/// Port SATA Error (SCR1: SError).
const PORT_SERR: usize = 0x30;
/// Port SATA Active (used with NCQ).
const PORT_SACT: usize = 0x34;
/// Port Command Issue. Writing a bit issues the corresponding command slot.
const PORT_CI: usize = 0x38;
/// Port SATA Notification.
const PORT_SNTF: usize = 0x3C;
/// Port FIS-based Switching Control.
const PORT_FBS: usize = 0x40;

// ---------------------------------------------------------------------------
// Port CMD bits
// ---------------------------------------------------------------------------

/// Start. When set, the HBA may process the command list.
pub const PORT_CMD_ST: u32 = 1 << 0;
/// Spin-Up Device (for staggered spin-up).
pub const PORT_CMD_SUD: u32 = 1 << 1;
/// Power On Device.
pub const PORT_CMD_POD: u32 = 1 << 2;
/// FIS Receive Enable. Must be set before setting ST.
pub const PORT_CMD_FRE: u32 = 1 << 4;
/// FIS Receive Running (read-only). Indicates FIS engine is running.
pub const PORT_CMD_FR: u32 = 1 << 14;
/// Command List Running (read-only). Indicates command engine is running.
pub const PORT_CMD_CR: u32 = 1 << 15;
/// Current command slot (bits 12:8, read-only).
pub const PORT_CMD_CCS_MASK: u32 = 0x1F00;
pub const PORT_CMD_CCS_SHIFT: u32 = 8;
/// Interface Communication Control (bits 31:28).
pub const PORT_CMD_ICC_MASK: u32 = 0xF000_0000;
/// ICC value: Active (transition to active).
pub const PORT_CMD_ICC_ACTIVE: u32 = 0x1000_0000;

// ---------------------------------------------------------------------------
// Port SSTS (SStatus) fields
// ---------------------------------------------------------------------------

/// Device Detection (bits 3:0).
pub const SSTS_DET_MASK: u32 = 0x0F;
/// Device present and PHY communication established.
pub const SSTS_DET_PRESENT: u32 = 0x03;
/// Interface Power Management (bits 11:8).
pub const SSTS_IPM_MASK: u32 = 0x0F00;
/// Interface in active state.
pub const SSTS_IPM_ACTIVE: u32 = 0x0100;

// ---------------------------------------------------------------------------
// Port SCTL (SControl) fields
// ---------------------------------------------------------------------------

/// Device detection initialization (bits 3:0).
pub const SCTL_DET_MASK: u32 = 0x0F;
/// Perform COMRESET / interface initialization sequence.
pub const SCTL_DET_COMRESET: u32 = 0x01;
/// No device detection / initialization action requested.
pub const SCTL_DET_NONE: u32 = 0x00;

// ---------------------------------------------------------------------------
// Port TFD (Task File Data) bits
// ---------------------------------------------------------------------------

/// Status: BSY (busy).
pub const TFD_STS_BSY: u32 = 1 << 7;
/// Status: DRQ (data request).
pub const TFD_STS_DRQ: u32 = 1 << 3;
/// Status: ERR (error).
pub const TFD_STS_ERR: u32 = 1 << 0;

// ---------------------------------------------------------------------------
// Port Interrupt Status bits
// ---------------------------------------------------------------------------

/// Device to Host Register FIS Interrupt.
pub const PORT_IS_DHRS: u32 = 1 << 0;
/// PIO Setup FIS Interrupt.
pub const PORT_IS_PSS: u32 = 1 << 1;
/// DMA Setup FIS Interrupt.
pub const PORT_IS_DSS: u32 = 1 << 2;
/// Set Device Bits Interrupt.
pub const PORT_IS_SDBS: u32 = 1 << 3;
/// Unknown FIS Interrupt.
pub const PORT_IS_UFS: u32 = 1 << 4;
/// Descriptor Processed.
pub const PORT_IS_DPS: u32 = 1 << 5;
/// Port Connect Change Status.
pub const PORT_IS_PCS: u32 = 1 << 6;
/// Device Mechanical Presence.
pub const PORT_IS_DMPS: u32 = 1 << 7;
/// PhyRdy Change Status.
pub const PORT_IS_PRCS: u32 = 1 << 22;
/// Incorrect Port Multiplier Status.
pub const PORT_IS_IPMS: u32 = 1 << 23;
/// Overflow Status.
pub const PORT_IS_OFS: u32 = 1 << 24;
/// Interface Non-fatal Error Status.
pub const PORT_IS_INFS: u32 = 1 << 26;
/// Interface Fatal Error Status.
pub const PORT_IS_IFS: u32 = 1 << 27;
/// Host Bus Data Error Status.
pub const PORT_IS_HBDS: u32 = 1 << 28;
/// Host Bus Fatal Error Status.
pub const PORT_IS_HBFS: u32 = 1 << 29;
/// Task File Error Status.
pub const PORT_IS_TFES: u32 = 1 << 30;
/// Cold Port Detect Status.
pub const PORT_IS_CPDS: u32 = 1 << 31;

// ---------------------------------------------------------------------------
// Port signature values
// ---------------------------------------------------------------------------

/// SATA drive (ATA device).
pub const SATA_SIG_ATA: u32 = 0x0000_0101;
/// SATAPI device (ATAPI, e.g. optical drive).
pub const SATA_SIG_ATAPI: u32 = 0xEB14_0101;
/// Enclosure management bridge.
pub const SATA_SIG_SEMB: u32 = 0xC33C_0101;
/// Port multiplier.
pub const SATA_SIG_PM: u32 = 0x9669_0101;

// ===========================================================================
// HBA memory-mapped register access
// ===========================================================================

/// Handle to the AHCI HBA memory-mapped registers.
///
/// All reads and writes go through volatile operations to prevent the compiler
/// from reordering or eliding MMIO accesses.
#[derive(Debug)]
pub struct HbaRegs {
    base: usize,
}

impl HbaRegs {
    /// Create an HBA register handle from a physical/virtual base address.
    ///
    /// # Safety
    ///
    /// `phys_addr` must point to a valid, identity-mapped AHCI ABAR region.
    /// ClaudioOS uses identity mapping for all MMIO, so the physical address
    /// *is* the virtual address.
    pub unsafe fn from_base_addr(phys_addr: u64) -> Self {
        log::info!("[ahci] HBA registers at {:#x}", phys_addr);
        // SAFETY: Caller guarantees phys_addr is a valid, identity-mapped ABAR.
        Self {
            base: phys_addr as usize,
        }
    }

    // -----------------------------------------------------------------------
    // Generic Host Control registers
    // -----------------------------------------------------------------------

    /// Read Host Capabilities register.
    pub fn read_cap(&self) -> u32 {
        self.read32(CAP_OFFSET)
    }

    /// Read Global HBA Control register.
    pub fn read_ghc(&self) -> u32 {
        self.read32(GHC_OFFSET)
    }

    /// Write Global HBA Control register.
    pub fn write_ghc(&self, val: u32) {
        self.write32(GHC_OFFSET, val);
    }

    /// Read Interrupt Status (global, one bit per port).
    pub fn read_is(&self) -> u32 {
        self.read32(IS_OFFSET)
    }

    /// Write (clear) Interrupt Status.
    pub fn write_is(&self, val: u32) {
        self.write32(IS_OFFSET, val);
    }

    /// Read Ports Implemented bitmask.
    pub fn read_pi(&self) -> u32 {
        self.read32(PI_OFFSET)
    }

    /// Read AHCI Version.
    pub fn read_vs(&self) -> u32 {
        self.read32(VS_OFFSET)
    }

    /// Read Command Completion Coalescing Control.
    pub fn read_ccc_ctl(&self) -> u32 {
        self.read32(CCC_CTL_OFFSET)
    }

    /// Read Command Completion Coalescing Ports.
    pub fn read_ccc_ports(&self) -> u32 {
        self.read32(CCC_PORTS_OFFSET)
    }

    /// Read Enclosure Management Location.
    pub fn read_em_loc(&self) -> u32 {
        self.read32(EM_LOC_OFFSET)
    }

    /// Read Enclosure Management Control.
    pub fn read_em_ctl(&self) -> u32 {
        self.read32(EM_CTL_OFFSET)
    }

    /// Read Host Capabilities Extended.
    pub fn read_cap2(&self) -> u32 {
        self.read32(CAP2_OFFSET)
    }

    /// Read BIOS/OS Handoff Control and Status.
    pub fn read_bohc(&self) -> u32 {
        self.read32(BOHC_OFFSET)
    }

    /// Write BIOS/OS Handoff Control and Status.
    pub fn write_bohc(&self, val: u32) {
        self.write32(BOHC_OFFSET, val);
    }

    // -----------------------------------------------------------------------
    // Derived helpers
    // -----------------------------------------------------------------------

    /// Number of ports the HBA supports (1..32).
    pub fn num_ports(&self) -> u32 {
        (self.read_cap() & CAP_NP_MASK) + 1
    }

    /// Number of command slots per port (1..32).
    pub fn num_cmd_slots(&self) -> u32 {
        ((self.read_cap() & CAP_NCS_MASK) >> CAP_NCS_SHIFT) + 1
    }

    /// Whether the HBA supports 64-bit addressing.
    pub fn supports_64bit(&self) -> bool {
        self.read_cap() & CAP_S64A != 0
    }

    /// Format the AHCI version as (major, minor).
    pub fn version(&self) -> (u16, u16) {
        let vs = self.read_vs();
        ((vs >> 16) as u16, vs as u16)
    }

    // -----------------------------------------------------------------------
    // Port register access
    // -----------------------------------------------------------------------

    /// Compute the base offset for a given port (0..31).
    ///
    /// Per AHCI spec, port registers start at ABAR + 0x100 and each port
    /// occupies 0x80 (128) bytes. So port N is at offset 0x100 + N * 0x80.
    fn port_offset(&self, port: u32) -> usize {
        0x100 + (port as usize) * 0x80
    }

    /// Read a port register.
    pub fn port_read(&self, port: u32, reg_offset: usize) -> u32 {
        self.read32(self.port_offset(port) + reg_offset)
    }

    /// Write a port register.
    pub fn port_write(&self, port: u32, reg_offset: usize, val: u32) {
        self.write32(self.port_offset(port) + reg_offset, val);
    }

    // -- Convenience port register accessors --

    pub fn port_read_clb(&self, port: u32) -> u32 {
        self.port_read(port, PORT_CLB)
    }
    pub fn port_write_clb(&self, port: u32, val: u32) {
        self.port_write(port, PORT_CLB, val);
    }
    pub fn port_read_clbu(&self, port: u32) -> u32 {
        self.port_read(port, PORT_CLBU)
    }
    pub fn port_write_clbu(&self, port: u32, val: u32) {
        self.port_write(port, PORT_CLBU, val);
    }
    pub fn port_read_fb(&self, port: u32) -> u32 {
        self.port_read(port, PORT_FB)
    }
    pub fn port_write_fb(&self, port: u32, val: u32) {
        self.port_write(port, PORT_FB, val);
    }
    pub fn port_read_fbu(&self, port: u32) -> u32 {
        self.port_read(port, PORT_FBU)
    }
    pub fn port_write_fbu(&self, port: u32, val: u32) {
        self.port_write(port, PORT_FBU, val);
    }
    pub fn port_read_is(&self, port: u32) -> u32 {
        self.port_read(port, PORT_IS)
    }
    pub fn port_write_is(&self, port: u32, val: u32) {
        self.port_write(port, PORT_IS, val);
    }
    pub fn port_read_ie(&self, port: u32) -> u32 {
        self.port_read(port, PORT_IE)
    }
    pub fn port_write_ie(&self, port: u32, val: u32) {
        self.port_write(port, PORT_IE, val);
    }
    pub fn port_read_cmd(&self, port: u32) -> u32 {
        self.port_read(port, PORT_CMD)
    }
    pub fn port_write_cmd(&self, port: u32, val: u32) {
        self.port_write(port, PORT_CMD, val);
    }
    pub fn port_read_tfd(&self, port: u32) -> u32 {
        self.port_read(port, PORT_TFD)
    }
    pub fn port_read_sig(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SIG)
    }
    pub fn port_read_ssts(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SSTS)
    }
    pub fn port_read_sctl(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SCTL)
    }
    pub fn port_write_sctl(&self, port: u32, val: u32) {
        self.port_write(port, PORT_SCTL, val);
    }
    pub fn port_read_serr(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SERR)
    }
    pub fn port_write_serr(&self, port: u32, val: u32) {
        self.port_write(port, PORT_SERR, val);
    }
    pub fn port_read_sact(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SACT)
    }
    pub fn port_write_sact(&self, port: u32, val: u32) {
        self.port_write(port, PORT_SACT, val);
    }
    pub fn port_read_ci(&self, port: u32) -> u32 {
        self.port_read(port, PORT_CI)
    }
    pub fn port_write_ci(&self, port: u32, val: u32) {
        self.port_write(port, PORT_CI, val);
    }
    pub fn port_read_sntf(&self, port: u32) -> u32 {
        self.port_read(port, PORT_SNTF)
    }
    pub fn port_write_sntf(&self, port: u32, val: u32) {
        self.port_write(port, PORT_SNTF, val);
    }
    pub fn port_read_fbs(&self, port: u32) -> u32 {
        self.port_read(port, PORT_FBS)
    }
    pub fn port_write_fbs(&self, port: u32, val: u32) {
        self.port_write(port, PORT_FBS, val);
    }

    // -----------------------------------------------------------------------
    // Low-level volatile MMIO
    // -----------------------------------------------------------------------

    /// Read a 32-bit MMIO register at the given byte offset from ABAR.
    ///
    /// # Safety
    /// Uses volatile read to ensure the compiler does not optimize away or
    /// reorder this access. The ABAR base was validated at construction time.
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: self.base is a valid ABAR address (caller-guaranteed at construction).
        // Volatile read is required for MMIO -- hardware state may change between reads.
        unsafe { ptr::read_volatile((self.base + offset) as *const u32) }
    }

    /// Write a 32-bit MMIO register at the given byte offset from ABAR.
    ///
    /// # Safety
    /// Uses volatile write to ensure the compiler does not elide or reorder this
    /// access. Some registers are write-1-to-clear (W1C) -- the caller must be
    /// aware of register semantics.
    fn write32(&self, offset: usize, val: u32) {
        // SAFETY: self.base is a valid ABAR address. Volatile write ensures the
        // hardware sees this store immediately and in program order.
        unsafe { ptr::write_volatile((self.base + offset) as *mut u32, val) }
    }
}
