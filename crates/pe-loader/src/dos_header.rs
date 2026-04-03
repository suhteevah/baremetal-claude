//! DOS MZ header parsing.
//!
//! Every PE file starts with a legacy DOS header. We only care about two fields:
//! `e_magic` (must be 0x5A4D = "MZ") and `e_lfanew` (file offset to the PE header).

/// DOS MZ header magic number.
pub const DOS_MAGIC: u16 = 0x5A4D; // 'MZ'

/// IMAGE_DOS_HEADER — the first 64 bytes of any PE file.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct DosHeader {
    /// Magic number — must be 0x5A4D ("MZ").
    pub e_magic: u16,
    /// Bytes on last page of file.
    pub e_cblp: u16,
    /// Pages in file.
    pub e_cp: u16,
    /// Relocations.
    pub e_crlc: u16,
    /// Size of header in paragraphs.
    pub e_cparhdr: u16,
    /// Minimum extra paragraphs needed.
    pub e_minalloc: u16,
    /// Maximum extra paragraphs needed.
    pub e_maxalloc: u16,
    /// Initial (relative) SS value.
    pub e_ss: u16,
    /// Initial SP value.
    pub e_sp: u16,
    /// Checksum.
    pub e_csum: u16,
    /// Initial IP value.
    pub e_ip: u16,
    /// Initial (relative) CS value.
    pub e_cs: u16,
    /// File address of relocation table.
    pub e_lfarlc: u16,
    /// Overlay number.
    pub e_ovno: u16,
    /// Reserved words.
    pub e_res: [u16; 4],
    /// OEM identifier.
    pub e_oemid: u16,
    /// OEM information.
    pub e_oeminfo: u16,
    /// Reserved words.
    pub e_res2: [u16; 10],
    /// File address of new exe header (PE header offset).
    pub e_lfanew: i32,
}

impl DosHeader {
    /// Size of the DOS header in bytes.
    pub const SIZE: usize = 64;

    /// Parse a DOS header from raw bytes.
    ///
    /// Returns `None` if the buffer is too small or the magic is wrong.
    pub fn parse(data: &[u8]) -> Option<&DosHeader> {
        if data.len() < Self::SIZE {
            log::error!("[pe-loader] DOS header: buffer too small ({} < {})", data.len(), Self::SIZE);
            return None;
        }

        let header = unsafe { &*(data.as_ptr() as *const DosHeader) };

        let magic = { header.e_magic };
        if magic != DOS_MAGIC {
            log::error!(
                "[pe-loader] DOS header: bad magic 0x{:04X} (expected 0x{:04X})",
                magic, DOS_MAGIC
            );
            return None;
        }

        let lfanew = header.e_lfanew;
        if lfanew < Self::SIZE as i32 || lfanew as usize >= data.len() {
            log::error!("[pe-loader] DOS header: e_lfanew={} out of range", lfanew);
            return None;
        }

        log::trace!("[pe-loader] DOS header OK, PE header at offset 0x{:X}", lfanew);
        Some(header)
    }

    /// Get the file offset to the PE signature.
    pub fn pe_offset(&self) -> usize {
        self.e_lfanew as usize
    }
}
