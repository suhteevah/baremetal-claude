//! RSDP (Root System Description Pointer) parsing.
//!
//! The RSDP is the entry point to all ACPI tables. It can be found by:
//! - Searching EBDA (0x9FC00..0xA0000) and BIOS area (0xE0000..0xFFFFF)
//! - Reading from the UEFI system table (EFI_ACPI_20_TABLE_GUID)

use crate::AcpiError;

/// RSDP v1 size (ACPI 1.0).
const RSDP_V1_SIZE: usize = 20;

/// RSDP v2 size (ACPI 2.0+).
const RSDP_V2_SIZE: usize = 36;

/// RSDP signature: "RSD PTR " (8 bytes, note trailing space).
const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";

/// EBDA search start address.
const EBDA_START: u64 = 0x9_FC00;

/// EBDA search end address.
const EBDA_END: u64 = 0xA_0000;

/// BIOS ROM search start address.
const BIOS_START: u64 = 0xE_0000;

/// BIOS ROM search end address.
const BIOS_END: u64 = 0xF_FFFF;

/// Raw RSDP descriptor read from memory.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct RsdpDescriptor {
    /// "RSD PTR " (8 bytes).
    pub signature: [u8; 8],
    /// Checksum of first 20 bytes (must sum to 0 mod 256).
    pub checksum: u8,
    /// OEM ID string (6 bytes).
    pub oem_id: [u8; 6],
    /// ACPI revision: 0 = ACPI 1.0, 2 = ACPI 2.0+.
    pub revision: u8,
    /// Physical address of the RSDT (32-bit).
    pub rsdt_address: u32,

    // --- ACPI 2.0+ fields (only valid if revision >= 2) ---

    /// Total length of the RSDP structure including extended fields.
    pub length: u32,
    /// Physical address of the XSDT (64-bit).
    pub xsdt_address: u64,
    /// Checksum of the entire RSDP structure (v2).
    pub extended_checksum: u8,
    /// Reserved bytes.
    pub reserved: [u8; 3],
}

/// Parsed RSDP with validated data.
#[derive(Debug, Clone, Copy)]
pub struct Rsdp {
    /// ACPI revision (0 = 1.0, 2 = 2.0+).
    pub revision: u8,
    /// OEM ID (6 bytes, may not be null-terminated).
    pub oem_id: [u8; 6],
    /// Physical address of RSDT (always present).
    pub rsdt_address: u32,
    /// Physical address of XSDT (only valid for revision >= 2).
    pub xsdt_address: Option<u64>,
}

impl Rsdp {
    /// Parse and validate an RSDP from a known physical address.
    ///
    /// # Safety
    ///
    /// `phys_addr` must point to a valid, mapped RSDP structure.
    pub unsafe fn from_address(phys_addr: u64) -> Result<Self, AcpiError> {
        log::debug!("rsdp: reading descriptor at {:#X}", phys_addr);

        let ptr = phys_addr as *const RsdpDescriptor;
        let desc = unsafe { core::ptr::read_unaligned(ptr) };

        // Validate signature
        if desc.signature != RSDP_SIGNATURE {
            log::error!("rsdp: invalid signature at {:#X}: {:?}", phys_addr, desc.signature);
            return Err(AcpiError::RsdpNotFound);
        }

        log::trace!("rsdp: found signature at {:#X}", phys_addr);

        // Validate v1 checksum (first 20 bytes)
        let bytes = core::slice::from_raw_parts(phys_addr as *const u8, RSDP_V1_SIZE);
        let checksum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        if checksum != 0 {
            log::error!("rsdp: v1 checksum failed at {:#X}: sum={}", phys_addr, checksum);
            return Err(AcpiError::ChecksumFailed);
        }

        log::debug!("rsdp: v1 checksum OK, revision={}", desc.revision);

        let mut rsdp = Rsdp {
            revision: desc.revision,
            oem_id: desc.oem_id,
            rsdt_address: desc.rsdt_address,
            xsdt_address: None,
        };

        // For ACPI 2.0+, validate extended checksum and read XSDT address
        if desc.revision >= 2 {
            let ext_bytes = core::slice::from_raw_parts(phys_addr as *const u8, RSDP_V2_SIZE);
            let ext_checksum: u8 = ext_bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            if ext_checksum != 0 {
                log::error!(
                    "rsdp: v2 extended checksum failed at {:#X}: sum={}",
                    phys_addr,
                    ext_checksum
                );
                return Err(AcpiError::ChecksumFailed);
            }

            let xsdt_addr = desc.xsdt_address;
            if xsdt_addr != 0 {
                log::info!("rsdp: ACPI 2.0+ XSDT at {:#X}", xsdt_addr);
                rsdp.xsdt_address = Some(xsdt_addr);
            } else {
                log::warn!("rsdp: ACPI 2.0+ but XSDT address is null, falling back to RSDT");
            }
        }

        log::info!(
            "rsdp: revision={}, RSDT={:#X}, XSDT={:?}, OEM={:?}",
            rsdp.revision,
            rsdp.rsdt_address,
            rsdp.xsdt_address,
            core::str::from_utf8(&rsdp.oem_id).unwrap_or("<invalid>")
        );

        Ok(rsdp)
    }

