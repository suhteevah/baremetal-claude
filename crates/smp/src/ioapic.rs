//! I/O APIC driver for ClaudioOS.
//!
//! Implements indirect register access and redirection table management per
//! Intel 82093AA I/O APIC datasheet and Intel SDM Volume 3, Chapter 10.

use alloc::vec::Vec;
use core::ptr;

// ---------------------------------------------------------------------------
// I/O APIC register access (indirect via IOREGSEL / IOWIN)
// ---------------------------------------------------------------------------

/// Offset of IOREGSEL (I/O Register Select) from I/O APIC base.
const IOREGSEL: u32 = 0x00;
/// Offset of IOWIN (I/O Window) from I/O APIC base.
const IOWIN: u32 = 0x10;

// ---------------------------------------------------------------------------
// I/O APIC register indices (written to IOREGSEL)
// ---------------------------------------------------------------------------

/// I/O APIC ID register.
const REG_IOAPICID: u8 = 0x00;
/// I/O APIC Version register.
const REG_IOAPICVER: u8 = 0x01;
/// I/O APIC Arbitration register.
const REG_IOAPICARB: u8 = 0x02;
/// Base index for redirection table entries (each entry is 2 x 32-bit).
const REG_REDTBL_BASE: u8 = 0x10;

// ---------------------------------------------------------------------------
// Redirection entry fields
// ---------------------------------------------------------------------------

/// Delivery mode: Fixed.
pub const DELMODE_FIXED: u64 = 0b000 << 8;
/// Delivery mode: Lowest Priority.
pub const DELMODE_LOWEST: u64 = 0b001 << 8;
/// Delivery mode: SMI.
pub const DELMODE_SMI: u64 = 0b010 << 8;
/// Delivery mode: NMI.
pub const DELMODE_NMI: u64 = 0b100 << 8;
/// Delivery mode: INIT.
pub const DELMODE_INIT: u64 = 0b101 << 8;
/// Delivery mode: ExtINT.
pub const DELMODE_EXTINT: u64 = 0b111 << 8;

/// Destination mode: Physical.
pub const DESTMODE_PHYSICAL: u64 = 0 << 11;
/// Destination mode: Logical.
pub const DESTMODE_LOGICAL: u64 = 1 << 11;

/// Polarity: Active high.
pub const POLARITY_HIGH: u64 = 0 << 13;
/// Polarity: Active low.
pub const POLARITY_LOW: u64 = 1 << 13;

/// Trigger mode: Edge.
pub const TRIGGER_EDGE: u64 = 0 << 15;
/// Trigger mode: Level.
pub const TRIGGER_LEVEL: u64 = 1 << 15;

/// Interrupt mask bit.
pub const MASKED: u64 = 1 << 16;

// ---------------------------------------------------------------------------
// Redirection entry
// ---------------------------------------------------------------------------

/// A 64-bit I/O APIC redirection table entry.
#[derive(Debug, Clone, Copy)]
pub struct RedirectionEntry {
    /// Raw 64-bit value.
    pub raw: u64,
}

impl RedirectionEntry {
    /// Create a new redirection entry with all fields specified.
    pub fn new(
        vector: u8,
        delivery_mode: u64,
        dest_mode: u64,
        polarity: u64,
        trigger_mode: u64,
        masked: bool,
        destination: u8,
    ) -> Self {
        let mut raw = (vector as u64)
            | delivery_mode
            | dest_mode
            | polarity
            | trigger_mode
            | ((destination as u64) << 56);
        if masked {
            raw |= MASKED;
        }
        log::trace!(
            "ioapic: created redir entry: vec={:#04X} dest={} masked={} raw={:#018X}",
            vector,
            destination,
            masked,
            raw
        );
        Self { raw }
    }

    /// Create a simple edge-triggered, fixed-delivery, physical-destination entry.
    pub fn simple(vector: u8, dest_apic_id: u8) -> Self {
        Self::new(
            vector,
            DELMODE_FIXED,
            DESTMODE_PHYSICAL,
            POLARITY_HIGH,
            TRIGGER_EDGE,
            false,
            dest_apic_id,
        )
    }

    /// Create a masked (disabled) entry.
    pub fn masked() -> Self {
        Self { raw: MASKED }
    }

    /// Get the vector field.
    pub fn vector(&self) -> u8 {
        (self.raw & 0xFF) as u8
    }

