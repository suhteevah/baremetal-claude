//! PE loader: ties together all parsing modules to load a PE executable.
//!
//! `load_pe()` takes raw binary data, validates headers, maps sections into
//! a contiguous image buffer, parses imports/exports/relocations, and returns
//! a `LoadedPe` structure ready for import resolution and execution.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::dos_header::DosHeader;
use crate::pe_header::{PeSignature, CoffHeader, OptionalHeader, ImageSubsystem, data_dir_index};
use crate::sections::{SectionHeader, MappedSection};
use crate::imports::{ImportDirectory, ImportEntry};
use crate::exports::ExportDirectory;
use crate::relocations::{parse_relocations, apply_relocations, BaseRelocation};

/// Errors that can occur during PE loading.
#[derive(Debug)]
pub enum PeError {
    /// Invalid DOS header or missing MZ magic.
    InvalidDosHeader,
    /// Invalid PE signature.
    InvalidPeSignature,
    /// Invalid or unsupported COFF header.
    InvalidCoffHeader,
    /// Invalid or unsupported Optional header.
    InvalidOptionalHeader,
    /// Section data extends past end of file.
    SectionOutOfBounds(String),
    /// Image is not for x86_64.
    WrongArchitecture,
    /// Unresolved import that we cannot satisfy.
    UnresolvedImport(String, String),
    /// Image too large to allocate.
    ImageTooLarge(u32),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDosHeader => write!(f, "invalid DOS header"),
            Self::InvalidPeSignature => write!(f, "invalid PE signature"),
            Self::InvalidCoffHeader => write!(f, "invalid COFF header"),
            Self::InvalidOptionalHeader => write!(f, "invalid optional header"),
            Self::SectionOutOfBounds(name) => write!(f, "section '{}' out of bounds", name),
            Self::WrongArchitecture => write!(f, "not an x86_64 PE"),
            Self::UnresolvedImport(dll, func) => write!(f, "unresolved import: {}!{}", dll, func),
            Self::ImageTooLarge(size) => write!(f, "image too large: {} bytes", size),
        }
    }
}

/// Thread Environment Block (TEB) layout for Win64.
///
/// Located via the GS segment register (GS:[0x30] = self pointer).
/// We allocate this on the heap and set GS base to point to it.
#[derive(Debug)]
#[repr(C)]
pub struct Teb {
    /// SEH exception list (not used, null).
    pub exception_list: u64,
    /// Stack base (high address).
    pub stack_base: u64,
    /// Stack limit (low address).
    pub stack_limit: u64,
    /// SubSystemTib.
    pub sub_system_tib: u64,
    /// Fiber data / version.
    pub fiber_data: u64,
    /// Arbitrary data pointer.
    pub arbitrary_user_pointer: u64,
    /// Self pointer — GS:[0x30] must point back to the TEB.
    pub teb_self: u64,
    /// Environment pointer.
    pub environment_pointer: u64,
    /// Process ID.
    pub process_id: u64,
    /// Thread ID.
    pub thread_id: u64,
    /// Padding to offset 0x58 for LastErrorValue.
    pub _padding1: [u64; 2],
    /// Last error value (GetLastError / SetLastError).
    pub last_error_value: u32,
    /// Padding.
    pub _padding2: u32,
    /// More fields up to TLS slots...
    pub _reserved: [u8; 0x700],
    /// Thread-local storage slots (64 slots, at offset 0xe10 in real Windows).
    pub tls_slots: [u64; 64],
}

/// Process Environment Block (PEB) layout for Win64.
///
/// Accessible via TEB.ProcessEnvironmentBlock (offset 0x60 in real TEB).
#[derive(Debug)]
#[repr(C)]
pub struct Peb {
    /// Is being debugged flag.
    pub being_debugged: u8,
    pub _padding1: [u8; 7],
    /// Image base address of the loaded PE.
    pub image_base_address: u64,
    /// Pointer to PEB_LDR_DATA (loader data).
    pub ldr: u64,
    /// Process parameters (RTL_USER_PROCESS_PARAMETERS).
    pub process_parameters: u64,
    /// Reserved.
    pub _reserved: [u8; 0x100],
    /// Process heap handle.
    pub process_heap: u64,
}

