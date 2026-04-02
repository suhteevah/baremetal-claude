//! HPET (High Precision Event Timer) table parsing.
//!
//! The HPET table (signature "HPET") describes the High Precision Event Timer,
//! which provides femtosecond-resolution timing — vastly superior to the legacy
//! PIT (Programmable Interval Timer) or RTC.

use crate::sdt::{SdtHeader, SDT_HEADER_SIZE};
use crate::AcpiError;

/// HPET address structure (similar to GAS but HPET-specific).
#[derive(Debug, Clone, Copy)]
pub struct HpetAddressStructure {
    /// Address space ID (0=Memory, 1=I/O).
    pub address_space: u8,
    /// Register bit width.
    pub bit_width: u8,
    /// Register bit offset.
    pub bit_offset: u8,
    /// Access size.
    pub access_size: u8,
    /// 64-bit base address.
    pub address: u64,
}

/// Parsed HPET table.
#[derive(Debug, Clone, Copy)]
pub struct Hpet {
    /// Physical address of the HPET table.
    pub table_address: u64,
    /// SDT header.
    pub header: SdtHeader,
    /// Hardware revision ID.
    pub hardware_rev_id: u8,
    /// Number of comparators (0 = 1 comparator, etc.).
    pub comparator_count: u8,
    /// Counter size: true = 64-bit, false = 32-bit.
    pub counter_64bit: bool,
    /// Legacy replacement IRQ routing capable.
    pub legacy_replacement: bool,
    /// PCI vendor ID of the HPET hardware.
    pub pci_vendor_id: u16,
    /// Base address structure.
    pub base_address: HpetAddressStructure,
    /// HPET sequence number (for systems with multiple HPETs).
    pub hpet_number: u8,
    /// Minimum clock tick in femtoseconds.
    pub minimum_tick: u16,
    /// Page protection attribute.
    pub page_protection: u8,
}

/// HPET register offsets (from the base address).
pub mod regs {
    /// General Capabilities and ID register.
    pub const GENERAL_CAP_ID: u64 = 0x000;
    /// General Configuration register.
    pub const GENERAL_CONFIG: u64 = 0x010;
    /// General Interrupt Status register.
    pub const GENERAL_INT_STATUS: u64 = 0x020;
    /// Main Counter Value register.
    pub const MAIN_COUNTER: u64 = 0x0F0;
    /// Timer N Configuration and Capability (N = 0..31).
    pub const fn timer_config(n: u8) -> u64 {
        0x100 + 0x20 * n as u64
    }
    /// Timer N Comparator Value (N = 0..31).
    pub const fn timer_comparator(n: u8) -> u64 {
        0x108 + 0x20 * n as u64
    }
}

