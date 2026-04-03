//! Block device abstraction and partition table parsing.
//!
//! Provides the `BlockDevice` trait that storage drivers (AHCI, NVMe, VirtIO-blk)
//! implement, plus GPT and MBR partition table parsing with filesystem type
//! auto-detection via partition type GUIDs (GPT) or type bytes (MBR).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::fs_trait::FsType;

// ── Well-known GPT partition type GUIDs ──────────────────────────────────────
//
// GPT partition type GUIDs identify what kind of content a partition holds.
// These are stored in "mixed-endian" format on disk: the first 3 components
// are little-endian, the last 2 are big-endian. The byte arrays below are
// in on-disk order (mixed-endian), matching what we read directly from the
// GPT partition entry.

/// Microsoft Basic Data (NTFS / FAT32).
/// GUID: EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
pub const GPT_GUID_MICROSOFT_BASIC: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44,
    0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// Linux filesystem data.
/// GUID: 0FC63DAF-8483-4772-8E79-3D69D8477DE4
pub const GPT_GUID_LINUX_FS: [u8; 16] = [
    0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47,
    0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
];

/// Linux swap.
pub const GPT_GUID_LINUX_SWAP: [u8; 16] = [
    0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43,
    0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B, 0x4F, 0x4F,
];

/// EFI System Partition.
pub const GPT_GUID_EFI_SYSTEM: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

// ── Well-known MBR partition type bytes ──────────────────────────────────────

/// MBR type for FAT32 (LBA).
pub const MBR_TYPE_FAT32_LBA: u8 = 0x0C;
/// MBR type for FAT32 (CHS).
pub const MBR_TYPE_FAT32_CHS: u8 = 0x0B;
/// MBR type for NTFS / exFAT.
pub const MBR_TYPE_NTFS: u8 = 0x07;
/// MBR type for Linux.
pub const MBR_TYPE_LINUX: u8 = 0x83;
/// MBR type for Linux swap.
pub const MBR_TYPE_LINUX_SWAP: u8 = 0x82;
/// MBR type for EFI System Partition.
pub const MBR_TYPE_EFI: u8 = 0xEF;

// ── Block device trait ───────────────────────────────────────────────────────

/// Trait for raw block storage backends (AHCI, NVMe, VirtIO-blk, etc.).
///
/// Implementations must be safe to share across async tasks (`Send + Sync`).
pub trait BlockDevice: Send + Sync {
    /// Read `buf.len()` bytes starting at byte offset `offset` from the device.
    ///
    /// Returns the number of bytes actually read.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<usize, DeviceError>;

    /// Write `data` starting at byte offset `offset` to the device.
    ///
    /// Returns the number of bytes actually written.
    fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<usize, DeviceError>;

    /// Flush any cached writes to persistent storage.
    fn flush(&self) -> Result<(), DeviceError>;

    /// Return the sector size in bytes (typically 512 or 4096).
    fn sector_size(&self) -> u32;

    /// Return the total size of the device in bytes.
    fn total_size(&self) -> u64;
}

/// Errors from block device operations.
#[derive(Debug)]
pub enum DeviceError {
    /// A hardware or bus-level I/O error.
    IoError,
    /// The requested offset or length is beyond the device boundary.
    OutOfBounds,
    /// The device is not ready (e.g. not initialized, link down).
    NotReady,
    /// The operation timed out waiting for hardware.
    Timeout,
}

impl core::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DeviceError::IoError => write!(f, "device I/O error"),
            DeviceError::OutOfBounds => write!(f, "access out of device bounds"),
            DeviceError::NotReady => write!(f, "device not ready"),
            DeviceError::Timeout => write!(f, "device timeout"),
        }
    }
}

// ── Partition abstraction ────────────────────────────────────────────────────

/// The partitioning scheme detected on a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionScheme {
    /// GUID Partition Table (modern UEFI disks).
    Gpt,
    /// Master Boot Record (legacy BIOS disks).
    Mbr,
    /// No partition table detected — the device may be a raw filesystem.
    None,
}

/// A single partition discovered on a block device.
#[derive(Debug, Clone)]
pub struct PartitionEntry {
    /// Partition index (0-based).
    pub index: usize,
    /// Starting LBA (logical block address).
    pub start_lba: u64,
    /// Number of sectors in this partition.
    pub sector_count: u64,
    /// Human-readable name (from GPT), or empty for MBR.
    pub name: String,
    /// Detected or hinted filesystem type.
    pub fs_type_hint: FsType,
    /// Raw partition type GUID (GPT) or type byte (MBR, stored in first byte).
    pub type_id: [u8; 16],
}

impl PartitionEntry {
    /// Byte offset of the partition start on the device.
    pub fn start_offset(&self, sector_size: u32) -> u64 {
        self.start_lba * sector_size as u64
    }

    /// Total size of the partition in bytes.
    pub fn size_bytes(&self, sector_size: u32) -> u64 {
        self.sector_count * sector_size as u64
    }
}

