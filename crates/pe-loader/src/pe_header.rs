//! PE signature, COFF header, and Optional header parsing.
//!
//! After the DOS header, the PE file contains:
//! 1. PE signature (4 bytes): "PE\0\0" = 0x00004550
//! 2. COFF header (20 bytes): machine type, section count, timestamps
//! 3. Optional header (variable): PE32+ for x86_64 with entry point, image base,
//!    section/file alignment, data directories (import, export, reloc, etc.)

use alloc::vec::Vec;

/// PE signature magic.
pub const PE_SIGNATURE: u32 = 0x0000_4550; // "PE\0\0"

/// PE32+ magic (64-bit).
pub const PE32_PLUS_MAGIC: u16 = 0x020B;

/// Machine type: AMD64 / x86_64.
pub const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;

/// PE Signature — 4 bytes, must be 0x00004550.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct PeSignature {
    pub signature: u32,
}

impl PeSignature {
    pub const SIZE: usize = 4;

    pub fn parse(data: &[u8]) -> Option<&PeSignature> {
        if data.len() < Self::SIZE {
            return None;
        }
        let sig = unsafe { &*(data.as_ptr() as *const PeSignature) };
        let sig_val = { sig.signature };
        if sig_val != PE_SIGNATURE {
            log::error!("[pe-loader] Bad PE signature: 0x{:08X}", sig_val);
            return None;
        }
        log::trace!("[pe-loader] PE signature valid");
        Some(sig)
    }
}

/// COFF File Header — immediately follows the PE signature.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct CoffHeader {
    /// Target machine architecture (0x8664 for x86_64).
    pub machine: u16,
    /// Number of section headers.
    pub number_of_sections: u16,
    /// Time/date stamp (Unix epoch).
    pub time_date_stamp: u32,
    /// File pointer to COFF symbol table (usually 0 for PE images).
    pub pointer_to_symbol_table: u32,
    /// Number of symbols.
    pub number_of_symbols: u32,
    /// Size of the optional header.
    pub size_of_optional_header: u16,
    /// Characteristics flags.
    pub characteristics: u16,
}

impl CoffHeader {
    pub const SIZE: usize = 20;

    pub fn parse(data: &[u8]) -> Option<&CoffHeader> {
        if data.len() < Self::SIZE {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const CoffHeader) };

        let machine = { header.machine };
        if machine != IMAGE_FILE_MACHINE_AMD64 {
            log::error!(
                "[pe-loader] Unsupported machine type: 0x{:04X} (need x86_64 = 0x{:04X})",
                machine, IMAGE_FILE_MACHINE_AMD64
            );
            return None;
        }

        let num_sections = { header.number_of_sections };
        let opt_header_size = { header.size_of_optional_header };
        log::trace!(
            "[pe-loader] COFF: {} sections, optional header size={}",
            num_sections, opt_header_size
        );
        Some(header)
    }

    /// Check if the image is an executable (not a DLL or object file).
    pub fn is_executable(&self) -> bool {
        self.characteristics & 0x0002 != 0 // IMAGE_FILE_EXECUTABLE_IMAGE
    }

    /// Check if the image is a DLL.
    pub fn is_dll(&self) -> bool {
        self.characteristics & 0x2000 != 0 // IMAGE_FILE_DLL
    }
}

/// COFF characteristic flags.
pub mod characteristics {
    pub const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
    pub const IMAGE_FILE_LARGE_ADDRESS_AWARE: u16 = 0x0020;
    pub const IMAGE_FILE_DLL: u16 = 0x2000;
}

/// Image subsystem values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ImageSubsystem {
    Unknown = 0,
    Native = 1,
    WindowsGui = 2,
    WindowsCui = 3,
    Os2Cui = 5,
    PosixCui = 7,
    WindowsCeGui = 9,
    EfiApplication = 10,
    EfiBootServiceDriver = 11,
    EfiRuntimeDriver = 12,
    EfiRom = 13,
    Xbox = 14,
    WindowsBootApplication = 16,
}

