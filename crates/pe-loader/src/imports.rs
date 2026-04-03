//! Import Directory Table parsing.
//!
//! PE executables import functions from DLLs via the Import Directory Table.
//! Each entry describes one DLL and contains:
//! - The DLL name (e.g., "kernel32.dll")
//! - Import Lookup Table (ILT) — array of entries, each naming a function or ordinal
//! - Import Address Table (IAT) — initially identical to ILT, but the loader overwrites
//!   each entry with the actual function address at load time
//!
//! Our loader patches the IAT to point to our native Win32 implementations.

use alloc::string::String;
use alloc::vec::Vec;

/// A parsed import directory entry — represents one imported DLL.
#[derive(Debug, Clone)]
pub struct ImportEntry {
    /// DLL name (e.g., "KERNEL32.dll").
    pub dll_name: String,
    /// Imported functions from this DLL.
    pub functions: Vec<ImportLookup>,
    /// RVA of the Import Address Table for this DLL.
    pub iat_rva: u32,
}

/// An individual import — either by name or by ordinal.
#[derive(Debug, Clone)]
pub enum ImportLookup {
    /// Import by name: (hint index, function name).
    Name(u16, String),
    /// Import by ordinal number.
    Ordinal(u16),
}

impl ImportLookup {
    /// Get the function name, if this is a named import.
    pub fn name(&self) -> Option<&str> {
        match self {
            ImportLookup::Name(_, name) => Some(name.as_str()),
            ImportLookup::Ordinal(_) => None,
        }
    }
}

/// Raw IMAGE_IMPORT_DESCRIPTOR — 20 bytes each.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct RawImportDescriptor {
    /// RVA of the Import Lookup Table (or characteristics).
    original_first_thunk: u32,
    /// Time/date stamp (0 if not bound).
    time_date_stamp: u32,
    /// Forwarder chain index.
    forwarder_chain: u32,
    /// RVA of the DLL name string.
    name_rva: u32,
    /// RVA of the Import Address Table.
    first_thunk: u32,
}

/// Parsed import directory containing all DLL imports.
#[derive(Debug, Clone)]
pub struct ImportDirectory {
    pub entries: Vec<ImportEntry>,
}

impl ImportDirectory {
    /// Parse the import directory from the image data.
    ///
    /// `data` is the full loaded image. `import_rva` and `import_size` come from
    /// the data directory entry for IMAGE_DIRECTORY_ENTRY_IMPORT.
    pub fn parse(data: &[u8], import_rva: u32, _import_size: u32) -> ImportDirectory {
        let mut entries = Vec::new();
        let mut offset = import_rva as usize;

        loop {
            if offset + 20 > data.len() {
                break;
            }

            let desc = unsafe { &*(data[offset..].as_ptr() as *const RawImportDescriptor) };

            // Null terminator — all fields zero means end of import directory
            if desc.name_rva == 0 && desc.first_thunk == 0 {
                break;
            }

            // Read DLL name
            let dll_name = read_cstring(data, desc.name_rva as usize);

            // Parse the Import Lookup Table (or IAT if ILT is absent)
            let lookup_rva = if desc.original_first_thunk != 0 {
                desc.original_first_thunk
            } else {
                desc.first_thunk
            };

            let functions = parse_import_lookup_table(data, lookup_rva as usize);

            let first_thunk_val = { desc.first_thunk };
            log::debug!(
                "[pe-loader] Import: '{}' — {} functions, IAT RVA=0x{:08X}",
                dll_name, functions.len(), first_thunk_val
            );

            entries.push(ImportEntry {
                dll_name,
                functions,
                iat_rva: first_thunk_val,
            });

            offset += 20;
        }

        log::info!("[pe-loader] Parsed {} import DLLs", entries.len());
        ImportDirectory { entries }
    }

    /// Get all imported DLL names (lowercased for matching).
    pub fn dll_names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.dll_name.clone()).collect()
    }
}

/// Parse an Import Lookup Table (array of 64-bit entries, null-terminated).
fn parse_import_lookup_table(data: &[u8], rva: usize) -> Vec<ImportLookup> {
    let mut functions = Vec::new();
    let mut offset = rva;

    loop {
        if offset + 8 > data.len() {
            break;
        }

        let entry = u64::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ]);

        if entry == 0 {
            break; // Null terminator
        }

        if entry & (1 << 63) != 0 {
            // Import by ordinal — bit 63 set, ordinal in bits 0-15
            let ordinal = (entry & 0xFFFF) as u16;
            functions.push(ImportLookup::Ordinal(ordinal));
        } else {
            // Import by name — entry is RVA to IMAGE_IMPORT_BY_NAME
            let hint_name_rva = entry as u32 as usize;
            if hint_name_rva + 2 < data.len() {
                let hint = u16::from_le_bytes([data[hint_name_rva], data[hint_name_rva + 1]]);
                let name = read_cstring(data, hint_name_rva + 2);
                functions.push(ImportLookup::Name(hint, name));
            }
        }

        offset += 8;
    }

    functions
}

/// Read a null-terminated C string from the data at the given offset.
fn read_cstring(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    let end = data[offset..].iter().position(|&b| b == 0).unwrap_or(data.len() - offset);
    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
}