/// A view of a block device restricted to a single partition.
///
/// Translates byte offsets so that offset 0 maps to the partition's start LBA.
/// This allows filesystem implementations to work with partition-relative
/// offsets without knowing the partition's physical location on the device.
/// Bounds checking prevents reads/writes from escaping the partition boundary.
pub struct Partition<'a> {
    /// The underlying full device.
    pub device: &'a dyn BlockDevice,
    /// Byte offset of the partition start on the device.
    pub offset: u64,
    /// Size of the partition in bytes.
    pub size: u64,
}

impl<'a> Partition<'a> {
    /// Create a new partition view.
    pub fn new(device: &'a dyn BlockDevice, entry: &PartitionEntry) -> Self {
        let sector_size = device.sector_size();
        let offset = entry.start_offset(sector_size);
        let size = entry.size_bytes(sector_size);
        log::debug!(
            "device: partition view at offset={:#x}, size={:#x} ({} MiB)",
            offset,
            size,
            size / (1024 * 1024)
        );
        Self {
            device,
            offset,
            size,
        }
    }

    /// Read bytes from within this partition.
    pub fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<usize, DeviceError> {
        if offset + buf.len() as u64 > self.size {
            log::error!(
                "device: partition read out of bounds: offset={:#x}, len={}, partition_size={:#x}",
                offset,
                buf.len(),
                self.size
            );
            return Err(DeviceError::OutOfBounds);
        }
        self.device.read_bytes(self.offset + offset, buf)
    }

    /// Write bytes within this partition.
    pub fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<usize, DeviceError> {
        if offset + data.len() as u64 > self.size {
            log::error!(
                "device: partition write out of bounds: offset={:#x}, len={}, partition_size={:#x}",
                offset,
                data.len(),
                self.size
            );
            return Err(DeviceError::OutOfBounds);
        }
        self.device.write_bytes(self.offset + offset, data)
    }
}

// ── Partition table parsing ──────────────────────────────────────────────────

/// The GPT header signature: "EFI PART".
const GPT_SIGNATURE: [u8; 8] = *b"EFI PART";

/// The MBR boot signature at offset 510.
const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];

/// Detect the partition scheme present on a device.
pub fn detect_partition_scheme(device: &dyn BlockDevice) -> PartitionScheme {
    log::info!("device: detecting partition scheme");

    // Check for GPT at LBA 1
    let sector_size = device.sector_size() as u64;
    let mut gpt_header = vec![0u8; 512];
    if device.read_bytes(sector_size, &mut gpt_header).is_ok() {
        if gpt_header[0..8] == GPT_SIGNATURE {
            log::info!("device: detected GPT partition table");
            return PartitionScheme::Gpt;
        }
    }

    // Check for MBR at LBA 0
    let mut mbr = vec![0u8; 512];
    if device.read_bytes(0, &mut mbr).is_ok() {
        if mbr[510..512] == MBR_SIGNATURE {
            // Verify at least one partition entry is non-zero
            let has_entries = mbr[446..510].iter().any(|&b| b != 0);
            if has_entries {
                log::info!("device: detected MBR partition table");
                return PartitionScheme::Mbr;
            }
        }
    }

    log::info!("device: no partition table detected");
    PartitionScheme::None
}

/// Parse GPT partition entries from a device.
pub fn parse_gpt(device: &dyn BlockDevice) -> Result<Vec<PartitionEntry>, DeviceError> {
    log::info!("device: parsing GPT partition table");

    let sector_size = device.sector_size() as u64;
    let mut header = vec![0u8; 512];
    device.read_bytes(sector_size, &mut header)?;

    if header[0..8] != GPT_SIGNATURE {
        log::error!("device: GPT signature mismatch");
        return Err(DeviceError::IoError);
    }

    // Number of partition entries (at offset 80, u32 LE)
    let num_entries = u32::from_le_bytes([header[80], header[81], header[82], header[83]]) as usize;
    // Size of each entry (at offset 84, u32 LE)
    let entry_size = u32::from_le_bytes([header[84], header[85], header[86], header[87]]) as usize;
    // Partition entry array start LBA (at offset 72, u64 LE)
    let entries_lba = u64::from_le_bytes([
        header[72], header[73], header[74], header[75],
        header[76], header[77], header[78], header[79],
    ]);

    log::debug!(
        "device: GPT: {} entries, {} bytes each, starting at LBA {}",
        num_entries,
        entry_size,
        entries_lba
    );

    let entries_offset = entries_lba * sector_size;
    let total_bytes = num_entries * entry_size;
    let mut raw = vec![0u8; total_bytes];
    device.read_bytes(entries_offset, &mut raw)?;

    let mut partitions = Vec::new();
    for i in 0..num_entries {
        let base = i * entry_size;
        let entry = &raw[base..base + entry_size];

        // Type GUID at offset 0 (16 bytes)
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&entry[0..16]);

        // Skip empty entries (all-zero type GUID)
        if type_guid.iter().all(|&b| b == 0) {
            continue;
        }

        // First LBA at offset 32 (u64 LE)
        let first_lba = u64::from_le_bytes([
            entry[32], entry[33], entry[34], entry[35],
            entry[36], entry[37], entry[38], entry[39],
        ]);
        // Last LBA at offset 40 (u64 LE)
        let last_lba = u64::from_le_bytes([
            entry[40], entry[41], entry[42], entry[43],
            entry[44], entry[45], entry[46], entry[47],
        ]);

        // Name at offset 56, up to 72 bytes of UTF-16LE
        let name_bytes = &entry[56..core::cmp::min(entry_size, 128)];
        let name = parse_utf16le_name(name_bytes);

        let fs_hint = gpt_guid_to_fs_type(&type_guid);
        let sector_count = last_lba - first_lba + 1;

        log::info!(
            "device: GPT partition {}: name={:?}, LBA {}-{}, sectors={}, type_hint={}",
            i,
            name,
            first_lba,
            last_lba,
            sector_count,
            fs_hint
        );

        partitions.push(PartitionEntry {
            index: partitions.len(),
            start_lba: first_lba,
            sector_count,
            name,
            fs_type_hint: fs_hint,
            type_id: type_guid,
        });
    }

    log::info!("device: GPT: found {} partition(s)", partitions.len());
    Ok(partitions)
}