impl Hpet {
    /// Parse the HPET table from its physical address.
    ///
    /// # Safety
    ///
    /// `phys_addr` must point to a valid, mapped HPET table.
    pub unsafe fn from_address(phys_addr: u64) -> Result<Self, AcpiError> {
        log::info!("hpet: parsing at {:#X}", phys_addr);

        let header = unsafe { SdtHeader::from_address(phys_addr)? };

        if &header.signature != b"HPET" {
            log::error!(
                "hpet: expected 'HPET' signature, got '{}'",
                header.signature_str()
            );
            return Err(AcpiError::SignatureMismatch);
        }

        unsafe { header.validate_checksum(phys_addr)?; }

        let body_base = phys_addr + SDT_HEADER_SIZE as u64;

        // Event timer block ID (4 bytes)
        let event_timer_block_id: u32 =
            unsafe { core::ptr::read_unaligned(body_base as *const u32) };

        let hardware_rev_id = (event_timer_block_id & 0xFF) as u8;
        let comparator_count = ((event_timer_block_id >> 8) & 0x1F) as u8;
        let counter_64bit = (event_timer_block_id >> 13) & 1 != 0;
        let legacy_replacement = (event_timer_block_id >> 15) & 1 != 0;
        let pci_vendor_id = ((event_timer_block_id >> 16) & 0xFFFF) as u16;

        // Base address structure (12 bytes at offset 4)
        let gas_base = body_base + 4;
        let address_space: u8 = unsafe { core::ptr::read_unaligned(gas_base as *const u8) };
        let bit_width: u8 =
            unsafe { core::ptr::read_unaligned((gas_base + 1) as *const u8) };
        let bit_offset: u8 =
            unsafe { core::ptr::read_unaligned((gas_base + 2) as *const u8) };
        let access_size: u8 =
            unsafe { core::ptr::read_unaligned((gas_base + 3) as *const u8) };
        let address: u64 =
            unsafe { core::ptr::read_unaligned((gas_base + 4) as *const u64) };

        let base_address = HpetAddressStructure {
            address_space,
            bit_width,
            bit_offset,
            access_size,
            address,
        };

        // HPET number (1 byte at offset 16)
        let hpet_number: u8 =
            unsafe { core::ptr::read_unaligned((body_base + 16) as *const u8) };

        // Minimum tick (2 bytes at offset 17)
        let minimum_tick: u16 =
            unsafe { core::ptr::read_unaligned((body_base + 17) as *const u16) };

        // Page protection (1 byte at offset 19)
        let page_protection: u8 =
            unsafe { core::ptr::read_unaligned((body_base + 19) as *const u8) };

        log::info!(
            "hpet: base_addr={:#X} hw_rev={} comparators={} counter_64bit={} legacy={}",
            base_address.address,
            hardware_rev_id,
            comparator_count + 1, // count is 0-based
            counter_64bit,
            legacy_replacement,
        );
        log::debug!(
            "hpet: vendor_id={:#06X} hpet_number={} min_tick={} page_prot={}",
            pci_vendor_id,
            hpet_number,
            minimum_tick,
            page_protection,
        );

        Ok(Hpet {
            table_address: phys_addr,
            header,
            hardware_rev_id,
            comparator_count,
            counter_64bit,
            legacy_replacement,
            pci_vendor_id,
            base_address,
            hpet_number,
            minimum_tick,
            page_protection,
        })
    }

    /// Get the MMIO base address for HPET registers.
    pub fn mmio_base(&self) -> u64 {
        self.base_address.address
    }

    /// Get the total number of comparators (timers).
    pub fn num_comparators(&self) -> u8 {
        self.comparator_count + 1 // field is 0-based
    }

    /// Read the HPET period from the capabilities register (in femtoseconds).
    ///
    /// # Safety
    ///
    /// The HPET MMIO base must be mapped and accessible.
    pub unsafe fn read_period_fs(&self) -> u32 {
        let cap = unsafe {
            core::ptr::read_volatile((self.mmio_base() + regs::GENERAL_CAP_ID) as *const u64)
        };
        let period = (cap >> 32) as u32;
        log::trace!("hpet: period = {} femtoseconds", period);
        period
    }

    /// Read the current counter value.
    ///
    /// # Safety
    ///
    /// The HPET MMIO base must be mapped and accessible.
    pub unsafe fn read_counter(&self) -> u64 {
        let val = unsafe {
            core::ptr::read_volatile((self.mmio_base() + regs::MAIN_COUNTER) as *const u64)
        };
        log::trace!("hpet: counter = {}", val);
        val
    }

    /// Enable the HPET main counter.
    ///
    /// # Safety
    ///
    /// The HPET MMIO base must be mapped and accessible.
    pub unsafe fn enable(&self) {
        let config_addr = (self.mmio_base() + regs::GENERAL_CONFIG) as *mut u64;
        let mut config = unsafe { core::ptr::read_volatile(config_addr) };
        config |= 1; // Set ENABLE_CNF bit
        unsafe { core::ptr::write_volatile(config_addr, config); }
        log::info!("hpet: main counter enabled");
    }

    /// Disable the HPET main counter.
    ///
    /// # Safety
    ///
    /// The HPET MMIO base must be mapped and accessible.
    pub unsafe fn disable(&self) {
        let config_addr = (self.mmio_base() + regs::GENERAL_CONFIG) as *mut u64;
        let mut config = unsafe { core::ptr::read_volatile(config_addr) };
        config &= !1; // Clear ENABLE_CNF bit
        unsafe { core::ptr::write_volatile(config_addr, config); }
        log::info!("hpet: main counter disabled");
    }
}