    /// Search for RSDP in standard BIOS memory regions.
    ///
    /// Scans the EBDA (0x9FC00..0xA0000) and BIOS ROM area (0xE0000..0xFFFFF)
    /// on 16-byte aligned boundaries.
    ///
    /// # Safety
    ///
    /// The EBDA and BIOS ROM memory regions must be identity-mapped and readable.
    pub unsafe fn search_bios() -> Result<Self, AcpiError> {
        log::info!("rsdp: searching EBDA ({:#X}..{:#X})", EBDA_START, EBDA_END);

        // Search EBDA
        if let Some(rsdp) = Self::search_region(EBDA_START, EBDA_END) {
            return rsdp;
        }

        log::info!(
            "rsdp: not found in EBDA, searching BIOS area ({:#X}..{:#X})",
            BIOS_START,
            BIOS_END
        );

        // Search BIOS ROM area
        if let Some(rsdp) = Self::search_region(BIOS_START, BIOS_END) {
            return rsdp;
        }

        log::error!("rsdp: not found in any BIOS memory region");
        Err(AcpiError::RsdpNotFound)
    }

    /// Search a memory region for the RSDP signature on 16-byte boundaries.
    ///
    /// # Safety
    ///
    /// The region must be identity-mapped and readable.
    unsafe fn search_region(start: u64, end: u64) -> Option<Result<Self, AcpiError>> {
        let mut addr = start;
        while addr + RSDP_V1_SIZE as u64 <= end {
            let sig_ptr = addr as *const [u8; 8];
            let sig = unsafe { core::ptr::read_unaligned(sig_ptr) };
            if sig == RSDP_SIGNATURE {
                log::debug!("rsdp: candidate signature at {:#X}", addr);
                match unsafe { Self::from_address(addr) } {
                    Ok(rsdp) => return Some(Ok(rsdp)),
                    Err(AcpiError::ChecksumFailed) => {
                        log::warn!("rsdp: checksum failed at {:#X}, continuing search", addr);
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
            addr += 16; // RSDP is always 16-byte aligned
        }
        None
    }

    /// Create an RSDP from a UEFI configuration table pointer.
    ///
    /// Use this when the bootloader provides the RSDP address from the
    /// EFI_ACPI_20_TABLE_GUID or EFI_ACPI_TABLE_GUID config table entry.
    ///
    /// # Safety
    ///
    /// `uefi_rsdp_ptr` must point to a valid RSDP provided by UEFI firmware.
    pub unsafe fn from_uefi(uefi_rsdp_ptr: u64) -> Result<Self, AcpiError> {
        log::info!("rsdp: loading from UEFI config table at {:#X}", uefi_rsdp_ptr);
        if uefi_rsdp_ptr == 0 {
            log::error!("rsdp: UEFI provided null RSDP pointer");
            return Err(AcpiError::InvalidPointer);
        }
        unsafe { Self::from_address(uefi_rsdp_ptr) }
    }
}
