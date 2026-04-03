//! $ATTRIBUTE_LIST (type 0x20) support for NTFS.
//!
//! When a file has many attributes that don't all fit in a single MFT entry,
//! NTFS uses extension records and places an $ATTRIBUTE_LIST in the base
//! MFT entry. This attribute contains entries that map (attribute type, name)
//! pairs to the MFT entry that actually holds the attribute data.
//!
//! Each $ATTRIBUTE_LIST entry has:
//! - Attribute type (4 bytes)
//! - Entry length (2 bytes)
//! - Name length in UTF-16 characters (1 byte)
//! - Name offset (1 byte)
//! - Starting VCN (8 bytes) — for non-resident attributes split across entries
//! - MFT reference (8 bytes) — points to the MFT entry containing the attribute
//! - Attribute ID (2 bytes)
//! - Name (variable, UTF-16LE)
//!
//! Reference: <https://flatcap.github.io/linux-ntfs/ntfs/attributes/attribute_list.html>

use alloc::string::String;
use alloc::vec::Vec;

use crate::attribute::AttributeType;
use crate::mft;

/// Minimum size of an $ATTRIBUTE_LIST entry (26 bytes).
pub const ATTR_LIST_ENTRY_MIN_SIZE: usize = 26;

/// A single entry in the $ATTRIBUTE_LIST attribute.
#[derive(Debug, Clone)]
pub struct AttributeListEntry {
    /// Attribute type of the referenced attribute.
    pub attr_type: AttributeType,
    /// Total size of this list entry in bytes.
    pub entry_length: u16,
    /// Length of the attribute name in UTF-16 characters.
    pub name_length: u8,
    /// Offset to the name from the start of this entry.
    pub name_offset: u8,
    /// Starting VCN for this attribute record (0 for the first/only extent).
    pub starting_vcn: u64,
    /// MFT reference to the entry containing this attribute.
    pub mft_reference: u64,
    /// Attribute instance ID.
    pub attribute_id: u16,
    /// The attribute name (empty string if unnamed).
    pub name: String,
    /// Raw UTF-16 name.
    pub name_utf16: Vec<u16>,
}

impl AttributeListEntry {
    /// Parse a single $ATTRIBUTE_LIST entry from bytes.
    ///
    /// Returns `(entry, bytes_consumed)` or `None` on error.
    pub fn from_bytes(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < ATTR_LIST_ENTRY_MIN_SIZE {
            log::error!("[ntfs::attr_list] entry too small: {} bytes (need >= {})",
                buf.len(), ATTR_LIST_ENTRY_MIN_SIZE);
            return None;
        }

        let type_val = read_u32(buf, 0x00);
        let attr_type = AttributeType::from_u32(type_val)?;

        if attr_type == AttributeType::End {
            return None;
        }

        let entry_length = read_u16(buf, 0x04);
        if (entry_length as usize) < ATTR_LIST_ENTRY_MIN_SIZE {
            log::error!("[ntfs::attr_list] invalid entry_length: {}", entry_length);
            return None;
        }
        if entry_length as usize > buf.len() {
            log::error!("[ntfs::attr_list] entry extends beyond buffer: {} > {}",
                entry_length, buf.len());
            return None;
        }

        let name_length = buf[0x06];
        let name_offset = buf[0x07];
        let starting_vcn = read_u64(buf, 0x08);
        let mft_reference = read_u64(buf, 0x10);
        let attribute_id = read_u16(buf, 0x18);

        // Parse the name if present
        let (name, name_utf16) = if name_length > 0 {
            let name_start = name_offset as usize;
            let name_bytes = name_length as usize * 2;
            if name_start + name_bytes > entry_length as usize {
                log::warn!("[ntfs::attr_list] name extends beyond entry");
                (String::new(), Vec::new())
            } else {
                let utf16: Vec<u16> = (0..name_length as usize)
                    .map(|i| {
                        let off = name_start + i * 2;
                        u16::from_le_bytes([buf[off], buf[off + 1]])
                    })
                    .collect();
                let name = String::from_utf16_lossy(&utf16);
                (name, utf16)
            }
        } else {
            (String::new(), Vec::new())
        };

        let entry_num = mft::mft_reference_number(mft_reference);
        log::trace!("[ntfs::attr_list] entry: type={} (0x{:08X}), name='{}', vcn={}, mft_ref=#{}, id={}",
            attr_type.name(), type_val, name, starting_vcn, entry_num, attribute_id);

        Some((AttributeListEntry {
            attr_type,
            entry_length,
            name_length,
            name_offset,
            starting_vcn,
            mft_reference,
            attribute_id,
            name,
            name_utf16,
        }, entry_length as usize))
    }

    /// Get the MFT entry number from the reference.
    #[inline]
    pub fn entry_number(&self) -> u64 {
        mft::mft_reference_number(self.mft_reference)
    }

    /// Get the sequence number from the reference.
    #[inline]
    pub fn sequence_number(&self) -> u16 {
        mft::mft_reference_sequence(self.mft_reference)
    }

    /// Whether this entry points to an unnamed attribute.
    #[inline]
    pub fn is_unnamed(&self) -> bool {
        self.name_length == 0
    }
}