    /// Check if the entry is masked.
    pub fn is_masked(&self) -> bool {
        (self.raw & MASKED) != 0
    }

    /// Set the mask bit.
    pub fn set_masked(&mut self, mask: bool) {
        if mask {
            self.raw |= MASKED;
        } else {
            self.raw &= !MASKED;
        }
    }

    /// Get the destination field (bits 56..63).
    pub fn destination(&self) -> u8 {
        ((self.raw >> 56) & 0xFF) as u8
    }
}

// ---------------------------------------------------------------------------
// Single I/O APIC instance
// ---------------------------------------------------------------------------

/// Driver for a single I/O APIC.
pub struct IoApic {
    /// Base virtual address of this I/O APIC's MMIO registers.
    base_addr: u64,
    /// Global System Interrupt base for this I/O APIC.
    gsi_base: u32,
    /// Number of redirection entries (from IOAPICVER).
    max_entries: u8,
}

impl IoApic {
    /// Create a new I/O APIC driver for the given MMIO base address.
    ///
    /// `base_vaddr` — virtual address of the I/O APIC registers.
    /// `gsi_base` — the Global System Interrupt number of the first pin.
    pub fn new(base_vaddr: u64, gsi_base: u32) -> Self {
        log::info!(
            "ioapic: creating IoApic driver at base {:#X}, GSI base {}",
            base_vaddr,
            gsi_base
        );
        let mut ioapic = Self {
            base_addr: base_vaddr,
            gsi_base,
            max_entries: 0,
        };
        // Read version to determine max redirection entries
        let ver = ioapic.read_version();
        ioapic.max_entries = ((ver >> 16) & 0xFF) as u8 + 1;
        log::info!(
            "ioapic: version={:#010X}, max_entries={}",
            ver,
            ioapic.max_entries
        );
        ioapic
    }

    // -----------------------------------------------------------------------
    // Indirect register access
    // -----------------------------------------------------------------------

    /// Select a register index via IOREGSEL and read from IOWIN.
    fn read_reg(&self, index: u8) -> u32 {
        unsafe {
            ptr::write_volatile((self.base_addr + IOREGSEL as u64) as *mut u32, index as u32);
            ptr::read_volatile((self.base_addr + IOWIN as u64) as *const u32)
        }
    }

    /// Select a register index via IOREGSEL and write to IOWIN.
    fn write_reg(&self, index: u8, value: u32) {
        unsafe {
            ptr::write_volatile((self.base_addr + IOREGSEL as u64) as *mut u32, index as u32);
            ptr::write_volatile((self.base_addr + IOWIN as u64) as *mut u32, value);
        }
    }

    // -----------------------------------------------------------------------
    // Standard registers
    // -----------------------------------------------------------------------

    /// Read the IOAPICID register.
    pub fn read_id(&self) -> u32 {
        let id = self.read_reg(REG_IOAPICID);
        log::trace!("ioapic: IOAPICID = {:#010X} (id={})", id, (id >> 24) & 0xF);
        id
    }

    /// Read the IOAPICVER register.
    pub fn read_version(&self) -> u32 {
        let ver = self.read_reg(REG_IOAPICVER);
        log::trace!("ioapic: IOAPICVER = {:#010X}", ver);
        ver
    }

    /// Read the IOAPICARB register.
    pub fn read_arbitration(&self) -> u32 {
        let arb = self.read_reg(REG_IOAPICARB);
        log::trace!("ioapic: IOAPICARB = {:#010X}", arb);
        arb
    }

    /// Return the number of supported redirection entries.
    pub fn max_entries(&self) -> u8 {
        self.max_entries
    }

    /// Return the GSI base for this I/O APIC.
    pub fn gsi_base(&self) -> u32 {
        self.gsi_base
    }

    // -----------------------------------------------------------------------
    // Redirection table access
    // -----------------------------------------------------------------------

    /// Read a 64-bit redirection entry by IRQ pin index (0-based).
    pub fn read_redirection(&self, irq: u8) -> RedirectionEntry {
        assert!(
            irq < self.max_entries,
            "ioapic: IRQ {} out of range (max {})",
            irq,
            self.max_entries
        );
        let reg_base = REG_REDTBL_BASE + irq * 2;
        let low = self.read_reg(reg_base);
        let high = self.read_reg(reg_base + 1);
        let raw = (low as u64) | ((high as u64) << 32);
        log::trace!(
            "ioapic: read redir[{}] = {:#018X} (vec={:#04X} masked={})",
            irq,
            raw,
            raw & 0xFF,
            (raw & MASKED) != 0
        );
        RedirectionEntry { raw }
    }