/// Parse MBR partition entries from a device.
pub fn parse_mbr(device: &dyn BlockDevice) -> Result<Vec<PartitionEntry>, DeviceError> {
    log::info!("device: parsing MBR partition table");

    let mut mbr = vec![0u8; 512];
    device.read_bytes(0, &mut mbr)?;

    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        log::error!("device: MBR signature mismatch");
        return Err(DeviceError::IoError);
    }

    // MBR partition table: 4 entries of 16 bytes each, starting at offset 446.
    // Each entry layout:
    //   Offset 0: boot indicator (0x80 = bootable, 0x00 = not)
    //   Offset 1-3: CHS address of first sector (ignored, we use LBA)
    //   Offset 4: partition type byte (0x07=NTFS, 0x83=Linux, 0x0C=FAT32, etc.)
    //   Offset 5-7: CHS address of last sector (ignored)
    //   Offset 8-11: LBA of first sector (u32 LE)
    //   Offset 12-15: number of sectors (u32 LE)
    let mut partitions = Vec::new();
    for i in 0..4 {
        let base = 446 + i * 16;
        let entry = &mbr[base..base + 16];

        let type_byte = entry[4];
        if type_byte == 0x00 {
            continue; // Empty entry
        }

        // Start LBA at offset 8 (u32 LE)
        let start_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as u64;
        // Sector count at offset 12 (u32 LE)
        let sector_count = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as u64;

        let fs_hint = mbr_type_to_fs_type(type_byte);

        let mut type_id = [0u8; 16];
        type_id[0] = type_byte;

        log::info!(
            "device: MBR partition {}: type=0x{:02X}, LBA start={}, sectors={}, hint={}",
            i,
            type_byte,
            start_lba,
            sector_count,
            fs_hint
        );

        partitions.push(PartitionEntry {
            index: partitions.len(),
            start_lba,
            sector_count,
            name: String::new(),
            fs_type_hint: fs_hint,
            type_id,
        });
    }

    log::info!("device: MBR: found {} partition(s)", partitions.len());
    Ok(partitions)
}

/// Map a GPT type GUID to a filesystem type hint.
///
/// GPT type GUIDs identify the partition type but not the specific filesystem.
/// For example, `GPT_GUID_LINUX_FS` is used for both ext4 and btrfs. The
/// caller should use `detect_filesystem()` (superblock probing) to determine
/// the actual filesystem type. This function provides a "best guess" hint.
fn gpt_guid_to_fs_type(guid: &[u8; 16]) -> FsType {
    if guid == &GPT_GUID_LINUX_FS {
        // Could be ext4 or btrfs -- caller must probe the superblock.
        FsType::Ext4
    } else if guid == &GPT_GUID_MICROSOFT_BASIC {
        // Could be NTFS or FAT32 — caller must probe the superblock.
        FsType::Ntfs
    } else if guid == &GPT_GUID_EFI_SYSTEM {
        FsType::Fat32
    } else {
        FsType::Unknown
    }
}

/// Map an MBR partition type byte to a filesystem type hint.
fn mbr_type_to_fs_type(type_byte: u8) -> FsType {
    match type_byte {
        MBR_TYPE_FAT32_LBA | MBR_TYPE_FAT32_CHS => FsType::Fat32,
        MBR_TYPE_NTFS => FsType::Ntfs,
        MBR_TYPE_LINUX => FsType::Ext4,
        _ => FsType::Unknown,
    }
}

/// Parse a UTF-16LE encoded name, stopping at the first null character.
///
/// GPT partition names are stored as up to 36 UTF-16LE code units (72 bytes)
/// starting at offset 56 in each partition entry. This function decodes them
/// to a Rust `String`, handling only BMP characters (no surrogate pair support).
fn parse_utf16le_name(bytes: &[u8]) -> String {
    let mut name = String::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let code_unit = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if code_unit == 0 {
            break;
        }
        if let Some(c) = char::from_u32(code_unit as u32) {
            name.push(c);
        }
        i += 2;
    }
    name
}
