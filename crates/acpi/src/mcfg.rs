//! MCFG (PCI Express Memory-Mapped Configuration) table parsing.
//!
//! The MCFG table (signature "MCFG") provides the base address for PCIe
//! ECAM (Enhanced Configuration Access Mechanism), enabling memory-mapped
//! access to the full 4096-byte PCIe configuration space per function
//! (instead of the legacy 256-byte PCI space via I/O ports 0xCF8/0xCFC).

use alloc::vec::Vec;
use crate::sdt::{SdtHeader, SDT_HEADER_SIZE};
use crate::AcpiError;

/// Size of the MCFG fixed fields after the SDT header (8 bytes reserved).
const MCFG_FIXED_SIZE: usize = 8;

/// Size of each MCFG allocation entry (16 bytes).
const MCFG_ENTRY_SIZE: usize = 16;

/// Parsed MCFG table.
#[derive(Debug)]
pub struct Mcfg {
    /// Physical address of the MCFG table.
    pub address: u64,
    /// SDT header.
    pub header: SdtHeader,
    /// PCIe configuration space allocation entries.
    pub entries: Vec<McfgEntry>,
}

/// A single MCFG allocation entry describing a PCIe segment's ECAM region.
#[derive(Debug, Clone, Copy)]
pub struct McfgEntry {
    /// Base physical address of the ECAM region.
    pub base_address: u64,
    /// PCI segment group number.
    pub segment_group: u16,
    /// Start PCI bus number covered by this entry.
    pub start_bus: u8,
    /// End PCI bus number covered by this entry.
    pub end_bus: u8,
}

impl McfgEntry {
    /// Calculate the physical address for a specific PCIe device's configuration space.
    ///
    /// Returns `None` if the bus number is outside this entry's range.
    pub fn config_address(&self, bus: u8, device: u8, function: u8) -> Option<u64> {
        if bus < self.start_bus || bus > self.end_bus {
            return None;
        }
        if device > 31 || function > 7 {
            return None;
        }

        let offset = ((bus as u64 - self.start_bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12);

        Some(self.base_address + offset)
    }

    /// Size of the ECAM region in bytes.
    pub fn region_size(&self) -> u64 {
        let bus_count = (self.end_bus as u64 - self.start_bus as u64) + 1;
        // Each bus has 32 devices * 8 functions * 4096 bytes
        bus_count * 32 * 8 * 4096
    }
}

impl Mcfg {
    /// Parse the MCFG table from its physical address.
    ///
    /// # Safety
    ///
    /// `phys_addr` must point to a valid, mapped MCFG table.
    pub unsafe fn from_address(phys_addr: u64) -> Result<Self, AcpiError> {
        log::info!("mcfg: parsing at {:#X}", phys_addr);

        let header = unsafe { SdtHeader::from_address(phys_addr)? };

        if &header.signature != b"MCFG" {
            log::error!(
                "mcfg: expected 'MCFG' signature, got '{}'",
                header.signature_str()
            );
            return Err(AcpiError::SignatureMismatch);
        }

        unsafe { header.validate_checksum(phys_addr)?; }

        let body_len = (header.length as usize).saturating_sub(SDT_HEADER_SIZE);
        if body_len < MCFG_FIXED_SIZE {
            log::warn!("mcfg: table body too small ({} bytes), no entries", body_len);
            return Ok(Mcfg {
                address: phys_addr,
                header,
                entries: Vec::new(),
            });
        }

        let entry_area_len = body_len - MCFG_FIXED_SIZE;
        let entry_count = entry_area_len / MCFG_ENTRY_SIZE;
        log::debug!("mcfg: {} allocation entries", entry_count);

        let entry_base = phys_addr + SDT_HEADER_SIZE as u64 + MCFG_FIXED_SIZE as u64;
        let mut entries = Vec::with_capacity(entry_count);

        for i in 0..entry_count {
            let offset = entry_base + (i * MCFG_ENTRY_SIZE) as u64;

            let base_address: u64 =
                unsafe { core::ptr::read_unaligned(offset as *const u64) };
            let segment_group: u16 =
                unsafe { core::ptr::read_unaligned((offset + 8) as *const u16) };
            let start_bus: u8 =
                unsafe { core::ptr::read_unaligned((offset + 10) as *const u8) };
            let end_bus: u8 =
                unsafe { core::ptr::read_unaligned((offset + 11) as *const u8) };
            // bytes 12-15 are reserved

            let entry = McfgEntry {
                base_address,
                segment_group,
                start_bus,
                end_bus,
            };

            log::info!(
                "mcfg: entry[{}]: ECAM base={:#X} segment={} bus={}-{} (region size={:#X})",
                i,
                entry.base_address,
                entry.segment_group,
                entry.start_bus,
                entry.end_bus,
                entry.region_size(),
            );

            entries.push(entry);
        }

        Ok(Mcfg {
            address: phys_addr,
            header,
            entries,
        })
    }

    /// Find the ECAM entry covering a specific PCI segment and bus.
    pub fn find_segment(&self, segment: u16, bus: u8) -> Option<&McfgEntry> {
        for entry in &self.entries {
            if entry.segment_group == segment && bus >= entry.start_bus && bus <= entry.end_bus {
                return Some(entry);
            }
        }
        None
    }
}
