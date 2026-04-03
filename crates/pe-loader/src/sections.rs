//! PE section header parsing and memory mapping.
//!
//! Each section describes a contiguous region in the PE file and where it should
//! be loaded in memory. Common sections: .text (code), .data (initialized data),
//! .rdata (read-only data), .bss (uninitialized data), .idata (imports),
//! .edata (exports), .rsrc (resources), .reloc (relocations).

use alloc::string::String;
use alloc::vec::Vec;

/// IMAGE_SECTION_HEADER — 40 bytes each, follows the optional header.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct RawSectionHeader {
    /// Section name (8 bytes, null-padded, NOT null-terminated if exactly 8 chars).
    pub name: [u8; 8],
    /// Virtual size — total size of the section when loaded into memory.
    pub virtual_size: u32,
    /// Virtual address — RVA of the section in memory (relative to image base).
    pub virtual_address: u32,
    /// Size of raw data — size of the section on disk (file-aligned).
    pub size_of_raw_data: u32,
    /// Pointer to raw data — file offset of the section's data.
    pub pointer_to_raw_data: u32,
    /// Pointer to relocations (object files only, 0 for images).
    pub pointer_to_relocations: u32,
    /// Pointer to line numbers (deprecated).
    pub pointer_to_linenumbers: u32,
    /// Number of relocations.
    pub number_of_relocations: u16,
    /// Number of line numbers.
    pub number_of_linenumbers: u16,
    /// Section characteristics (flags: code, data, read, write, execute).
    pub characteristics: u32,
}

/// Parsed section header with a proper string name.
#[derive(Debug, Clone)]
pub struct SectionHeader {
    /// Section name (e.g., ".text", ".data", ".rdata").
    pub name: String,
    /// Virtual size when loaded.
    pub virtual_size: u32,
    /// RVA in memory.
    pub virtual_address: u32,
    /// Size on disk.
    pub size_of_raw_data: u32,
    /// File offset of data.
    pub pointer_to_raw_data: u32,
    /// Characteristics flags.
    pub characteristics: u32,
}

impl SectionHeader {
    pub const SIZE: usize = 40;

    /// Parse section headers from the raw bytes following the optional header.
    pub fn parse_all(data: &[u8], count: usize) -> Vec<SectionHeader> {
        let mut sections = Vec::with_capacity(count);

        for i in 0..count {
            let offset = i * Self::SIZE;
            if offset + Self::SIZE > data.len() {
                log::error!("[pe-loader] Section header {} extends past data", i);
                break;
            }

            let raw = unsafe { &*(data[offset..].as_ptr() as *const RawSectionHeader) };

            // Extract name — up to 8 bytes, stop at null
            let name_len = raw.name.iter().position(|&b| b == 0).unwrap_or(8);
            let name = String::from_utf8_lossy(&raw.name[..name_len]).into_owned();

            let section = SectionHeader {
                name,
                virtual_size: raw.virtual_size,
                virtual_address: raw.virtual_address,
                size_of_raw_data: raw.size_of_raw_data,
                pointer_to_raw_data: raw.pointer_to_raw_data,
                characteristics: raw.characteristics,
            };

            log::trace!(
                "[pe-loader] Section '{}': RVA=0x{:08X} vsize=0x{:X} rawsize=0x{:X} flags=0x{:08X}",
                section.name, section.virtual_address, section.virtual_size,
                section.size_of_raw_data, section.characteristics
            );

            sections.push(section);
        }

        sections
    }

    /// Check if this section contains executable code.
    pub fn is_code(&self) -> bool {
        self.characteristics & SectionFlags::CODE != 0
    }

    /// Check if this section contains initialized data.
    pub fn is_initialized_data(&self) -> bool {
        self.characteristics & SectionFlags::INITIALIZED_DATA != 0
    }

    /// Check if this section contains uninitialized data (.bss).
    pub fn is_uninitialized_data(&self) -> bool {
        self.characteristics & SectionFlags::UNINITIALIZED_DATA != 0
    }

    /// Check if this section is readable.
    pub fn is_readable(&self) -> bool {
        self.characteristics & SectionFlags::MEM_READ != 0
    }

    /// Check if this section is writable.
    pub fn is_writable(&self) -> bool {
        self.characteristics & SectionFlags::MEM_WRITE != 0
    }

    /// Check if this section is executable.
    pub fn is_executable(&self) -> bool {
        self.characteristics & SectionFlags::MEM_EXECUTE != 0
    }
}

/// Section characteristic flags.
pub struct SectionFlags;

impl SectionFlags {
    pub const CODE: u32 = 0x0000_0020;
    pub const INITIALIZED_DATA: u32 = 0x0000_0040;
    pub const UNINITIALIZED_DATA: u32 = 0x0000_0080;
    pub const MEM_DISCARDABLE: u32 = 0x0200_0000;
    pub const MEM_NOT_CACHED: u32 = 0x0400_0000;
    pub const MEM_NOT_PAGED: u32 = 0x0800_0000;
    pub const MEM_SHARED: u32 = 0x1000_0000;
    pub const MEM_EXECUTE: u32 = 0x2000_0000;
    pub const MEM_READ: u32 = 0x4000_0000;
    pub const MEM_WRITE: u32 = 0x8000_0000;
}

/// A section that has been mapped into memory.
#[derive(Debug, Clone)]
pub struct MappedSection {
    /// Section name.
    pub name: String,
    /// Base address in the loaded image.
    pub base: u64,
    /// Size in memory.
    pub size: usize,
    /// The raw data (copied into the image buffer).
    pub data: Vec<u8>,
    /// Is this section executable?
    pub executable: bool,
    /// Is this section writable?
    pub writable: bool,
}
