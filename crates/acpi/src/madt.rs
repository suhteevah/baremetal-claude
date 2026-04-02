//! MADT (Multiple APIC Description Table) parsing.
//!
//! The MADT (signature "APIC") describes the interrupt controller configuration
//! including all local APICs (one per CPU core) and I/O APICs. Essential for SMP.

use alloc::vec::Vec;
use crate::sdt::{SdtHeader, SDT_HEADER_SIZE};
use crate::AcpiError;

/// MADT entry type: Processor Local APIC.
const ENTRY_LOCAL_APIC: u8 = 0;
/// MADT entry type: I/O APIC.
const ENTRY_IO_APIC: u8 = 1;
/// MADT entry type: Interrupt Source Override.
const ENTRY_INT_SRC_OVERRIDE: u8 = 2;
/// MADT entry type: NMI Source.
const ENTRY_NMI_SOURCE: u8 = 3;
/// MADT entry type: Local APIC NMI.
const ENTRY_LOCAL_APIC_NMI: u8 = 4;
/// MADT entry type: Local APIC Address Override.
const ENTRY_LOCAL_APIC_ADDR_OVERRIDE: u8 = 5;
/// MADT entry type: I/O SAPIC.
const ENTRY_IO_SAPIC: u8 = 6;
/// MADT entry type: Processor Local x2APIC.
const ENTRY_LOCAL_X2APIC: u8 = 9;

/// Parsed MADT.
#[derive(Debug)]
pub struct Madt {
    /// Physical address of the MADT.
    pub address: u64,
    /// SDT header.
    pub header: SdtHeader,
    /// Local APIC physical address (from the MADT fixed fields).
    pub local_apic_address: u32,
    /// MADT flags. Bit 0: PCAT_COMPAT (dual 8259 PICs installed).
    pub flags: u32,
    /// All parsed entries.
    pub entries: Vec<MadtEntry>,
}

/// A single MADT entry (interrupt controller structure).
#[derive(Debug, Clone)]
pub enum MadtEntry {
    /// Type 0: Processor Local APIC.
    LocalApic(LocalApic),
    /// Type 1: I/O APIC.
    IoApic(IoApic),
    /// Type 2: Interrupt Source Override.
    InterruptSourceOverride(InterruptSourceOverride),
    /// Type 3: NMI Source.
    NmiSource(NmiSource),
    /// Type 4: Local APIC NMI.
    LocalApicNmi(LocalApicNmi),
    /// Type 5: Local APIC Address Override (64-bit).
    LocalApicAddressOverride(LocalApicAddressOverride),
    /// Type 6: I/O SAPIC.
    IoSapic(IoSapic),
    /// Type 9: Processor Local x2APIC.
    LocalX2Apic(LocalX2Apic),
    /// Unknown entry type.
    Unknown { entry_type: u8, length: u8 },
}

/// Type 0: Processor Local APIC.
#[derive(Debug, Clone, Copy)]
pub struct LocalApic {
    /// ACPI processor UID.
    pub acpi_processor_id: u8,
    /// Local APIC ID.
    pub apic_id: u8,
    /// Flags. Bit 0: enabled. Bit 1: online capable.
    pub flags: u32,
}

impl LocalApic {
    /// Returns true if this processor is enabled.
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Returns true if this processor is online-capable (can be enabled at runtime).
    pub fn is_online_capable(&self) -> bool {
        self.flags & 2 != 0
    }
}

/// Type 1: I/O APIC.
#[derive(Debug, Clone, Copy)]
pub struct IoApic {
    /// I/O APIC ID.
    pub io_apic_id: u8,
    /// Physical address of the I/O APIC registers.
    pub io_apic_address: u32,
    /// Global system interrupt base.
    pub global_system_interrupt_base: u32,
}