/// Parse all entries from an $ATTRIBUTE_LIST attribute value.
///
/// The input is the raw attribute value bytes (resident or assembled from
/// non-resident data).
pub fn parse_attribute_list(buf: &[u8]) -> Vec<AttributeListEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos + ATTR_LIST_ENTRY_MIN_SIZE <= buf.len() {
        // Check for padding/end
        if buf[pos..pos + 4] == [0, 0, 0, 0] {
            break;
        }

        match AttributeListEntry::from_bytes(&buf[pos..]) {
            Some((entry, consumed)) => {
                // Ensure we advance by at least the minimum
                let advance = consumed.max(ATTR_LIST_ENTRY_MIN_SIZE);
                entries.push(entry);
                pos += advance;
            }
            None => {
                log::warn!("[ntfs::attr_list] failed to parse entry at offset 0x{:04X}", pos);
                break;
            }
        }
    }

    log::debug!("[ntfs::attr_list] parsed {} attribute list entries", entries.len());
    entries
}

/// Find entries for a specific attribute type in the attribute list.
pub fn find_in_attribute_list(
    entries: &[AttributeListEntry],
    attr_type: AttributeType,
) -> Vec<&AttributeListEntry> {
    entries.iter()
        .filter(|e| e.attr_type == attr_type)
        .collect()
}

/// Find entries for a specific attribute type and name.
pub fn find_named_in_attribute_list<'a>(
    entries: &'a [AttributeListEntry],
    attr_type: AttributeType,
    name: &str,
) -> Vec<&'a AttributeListEntry> {
    let target_utf16: Vec<u16> = name.encode_utf16().collect();
    entries.iter()
        .filter(|e| e.attr_type == attr_type && e.name_utf16 == target_utf16)
        .collect()
}

// --- Little-endian byte helpers ---

#[inline]
fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

#[inline]
fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
}

#[inline]
fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
        buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_entry() {
        // Build a minimal $ATTRIBUTE_LIST entry for $DATA (0x80), unnamed
        let mut buf = [0u8; 26];
        // attr_type = 0x80 ($DATA)
        buf[0..4].copy_from_slice(&0x00000080u32.to_le_bytes());
        // entry_length = 26
        buf[4..6].copy_from_slice(&26u16.to_le_bytes());
        // name_length = 0
        buf[6] = 0;
        // name_offset = 0x1A (26)
        buf[7] = 0x1A;
        // starting_vcn = 0
        buf[8..16].copy_from_slice(&0u64.to_le_bytes());
        // mft_reference = entry 42, seq 1
        let mft_ref = crate::mft::make_mft_reference(42, 1);
        buf[16..24].copy_from_slice(&mft_ref.to_le_bytes());
        // attribute_id = 3
        buf[24..26].copy_from_slice(&3u16.to_le_bytes());

        let (entry, consumed) = AttributeListEntry::from_bytes(&buf).unwrap();
        assert_eq!(consumed, 26);
        assert_eq!(entry.attr_type, AttributeType::Data);
        assert_eq!(entry.entry_number(), 42);
        assert_eq!(entry.attribute_id, 3);
        assert!(entry.is_unnamed());
    }

    #[test]
    fn test_parse_attribute_list() {
        // Build two entries
        let mut buf = [0u8; 52];

        // Entry 1: $STANDARD_INFORMATION
        buf[0..4].copy_from_slice(&0x00000010u32.to_le_bytes());
        buf[4..6].copy_from_slice(&26u16.to_le_bytes());
        buf[6] = 0;
        buf[7] = 0x1A;
        buf[8..16].copy_from_slice(&0u64.to_le_bytes());
        let ref1 = crate::mft::make_mft_reference(100, 1);
        buf[16..24].copy_from_slice(&ref1.to_le_bytes());
        buf[24..26].copy_from_slice(&0u16.to_le_bytes());

        // Entry 2: $DATA
        buf[26..30].copy_from_slice(&0x00000080u32.to_le_bytes());
        buf[30..32].copy_from_slice(&26u16.to_le_bytes());
        buf[32] = 0;
        buf[33] = 0x1A;
        buf[34..42].copy_from_slice(&0u64.to_le_bytes());
        let ref2 = crate::mft::make_mft_reference(200, 1);
        buf[42..50].copy_from_slice(&ref2.to_le_bytes());
        buf[50..52].copy_from_slice(&1u16.to_le_bytes());

        let entries = parse_attribute_list(&buf);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].attr_type, AttributeType::StandardInformation);
        assert_eq!(entries[0].entry_number(), 100);
        assert_eq!(entries[1].attr_type, AttributeType::Data);
        assert_eq!(entries[1].entry_number(), 200);
    }

    #[test]
    fn test_find_in_list() {
        let mut buf = [0u8; 26];
        buf[0..4].copy_from_slice(&0x00000080u32.to_le_bytes());
        buf[4..6].copy_from_slice(&26u16.to_le_bytes());
        buf[6] = 0;
        buf[7] = 0x1A;
        let mft_ref = crate::mft::make_mft_reference(50, 1);
        buf[16..24].copy_from_slice(&mft_ref.to_le_bytes());
        buf[24..26].copy_from_slice(&0u16.to_le_bytes());

        let entries = parse_attribute_list(&buf);
        let data_entries = find_in_attribute_list(&entries, AttributeType::Data);
        assert_eq!(data_entries.len(), 1);
        let si_entries = find_in_attribute_list(&entries, AttributeType::StandardInformation);
        assert_eq!(si_entries.len(), 0);
    }
}
