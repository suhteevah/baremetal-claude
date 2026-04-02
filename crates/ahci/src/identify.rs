//! ATA IDENTIFY DEVICE response parsing.
//!
//! The IDENTIFY DEVICE command (0xEC) returns 512 bytes (256 words) of device
//! information. This module parses the most important fields.
//!
//! Reference: ATA/ATAPI Command Set (ACS-3), Section 7.12.

use alloc::string::String;

/// Parsed information from an ATA IDENTIFY DEVICE response.
#[derive(Debug, Clone)]
pub struct IdentifyData {
    /// Serial number (20 ASCII characters, trimmed).
    pub serial: String,
    /// Firmware revision (8 ASCII characters, trimmed).
    pub firmware_rev: String,
    /// Model number (40 ASCII characters, trimmed).
    pub model: String,
    /// Total addressable sectors (LBA28). 0 if LBA48 is used instead.
    pub total_sectors_lba28: u32,
    /// Total addressable sectors (LBA48). Use this if non-zero.
    pub total_sectors_lba48: u64,
    /// Logical sector size in bytes (typically 512).
    pub logical_sector_size: u32,
    /// Physical sector size in bytes (typically 512 or 4096).
    pub physical_sector_size: u32,
    /// Whether 48-bit LBA is supported.
    pub supports_lba48: bool,
    /// Whether the write cache is supported.
    pub supports_write_cache: bool,
    /// Whether NCQ (Native Command Queuing) is supported.
    pub supports_ncq: bool,
    /// NCQ queue depth (1..32) if NCQ is supported.
    pub ncq_queue_depth: u8,
    /// Raw 256-word buffer for anything else the caller needs.
    pub raw: [u16; 256],
}

impl IdentifyData {
    /// Parse an IDENTIFY DEVICE response from a raw 512-byte buffer.
    ///
    /// The buffer must be exactly 512 bytes. Data is in little-endian word format,
    /// but ATA string fields have their bytes swapped within each word.
    pub fn parse(buf: &[u8; 512]) -> Self {
        // Interpret as 256 little-endian 16-bit words.
        let mut words = [0u16; 256];
        for i in 0..256 {
            words[i] = u16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
        }

        // --- Serial Number: words 10-19 (20 chars, byte-swapped per word) ---
        let serial = ata_string_from_words(&words[10..20]);
        log::info!("[ahci] IDENTIFY: serial = \"{}\"", serial);

        // --- Firmware Revision: words 23-26 (8 chars) ---
        let firmware_rev = ata_string_from_words(&words[23..27]);
        log::info!("[ahci] IDENTIFY: firmware = \"{}\"", firmware_rev);

        // --- Model Number: words 27-46 (40 chars) ---
        let model = ata_string_from_words(&words[27..47]);
        log::info!("[ahci] IDENTIFY: model = \"{}\"", model);

        // --- Total sectors LBA28: words 60-61 ---
        let total_sectors_lba28 = (words[61] as u32) << 16 | (words[60] as u32);
        log::info!(
            "[ahci] IDENTIFY: LBA28 sectors = {} ({} MiB)",
            total_sectors_lba28,
            (total_sectors_lba28 as u64 * 512) / (1024 * 1024)
        );

        // --- LBA48 support: word 83 bit 10 ---
        let supports_lba48 = words[83] & (1 << 10) != 0;
        log::info!("[ahci] IDENTIFY: LBA48 supported = {}", supports_lba48);

        // --- Total sectors LBA48: words 100-103 ---
        let total_sectors_lba48 = if supports_lba48 {
            (words[103] as u64) << 48
                | (words[102] as u64) << 32
                | (words[101] as u64) << 16
                | (words[100] as u64)
        } else {
            0
        };
        if supports_lba48 {
            log::info!(
                "[ahci] IDENTIFY: LBA48 sectors = {} ({} GiB)",
                total_sectors_lba48,
                (total_sectors_lba48 * 512) / (1024 * 1024 * 1024)
            );
        }

        // --- Sector size: word 106 ---
        // Bit 14 = 1, bit 15 = 0 means the field is valid.
        // Bit 12: device logical sector size > 256 words.
        // Bit 13: device has multiple logical sectors per physical sector.
        let word106 = words[106];
        let sector_size_valid = (word106 & (1 << 14)) != 0 && (word106 & (1 << 15)) == 0;

        let logical_sector_size = if sector_size_valid && (word106 & (1 << 12)) != 0 {
            // Words 117-118 contain logical sector size in words.
            let lss_words = (words[118] as u32) << 16 | (words[117] as u32);
            lss_words * 2 // Convert words to bytes
        } else {
            512
        };

        let physical_sector_size = if sector_size_valid && (word106 & (1 << 13)) != 0 {
            // Physical sector = 2^N logical sectors, N = bits 3:0.
            let exponent = word106 & 0x0F;
            logical_sector_size * (1u32 << exponent)
        } else {
            logical_sector_size
        };

        log::info!(
            "[ahci] IDENTIFY: logical sector = {} bytes, physical sector = {} bytes",
            logical_sector_size, physical_sector_size
        );

        // --- Write cache: word 82 bit 5 ---
        let supports_write_cache = words[82] & (1 << 5) != 0;
        log::info!(
            "[ahci] IDENTIFY: write cache supported = {}",
            supports_write_cache
        );

        // --- NCQ: word 76 ---
        // If word 76 != 0x0000 and != 0xFFFF, SATA features are valid.
        // Bit 8 = NCQ supported. Bits 4:0 = queue depth - 1.
        let supports_ncq;
        let ncq_queue_depth;
        if words[76] != 0x0000 && words[76] != 0xFFFF {
            supports_ncq = words[76] & (1 << 8) != 0;
            ncq_queue_depth = if supports_ncq {
                ((words[75] & 0x1F) + 1) as u8
            } else {
                0
            };
        } else {
            supports_ncq = false;
            ncq_queue_depth = 0;
        }
        log::info!(
            "[ahci] IDENTIFY: NCQ supported = {}, queue depth = {}",
            supports_ncq, ncq_queue_depth
        );

        Self {
            serial,
            firmware_rev,
            model,
            total_sectors_lba28,
            total_sectors_lba48,
            logical_sector_size,
            physical_sector_size,
            supports_lba48,
            supports_write_cache,
            supports_ncq,
            ncq_queue_depth,
            raw: words,
        }
    }

    /// Return the total addressable sector count (prefers LBA48 if available).
    pub fn total_sectors(&self) -> u64 {
        if self.supports_lba48 && self.total_sectors_lba48 > 0 {
            self.total_sectors_lba48
        } else {
            self.total_sectors_lba28 as u64
        }
    }

    /// Return the total capacity in bytes.
    pub fn capacity_bytes(&self) -> u64 {
        self.total_sectors() * self.logical_sector_size as u64
    }
}

/// Decode an ATA string from a slice of 16-bit words.
///
/// ATA strings are stored with bytes swapped within each word (big-endian per word),
/// and padded with spaces on the right.
fn ata_string_from_words(words: &[u16]) -> String {
    let mut chars = alloc::vec::Vec::with_capacity(words.len() * 2);
    for &word in words {
        // High byte first, then low byte (ATA byte-swap convention).
        chars.push((word >> 8) as u8);
        chars.push((word & 0xFF) as u8);
    }
    // Convert to string, replace non-ASCII with '?', and trim trailing spaces.
    let s: String = chars
        .iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect();
    s.trim().into()
}
