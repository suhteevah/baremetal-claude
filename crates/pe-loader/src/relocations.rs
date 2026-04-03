//! Base relocation table processing.
//!
//! When a PE image is loaded at an address different from its preferred image base,
//! all absolute addresses embedded in the code/data must be adjusted. The .reloc
//! section contains a table of fixup locations grouped by 4KB page.
//!
//! For x86_64, the dominant relocation type is IMAGE_REL_BASED_DIR64 (type 10),
//! which patches a 64-bit absolute address.

use alloc::vec::Vec;

/// Relocation block header — one per 4KB page of fixups.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct RawBaseRelocBlock {
    /// RVA of the page this block applies to.
    page_rva: u32,
    /// Total size of this block in bytes (including header and entries).
    block_size: u32,
}

/// Relocation type constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RelocationType {
    /// Padding — skip this entry.
    Absolute = 0,
    /// Add high 16 bits of delta.
    High = 1,
    /// Add low 16 bits of delta.
    Low = 2,
    /// Add full 32-bit delta to 16:16 value.
    HighLow = 3,
    /// Add high 16 bits, adjusted for sign extension of low 16 bits.
    HighAdj = 4,
    /// MIPS: jump address.
    MipsJmpAddr = 5,
    /// 64-bit address relocation — the main one for x86_64.
    Dir64 = 10,
}

impl RelocationType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Absolute),
            1 => Some(Self::High),
            2 => Some(Self::Low),
            3 => Some(Self::HighLow),
            4 => Some(Self::HighAdj),
            5 => Some(Self::MipsJmpAddr),
            10 => Some(Self::Dir64),
            _ => None,
        }
    }
}

/// A single base relocation entry.
#[derive(Debug, Clone)]
pub struct BaseRelocation {
    /// RVA of the location to patch.
    pub rva: u32,
    /// Relocation type.
    pub reloc_type: RelocationType,
}

/// Parse all base relocation entries from the .reloc section data.
///
/// `data` is the full loaded image. `reloc_rva` and `reloc_size` come from
/// data directory index 5 (IMAGE_DIRECTORY_ENTRY_BASERELOC).
pub fn parse_relocations(data: &[u8], reloc_rva: u32, reloc_size: u32) -> Vec<BaseRelocation> {
    let mut relocations = Vec::new();
    let mut offset = reloc_rva as usize;
    let end = offset + reloc_size as usize;

    while offset + 8 <= end && offset + 8 <= data.len() {
        let block = unsafe { &*(data[offset..].as_ptr() as *const RawBaseRelocBlock) };

        let page_rva = block.page_rva;
        let block_size = block.block_size as usize;

        if block_size < 8 || offset + block_size > data.len() {
            break;
        }

        // Number of 16-bit entries following the 8-byte header
        let entry_count = (block_size - 8) / 2;
        let entries_start = offset + 8;

        for i in 0..entry_count {
            let entry_offset = entries_start + i * 2;
            if entry_offset + 2 > data.len() {
                break;
            }

            let entry = u16::from_le_bytes([data[entry_offset], data[entry_offset + 1]]);
            let reloc_type_raw = (entry >> 12) as u8;
            let page_offset = entry & 0x0FFF;

            if let Some(reloc_type) = RelocationType::from_u8(reloc_type_raw) {
                if reloc_type != RelocationType::Absolute {
                    relocations.push(BaseRelocation {
                        rva: page_rva + page_offset as u32,
                        reloc_type,
                    });
                }
                // Absolute entries are padding — skip silently
            } else {
                log::warn!(
                    "[pe-loader] Unknown relocation type {} at RVA 0x{:08X}",
                    reloc_type_raw, page_rva + page_offset as u32
                );
            }
        }

        offset += block_size;
    }

    log::info!("[pe-loader] Parsed {} base relocations", relocations.len());
    relocations
}

/// Apply base relocations to the loaded image.
///
/// `image` is the loaded PE image buffer (mutable).
/// `actual_base` is the address where the image was actually loaded.
/// `preferred_base` is the image's preferred base from the optional header.
/// `relocations` is the parsed relocation table.
pub fn apply_relocations(
    image: &mut [u8],
    actual_base: u64,
    preferred_base: u64,
    relocations: &[BaseRelocation],
) {
    let delta = actual_base.wrapping_sub(preferred_base);
    if delta == 0 {
        log::trace!("[pe-loader] Image loaded at preferred base, no relocations needed");
        return;
    }

    log::info!(
        "[pe-loader] Applying {} relocations, delta=0x{:X} (actual=0x{:X}, preferred=0x{:X})",
        relocations.len(), delta, actual_base, preferred_base
    );

    let mut applied = 0u32;
    for reloc in relocations {
        let offset = reloc.rva as usize;

        match reloc.reloc_type {
            RelocationType::Dir64 => {
                if offset + 8 <= image.len() {
                    let val = u64::from_le_bytes([
                        image[offset], image[offset + 1], image[offset + 2], image[offset + 3],
                        image[offset + 4], image[offset + 5], image[offset + 6], image[offset + 7],
                    ]);
                    let new_val = val.wrapping_add(delta);
                    image[offset..offset + 8].copy_from_slice(&new_val.to_le_bytes());
                    applied += 1;
                }
            }
            RelocationType::HighLow => {
                if offset + 4 <= image.len() {
                    let val = u32::from_le_bytes([
                        image[offset], image[offset + 1], image[offset + 2], image[offset + 3],
                    ]);
                    let new_val = val.wrapping_add(delta as u32);
                    image[offset..offset + 4].copy_from_slice(&new_val.to_le_bytes());
                    applied += 1;
                }
            }
            RelocationType::High => {
                if offset + 2 <= image.len() {
                    let val = u16::from_le_bytes([image[offset], image[offset + 1]]);
                    let new_val = val.wrapping_add((delta >> 16) as u16);
                    image[offset..offset + 2].copy_from_slice(&new_val.to_le_bytes());
                    applied += 1;
                }
            }
            RelocationType::Low => {
                if offset + 2 <= image.len() {
                    let val = u16::from_le_bytes([image[offset], image[offset + 1]]);
                    let new_val = val.wrapping_add(delta as u16);
                    image[offset..offset + 2].copy_from_slice(&new_val.to_le_bytes());
                    applied += 1;
                }
            }
            RelocationType::Absolute => {} // Padding, skip
            _ => {
                log::warn!("[pe-loader] Unhandled relocation type {:?} at 0x{:08X}", reloc.reloc_type, offset);
            }
        }
    }

    log::debug!("[pe-loader] Applied {} relocations", applied);
}