/// A fully loaded PE image, ready for import patching and execution.
#[derive(Debug)]
pub struct LoadedPe {
    /// The loaded image buffer (sections mapped at correct RVAs).
    pub image: Vec<u8>,
    /// Actual base address of the image in our address space.
    pub image_base: u64,
    /// Entry point absolute address.
    pub entry_point: u64,
    /// Subsystem (GUI, CUI, Native).
    pub subsystem: ImageSubsystem,
    /// Is this a DLL?
    pub is_dll: bool,
    /// Parsed sections with their mapped data.
    pub sections: Vec<MappedSection>,
    /// Parsed import directory — lists all DLL imports to resolve.
    pub imports: ImportDirectory,
    /// Parsed export directory (if present).
    pub exports: Option<ExportDirectory>,
    /// Base relocations (already applied).
    pub relocations: Vec<BaseRelocation>,
    /// Stack reserve size from PE header.
    pub stack_reserve: u64,
    /// Stack commit size from PE header.
    pub stack_commit: u64,
    /// Heap reserve size from PE header.
    pub heap_reserve: u64,
    /// Heap commit size from PE header.
    pub heap_commit: u64,
}

/// Load a PE executable from raw bytes.
///
/// This performs the full loading process:
/// 1. Parse DOS header, PE signature, COFF header, Optional header
/// 2. Parse section headers
/// 3. Allocate image buffer and map sections
/// 4. Parse imports, exports, relocations
/// 5. Apply base relocations (if loaded at non-preferred address)
///
/// After this, the caller must resolve imports (patch the IAT) before jumping
/// to the entry point.
pub fn load_pe(binary: &[u8]) -> Result<LoadedPe, PeError> {
    log::info!("[pe-loader] Loading PE binary ({} bytes)", binary.len());

    // --- Step 1: Parse DOS header ---
    let dos = DosHeader::parse(binary).ok_or(PeError::InvalidDosHeader)?;
    let pe_offset = dos.pe_offset();

    // --- Step 2: Parse PE signature ---
    if pe_offset + PeSignature::SIZE > binary.len() {
        return Err(PeError::InvalidPeSignature);
    }
    let _pe_sig = PeSignature::parse(&binary[pe_offset..]).ok_or(PeError::InvalidPeSignature)?;

    // --- Step 3: Parse COFF header ---
    let coff_offset = pe_offset + PeSignature::SIZE;
    if coff_offset + CoffHeader::SIZE > binary.len() {
        return Err(PeError::InvalidCoffHeader);
    }
    let coff = CoffHeader::parse(&binary[coff_offset..]).ok_or(PeError::InvalidCoffHeader)?;

    // --- Step 4: Parse Optional header ---
    let opt_offset = coff_offset + CoffHeader::SIZE;
    let opt_size = coff.size_of_optional_header as usize;
    if opt_offset + opt_size > binary.len() {
        return Err(PeError::InvalidOptionalHeader);
    }
    let optional = OptionalHeader::parse(&binary[opt_offset..opt_offset + opt_size])
        .ok_or(PeError::InvalidOptionalHeader)?;

    // --- Step 5: Parse section headers ---
    let sections_offset = opt_offset + opt_size;
    let section_count = coff.number_of_sections as usize;
    let section_headers = SectionHeader::parse_all(&binary[sections_offset..], section_count);

    // --- Step 6: Allocate image buffer ---
    let image_size = optional.size_of_image as usize;
    if image_size > 256 * 1024 * 1024 {
        // Sanity check: refuse images > 256 MiB
        return Err(PeError::ImageTooLarge(optional.size_of_image));
    }
    let mut image = vec![0u8; image_size];

    // Copy headers into the image
    let header_size = optional.size_of_headers as usize;
    if header_size <= binary.len() && header_size <= image.len() {
        image[..header_size].copy_from_slice(&binary[..header_size]);
    }

    // --- Step 7: Map sections ---
    let mut mapped_sections = Vec::with_capacity(section_headers.len());
    for section in &section_headers {
        let vaddr = section.virtual_address as usize;
        let vsize = section.virtual_size as usize;
        let raw_offset = section.pointer_to_raw_data as usize;
        let raw_size = section.size_of_raw_data as usize;

        // Copy section data from file into image at the correct RVA
        if raw_size > 0 && raw_offset + raw_size <= binary.len() {
            let copy_size = raw_size.min(vsize).min(image.len().saturating_sub(vaddr));
            if vaddr + copy_size <= image.len() {
                image[vaddr..vaddr + copy_size].copy_from_slice(&binary[raw_offset..raw_offset + copy_size]);
            }
        }

        // If virtual size > raw size, the remainder is zero-filled (already done by vec![0])

        let data = if vaddr + vsize <= image.len() {
            image[vaddr..vaddr + vsize].to_vec()
        } else {
            Vec::new()
        };

        mapped_sections.push(MappedSection {
            name: section.name.clone(),
            base: optional.image_base + vaddr as u64,
            size: vsize,
            data,
            executable: section.is_executable(),
            writable: section.is_writable(),
        });

        log::debug!(
            "[pe-loader] Mapped section '{}' at 0x{:08X} ({} bytes)",
            section.name, vaddr, vsize
        );
    }

    // --- Step 8: Parse imports ---
    let imports = if let Some(dir) = optional.data_directory(data_dir_index::IMPORT) {
        ImportDirectory::parse(&image, dir.virtual_address, dir.size)
    } else {
        ImportDirectory { entries: Vec::new() }
    };

    // --- Step 9: Parse exports ---
    let exports = if let Some(dir) = optional.data_directory(data_dir_index::EXPORT) {
        ExportDirectory::parse(&image, dir.virtual_address, dir.size)
    } else {
        None
    };

    // --- Step 10: Parse and apply relocations ---
    let relocations = if let Some(dir) = optional.data_directory(data_dir_index::BASERELOC) {
        parse_relocations(&image, dir.virtual_address, dir.size)
    } else {
        Vec::new()
    };

    // For now, we load at the preferred base, so no relocation needed.
    // When we implement proper virtual memory, we may need to relocate.
    let actual_base = optional.image_base;
    apply_relocations(&mut image, actual_base, optional.image_base, &relocations);

    let entry_point = actual_base + optional.address_of_entry_point as u64;

    log::info!(
        "[pe-loader] PE loaded: base=0x{:016X}, entry=0x{:016X}, {} sections, {} imports, {} relocations",
        actual_base, entry_point, mapped_sections.len(), imports.entries.len(), relocations.len()
    );

    Ok(LoadedPe {
        image,
        image_base: actual_base,
        entry_point,
        subsystem: optional.subsystem,
        is_dll: coff.is_dll(),
        sections: mapped_sections,
        imports,
        exports,
        relocations,
        stack_reserve: optional.size_of_stack_reserve,
        stack_commit: optional.size_of_stack_commit,
        heap_reserve: optional.size_of_heap_reserve,
        heap_commit: optional.size_of_heap_commit,
    })
}