/// Type 2: Interrupt Source Override.
#[derive(Debug, Clone, Copy)]
pub struct InterruptSourceOverride {
    /// Bus source (always 0 = ISA).
    pub bus_source: u8,
    /// IRQ source (ISA IRQ number).
    pub irq_source: u8,
    /// Global system interrupt that this IRQ maps to.
    pub global_system_interrupt: u32,
    /// MPS INTI flags (polarity + trigger mode).
    pub flags: u16,
}

/// Type 3: NMI Source.
#[derive(Debug, Clone, Copy)]
pub struct NmiSource {
    /// MPS INTI flags.
    pub flags: u16,
    /// Global system interrupt for this NMI.
    pub global_system_interrupt: u32,
}

/// Type 4: Local APIC NMI.
#[derive(Debug, Clone, Copy)]
pub struct LocalApicNmi {
    /// ACPI processor UID (0xFF = all processors).
    pub acpi_processor_id: u8,
    /// MPS INTI flags.
    pub flags: u16,
    /// Local APIC LINT# (0 or 1).
    pub lint: u8,
}

/// Type 5: Local APIC Address Override.
#[derive(Debug, Clone, Copy)]
pub struct LocalApicAddressOverride {
    /// 64-bit physical address of local APIC (overrides the 32-bit field).
    pub local_apic_address: u64,
}

/// Type 6: I/O SAPIC.
#[derive(Debug, Clone, Copy)]
pub struct IoSapic {
    /// I/O APIC ID.
    pub io_apic_id: u8,
    /// Global system interrupt base.
    pub global_system_interrupt_base: u32,
    /// 64-bit physical address of the I/O SAPIC.
    pub io_sapic_address: u64,
}

/// Type 9: Processor Local x2APIC.
#[derive(Debug, Clone, Copy)]
pub struct LocalX2Apic {
    /// Local x2APIC ID.
    pub x2apic_id: u32,
    /// Flags (same as Local APIC flags).
    pub flags: u32,
    /// ACPI processor UID.
    pub acpi_uid: u32,
}

impl LocalX2Apic {
    /// Returns true if this processor is enabled.
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
}