impl From<u16> for ImageSubsystem {
    fn from(v: u16) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::Native,
            2 => Self::WindowsGui,
            3 => Self::WindowsCui,
            5 => Self::Os2Cui,
            7 => Self::PosixCui,
            9 => Self::WindowsCeGui,
            10 => Self::EfiApplication,
            11 => Self::EfiBootServiceDriver,
            12 => Self::EfiRuntimeDriver,
            13 => Self::EfiRom,
            14 => Self::Xbox,
            16 => Self::WindowsBootApplication,
            _ => Self::Unknown,
        }
    }
}

/// Data directory entry — RVA and size of a PE data directory.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct DataDirectory {
    pub virtual_address: u32,
    pub size: u32,
}

impl DataDirectory {
    pub fn is_present(&self) -> bool {
        self.virtual_address != 0 && self.size != 0
    }
}

/// Well-known data directory indices.
pub mod data_dir_index {
    pub const EXPORT: usize = 0;
    pub const IMPORT: usize = 1;
    pub const RESOURCE: usize = 2;
    pub const EXCEPTION: usize = 3;
    pub const SECURITY: usize = 4;
    pub const BASERELOC: usize = 5;
    pub const DEBUG: usize = 6;
    pub const ARCHITECTURE: usize = 7;
    pub const GLOBALPTR: usize = 8;
    pub const TLS: usize = 9;
    pub const LOAD_CONFIG: usize = 10;
    pub const BOUND_IMPORT: usize = 11;
    pub const IAT: usize = 12;
    pub const DELAY_IMPORT: usize = 13;
    pub const CLR_RUNTIME: usize = 14;
    pub const RESERVED: usize = 15;
}

/// PE32+ Optional Header for x86_64 images.
#[derive(Debug, Clone)]
pub struct OptionalHeader {
    /// Magic number — must be 0x020B for PE32+.
    pub magic: u16,
    /// Linker major version.
    pub major_linker_version: u8,
    /// Linker minor version.
    pub minor_linker_version: u8,
    /// Size of .text section (or sum of all code sections).
    pub size_of_code: u32,
    /// Size of initialized data.
    pub size_of_initialized_data: u32,
    /// Size of uninitialized data (.bss).
    pub size_of_uninitialized_data: u32,
    /// RVA of entry point.
    pub address_of_entry_point: u32,
    /// RVA of the start of code section.
    pub base_of_code: u32,
    /// Preferred image base address.
    pub image_base: u64,
    /// Section alignment in memory (usually 4096).
    pub section_alignment: u32,
    /// File alignment on disk (usually 512).
    pub file_alignment: u32,
    /// Required OS major version.
    pub major_os_version: u16,
    /// Required OS minor version.
    pub minor_os_version: u16,
    /// Image major version.
    pub major_image_version: u16,
    /// Image minor version.
    pub minor_image_version: u16,
    /// Subsystem major version.
    pub major_subsystem_version: u16,
    /// Subsystem minor version.
    pub minor_subsystem_version: u16,
    /// Reserved (Win32VersionValue).
    pub win32_version_value: u32,
    /// Total size of image in memory, aligned to section_alignment.
    pub size_of_image: u32,
    /// Total size of headers, aligned to file_alignment.
    pub size_of_headers: u32,
    /// PE checksum.
    pub checksum: u32,
    /// Subsystem (CUI, GUI, Native, etc.).
    pub subsystem: ImageSubsystem,
    /// DLL characteristics (ASLR, DEP, etc.).
    pub dll_characteristics: u16,
    /// Size of stack reserve.
    pub size_of_stack_reserve: u64,
    /// Size of stack commit.
    pub size_of_stack_commit: u64,
    /// Size of heap reserve.
    pub size_of_heap_reserve: u64,
    /// Size of heap commit.
    pub size_of_heap_commit: u64,
    /// Loader flags (reserved, usually 0).
    pub loader_flags: u32,
    /// Number of data directory entries.
    pub number_of_rva_and_sizes: u32,
    /// Data directories (import, export, reloc, etc.).
    pub data_directories: Vec<DataDirectory>,
}

impl OptionalHeader {
    /// Minimum size of the fixed part of PE32+ optional header (before data directories).
    pub const FIXED_SIZE: usize = 112;

