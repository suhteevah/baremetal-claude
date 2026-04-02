//! ACPI table parser for ClaudioOS.
//!
//! Parses ACPI tables from physical memory for hardware discovery and power management.
//! Supports RSDP v1/v2, RSDT, XSDT, MADT, FADT, MCFG, and HPET tables.
//!
//! This crate is `#![no_std]` and uses `extern crate alloc` for dynamic collections.

#![no_std]

extern crate alloc;

pub mod rsdp;
pub mod sdt;
pub mod madt;
pub mod fadt;
pub mod mcfg;
pub mod hpet;
pub mod power;

pub use rsdp::{Rsdp, RsdpDescriptor};
pub use sdt::{SdtHeader, Rsdt, Xsdt, AcpiTables};
pub use madt::{Madt, MadtEntry, LocalApic, IoApic, InterruptSourceOverride};
pub use fadt::{Fadt, GenericAddressStructure};
pub use mcfg::{Mcfg, McfgEntry};
pub use hpet::{Hpet, HpetAddressStructure};
pub use power::PowerManager;

/// ACPI error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// RSDP signature not found in search areas.
    RsdpNotFound,
    /// Checksum validation failed.
    ChecksumFailed,
    /// Table signature mismatch.
    SignatureMismatch,
    /// Table pointer is null or invalid.
    InvalidPointer,
    /// RSDP revision not supported.
    UnsupportedRevision,
    /// Requested table not found in RSDT/XSDT.
    TableNotFound,
    /// FADT not available (required for power management).
    FadtNotAvailable,
    /// DSDT not available (required for S5 shutdown).
    DsdtNotAvailable,
    /// S5 sleep type object not found in DSDT.
    S5NotFound,
    /// Generic I/O error during ACPI register access.
    IoError,
}

/// Initialize ACPI from a known RSDP physical address (e.g., from UEFI config table).
///
/// # Safety
///
/// `rsdp_phys_addr` must point to a valid RSDP structure in mapped memory.
pub unsafe fn init_from_rsdp_addr(rsdp_phys_addr: u64) -> Result<AcpiTables, AcpiError> {
    log::info!("acpi: initializing from RSDP at {:#X}", rsdp_phys_addr);
    let rsdp = Rsdp::from_address(rsdp_phys_addr)?;
    AcpiTables::from_rsdp(&rsdp)
}

/// Search for RSDP in standard BIOS memory regions and initialize ACPI tables.
///
/// # Safety
///
/// Reads from physical memory addresses in the EBDA and BIOS ROM area.
/// These regions must be identity-mapped.
pub unsafe fn init_from_bios_search() -> Result<AcpiTables, AcpiError> {
    log::info!("acpi: searching for RSDP in BIOS memory regions");
    let rsdp = Rsdp::search_bios()?;
    AcpiTables::from_rsdp(&rsdp)
}