impl Madt {
    /// Parse the MADT from its physical address.
    ///
    /// # Safety
    ///
    /// `phys_addr` must point to a valid, mapped MADT (signature "APIC").
    pub unsafe fn from_address(phys_addr: u64) -> Result<Self, AcpiError> {
        log::info!("madt: parsing at {:#X}", phys_addr);

        let header = unsafe { SdtHeader::from_address(phys_addr)? };

        if &header.signature != b"APIC" {
            log::error!(
                "madt: expected 'APIC' signature, got '{}'",
                header.signature_str()
            );
            return Err(AcpiError::SignatureMismatch);
        }

        unsafe { header.validate_checksum(phys_addr)?; }

        // Read MADT fixed fields (after header)
        let fixed_base = phys_addr + SDT_HEADER_SIZE as u64;
        let local_apic_address: u32 =
            unsafe { core::ptr::read_unaligned(fixed_base as *const u32) };
        let flags: u32 =
            unsafe { core::ptr::read_unaligned((fixed_base + 4) as *const u32) };

        log::info!(
            "madt: local APIC address={:#X}, flags={:#X} (PCAT_COMPAT={})",
            local_apic_address,
            flags,
            flags & 1,
        );

        // Parse variable-length entries
        let entries_base = fixed_base + 8; // 4 (LAPIC addr) + 4 (flags)
        let entries_end = phys_addr + header.length as u64;
        let mut entries = Vec::new();
        let mut offset = entries_base;

        while offset + 2 <= entries_end {
            let entry_type: u8 = unsafe { core::ptr::read_unaligned(offset as *const u8) };
            let entry_length: u8 =
                unsafe { core::ptr::read_unaligned((offset + 1) as *const u8) };

            if entry_length < 2 {
                log::error!(
                    "madt: entry at offset {:#X} has invalid length {}",
                    offset,
                    entry_length,
                );
                break;
            }

            if offset + entry_length as u64 > entries_end {
                log::error!(
                    "madt: entry at {:#X} extends past table end (type={}, len={})",
                    offset,
                    entry_type,
                    entry_length,
                );
                break;
            }

            let entry = unsafe { Self::parse_entry(offset, entry_type, entry_length) };
            entries.push(entry);

            offset += entry_length as u64;
        }

        let mut cpu_count = 0u32;
        let mut ioapic_count = 0u32;
        for entry in &entries {
            match entry {
                MadtEntry::LocalApic(lapic) => {
                    if lapic.is_enabled() {
                        cpu_count += 1;
                    }
                    log::debug!(
                        "madt: Local APIC: processor_id={} apic_id={} enabled={} online_capable={}",
                        lapic.acpi_processor_id,
                        lapic.apic_id,
                        lapic.is_enabled(),
                        lapic.is_online_capable(),
                    );
                }
                MadtEntry::IoApic(ioapic) => {
                    ioapic_count += 1;
                    log::debug!(
                        "madt: I/O APIC: id={} addr={:#X} gsi_base={}",
                        ioapic.io_apic_id,
                        ioapic.io_apic_address,
                        ioapic.global_system_interrupt_base,
                    );
                }
                MadtEntry::InterruptSourceOverride(iso) => {
                    log::debug!(
                        "madt: IRQ override: bus={} irq={} -> gsi={} flags={:#X}",
                        iso.bus_source,
                        iso.irq_source,
                        iso.global_system_interrupt,
                        iso.flags,
                    );
                }
                MadtEntry::LocalX2Apic(x2) => {
                    if x2.is_enabled() {
                        cpu_count += 1;
                    }
                    log::debug!(
                        "madt: x2APIC: id={} uid={} enabled={}",
                        x2.x2apic_id,
                        x2.acpi_uid,
                        x2.is_enabled(),
                    );
                }
                _ => {
                    log::trace!("madt: entry {:?}", entry);
                }
            }
        }

        log::info!(
            "madt: {} entries parsed, {} enabled CPUs, {} I/O APICs",
            entries.len(),
            cpu_count,
            ioapic_count,
        );

        Ok(Madt {
            address: phys_addr,
            header,
            local_apic_address,
            flags,
            entries,
        })
    }

