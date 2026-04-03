//! Export Directory Table parsing.
//!
//! DLLs and some executables export functions that other modules can import.
//! The export table maps function names and ordinals to RVAs within the image.

use alloc::string::String;
use alloc::vec::Vec;

/// A parsed export entry.
#[derive(Debug, Clone)]
pub struct ExportEntry {
    /// Ordinal number (biased by ordinal base).
    pub ordinal: u16,
    /// Function name, if exported by name (some exports are ordinal-only).
    pub name: Option<String>,
    /// RVA of the exported function.
    pub function_rva: u32,
    /// If this is a forwarder, the forwarded string (e.g., "NTDLL.RtlMoveMemory").
    pub forwarder: Option<String>,
}

/// Raw IMAGE_EXPORT_DIRECTORY — 40 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct RawExportDirectory {
    characteristics: u32,
    time_date_stamp: u32,
    major_version: u16,
    minor_version: u16,
    name_rva: u32,
    ordinal_base: u32,
    number_of_functions: u32,
    number_of_names: u32,
    address_of_functions: u32,
    address_of_names: u32,
    address_of_name_ordinals: u32,
}

/// Parsed export directory.
#[derive(Debug, Clone)]
pub struct ExportDirectory {
    /// DLL name from the export directory.
    pub dll_name: String,
    /// Ordinal base.
    pub ordinal_base: u32,
    /// Exported functions.
    pub entries: Vec<ExportEntry>,
}

impl ExportDirectory {
    /// Parse the export directory from the image data.
    ///
    /// `export_rva` and `export_size` come from data directory index 0.
    pub fn parse(data: &[u8], export_rva: u32, export_size: u32) -> Option<ExportDirectory> {
        let rva = export_rva as usize;
        if rva + 40 > data.len() {
            return None;
        }

        let raw = unsafe { &*(data[rva..].as_ptr() as *const RawExportDirectory) };

        let dll_name = read_cstring(data, raw.name_rva as usize);
        let ordinal_base = raw.ordinal_base;
        let num_functions = raw.number_of_functions as usize;
        let num_names = raw.number_of_names as usize;
        let func_table_rva = raw.address_of_functions as usize;
        let name_table_rva = raw.address_of_names as usize;
        let ordinal_table_rva = raw.address_of_name_ordinals as usize;

        // The export address range — used to detect forwarders
        let export_start = export_rva as usize;
        let export_end = export_start + export_size as usize;

        let mut entries = Vec::with_capacity(num_functions);

        // First, create entries for all functions by ordinal
        for i in 0..num_functions {
            let func_offset = func_table_rva + i * 4;
            if func_offset + 4 > data.len() {
                break;
            }
            let func_rva = u32::from_le_bytes([
                data[func_offset], data[func_offset + 1],
                data[func_offset + 2], data[func_offset + 3],
            ]);

            // Check if this is a forwarder (RVA points within the export directory)
            let forwarder = if (func_rva as usize) >= export_start && (func_rva as usize) < export_end {
                Some(read_cstring(data, func_rva as usize))
            } else {
                None
            };

            entries.push(ExportEntry {
                ordinal: (ordinal_base as u16).wrapping_add(i as u16),
                name: None,
                function_rva: func_rva,
                forwarder,
            });
        }

        // Then, assign names to those entries that have them
        for i in 0..num_names {
            let name_ptr_offset = name_table_rva + i * 4;
            let ordinal_offset = ordinal_table_rva + i * 2;

            if name_ptr_offset + 4 > data.len() || ordinal_offset + 2 > data.len() {
                break;
            }

            let name_rva = u32::from_le_bytes([
                data[name_ptr_offset], data[name_ptr_offset + 1],
                data[name_ptr_offset + 2], data[name_ptr_offset + 3],
            ]);
            let ordinal_index = u16::from_le_bytes([
                data[ordinal_offset], data[ordinal_offset + 1],
            ]) as usize;

            if ordinal_index < entries.len() {
                entries[ordinal_index].name = Some(read_cstring(data, name_rva as usize));
            }
        }

        log::info!(
            "[pe-loader] Exports: '{}' — {} functions, {} named, base ordinal={}",
            dll_name, num_functions, num_names, ordinal_base
        );

        Some(ExportDirectory {
            dll_name,
            ordinal_base,
            entries,
        })
    }

    /// Look up an export by name.
    pub fn find_by_name(&self, name: &str) -> Option<&ExportEntry> {
        self.entries.iter().find(|e| e.name.as_deref() == Some(name))
    }

    /// Look up an export by ordinal.
    pub fn find_by_ordinal(&self, ordinal: u16) -> Option<&ExportEntry> {
        let index = ordinal.wrapping_sub(self.ordinal_base as u16) as usize;
        self.entries.get(index)
    }
}

/// Read a null-terminated C string.
fn read_cstring(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    let end = data[offset..].iter().position(|&b| b == 0).unwrap_or(data.len() - offset);
    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
}