/// Patch a single IAT entry with a function pointer.
///
/// `image` is the loaded PE image buffer.
/// `iat_rva` is the RVA of the IAT slot to patch.
/// `func_addr` is the address of the implementation function.
pub fn patch_iat_entry(image: &mut [u8], iat_rva: usize, func_addr: u64) {
    if iat_rva + 8 <= image.len() {
        image[iat_rva..iat_rva + 8].copy_from_slice(&func_addr.to_le_bytes());
    }
}

/// Patch all IAT entries for an import entry, given a resolver function.
///
/// The resolver takes a function name (or ordinal) and returns the address
/// of our implementation, or None if unresolved.
pub fn patch_import_entry<F>(
    image: &mut [u8],
    entry: &ImportEntry,
    resolver: F,
) -> Result<u32, PeError>
where
    F: Fn(&str, &crate::imports::ImportLookup) -> Option<u64>,
{
    let mut iat_offset = entry.iat_rva as usize;
    let mut resolved = 0u32;

    for func in &entry.functions {
        let addr = resolver(&entry.dll_name, func);
        if let Some(addr) = addr {
            patch_iat_entry(image, iat_offset, addr);
            resolved += 1;
        } else {
            let func_name = match func {
                crate::imports::ImportLookup::Name(_, name) => name.clone(),
                crate::imports::ImportLookup::Ordinal(ord) => alloc::format!("ordinal#{}", ord),
            };
            log::warn!(
                "[pe-loader] Unresolved import: {}!{}",
                entry.dll_name, func_name
            );
            // Write a stub that will trap if called (INT3 sled)
            patch_iat_entry(image, iat_offset, 0xCCCC_CCCC_CCCC_CCCCu64);
        }
        iat_offset += 8;
    }

    Ok(resolved)
}