    /// Parse a single MADT entry.
    ///
    /// # Safety
    ///
    /// `offset` must point to a valid MADT entry of the given type and length.
    unsafe fn parse_entry(offset: u64, entry_type: u8, entry_length: u8) -> MadtEntry {
        let data = offset + 2; // skip type + length bytes

        match entry_type {
            ENTRY_LOCAL_APIC => {
                let acpi_processor_id: u8 =
                    unsafe { core::ptr::read_unaligned(data as *const u8) };
                let apic_id: u8 =
                    unsafe { core::ptr::read_unaligned((data + 1) as *const u8) };
                let flags: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                MadtEntry::LocalApic(LocalApic {
                    acpi_processor_id,
                    apic_id,
                    flags,
                })
            }
            ENTRY_IO_APIC => {
                let io_apic_id: u8 =
                    unsafe { core::ptr::read_unaligned(data as *const u8) };
                // data+1 is reserved
                let io_apic_address: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                let global_system_interrupt_base: u32 =
                    unsafe { core::ptr::read_unaligned((data + 6) as *const u32) };
                MadtEntry::IoApic(IoApic {
                    io_apic_id,
                    io_apic_address,
                    global_system_interrupt_base,
                })
            }
            ENTRY_INT_SRC_OVERRIDE => {
                let bus_source: u8 =
                    unsafe { core::ptr::read_unaligned(data as *const u8) };
                let irq_source: u8 =
                    unsafe { core::ptr::read_unaligned((data + 1) as *const u8) };
                let global_system_interrupt: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                let flags: u16 =
                    unsafe { core::ptr::read_unaligned((data + 6) as *const u16) };
                MadtEntry::InterruptSourceOverride(InterruptSourceOverride {
                    bus_source,
                    irq_source,
                    global_system_interrupt,
                    flags,
                })
            }
            ENTRY_NMI_SOURCE => {
                let flags: u16 =
                    unsafe { core::ptr::read_unaligned(data as *const u16) };
                let global_system_interrupt: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                MadtEntry::NmiSource(NmiSource {
                    flags,
                    global_system_interrupt,
                })
            }
            ENTRY_LOCAL_APIC_NMI => {
                let acpi_processor_id: u8 =
                    unsafe { core::ptr::read_unaligned(data as *const u8) };
                let flags: u16 =
                    unsafe { core::ptr::read_unaligned((data + 1) as *const u16) };
                let lint: u8 =
                    unsafe { core::ptr::read_unaligned((data + 3) as *const u8) };
                MadtEntry::LocalApicNmi(LocalApicNmi {
                    acpi_processor_id,
                    flags,
                    lint,
                })
            }
            ENTRY_LOCAL_APIC_ADDR_OVERRIDE => {
                // data+0..1 is reserved
                let local_apic_address: u64 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u64) };
                MadtEntry::LocalApicAddressOverride(LocalApicAddressOverride {
                    local_apic_address,
                })
            }
            ENTRY_IO_SAPIC => {
                let io_apic_id: u8 =
                    unsafe { core::ptr::read_unaligned(data as *const u8) };
                // data+1 is reserved
                let global_system_interrupt_base: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                let io_sapic_address: u64 =
                    unsafe { core::ptr::read_unaligned((data + 6) as *const u64) };
                MadtEntry::IoSapic(IoSapic {
                    io_apic_id,
                    global_system_interrupt_base,
                    io_sapic_address,
                })
            }
            ENTRY_LOCAL_X2APIC => {
                // data+0..1 is reserved (2 bytes)
                let x2apic_id: u32 =
                    unsafe { core::ptr::read_unaligned((data + 2) as *const u32) };
                let flags: u32 =
                    unsafe { core::ptr::read_unaligned((data + 6) as *const u32) };
                let acpi_uid: u32 =
                    unsafe { core::ptr::read_unaligned((data + 10) as *const u32) };
                MadtEntry::LocalX2Apic(LocalX2Apic {
                    x2apic_id,
                    flags,
                    acpi_uid,
                })
            }
            _ => {
                log::trace!(
                    "madt: unknown entry type {} (len={}) at {:#X}",
                    entry_type,
                    entry_length,
                    offset,
                );
                MadtEntry::Unknown {
                    entry_type,
                    length: entry_length,
                }
            }
        }
    }

    /// Get all enabled Local APIC entries (CPU cores).
    pub fn local_apics(&self) -> Vec<LocalApic> {
        let mut result = Vec::new();
        for entry in &self.entries {
            if let MadtEntry::LocalApic(lapic) = entry {
                if lapic.is_enabled() || lapic.is_online_capable() {
                    result.push(*lapic);
                }
            }
        }
        result
    }

    /// Get all I/O APIC entries.
    pub fn io_apics(&self) -> Vec<IoApic> {
        let mut result = Vec::new();
        for entry in &self.entries {
            if let MadtEntry::IoApic(ioapic) = entry {
                result.push(*ioapic);
            }
        }
        result
    }

    /// Get all interrupt source overrides.
    pub fn interrupt_overrides(&self) -> Vec<InterruptSourceOverride> {
        let mut result = Vec::new();
        for entry in &self.entries {
            if let MadtEntry::InterruptSourceOverride(iso) = entry {
                result.push(*iso);
            }
        }
        result
    }

    /// Check if dual 8259 PICs are present (PCAT_COMPAT flag).
    pub fn has_legacy_pics(&self) -> bool {
        self.flags & 1 != 0
    }
}