    /// Write a 64-bit redirection entry by IRQ pin index (0-based).
    pub fn write_redirection(&self, irq: u8, entry: RedirectionEntry) {
        assert!(
            irq < self.max_entries,
            "ioapic: IRQ {} out of range (max {})",
            irq,
            self.max_entries
        );
        log::debug!(
            "ioapic: write redir[{}] = {:#018X} (vec={:#04X} dest={} masked={})",
            irq,
            entry.raw,
            entry.vector(),
            entry.destination(),
            entry.is_masked()
        );
        let reg_base = REG_REDTBL_BASE + irq * 2;
        self.write_reg(reg_base, entry.raw as u32);
        self.write_reg(reg_base + 1, (entry.raw >> 32) as u32);
    }

    /// Route a GSI (Global System Interrupt) to a specific CPU core.
    ///
    /// `gsi` — the global system interrupt number.
    /// `vector` — interrupt vector to deliver.
    /// `dest_apic_id` — target APIC ID.
    pub fn route_irq(&self, gsi: u32, vector: u8, dest_apic_id: u8) {
        let pin = (gsi - self.gsi_base) as u8;
        log::info!(
            "ioapic: routing GSI {} (pin {}) -> vector {:#04X}, dest APIC {}",
            gsi,
            pin,
            vector,
            dest_apic_id
        );
        let entry = RedirectionEntry::simple(vector, dest_apic_id);
        self.write_redirection(pin, entry);
    }

    /// Mask (disable) an IRQ pin.
    pub fn mask_irq(&self, irq: u8) {
        log::debug!("ioapic: masking IRQ {}", irq);
        let mut entry = self.read_redirection(irq);
        entry.set_masked(true);
        self.write_redirection(irq, entry);
    }

    /// Unmask (enable) an IRQ pin.
    pub fn unmask_irq(&self, irq: u8) {
        log::debug!("ioapic: unmasking IRQ {}", irq);
        let mut entry = self.read_redirection(irq);
        entry.set_masked(false);
        self.write_redirection(irq, entry);
    }

    /// Mask all IRQ pins on this I/O APIC.
    pub fn mask_all(&self) {
        log::info!("ioapic: masking all {} entries", self.max_entries);
        for i in 0..self.max_entries {
            self.write_redirection(i, RedirectionEntry::masked());
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-I/O-APIC manager
// ---------------------------------------------------------------------------

/// Manages multiple I/O APICs in the system.
pub struct IoApicManager {
    /// All discovered I/O APICs.
    ioapics: Vec<IoApic>,
}

impl IoApicManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        log::info!("ioapic: creating IoApicManager");
        Self {
            ioapics: Vec::new(),
        }
    }

    /// Add an I/O APIC to the manager.
    pub fn add(&mut self, ioapic: IoApic) {
        log::info!(
            "ioapic: manager adding I/O APIC at base {:#X}, GSI base {}, {} entries",
            ioapic.base_addr,
            ioapic.gsi_base,
            ioapic.max_entries
        );
        self.ioapics.push(ioapic);
    }

    /// Find the I/O APIC responsible for a given GSI.
    pub fn find_for_gsi(&self, gsi: u32) -> Option<&IoApic> {
        self.ioapics.iter().find(|ioapic| {
            gsi >= ioapic.gsi_base && gsi < ioapic.gsi_base + ioapic.max_entries as u32
        })
    }

    /// Route a GSI to a specific CPU through the correct I/O APIC.
    pub fn route_gsi(&self, gsi: u32, vector: u8, dest_apic_id: u8) {
        if let Some(ioapic) = self.find_for_gsi(gsi) {
            ioapic.route_irq(gsi, vector, dest_apic_id);
        } else {
            log::error!("ioapic: no I/O APIC found for GSI {}", gsi);
        }
    }

    /// Mask all IRQs on all I/O APICs.
    pub fn mask_all(&self) {
        log::info!("ioapic: masking all IRQs on all I/O APICs");
        for ioapic in &self.ioapics {
            ioapic.mask_all();
        }
    }

    /// Return the total number of managed I/O APICs.
    pub fn count(&self) -> usize {
        self.ioapics.len()
    }
}