    /// Parse a PE32+ optional header from raw bytes.
    pub fn parse(data: &[u8]) -> Option<OptionalHeader> {
        if data.len() < Self::FIXED_SIZE {
            log::error!("[pe-loader] Optional header too small");
            return None;
        }

        let magic = u16::from_le_bytes([data[0], data[1]]);
        if magic != PE32_PLUS_MAGIC {
            log::error!("[pe-loader] Not PE32+: magic=0x{:04X}", magic);
            return None;
        }

        let read_u16 = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);
        let read_u32 = |off: usize| {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };
        let read_u64 = |off: usize| {
            u64::from_le_bytes([
                data[off], data[off + 1], data[off + 2], data[off + 3],
                data[off + 4], data[off + 5], data[off + 6], data[off + 7],
            ])
        };

        let number_of_rva_and_sizes = read_u32(108);
        let dir_count = number_of_rva_and_sizes as usize;
        let total_size = Self::FIXED_SIZE + dir_count * 8;
        if data.len() < total_size {
            log::error!("[pe-loader] Optional header too small for {} data directories", dir_count);
            return None;
        }

        let mut data_directories = Vec::with_capacity(dir_count);
        for i in 0..dir_count {
            let base = Self::FIXED_SIZE + i * 8;
            data_directories.push(DataDirectory {
                virtual_address: read_u32(base),
                size: read_u32(base + 4),
            });
        }

        let subsystem_raw = read_u16(68);

        let header = OptionalHeader {
            magic,
            major_linker_version: data[2],
            minor_linker_version: data[3],
            size_of_code: read_u32(4),
            size_of_initialized_data: read_u32(8),
            size_of_uninitialized_data: read_u32(12),
            address_of_entry_point: read_u32(16),
            base_of_code: read_u32(20),
            image_base: read_u64(24),
            section_alignment: read_u32(32),
            file_alignment: read_u32(36),
            major_os_version: read_u16(40),
            minor_os_version: read_u16(42),
            major_image_version: read_u16(44),
            minor_image_version: read_u16(46),
            major_subsystem_version: read_u16(48),
            minor_subsystem_version: read_u16(50),
            win32_version_value: read_u32(52),
            size_of_image: read_u32(56),
            size_of_headers: read_u32(60),
            checksum: read_u32(64),
            subsystem: ImageSubsystem::from(subsystem_raw),
            dll_characteristics: read_u16(70),
            size_of_stack_reserve: read_u64(72),
            size_of_stack_commit: read_u64(80),
            size_of_heap_reserve: read_u64(88),
            size_of_heap_commit: read_u64(96),
            loader_flags: read_u32(104),
            number_of_rva_and_sizes,
            data_directories,
        };

        log::info!(
            "[pe-loader] PE32+: entry=0x{:08X}, image_base=0x{:016X}, size=0x{:08X}, subsystem={:?}",
            header.address_of_entry_point, header.image_base,
            header.size_of_image, header.subsystem
        );

        Some(header)
    }

    /// Get a data directory by index, if present.
    pub fn data_directory(&self, index: usize) -> Option<&DataDirectory> {
        self.data_directories.get(index).filter(|d| d.is_present())
    }
}

/// DLL characteristics flags.
pub mod dll_characteristics {
    pub const IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA: u16 = 0x0020;
    pub const IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE: u16 = 0x0040;
    pub const IMAGE_DLLCHARACTERISTICS_FORCE_INTEGRITY: u16 = 0x0080;
    pub const IMAGE_DLLCHARACTERISTICS_NX_COMPAT: u16 = 0x0100;
    pub const IMAGE_DLLCHARACTERISTICS_NO_ISOLATION: u16 = 0x0200;
    pub const IMAGE_DLLCHARACTERISTICS_NO_SEH: u16 = 0x0400;
    pub const IMAGE_DLLCHARACTERISTICS_NO_BIND: u16 = 0x0800;
    pub const IMAGE_DLLCHARACTERISTICS_APPCONTAINER: u16 = 0x1000;
    pub const IMAGE_DLLCHARACTERISTICS_WDM_DRIVER: u16 = 0x2000;
    pub const IMAGE_DLLCHARACTERISTICS_GUARD_CF: u16 = 0x4000;
    pub const IMAGE_DLLCHARACTERISTICS_TERMINAL_SERVER_AWARE: u16 = 0x8000;
}
