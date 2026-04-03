//! High-level NTFS filesystem read/write API.
//!
//! This module provides the main `NtfsFs` type that ties together the boot sector,
//! MFT, attributes, indexes, data runs, journal, compression, and attribute lists
//! into a usable filesystem interface.
//!
//! ## Usage
//!
//! Implement the `BlockDevice` trait for your storage backend, then:
//!
//! ```rust,no_run
//! use claudio_ntfs::{NtfsFs, BlockDevice};
//!
//! let fs = NtfsFs::mount(my_device).expect("mount failed");
//! let data = fs.read_file(b"/Windows/hello.txt").expect("read failed");
//! fs.write_file(b"/output.txt", &data).expect("write failed");
//! ```

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use crate::attribute::{AttributeHeader, AttributeType, ATTR_FLAG_COMPRESSED};
use crate::attribute_list;
use crate::boot_sector::{BootSector, BOOT_SECTOR_SIZE};
use crate::compression;
use crate::data_runs::{self, DataRun};
use crate::filename::{FileNameAttr, FileNamespace};
use crate::index::{self, IndexEntry, IndexNodeHeader, IndexRoot, INDEX_ENTRY_FLAG_LAST_ENTRY, INDEX_ENTRY_FLAG_HAS_SUB_NODE, INDEX_HEADER_FLAG_LARGE_INDEX};
use crate::journal::{Journal, JournalOp};
use crate::mft::{self, MftEntry, MFT_ENTRY_ROOT, MFT_ENTRY_UPCASE, MFT_ENTRY_LOGFILE};

use crate::upcase::UpCaseTable;

/// Errors that can occur during NTFS filesystem operations.
#[derive(Debug)]
pub enum NtfsError {
    /// The device returned an I/O error.
    IoError,
    /// The boot sector is invalid or not NTFS.
    InvalidBootSector,
    /// An MFT entry is corrupt or has invalid fixup.
    CorruptMftEntry(u64),
    /// A required attribute was not found.
    AttributeNotFound(&'static str),
    /// The requested path was not found.
    NotFound,
    /// A path component is not a directory.
    NotADirectory,
    /// The target path already exists.
    AlreadyExists,
    /// The filesystem is corrupt.
    Corrupt(&'static str),
    /// A filename exceeds the maximum length (255 UTF-16 characters).
    NameTooLong,
    /// The path is invalid (empty, etc.).
    InvalidPath,
    /// The target is a directory when a file was expected.
    IsADirectory,
    /// The target is a file when a directory was expected.
    IsNotADirectory,
    /// No free MFT entries available.
    NoFreeMftEntries,
    /// No free clusters available.
    NoFreeClusters,
    /// Compressed or encrypted attributes are not supported.
    Unsupported(&'static str),
}

impl fmt::Display for NtfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NtfsError::IoError => write!(f, "I/O error"),
            NtfsError::InvalidBootSector => write!(f, "invalid NTFS boot sector"),
            NtfsError::CorruptMftEntry(n) => write!(f, "corrupt MFT entry #{}", n),
            NtfsError::AttributeNotFound(a) => write!(f, "attribute not found: {}", a),
            NtfsError::NotFound => write!(f, "not found"),
            NtfsError::NotADirectory => write!(f, "not a directory"),
            NtfsError::AlreadyExists => write!(f, "already exists"),
            NtfsError::Corrupt(msg) => write!(f, "filesystem corrupt: {}", msg),
            NtfsError::NameTooLong => write!(f, "filename too long"),
            NtfsError::InvalidPath => write!(f, "invalid path"),
            NtfsError::IsADirectory => write!(f, "is a directory"),
            NtfsError::IsNotADirectory => write!(f, "is not a directory"),
            NtfsError::NoFreeMftEntries => write!(f, "no free MFT entries"),
            NtfsError::NoFreeClusters => write!(f, "no free clusters"),
            NtfsError::Unsupported(msg) => write!(f, "unsupported: {}", msg),
        }
    }
}

/// Trait for the underlying block storage device.
///
/// Implement this for your NVMe driver, virtio-blk, RAM disk, or disk image
/// to provide NTFS with raw byte access.
pub trait BlockDevice {
    /// Read `buf.len()` bytes from the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition.
    /// Returns `Ok(())` on success.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), NtfsError>;

    /// Write `buf.len()` bytes to the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition.
    /// Returns `Ok(())` on success.
    fn write_bytes(&self, offset: u64, buf: &[u8]) -> Result<(), NtfsError>;

    /// Flush any cached writes to the underlying storage.
    ///
    /// Called after metadata updates to ensure durability.
    fn flush(&self) -> Result<(), NtfsError> {
        Ok(())
    }
}

/// Directory entry returned by `list_dir`.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// MFT entry number for this file/directory.
    pub mft_entry: u64,
    /// Filename.
    pub name: String,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// Creation time (Windows FILETIME).
    pub creation_time: u64,
    /// Modification time (Windows FILETIME).
    pub modification_time: u64,
    /// File attribute flags.
    pub flags: u32,
}

/// Main NTFS filesystem handle.
///
/// Holds the parsed boot sector, cached MFT location, the $UpCase table,
/// and the transaction journal.
/// All operations go through this struct.
pub struct NtfsFs<D: BlockDevice> {
    /// The underlying block device.
    pub device: D,
    /// The parsed boot sector / BPB.
    pub boot_sector: BootSector,
    /// The $UpCase table for case-insensitive comparison.
    pub upcase: UpCaseTable,
    /// Cluster size in bytes (cached from boot sector).
    cluster_size: u64,
    /// MFT record size in bytes (cached from boot sector).
    mft_record_size: u64,
    /// Byte offset of the MFT on disk (cached).
    mft_offset: u64,
    /// Data runs for the $MFT itself (since $MFT can be non-contiguous).
    mft_data_runs: Vec<DataRun>,
    /// Transaction journal for crash recovery (interior mutability for &self API).
    journal: RefCell<Journal>,
}

impl<D: BlockDevice> NtfsFs<D> {
    /// Mount an NTFS filesystem from a block device.
    ///
    /// Reads the boot sector, locates the MFT, parses the $MFT entry to get
    /// the full MFT data run list, loads the $UpCase table, and initializes
    /// the journal (with recovery if needed).
    pub fn mount(device: D) -> Result<Self, NtfsError> {
        log::info!("[ntfs] mounting NTFS filesystem...");

        // Step 1: Read and parse the boot sector
        let mut boot_buf = [0u8; BOOT_SECTOR_SIZE];
        device.read_bytes(0, &mut boot_buf)?;
        let boot_sector = BootSector::from_bytes(&boot_buf)
            .ok_or(NtfsError::InvalidBootSector)?;

        let cluster_size = boot_sector.cluster_size();
        let mft_record_size = boot_sector.mft_record_size();
        let mft_offset = boot_sector.mft_byte_offset();

        log::info!("[ntfs] cluster_size={}, mft_record_size={}, mft_offset=0x{:X}",
            cluster_size, mft_record_size, mft_offset);

        // Step 2: Read the $MFT entry (entry 0) to get its own data runs
        let mut mft_entry_buf = vec![0u8; mft_record_size as usize];
        device.read_bytes(mft_offset, &mut mft_entry_buf)?;
        let mft_entry = MftEntry::from_bytes(&mft_entry_buf, mft_record_size as usize)
            .ok_or(NtfsError::CorruptMftEntry(0))?;

        log::debug!("[ntfs] $MFT entry parsed: {:?}", mft_entry.header);

        // Get the $DATA attribute of $MFT to find all MFT clusters
        let mft_data_runs = Self::read_data_runs_from_entry(&mft_entry, AttributeType::Data)?;
        log::info!("[ntfs] $MFT has {} data runs", mft_data_runs.len());
        for (i, run) in mft_data_runs.iter().enumerate() {
            log::debug!("[ntfs] $MFT run {}: lcn={}, length={} clusters, sparse={}",
                i, run.lcn, run.length, run.is_sparse);
        }

        // Step 3: Load the $UpCase table (MFT entry 10)
        let upcase = {
            let upcase_entry_offset = Self::resolve_mft_entry_offset(
                &mft_data_runs, MFT_ENTRY_UPCASE, mft_record_size, cluster_size, mft_offset
            )?;
            let mut upcase_buf = vec![0u8; mft_record_size as usize];
            device.read_bytes(upcase_entry_offset, &mut upcase_buf)?;
            let upcase_entry = MftEntry::from_bytes(&upcase_buf, mft_record_size as usize)
                .ok_or(NtfsError::CorruptMftEntry(MFT_ENTRY_UPCASE))?;

            log::debug!("[ntfs] $UpCase entry parsed: {:?}", upcase_entry.header);

            // Read the $DATA attribute of $UpCase
            let upcase_data = Self::read_attribute_data_static(
                &device, &upcase_entry, AttributeType::Data,
                &mft_data_runs, mft_record_size, cluster_size, mft_offset,
            )?;

            UpCaseTable::from_bytes(&upcase_data).unwrap_or_else(|| {
                log::warn!("[ntfs] failed to parse $UpCase, using ASCII fallback");
                UpCaseTable::default_ascii()
            })
        };

        // Step 4: Initialize journal
        // Try to read the $LogFile (MFT entry 2) to get journal location
        let journal = {
            let logfile_offset = Self::resolve_mft_entry_offset(
                &mft_data_runs, MFT_ENTRY_LOGFILE, mft_record_size, cluster_size, mft_offset,
            );

            match logfile_offset {
                Ok(offset) => {
                    // Use the $LogFile's disk location as journal storage area
                    let journal_offset = offset;
                    let journal_capacity = mft_record_size * 64; // Reserve space
                    // Try to read existing journal
                    let mut journal_buf = vec![0u8; journal_capacity as usize];
                    match device.read_bytes(journal_offset, &mut journal_buf) {
                        Ok(()) => {
                            match Journal::from_bytes(&journal_buf, journal_offset, journal_capacity) {
                                Some(mut j) => {
                                    if !j.header.clean_shutdown {
                                        log::warn!("[ntfs] unclean shutdown detected, checking journal...");
                                        // Rollback uncommitted entries
                                        let uncommitted = j.uncommitted_entries();
                                        if !uncommitted.is_empty() {
                                            log::warn!("[ntfs] rolling back {} uncommitted journal entries",
                                                uncommitted.len());
                                            for entry in uncommitted.iter().rev() {
                                                if !entry.undo_data.is_empty() {
                                                    log::debug!("[ntfs] rollback: offset=0x{:X}, {} bytes",
                                                        entry.target_offset, entry.undo_data.len());
                                                    let _ = device.write_bytes(
                                                        entry.target_offset, &entry.undo_data
                                                    );
                                                }
                                            }
                                            let _ = device.flush();
                                        }
                                    }
                                    j.mark_clean();
                                    j
                                }
                                None => {
                                    log::info!("[ntfs] no existing journal, creating new");
                                    Journal::new(journal_offset, journal_capacity)
                                }
                            }
                        }
                        Err(_) => {
                            log::warn!("[ntfs] failed to read journal area, creating new");
                            Journal::new(journal_offset, journal_capacity)
                        }
                    }
                }
                Err(_) => {
                    log::warn!("[ntfs] $LogFile not found, journal disabled");
                    Journal::new(0, 0)
                }
            }
        };

        log::info!("[ntfs] NTFS filesystem mounted successfully: volume_size={} bytes",
            boot_sector.volume_size());

        Ok(NtfsFs {
            device,
            boot_sector,
            upcase,
            cluster_size,
            mft_record_size,
            mft_offset,
            mft_data_runs,
            journal: RefCell::new(journal),
        })
    }

    // -----------------------------------------------------------------------
    // Journal helpers
    // -----------------------------------------------------------------------

    /// Perform a journaled write to the device.
    ///
    /// This implements write-ahead logging (WAL): before modifying any on-disk
    /// data, we first read the old content (for undo/rollback on crash), then
    /// log both old and new data to the journal. Only after the journal entry
    /// is recorded do we perform the actual write. On mount after a crash,
    /// uncommitted entries can be rolled back using the undo data, ensuring
    /// filesystem consistency.
    fn journaled_write(&self, op: JournalOp, offset: u64, new_data: &[u8]) -> Result<(), NtfsError> {
        // Read old data for undo
        let mut old_data = vec![0u8; new_data.len()];
        self.device.read_bytes(offset, &mut old_data)?;

        // Log the operation
        {
            let mut journal = self.journal.borrow_mut();
            journal.mark_dirty();
            let _lsn = journal.log_write(op, offset, &old_data, new_data);
        }

        // Perform the actual write
        self.device.write_bytes(offset, new_data)?;

        Ok(())
    }

    /// Flush the journal to disk.
    fn flush_journal(&self) -> Result<(), NtfsError> {
        let journal = self.journal.borrow();
        if journal.journal_capacity == 0 {
            return Ok(()); // Journal disabled
        }
        let journal_bytes = journal.to_bytes();
        if journal_bytes.len() as u64 <= journal.journal_capacity {
            self.device.write_bytes(journal.journal_offset, &journal_bytes)?;
        }
        self.device.flush()
    }

    // -----------------------------------------------------------------------
    // Attribute list support
    // -----------------------------------------------------------------------

    /// Find an attribute, checking $ATTRIBUTE_LIST if the attribute isn't
    /// directly present in the base MFT entry.
    fn find_attribute_with_list(
        &self,
        entry: &MftEntry,
        attr_type: AttributeType,
    ) -> Result<(MftEntry, AttributeHeader, usize), NtfsError> {
        // First try the direct approach
        if let Some((hdr, offset)) = entry.find_attribute(attr_type) {
            return Ok((entry.clone(), hdr, offset));
        }

        // Check for $ATTRIBUTE_LIST
        if let Some((_, al_offset)) = entry.find_attribute(AttributeType::AttributeList) {
            log::debug!("[ntfs] checking $ATTRIBUTE_LIST for type 0x{:08X}", attr_type as u32);

            // Read the attribute list data
            let al_data = if let Some(data) = entry.resident_data(al_offset) {
                data.to_vec()
            } else {
                // Non-resident attribute list
                self.read_non_resident_data(entry, al_offset)?
            };

            let al_entries = attribute_list::parse_attribute_list(&al_data);
            let matches = attribute_list::find_in_attribute_list(&al_entries, attr_type);

            for al_entry in matches {
                let ext_entry_num = al_entry.entry_number();
                if ext_entry_num == 0 {
                    continue; // Skip self-references to the base entry
                }

                log::trace!("[ntfs] $ATTRIBUTE_LIST: type 0x{:08X} in MFT#{}",
                    attr_type as u32, ext_entry_num);

                // Read the extension MFT entry
                let ext_entry = self.read_mft_entry(ext_entry_num)?;
                if let Some((hdr, offset)) = ext_entry.find_attribute(attr_type) {
                    return Ok((ext_entry, hdr, offset));
                }
            }
        }

        Err(NtfsError::AttributeNotFound(attr_type.name()))
    }

    // -----------------------------------------------------------------------
    // Core MFT / data run operations
    // -----------------------------------------------------------------------

    /// Read the data runs from a $DATA (or other) attribute in an MFT entry.
    fn read_data_runs_from_entry(entry: &MftEntry, attr_type: AttributeType) -> Result<Vec<DataRun>, NtfsError> {
        let (hdr, offset) = entry.find_attribute(attr_type)
            .ok_or(NtfsError::AttributeNotFound("$DATA"))?;

        if !hdr.non_resident {
            log::trace!("[ntfs] attribute is resident, no data runs");
            return Ok(Vec::new());
        }

        let run_bytes = entry.data_run_bytes(offset)
            .ok_or(NtfsError::Corrupt("failed to read data run bytes"))?;

        Ok(data_runs::decode_data_runs(run_bytes))
    }

    /// Resolve the byte offset of a specific MFT entry number on disk,
    /// accounting for non-contiguous MFT data runs.
    ///
    /// The $MFT file itself can be fragmented across the volume, described
    /// by its own data runs. This function translates a logical MFT entry
    /// number into a physical byte offset by:
    ///   1. Computing the byte offset within the MFT's logical data stream
    ///   2. Converting that to a VCN (Virtual Cluster Number)
    ///   3. Looking up the VCN in the MFT's data run mapping to get an LCN
    ///   4. Computing the final physical byte offset from LCN + offset
    fn resolve_mft_entry_offset(
        runs: &[DataRun],
        entry_number: u64,
        mft_record_size: u64,
        cluster_size: u64,
        mft_offset: u64,
    ) -> Result<u64, NtfsError> {
        if runs.is_empty() {
            // MFT is contiguous from mft_offset (simple case)
            let offset = mft_offset + entry_number * mft_record_size;
            log::trace!("[ntfs] MFT entry {} at offset 0x{:X} (contiguous)", entry_number, offset);
            return Ok(offset);
        }

        // Calculate which byte within the MFT's data we need
        let mft_byte = entry_number * mft_record_size;
        let vcn = mft_byte / cluster_size;
        let vcn_offset = mft_byte % cluster_size;

        let map = data_runs::build_vcn_map(runs);
        let (lcn, is_sparse) = map.resolve(vcn)
            .ok_or_else(|| {
                log::error!("[ntfs] MFT entry {} (VCN {}) not in data runs", entry_number, vcn);
                NtfsError::CorruptMftEntry(entry_number)
            })?;

        if is_sparse {
            log::error!("[ntfs] MFT entry {} is in a sparse region", entry_number);
            return Err(NtfsError::CorruptMftEntry(entry_number));
        }

        let offset = lcn * cluster_size + vcn_offset;
        log::trace!("[ntfs] MFT entry {} at offset 0x{:X} (VCN {} -> LCN {})",
            entry_number, offset, vcn, lcn);
        Ok(offset)
    }

    /// Read an MFT entry by entry number.
    pub fn read_mft_entry(&self, entry_number: u64) -> Result<MftEntry, NtfsError> {
        let offset = Self::resolve_mft_entry_offset(
            &self.mft_data_runs, entry_number,
            self.mft_record_size, self.cluster_size, self.mft_offset,
        )?;

        let mut buf = vec![0u8; self.mft_record_size as usize];
        self.device.read_bytes(offset, &mut buf)?;

        log::trace!("[ntfs] reading MFT entry {} from offset 0x{:X}", entry_number, offset);
        MftEntry::from_bytes(&buf, self.mft_record_size as usize)
            .ok_or(NtfsError::CorruptMftEntry(entry_number))
    }

    /// Write an MFT entry back to disk (with journal logging).
    pub fn write_mft_entry(&self, entry_number: u64, entry: &MftEntry) -> Result<(), NtfsError> {
        let offset = Self::resolve_mft_entry_offset(
            &self.mft_data_runs, entry_number,
            self.mft_record_size, self.cluster_size, self.mft_offset,
        )?;

        let buf = entry.to_bytes();
        log::debug!("[ntfs] writing MFT entry {} ({} bytes) to offset 0x{:X}",
            entry_number, buf.len(), offset);

        // Journal the MFT write
        self.journaled_write(JournalOp::MftWrite, offset, &buf)?;
        self.device.flush()
    }

    // -----------------------------------------------------------------------
    // Non-resident data reading with compression support
    // -----------------------------------------------------------------------

    /// Read all data for a non-resident attribute, following its data runs.
    ///
    /// Non-resident attributes store their data in clusters scattered across
    /// the volume, described by a compact "data runs" (mapping pairs) encoding.
    /// Each data run specifies a (cluster_count, starting_LCN_delta) pair.
    /// This function:
    ///   1. Decodes the data runs into absolute LCN ranges
    ///   2. Reads each cluster range from disk into a contiguous buffer
    ///   3. Handles sparse runs (fill with zeros -- no disk read needed)
    ///   4. If the attribute is LZNT1-compressed, decompresses the raw data
    ///
    /// For compressed attributes, we read `allocated_size` bytes (which
    /// includes compressed data + sparse padding) then decompress to get
    /// `data_size` bytes of output.
    fn read_non_resident_data(
        &self,
        entry: &MftEntry,
        attr_offset: usize,
    ) -> Result<Vec<u8>, NtfsError> {
        let hdr = AttributeHeader::from_bytes(&entry.data[attr_offset..])
            .ok_or(NtfsError::Corrupt("missing attribute header"))?;

        let nr = entry.non_resident_header(attr_offset)
            .ok_or(NtfsError::Corrupt("missing non-resident header"))?;

        let run_bytes = entry.data_run_bytes(attr_offset)
            .ok_or(NtfsError::Corrupt("missing data runs"))?;

        let runs = data_runs::decode_data_runs(run_bytes);

        let is_compressed = hdr.flags & ATTR_FLAG_COMPRESSED != 0 && nr.compression_unit > 0;

        // For compressed data, read the allocated_size (includes compressed + sparse runs)
        // For uncompressed, read data_size
        let read_size = if is_compressed {
            nr.allocated_size as usize
        } else {
            nr.data_size as usize
        };

        let mut raw_data = vec![0u8; read_size];
        let mut bytes_read = 0usize;

        log::debug!("[ntfs] reading non-resident data: {} bytes across {} runs (compressed={})",
            read_size, runs.len(), is_compressed);

        for run in &runs {
            if bytes_read >= read_size {
                break;
            }

            let run_bytes_total = run.length * self.cluster_size;
            let to_read = (read_size - bytes_read).min(run_bytes_total as usize);

            if run.is_sparse {
                // Sparse run: fill with zeros (already zeroed in vec)
                log::trace!("[ntfs] sparse run: {} bytes of zeros", to_read);
                bytes_read += to_read;
                continue;
            }

            let disk_offset = run.lcn * self.cluster_size;
            log::trace!("[ntfs] reading {} bytes from disk offset 0x{:X}", to_read, disk_offset);
            self.device.read_bytes(disk_offset, &mut raw_data[bytes_read..bytes_read + to_read])?;
            bytes_read += to_read;
        }

        // If compressed, decompress the raw data
        if is_compressed {
            log::debug!("[ntfs] decompressing LZNT1: {} raw bytes -> {} expected",
                raw_data.len(), nr.data_size);
            compression::decompress_attribute(
                &raw_data, nr.data_size, nr.compression_unit, self.cluster_size,
            ).ok_or(NtfsError::Corrupt("LZNT1 decompression failed"))
        } else {
            raw_data.truncate(nr.data_size as usize);
            log::debug!("[ntfs] read {} bytes of non-resident data", raw_data.len());
            Ok(raw_data)
        }
    }

    /// Read attribute data (works for both resident and non-resident).
    /// Also handles $ATTRIBUTE_LIST indirection.
    fn read_attribute_data_for_entry(
        &self,
        entry: &MftEntry,
        attr_type: AttributeType,
    ) -> Result<Vec<u8>, NtfsError> {
        // Try direct lookup first, then fall back to attribute list
        let (actual_entry, hdr, offset) = self.find_attribute_with_list(entry, attr_type)?;

        if !hdr.non_resident {
            let data = actual_entry.resident_data(offset)
                .ok_or(NtfsError::Corrupt("failed to read resident data"))?;
            log::trace!("[ntfs] read {} bytes of resident {} data", data.len(), attr_type.name());
            return Ok(data.to_vec());
        }

        self.read_non_resident_data(&actual_entry, offset)
    }

    /// Static version of attribute data reading (used during mount before self is available).
    fn read_attribute_data_static(
        device: &D,
        entry: &MftEntry,
        attr_type: AttributeType,
        _mft_runs: &[DataRun],
        _mft_record_size: u64,
        cluster_size: u64,
        _mft_offset: u64,
    ) -> Result<Vec<u8>, NtfsError> {
        let (hdr, offset) = entry.find_attribute(attr_type)
            .ok_or(NtfsError::AttributeNotFound(attr_type.name()))?;

        if !hdr.non_resident {
            // Resident: data is inline
            let data = entry.resident_data(offset)
                .ok_or(NtfsError::Corrupt("failed to read resident data"))?;
            log::trace!("[ntfs] read {} bytes of resident {} data", data.len(), attr_type.name());
            return Ok(data.to_vec());
        }

        // Non-resident: follow data runs
        let nr = entry.non_resident_header(offset)
            .ok_or(NtfsError::Corrupt("missing non-resident header"))?;

        let run_bytes = entry.data_run_bytes(offset)
            .ok_or(NtfsError::Corrupt("missing data runs"))?;

        let runs = data_runs::decode_data_runs(run_bytes);
        let data_size = nr.data_size as usize;
        let mut result = vec![0u8; data_size];
        let mut bytes_read = 0usize;

        for run in &runs {
            if bytes_read >= data_size {
                break;
            }
            let run_bytes_total = run.length * cluster_size;
            let to_read = (data_size - bytes_read).min(run_bytes_total as usize);

            if run.is_sparse {
                bytes_read += to_read;
                continue;
            }

            let disk_offset = run.lcn * cluster_size;
            device.read_bytes(disk_offset, &mut result[bytes_read..bytes_read + to_read])?;
            bytes_read += to_read;
        }

        log::trace!("[ntfs] read {} bytes of non-resident {} data", bytes_read, attr_type.name());
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Path resolution and directory operations
    // -----------------------------------------------------------------------

    /// Split a path into components.
    fn split_path(path: &[u8]) -> Result<Vec<&[u8]>, NtfsError> {
        if path.is_empty() {
            return Err(NtfsError::InvalidPath);
        }

        // Strip leading slash(es)
        let path = if path[0] == b'/' || path[0] == b'\\' {
            &path[1..]
        } else {
            path
        };

        if path.is_empty() {
            return Ok(Vec::new()); // Root directory
        }

        let components: Vec<&[u8]> = path
            .split(|&b| b == b'/' || b == b'\\')
            .filter(|c| !c.is_empty())
            .collect();

        Ok(components)
    }

    /// List the contents of a directory by reading its index entries.
    fn read_directory_entries(&self, dir_entry_number: u64) -> Result<Vec<IndexEntry>, NtfsError> {
        let dir_mft = self.read_mft_entry(dir_entry_number)?;

        if !dir_mft.header.is_directory() {
            log::error!("[ntfs] MFT entry {} is not a directory", dir_entry_number);
            return Err(NtfsError::NotADirectory);
        }

        // Read $INDEX_ROOT attribute
        let (_, ir_offset) = dir_mft.find_attribute(AttributeType::IndexRoot)
            .ok_or(NtfsError::AttributeNotFound("$INDEX_ROOT"))?;

        let ir_data = dir_mft.resident_data(ir_offset)
            .ok_or(NtfsError::Corrupt("$INDEX_ROOT must be resident"))?;

        let index_root = IndexRoot::from_bytes(ir_data)
            .ok_or(NtfsError::Corrupt("invalid $INDEX_ROOT"))?;

        log::debug!("[ntfs] directory MFT#{}: INDEX_ROOT parsed, large_index={}",
            dir_entry_number, index_root.has_large_index());

        // Parse entries from the root node
        let entries_data = index_root.entries_data(ir_data)
            .ok_or(NtfsError::Corrupt("failed to get INDEX_ROOT entries"))?;

        let mut all_entries = index::parse_index_entries(entries_data);

        // If there's an $INDEX_ALLOCATION, read overflow nodes
        if index_root.has_large_index() {
            if let Some((_, ia_offset)) = dir_mft.find_attribute(AttributeType::IndexAllocation) {
                log::debug!("[ntfs] reading $INDEX_ALLOCATION for directory MFT#{}",
                    dir_entry_number);

                // Read all INDX blocks
                let ia_data = self.read_non_resident_data(&dir_mft, ia_offset)?;
                let block_size = self.boot_sector.index_block_size() as usize;

                let mut block_offset = 0;
                while block_offset + block_size <= ia_data.len() {
                    let mut block = ia_data[block_offset..block_offset + block_size].to_vec();

                    // Apply fixup to the INDX block
                    if !IndexNodeHeader::apply_fixup(&mut block) {
                        log::warn!("[ntfs] failed to apply fixup to INDX block at offset 0x{:X}",
                            block_offset);
                        block_offset += block_size;
                        continue;
                    }

                    if let Some(node_hdr) = IndexNodeHeader::from_bytes(&block) {
                        if let Some(node_entries) = node_hdr.entries_data(&block) {
                            let entries = index::parse_index_entries(node_entries);
                            log::trace!("[ntfs] INDX block VCN {}: {} entries",
                                node_hdr.vcn, entries.len());
                            all_entries.extend(entries);
                        }
                    }

                    block_offset += block_size;
                }
            }
        }

        log::debug!("[ntfs] directory MFT#{}: total {} index entries", dir_entry_number, all_entries.len());
        Ok(all_entries)
    }

    /// Resolve a path to its MFT entry number, starting from the root directory.
    fn resolve_path(&self, path: &[u8]) -> Result<u64, NtfsError> {
        let components = Self::split_path(path)?;

        if components.is_empty() {
            log::trace!("[ntfs] path resolves to root directory");
            return Ok(MFT_ENTRY_ROOT);
        }

        let mut current_entry = MFT_ENTRY_ROOT;

        for component in &components {
            let name = core::str::from_utf8(component).map_err(|_| NtfsError::InvalidPath)?;
            log::trace!("[ntfs] resolving component '{}' in MFT#{}", name, current_entry);

            let entries = self.read_directory_entries(current_entry)?;

            // Search for the component (prefer Win32 or Win32+DOS namespace)
            let mut found = None;
            for entry in &entries {
                if entry.is_last() {
                    continue;
                }
                if let Some(ref fn_attr) = entry.filename {
                    // Skip DOS-only names
                    if fn_attr.namespace == FileNamespace::Dos {
                        continue;
                    }
                    if self.upcase.names_equal(
                        &fn_attr.name_utf16,
                        &name.encode_utf16().collect::<Vec<u16>>(),
                    ) {
                        found = Some(entry.entry_number());
                        break;
                    }
                }
            }

            current_entry = found.ok_or_else(|| {
                log::debug!("[ntfs] component '{}' not found in MFT#{}", name, current_entry);
                NtfsError::NotFound
            })?;
        }

        log::debug!("[ntfs] path '{}' resolves to MFT#{}",
            core::str::from_utf8(path).unwrap_or("<invalid>"), current_entry);
        Ok(current_entry)
    }

    // -----------------------------------------------------------------------
    // Read API
    // -----------------------------------------------------------------------

    /// Read a file by path, returning its contents.
    pub fn read_file(&self, path: &[u8]) -> Result<Vec<u8>, NtfsError> {
        let path_str = core::str::from_utf8(path).unwrap_or("<invalid>");
        log::info!("[ntfs] read_file: '{}'", path_str);

        let entry_number = self.resolve_path(path)?;
        let entry = self.read_mft_entry(entry_number)?;

        if entry.header.is_directory() {
            log::error!("[ntfs] '{}' is a directory, not a file", path_str);
            return Err(NtfsError::IsADirectory);
        }

        // Read the unnamed $DATA attribute
        self.read_attribute_data_for_entry(&entry, AttributeType::Data)
    }

    // -----------------------------------------------------------------------
    // Write API with resident-to-non-resident conversion and file growth
    // -----------------------------------------------------------------------

    /// Write data to a file, creating it if it does not exist, or overwriting if it does.
    pub fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), NtfsError> {
        let path_str = core::str::from_utf8(path).unwrap_or("<invalid>");
        log::info!("[ntfs] write_file: '{}' ({} bytes)", path_str, data.len());

        let components = Self::split_path(path)?;
        if components.is_empty() {
            return Err(NtfsError::InvalidPath);
        }

        // Find the parent directory
        let parent_entry_number = if components.len() == 1 {
            MFT_ENTRY_ROOT
        } else {
            let parent_path: Vec<u8> = {
                let mut p = vec![b'/'];
                for (i, c) in components[..components.len() - 1].iter().enumerate() {
                    if i > 0 {
                        p.push(b'/');
                    }
                    p.extend_from_slice(c);
                }
                p
            };
            self.resolve_path(&parent_path)?
        };

        let filename = core::str::from_utf8(components[components.len() - 1])
            .map_err(|_| NtfsError::InvalidPath)?;

        if filename.len() > 255 {
            return Err(NtfsError::NameTooLong);
        }

        // Check if file already exists
        match self.resolve_path(path) {
            Ok(existing_entry) => {
                // File exists: overwrite its $DATA attribute
                log::debug!("[ntfs] file '{}' exists at MFT#{}, overwriting", path_str, existing_entry);
                self.write_file_data(existing_entry, data)
            }
            Err(NtfsError::NotFound) => {
                // File does not exist: allocate new MFT entry and create it
                log::debug!("[ntfs] file '{}' does not exist, creating", path_str);
                self.create_file(parent_entry_number, filename, data)
            }
            Err(e) => Err(e),
        }
    }

    /// Write data to an existing file's $DATA attribute.
    ///
    /// Handles:
    /// - Resident data that fits in the existing space
    /// - Resident-to-non-resident conversion when data outgrows resident capacity
    /// - Non-resident file growth (extending data runs)
    fn write_file_data(&self, entry_number: u64, data: &[u8]) -> Result<(), NtfsError> {
        let txn = self.journal.borrow_mut().begin_transaction();
        let entry = self.read_mft_entry(entry_number)?;
        let (hdr, attr_offset) = entry.find_attribute(AttributeType::Data)
            .ok_or(NtfsError::AttributeNotFound("$DATA"))?;

        if hdr.non_resident {
            // Non-resident: write to existing clusters (may need growth)
            let runs = Self::read_data_runs_from_entry(&entry, AttributeType::Data)?;
            let current_allocated: u64 = runs.iter().map(|r| r.length * self.cluster_size).sum();

            if data.len() as u64 <= current_allocated {
                // Data fits in existing allocation — write directly
                let mut bytes_written = 0usize;
                for run in &runs {
                    if bytes_written >= data.len() {
                        break;
                    }
                    if run.is_sparse {
                        bytes_written += (run.length * self.cluster_size) as usize;
                        continue;
                    }

                    let disk_offset = run.lcn * self.cluster_size;
                    let run_capacity = (run.length * self.cluster_size) as usize;
                    let to_write = (data.len() - bytes_written).min(run_capacity);

                    log::trace!("[ntfs] writing {} bytes to disk offset 0x{:X}", to_write, disk_offset);
                    self.journaled_write(JournalOp::ClusterWrite, disk_offset,
                        &data[bytes_written..bytes_written + to_write])?;
                    bytes_written += to_write;
                }

                // Update data_size in the MFT entry's non-resident header
                let mut updated = entry.clone();
                let nr_base = attr_offset + AttributeHeader::HEADER_SIZE + 0x20; // data_size offset
                updated.data[nr_base..nr_base + 8].copy_from_slice(&(data.len() as u64).to_le_bytes());
                // Update initialized_size too
                let init_base = attr_offset + AttributeHeader::HEADER_SIZE + 0x28;
                updated.data[init_base..init_base + 8].copy_from_slice(&(data.len() as u64).to_le_bytes());
                // Update LSN
                updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

                self.write_mft_entry(entry_number, &updated)?;
            } else {
                // Need more clusters — allocate additional space
                let extra_bytes = data.len() as u64 - current_allocated;
                let extra_clusters = (extra_bytes + self.cluster_size - 1) / self.cluster_size;
                let new_lcn = self.allocate_clusters(extra_clusters)?;

                // Write existing portion
                let mut bytes_written = 0usize;
                for run in &runs {
                    if bytes_written >= data.len() {
                        break;
                    }
                    if run.is_sparse {
                        bytes_written += (run.length * self.cluster_size) as usize;
                        continue;
                    }
                    let disk_offset = run.lcn * self.cluster_size;
                    let run_capacity = (run.length * self.cluster_size) as usize;
                    let to_write = (data.len() - bytes_written).min(run_capacity);
                    self.journaled_write(JournalOp::ClusterWrite, disk_offset,
                        &data[bytes_written..bytes_written + to_write])?;
                    bytes_written += to_write;
                }

                // Write remaining to new clusters
                if bytes_written < data.len() {
                    let new_offset = new_lcn * self.cluster_size;
                    self.journaled_write(JournalOp::ClusterWrite, new_offset,
                        &data[bytes_written..])?;
                }

                // Build updated data runs (old runs + new run)
                let mut all_runs: Vec<DataRun> = runs;
                all_runs.push(DataRun { lcn: new_lcn, length: extra_clusters, is_sparse: false });
                let new_run_bytes = data_runs::encode_data_runs(&all_runs);
                let new_allocated = all_runs.iter().map(|r| r.length * self.cluster_size).sum::<u64>();

                // Rebuild the non-resident attribute in the MFT entry
                let mut updated = entry.clone();
                let record_size = self.mft_record_size as usize;

                // Rewrite from attr_offset
                let mut attr_pos = attr_offset;
                attr_pos = Self::write_non_resident_attribute(
                    &mut updated.data, attr_pos, AttributeType::Data,
                    &new_run_bytes, data.len() as u64, new_allocated,
                    hdr.instance,
                );

                // Write end marker after the data attribute
                if attr_pos + 4 <= record_size {
                    updated.data[attr_pos..attr_pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                    attr_pos += 4;
                }

                // Update used_size and LSN
                updated.data[0x18..0x1C].copy_from_slice(&(attr_pos as u32).to_le_bytes());
                updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

                // Re-parse to fix up header
                let updated = MftEntry::from_bytes(&updated.data, record_size)
                    .ok_or(NtfsError::Corrupt("failed to re-parse after data run extension"))?;
                self.write_mft_entry(entry_number, &updated)?;

                log::debug!("[ntfs] extended file MFT#{}: allocated {} additional clusters at LCN {}",
                    entry_number, extra_clusters, new_lcn);
            }
        } else {
            // Resident attribute
            let res_data = entry.resident_data(attr_offset)
                .ok_or(NtfsError::Corrupt("failed to read resident data"))?;

            // Check if we can fit in the MFT entry (with some margin)
            let record_size = self.mft_record_size as usize;
            let available_space = record_size - attr_offset - 32; // header + margin

            if data.len() <= res_data.len() {
                // Fits in existing resident space — update in place
                let mut updated = entry.clone();
                let res_hdr = crate::attribute::ResidentHeader::from_bytes(
                    &updated.data[attr_offset + AttributeHeader::HEADER_SIZE..]
                ).ok_or(NtfsError::Corrupt("bad resident header"))?;

                let data_start = attr_offset + res_hdr.value_offset as usize;
                updated.data[data_start..data_start + data.len()].copy_from_slice(data);

                // Zero out remaining space
                for i in data.len()..res_hdr.value_length as usize {
                    updated.data[data_start + i] = 0;
                }

                // Update value_length
                let vl_offset = attr_offset + AttributeHeader::HEADER_SIZE;
                updated.data[vl_offset..vl_offset + 4].copy_from_slice(&(data.len() as u32).to_le_bytes());
                // Update LSN
                updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

                self.write_mft_entry(entry_number, &updated)?;
                log::debug!("[ntfs] wrote {} bytes to MFT#{} resident data", data.len(), entry_number);
            } else if data.len() + 24 < available_space {
                // Fits in MFT entry if we grow the resident attribute
                let mut updated = entry.clone();
                let res_hdr = crate::attribute::ResidentHeader::from_bytes(
                    &updated.data[attr_offset + AttributeHeader::HEADER_SIZE..]
                ).ok_or(NtfsError::Corrupt("bad resident header"))?;

                let data_start = attr_offset + res_hdr.value_offset as usize;

                // Update value_length first
                let vl_offset = attr_offset + AttributeHeader::HEADER_SIZE;
                updated.data[vl_offset..vl_offset + 4].copy_from_slice(&(data.len() as u32).to_le_bytes());

                // Update attribute total length
                let new_total = ((res_hdr.value_offset as usize + data.len()) + 7) & !7;
                updated.data[attr_offset + 4..attr_offset + 8].copy_from_slice(&(new_total as u32).to_le_bytes());

                // Write data
                let end = data_start + data.len();
                if end <= record_size {
                    updated.data[data_start..end].copy_from_slice(data);
                    // Pad to alignment
                    for i in end..data_start + new_total - (res_hdr.value_offset as usize) {
                        if i < record_size {
                            updated.data[i] = 0;
                        }
                    }
                }

                // Write end marker
                let end_pos = attr_offset + new_total;
                if end_pos + 4 <= record_size {
                    updated.data[end_pos..end_pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                }

                // Update used_size + LSN
                updated.data[0x18..0x1C].copy_from_slice(&((end_pos + 4) as u32).to_le_bytes());
                updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

                self.write_mft_entry(entry_number, &updated)?;
                log::debug!("[ntfs] grew resident data for MFT#{} to {} bytes", entry_number, data.len());
            } else {
                // Resident-to-non-resident conversion
                log::info!("[ntfs] converting MFT#{} from resident to non-resident ({} bytes)",
                    entry_number, data.len());

                let clusters_needed = (data.len() as u64 + self.cluster_size - 1) / self.cluster_size;
                let start_lcn = self.allocate_clusters(clusters_needed)?;

                // Write data to allocated clusters
                let disk_offset = start_lcn * self.cluster_size;
                self.journaled_write(JournalOp::ClusterWrite, disk_offset, data)?;

                // Rebuild the MFT entry with non-resident $DATA
                let mut updated = entry.clone();
                let run = DataRun { lcn: start_lcn, length: clusters_needed, is_sparse: false };
                let run_bytes = data_runs::encode_data_runs(&[run]);
                let allocated_size = clusters_needed * self.cluster_size;

                // Overwrite the attribute at its current position
                let new_attr_end = Self::write_non_resident_attribute(
                    &mut updated.data, attr_offset, AttributeType::Data,
                    &run_bytes, data.len() as u64, allocated_size, hdr.instance,
                );

                // Write end marker
                if new_attr_end + 4 <= record_size {
                    updated.data[new_attr_end..new_attr_end + 4]
                        .copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                }

                // Update used_size + LSN
                updated.data[0x18..0x1C].copy_from_slice(&((new_attr_end + 4) as u32).to_le_bytes());
                updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

                let updated = MftEntry::from_bytes(&updated.data, record_size)
                    .ok_or(NtfsError::Corrupt("failed re-parse after resident-to-non-resident"))?;
                self.write_mft_entry(entry_number, &updated)?;

                log::info!("[ntfs] converted MFT#{} to non-resident: {} clusters at LCN {}",
                    entry_number, clusters_needed, start_lcn);
            }
        }

        self.journal.borrow_mut().commit_transaction(txn);
        self.flush_journal()?;
        self.device.flush()
    }

    // -----------------------------------------------------------------------
    // File/directory creation
    // -----------------------------------------------------------------------

    /// Create a new file in a parent directory.
    fn create_file(&self, parent_entry: u64, name: &str, data: &[u8]) -> Result<(), NtfsError> {
        log::info!("[ntfs] creating file '{}' in directory MFT#{}", name, parent_entry);
        let txn = self.journal.borrow_mut().begin_transaction();

        // Allocate a new MFT entry
        let new_entry_number = self.allocate_mft_entry()?;
        log::debug!("[ntfs] allocated MFT entry #{} for new file '{}'", new_entry_number, name);

        // Build the $FILE_NAME attribute
        let parent_mft = self.read_mft_entry(parent_entry)?;
        let parent_ref = mft::make_mft_reference(parent_entry, parent_mft.header.sequence_number);

        let name_utf16: Vec<u16> = name.encode_utf16().collect();
        let now = 0u64; // TODO: get current time as FILETIME

        let fn_attr = FileNameAttr {
            parent_reference: parent_ref,
            creation_time: now,
            modification_time: now,
            mft_modification_time: now,
            access_time: now,
            allocated_size: data.len() as u64,
            real_size: data.len() as u64,
            flags: 0,
            ea_reparse: 0,
            name_length: name_utf16.len() as u8,
            namespace: FileNamespace::Win32AndDos,
            name: String::from(name),
            name_utf16: name_utf16.clone(),
        };

        // Build the MFT entry
        let record_size = self.mft_record_size as usize;
        let mut entry_data = vec![0u8; record_size];

        // Write FILE header
        entry_data[0..4].copy_from_slice(b"FILE");
        // USA offset (just after the header at offset 0x30)
        entry_data[0x04..0x06].copy_from_slice(&0x0030u16.to_le_bytes());
        // USA count (record_size / 512 + 1)
        let usa_count = (record_size / 512 + 1) as u16;
        entry_data[0x06..0x08].copy_from_slice(&usa_count.to_le_bytes());
        // LSN
        entry_data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());
        // Sequence number = 1
        entry_data[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
        // Hard link count = 1
        entry_data[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
        // First attribute offset (after header + USA)
        let first_attr = 0x30 + usa_count as usize * 2;
        let first_attr_aligned = (first_attr + 7) & !7; // 8-byte align
        entry_data[0x14..0x16].copy_from_slice(&(first_attr_aligned as u16).to_le_bytes());
        // Flags: in use
        entry_data[0x16..0x18].copy_from_slice(&0x0001u16.to_le_bytes());
        // Allocated size
        entry_data[0x1C..0x20].copy_from_slice(&(record_size as u32).to_le_bytes());

        // Write $FILE_NAME attribute (resident)
        let fn_bytes = fn_attr.to_bytes();
        let mut attr_pos = first_attr_aligned;
        attr_pos = Self::write_resident_attribute(
            &mut entry_data, attr_pos, AttributeType::FileName, &fn_bytes, 0,
        );

        // Write $DATA attribute (resident if small enough)
        if data.len() + attr_pos + 32 < record_size - 8 {
            // Resident $DATA
            attr_pos = Self::write_resident_attribute(
                &mut entry_data, attr_pos, AttributeType::Data, data, 1,
            );
        } else {
            // Non-resident: allocate clusters
            log::debug!("[ntfs] file data too large for resident, allocating clusters");
            let clusters_needed = (data.len() as u64 + self.cluster_size - 1) / self.cluster_size;
            let start_lcn = self.allocate_clusters(clusters_needed)?;

            // Write data to allocated clusters
            let disk_offset = start_lcn * self.cluster_size;
            self.journaled_write(JournalOp::ClusterWrite, disk_offset, data)?;

            // Write non-resident $DATA attribute with data runs
            let run = DataRun { lcn: start_lcn, length: clusters_needed, is_sparse: false };
            let run_bytes = data_runs::encode_data_runs(&[run]);
            attr_pos = Self::write_non_resident_attribute(
                &mut entry_data, attr_pos, AttributeType::Data,
                &run_bytes, data.len() as u64, clusters_needed * self.cluster_size, 1,
            );
        }

        // Write end-of-attributes marker
        entry_data[attr_pos..attr_pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        attr_pos += 4;

        // Update used_size
        entry_data[0x18..0x1C].copy_from_slice(&(attr_pos as u32).to_le_bytes());

        // Write USA check value
        let check_value = 0x0001u16; // arbitrary check value
        entry_data[0x30..0x32].copy_from_slice(&check_value.to_le_bytes());

        let new_entry = MftEntry::from_bytes(&entry_data, record_size)
            .ok_or(NtfsError::Corrupt("failed to parse newly created MFT entry"))?;

        self.write_mft_entry(new_entry_number, &new_entry)?;

        // Insert index entry into parent directory
        self.insert_index_entry(parent_entry, new_entry_number, &fn_attr)?;

        self.journal.borrow_mut().commit_transaction(txn);
        self.flush_journal()?;

        log::info!("[ntfs] created file '{}' as MFT#{}", name, new_entry_number);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Directory index insertion (B+ tree)
    // -----------------------------------------------------------------------

    /// Insert a new index entry into a directory's index.
    ///
    /// This handles:
    /// - Inserting into $INDEX_ROOT if there's room
    /// - Splitting and promoting to $INDEX_ALLOCATION if the root is full
    fn insert_index_entry(
        &self,
        dir_entry_number: u64,
        file_entry_number: u64,
        fn_attr: &FileNameAttr,
    ) -> Result<(), NtfsError> {
        log::debug!("[ntfs] inserting index entry for MFT#{} into directory MFT#{}",
            file_entry_number, dir_entry_number);

        let dir_mft = self.read_mft_entry(dir_entry_number)?;

        let (_, ir_offset) = dir_mft.find_attribute(AttributeType::IndexRoot)
            .ok_or(NtfsError::AttributeNotFound("$INDEX_ROOT"))?;

        let ir_data = dir_mft.resident_data(ir_offset)
            .ok_or(NtfsError::Corrupt("$INDEX_ROOT must be resident"))?
            .to_vec();

        let index_root = IndexRoot::from_bytes(&ir_data)
            .ok_or(NtfsError::Corrupt("invalid $INDEX_ROOT"))?;

        // Build the new index entry
        let file_mft_ref = mft::make_mft_reference(file_entry_number, 1);
        let new_entry_bytes = Self::build_index_entry(file_mft_ref, fn_attr);

        // Get the current entries from the root
        let entries_data = index_root.entries_data(&ir_data)
            .ok_or(NtfsError::Corrupt("failed to get INDEX_ROOT entries"))?;
        let existing_entries = index::parse_index_entries(entries_data);

        // Find the insertion point (sorted by name)
        let insert_pos = self.find_insert_position(&existing_entries, fn_attr);

        // Calculate total size needed
        let current_entries_size: usize = existing_entries.iter()
            .map(|e| e.entry_length as usize)
            .sum();
        let new_total_size = current_entries_size + new_entry_bytes.len();

        // Check if we can fit in the $INDEX_ROOT
        let record_size = self.mft_record_size as usize;
        let index_root_max = record_size - ir_offset - 64; // Rough max for resident data

        if new_total_size + 32 < index_root_max {
            // Fits in $INDEX_ROOT — insert directly
            self.insert_into_index_root(
                dir_entry_number, &dir_mft, ir_offset,
                &ir_data, &existing_entries, insert_pos,
                &new_entry_bytes,
            )?;
        } else {
            // Node is full — need to split
            // Move entries to $INDEX_ALLOCATION and keep only a pointer in root
            log::debug!("[ntfs] $INDEX_ROOT full, splitting into $INDEX_ALLOCATION");
            self.split_index_node(
                dir_entry_number, &dir_mft, ir_offset,
                &ir_data, &existing_entries, insert_pos,
                &new_entry_bytes,
            )?;
        }

        log::debug!("[ntfs] index entry inserted for MFT#{} in directory MFT#{}",
            file_entry_number, dir_entry_number);
        Ok(())
    }

    /// Find the correct insertion position in sorted index entries.
    fn find_insert_position(&self, entries: &[IndexEntry], new_fn: &FileNameAttr) -> usize {
        for (i, entry) in entries.iter().enumerate() {
            if entry.is_last() {
                return i; // Insert before the sentinel
            }
            if let Some(ref fn_attr) = entry.filename {
                let cmp = self.upcase.compare_names(&new_fn.name_utf16, &fn_attr.name_utf16);
                if cmp == core::cmp::Ordering::Less {
                    return i;
                }
            }
        }
        // Insert at end (before sentinel if we somehow missed it)
        entries.len().saturating_sub(1)
    }

    /// Build an index entry from an MFT reference and $FILE_NAME attribute.
    fn build_index_entry(mft_ref: u64, fn_attr: &FileNameAttr) -> Vec<u8> {
        let fn_bytes = fn_attr.to_bytes();
        let entry_len = ((16 + fn_bytes.len()) + 7) & !7; // 8-byte aligned

        let mut buf = vec![0u8; entry_len];
        // MFT reference
        buf[0..8].copy_from_slice(&mft_ref.to_le_bytes());
        // Entry length
        buf[8..10].copy_from_slice(&(entry_len as u16).to_le_bytes());
        // Content length
        buf[10..12].copy_from_slice(&(fn_bytes.len() as u16).to_le_bytes());
        // Flags = 0 (no sub-node, not last)
        buf[12..14].copy_from_slice(&0u16.to_le_bytes());
        // $FILE_NAME content
        buf[16..16 + fn_bytes.len()].copy_from_slice(&fn_bytes);

        buf
    }

    /// Insert a new index entry into $INDEX_ROOT when it fits.
    fn insert_into_index_root(
        &self,
        dir_entry_number: u64,
        dir_mft: &MftEntry,
        ir_offset: usize,
        ir_data: &[u8],
        existing_entries: &[IndexEntry],
        insert_pos: usize,
        new_entry_bytes: &[u8],
    ) -> Result<(), NtfsError> {
        // Rebuild the index entries area with the new entry inserted
        let entries_data = self.rebuild_entries_with_insert(
            ir_data, existing_entries, insert_pos, new_entry_bytes,
        );

        // Rebuild the full $INDEX_ROOT value
        let new_ir_value = self.rebuild_index_root_value(ir_data, &entries_data);

        // Update the MFT entry
        let mut updated = dir_mft.clone();
        let record_size = self.mft_record_size as usize;

        // Rewrite the $INDEX_ROOT attribute
        let instance = updated.data[ir_offset + 14] as u16;
        let new_attr_end = Self::write_resident_attribute(
            &mut updated.data, ir_offset, AttributeType::IndexRoot,
            &new_ir_value, instance, // preserve instance
        );

        // Write end marker
        if new_attr_end + 4 <= record_size {
            updated.data[new_attr_end..new_attr_end + 4]
                .copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }

        // Update used_size + LSN
        updated.data[0x18..0x1C].copy_from_slice(&((new_attr_end + 4) as u32).to_le_bytes());
        updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

        let updated = MftEntry::from_bytes(&updated.data, record_size)
            .ok_or(NtfsError::Corrupt("failed re-parse after index insertion"))?;
        self.write_mft_entry(dir_entry_number, &updated)
    }

    /// Rebuild index entries area with a new entry inserted at the given position.
    fn rebuild_entries_with_insert(
        &self,
        ir_data: &[u8],
        existing_entries: &[IndexEntry],
        insert_pos: usize,
        new_entry_bytes: &[u8],
    ) -> Vec<u8> {
        let ir = IndexRoot::from_bytes(ir_data).unwrap();
        let entries_raw = ir.entries_data(ir_data).unwrap();

        let mut result = Vec::new();
        let mut raw_pos = 0;

        for (i, entry) in existing_entries.iter().enumerate() {
            if i == insert_pos {
                // Insert the new entry here
                result.extend_from_slice(new_entry_bytes);
            }
            // Copy the existing entry
            let len = entry.entry_length as usize;
            if raw_pos + len <= entries_raw.len() {
                result.extend_from_slice(&entries_raw[raw_pos..raw_pos + len]);
            }
            raw_pos += len;
        }

        // If insert_pos is at the end (shouldn't happen normally, but be safe)
        if insert_pos >= existing_entries.len() {
            result.extend_from_slice(new_entry_bytes);
        }

        result
    }

    /// Rebuild a complete $INDEX_ROOT value from the header and new entries data.
    fn rebuild_index_root_value(&self, old_ir_data: &[u8], entries_data: &[u8]) -> Vec<u8> {
        // Index root header: 16 bytes
        // Index header: 16 bytes
        // Then entries
        let entries_offset = 16u32; // from index header start
        let total_entries_size = entries_offset as usize + entries_data.len();

        let mut buf = vec![0u8; 16 + total_entries_size];

        // Copy the index root header (first 16 bytes)
        buf[0..16].copy_from_slice(&old_ir_data[..16.min(old_ir_data.len())]);

        // Index header at offset 16
        buf[16..20].copy_from_slice(&entries_offset.to_le_bytes()); // entries_offset
        buf[20..24].copy_from_slice(&(total_entries_size as u32).to_le_bytes()); // total_size
        buf[24..28].copy_from_slice(&(total_entries_size as u32).to_le_bytes()); // allocated_size

        // Preserve flags from old data
        if old_ir_data.len() > 0x1C {
            buf[28] = old_ir_data[0x1C];
        }

        // Copy entries
        buf[32..32 + entries_data.len()].copy_from_slice(entries_data);

        buf
    }

    /// Split an index node when it's too full.
    ///
    /// Moves all entries to an INDX block in $INDEX_ALLOCATION,
    /// and updates $INDEX_ROOT to be a large index with a pointer to the block.
    fn split_index_node(
        &self,
        dir_entry_number: u64,
        dir_mft: &MftEntry,
        ir_offset: usize,
        ir_data: &[u8],
        existing_entries: &[IndexEntry],
        insert_pos: usize,
        new_entry_bytes: &[u8],
    ) -> Result<(), NtfsError> {
        let block_size = self.boot_sector.index_block_size() as usize;

        // Build complete entries list with the new one inserted
        let all_entries_data = self.rebuild_entries_with_insert(
            ir_data, existing_entries, insert_pos, new_entry_bytes,
        );

        // Allocate a cluster for the INDX block
        let indx_clusters = (block_size as u64 + self.cluster_size - 1) / self.cluster_size;
        let indx_lcn = self.allocate_clusters(indx_clusters)?;

        // Build the INDX block
        let mut indx_buf = vec![0u8; block_size];

        // INDX header
        indx_buf[0..4].copy_from_slice(b"INDX");
        // USA offset
        let usa_offset = 0x28u16; // after INDX header
        indx_buf[4..6].copy_from_slice(&usa_offset.to_le_bytes());
        let usa_count = (block_size / 512 + 1) as u16;
        indx_buf[6..8].copy_from_slice(&usa_count.to_le_bytes());
        // LSN
        indx_buf[8..16].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());
        // VCN = 0
        indx_buf[16..24].copy_from_slice(&0u64.to_le_bytes());

        // Index header at 0x18
        let entries_start = ((usa_offset as usize + usa_count as usize * 2) + 7) & !7;
        let idx_entries_offset = (entries_start - 0x18) as u32;
        indx_buf[0x18..0x1C].copy_from_slice(&idx_entries_offset.to_le_bytes());
        let idx_total = idx_entries_offset as usize + all_entries_data.len();
        indx_buf[0x1C..0x20].copy_from_slice(&(idx_total as u32).to_le_bytes());
        indx_buf[0x20..0x24].copy_from_slice(&((block_size - 0x18) as u32).to_le_bytes());
        indx_buf[0x24] = 0; // flags (leaf node)

        // Write USA check value
        indx_buf[usa_offset as usize..usa_offset as usize + 2]
            .copy_from_slice(&0x0001u16.to_le_bytes());

        // Copy entries into the block
        let entries_abs_start = 0x18 + idx_entries_offset as usize;
        if entries_abs_start + all_entries_data.len() <= block_size {
            indx_buf[entries_abs_start..entries_abs_start + all_entries_data.len()]
                .copy_from_slice(&all_entries_data);
        }

        // Write the INDX block to disk
        let indx_disk_offset = indx_lcn * self.cluster_size;
        self.journaled_write(JournalOp::IndexInsert, indx_disk_offset, &indx_buf)?;

        // Update the directory MFT entry:
        // 1. $INDEX_ROOT becomes a "large index" with just a sentinel pointing to VCN 0
        // 2. Add $INDEX_ALLOCATION attribute with the data run
        let mut updated = dir_mft.clone();
        let record_size = self.mft_record_size as usize;

        // Rebuild $INDEX_ROOT as large index with sentinel -> VCN 0
        let new_ir_value = self.build_large_index_root(ir_data);
        let mut attr_pos = ir_offset;
        attr_pos = Self::write_resident_attribute(
            &mut updated.data, attr_pos, AttributeType::IndexRoot,
            &new_ir_value, 0, // instance 0
        );

        // Add $INDEX_ALLOCATION attribute (non-resident, points to the INDX block)
        let ia_run = DataRun { lcn: indx_lcn, length: indx_clusters, is_sparse: false };
        let ia_run_bytes = data_runs::encode_data_runs(&[ia_run]);
        attr_pos = Self::write_non_resident_attribute(
            &mut updated.data, attr_pos, AttributeType::IndexAllocation,
            &ia_run_bytes, block_size as u64, indx_clusters * self.cluster_size, 2,
        );

        // Add $BITMAP attribute for index allocation tracking
        let bitmap_value = [0x01u8]; // bit 0 = INDX block 0 is in use
        attr_pos = Self::write_resident_attribute(
            &mut updated.data, attr_pos, AttributeType::Bitmap, &bitmap_value, 3,
        );

        // Write end marker
        if attr_pos + 4 <= record_size {
            updated.data[attr_pos..attr_pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
            attr_pos += 4;
        }

        // Update used_size, flags (directory), LSN
        updated.data[0x18..0x1C].copy_from_slice(&(attr_pos as u32).to_le_bytes());
        updated.data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());

        let updated = MftEntry::from_bytes(&updated.data, record_size)
            .ok_or(NtfsError::Corrupt("failed re-parse after index split"))?;
        self.write_mft_entry(dir_entry_number, &updated)?;

        log::info!("[ntfs] split index for directory MFT#{}: INDX block at LCN {}",
            dir_entry_number, indx_lcn);
        Ok(())
    }

    /// Build a large $INDEX_ROOT value that points to VCN 0 via a sentinel entry.
    fn build_large_index_root(&self, old_ir_data: &[u8]) -> Vec<u8> {
        // Build a sentinel entry with HAS_SUB_NODE pointing to VCN 0
        let sentinel_len = 24usize; // 16 header + 8 VCN
        let entries_offset = 16u32;
        let total_size = entries_offset as usize + sentinel_len;

        let mut buf = vec![0u8; 16 + total_size];

        // Copy index root header (first 16 bytes)
        let copy_len = 16.min(old_ir_data.len());
        buf[0..copy_len].copy_from_slice(&old_ir_data[..copy_len]);

        // Index header at offset 16
        buf[16..20].copy_from_slice(&entries_offset.to_le_bytes());
        buf[20..24].copy_from_slice(&(total_size as u32).to_le_bytes());
        buf[24..28].copy_from_slice(&(total_size as u32).to_le_bytes());
        buf[28] = INDEX_HEADER_FLAG_LARGE_INDEX; // flags: large index

        // Sentinel entry at offset 32
        let sentinel_offset = 32;
        // MFT reference = 0
        // entry_length = 24
        buf[sentinel_offset + 8..sentinel_offset + 10]
            .copy_from_slice(&(sentinel_len as u16).to_le_bytes());
        // content_length = 0
        // flags = LAST_ENTRY | HAS_SUB_NODE
        let flags = INDEX_ENTRY_FLAG_LAST_ENTRY | INDEX_ENTRY_FLAG_HAS_SUB_NODE;
        buf[sentinel_offset + 12..sentinel_offset + 14].copy_from_slice(&flags.to_le_bytes());
        // VCN = 0 (last 8 bytes of entry)
        buf[sentinel_offset + 16..sentinel_offset + 24].copy_from_slice(&0u64.to_le_bytes());

        buf
    }

    // -----------------------------------------------------------------------
    // Attribute writing helpers
    // -----------------------------------------------------------------------

    /// Write a resident attribute into an MFT entry buffer.
    /// Returns the new position after the attribute.
    fn write_resident_attribute(
        buf: &mut [u8],
        pos: usize,
        attr_type: AttributeType,
        value: &[u8],
        instance: u16,
    ) -> usize {
        let value_offset = 24u16; // Header(16) + ResidentHeader(8) = 24 bytes to value
        let total_len = ((value_offset as usize + value.len()) + 7) & !7; // 8-byte aligned

        // Common header
        buf[pos..pos + 4].copy_from_slice(&(attr_type as u32).to_le_bytes());
        buf[pos + 4..pos + 8].copy_from_slice(&(total_len as u32).to_le_bytes());
        buf[pos + 8] = 0; // resident
        buf[pos + 9] = 0; // name_length
        buf[pos + 10..pos + 12].copy_from_slice(&0u16.to_le_bytes()); // name_offset
        buf[pos + 12..pos + 14].copy_from_slice(&0u16.to_le_bytes()); // flags
        buf[pos + 14..pos + 16].copy_from_slice(&instance.to_le_bytes());

        // Resident header
        buf[pos + 16..pos + 20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        buf[pos + 20..pos + 22].copy_from_slice(&value_offset.to_le_bytes());
        buf[pos + 22] = 0; // indexed_flag
        buf[pos + 23] = 0; // padding

        // Value
        let end = (pos + value_offset as usize + value.len()).min(buf.len());
        let copy_len = end - (pos + value_offset as usize);
        buf[pos + value_offset as usize..end]
            .copy_from_slice(&value[..copy_len]);

        log::trace!("[ntfs] wrote resident attribute {} ({} value bytes) at offset 0x{:04X}",
            attr_type.name(), value.len(), pos);

        pos + total_len
    }

    /// Write a non-resident attribute into an MFT entry buffer.
    /// Returns the new position after the attribute.
    fn write_non_resident_attribute(
        buf: &mut [u8],
        pos: usize,
        attr_type: AttributeType,
        run_data: &[u8],
        data_size: u64,
        allocated_size: u64,
        instance: u16,
    ) -> usize {
        let mapping_pairs_offset = 64u16; // Header(16) + NR-Header(48) = 64
        let total_len = ((mapping_pairs_offset as usize + run_data.len()) + 7) & !7;

        // Common header
        buf[pos..pos + 4].copy_from_slice(&(attr_type as u32).to_le_bytes());
        buf[pos + 4..pos + 8].copy_from_slice(&(total_len as u32).to_le_bytes());
        buf[pos + 8] = 1; // non-resident
        buf[pos + 9] = 0; // name_length
        buf[pos + 10..pos + 12].copy_from_slice(&0u16.to_le_bytes());
        buf[pos + 12..pos + 14].copy_from_slice(&0u16.to_le_bytes());
        buf[pos + 14..pos + 16].copy_from_slice(&instance.to_le_bytes());

        // Non-resident header (starting at pos + 16)
        let nr_base = pos + 16;
        // lowest_vcn = 0
        buf[nr_base..nr_base + 8].copy_from_slice(&0u64.to_le_bytes());
        // highest_vcn
        let cluster_size = if allocated_size > 0 && data_size > 0 {
            // Estimate cluster size
            allocated_size / ((allocated_size + data_size - 1) / data_size).max(1)
        } else {
            4096 // default
        };
        let highest_vcn = if allocated_size > 0 && cluster_size > 0 {
            (allocated_size / cluster_size).saturating_sub(1)
        } else {
            0
        };
        buf[nr_base + 8..nr_base + 16].copy_from_slice(&highest_vcn.to_le_bytes());
        // mapping_pairs_offset (from attribute start)
        buf[nr_base + 16..nr_base + 18].copy_from_slice(&mapping_pairs_offset.to_le_bytes());
        // compression_unit = 0
        buf[nr_base + 18..nr_base + 20].copy_from_slice(&0u16.to_le_bytes());
        // padding (4 bytes)
        // allocated_size
        buf[nr_base + 24..nr_base + 32].copy_from_slice(&allocated_size.to_le_bytes());
        // data_size
        buf[nr_base + 32..nr_base + 40].copy_from_slice(&data_size.to_le_bytes());
        // initialized_size
        buf[nr_base + 40..nr_base + 48].copy_from_slice(&data_size.to_le_bytes());

        // Data runs
        let runs_start = pos + mapping_pairs_offset as usize;
        buf[runs_start..runs_start + run_data.len()].copy_from_slice(run_data);

        log::trace!("[ntfs] wrote non-resident attribute {} ({} run bytes, data_size={}) at 0x{:04X}",
            attr_type.name(), run_data.len(), data_size, pos);

        pos + total_len
    }

    // -----------------------------------------------------------------------
    // Allocation
    // -----------------------------------------------------------------------

    /// Allocate a free MFT entry.
    ///
    /// Scans the $MFT bitmap for a free entry and marks it as allocated.
    fn allocate_mft_entry(&self) -> Result<u64, NtfsError> {
        log::debug!("[ntfs] searching for free MFT entry...");

        // Read the $Bitmap attribute of $MFT (entry 0)
        let mft_entry = self.read_mft_entry(mft::MFT_ENTRY_MFT)?;
        let mut bitmap_data = self.read_attribute_data_for_entry(&mft_entry, AttributeType::Bitmap)?;

        // Scan for first free bit, starting after reserved entries
        for byte_idx in (mft::MFT_ENTRY_FIRST_USER as usize / 8)..bitmap_data.len() {
            if bitmap_data[byte_idx] != 0xFF {
                for bit in 0..8 {
                    if bitmap_data[byte_idx] & (1 << bit) == 0 {
                        let entry_number = (byte_idx * 8 + bit) as u64;
                        log::info!("[ntfs] found free MFT entry #{}", entry_number);

                        // Mark the bit as allocated
                        bitmap_data[byte_idx] |= 1 << bit;

                        // Write the updated bitmap back (journal it)
                        // For simplicity, update the resident data inline
                        let (bmp_hdr, bmp_offset) = mft_entry.find_attribute(AttributeType::Bitmap)
                            .ok_or(NtfsError::AttributeNotFound("$BITMAP"))?;
                        if !bmp_hdr.non_resident {
                            let mut updated_mft = mft_entry.clone();
                            let res_hdr = crate::attribute::ResidentHeader::from_bytes(
                                &updated_mft.data[bmp_offset + AttributeHeader::HEADER_SIZE..]
                            ).ok_or(NtfsError::Corrupt("bad bitmap resident header"))?;
                            let data_start = bmp_offset + res_hdr.value_offset as usize;
                            if data_start + byte_idx < updated_mft.data.len() {
                                updated_mft.data[data_start + byte_idx] = bitmap_data[byte_idx];
                            }
                            self.write_mft_entry(mft::MFT_ENTRY_MFT, &updated_mft)?;
                        }

                        return Ok(entry_number);
                    }
                }
            }
        }

        log::error!("[ntfs] no free MFT entries available");
        Err(NtfsError::NoFreeMftEntries)
    }

    /// Allocate contiguous clusters from the volume bitmap.
    ///
    /// Scans the $Bitmap (MFT entry 6) for free clusters and marks them allocated.
    fn allocate_clusters(&self, count: u64) -> Result<u64, NtfsError> {
        log::debug!("[ntfs] allocating {} clusters...", count);

        // Read the volume $Bitmap (MFT entry 6)
        let bitmap_entry = self.read_mft_entry(mft::MFT_ENTRY_BITMAP)?;
        let mut bitmap_data = self.read_attribute_data_for_entry(&bitmap_entry, AttributeType::Data)?;

        // Simple first-fit allocation: find `count` contiguous free bits
        let total_bits = bitmap_data.len() * 8;
        let mut run_start = 0u64;
        let mut run_length = 0u64;

        for bit_idx in 0..total_bits {
            let byte_idx = bit_idx / 8;
            let bit = bit_idx % 8;

            if bitmap_data[byte_idx] & (1 << bit) == 0 {
                // Free
                if run_length == 0 {
                    run_start = bit_idx as u64;
                }
                run_length += 1;
                if run_length >= count {
                    log::info!("[ntfs] allocated {} clusters starting at LCN {}", count, run_start);

                    // Mark the bits as allocated
                    for i in 0..count {
                        let bi = (run_start + i) as usize;
                        bitmap_data[bi / 8] |= 1 << (bi % 8);
                    }

                    // Write updated bitmap back
                    // Journal the bitmap update
                    let (bmp_hdr, _bmp_offset) = bitmap_entry.find_attribute(AttributeType::Data)
                        .ok_or(NtfsError::AttributeNotFound("$DATA"))?;
                    if bmp_hdr.non_resident {
                        let runs = Self::read_data_runs_from_entry(&bitmap_entry, AttributeType::Data)?;
                        // Write the modified bytes to the appropriate run(s)
                        let start_byte = run_start as usize / 8;
                        let end_byte = ((run_start + count) as usize + 7) / 8;
                        let mut byte_offset = 0usize;
                        for run in &runs {
                            if run.is_sparse {
                                byte_offset += (run.length * self.cluster_size) as usize;
                                continue;
                            }
                            let run_end = byte_offset + (run.length * self.cluster_size) as usize;
                            if start_byte < run_end && end_byte > byte_offset {
                                let write_start = start_byte.max(byte_offset);
                                let write_end = end_byte.min(run_end);
                                let disk_off = run.lcn * self.cluster_size
                                    + (write_start - byte_offset) as u64;
                                self.journaled_write(
                                    JournalOp::BitmapUpdate,
                                    disk_off,
                                    &bitmap_data[write_start..write_end],
                                )?;
                            }
                            byte_offset = run_end;
                        }
                    }

                    return Ok(run_start);
                }
            } else {
                run_length = 0;
            }
        }

        log::error!("[ntfs] no contiguous run of {} free clusters", count);
        Err(NtfsError::NoFreeClusters)
    }

    // -----------------------------------------------------------------------
    // mkdir
    // -----------------------------------------------------------------------

    /// Create a directory.
    pub fn mkdir(&self, path: &[u8]) -> Result<(), NtfsError> {
        let path_str = core::str::from_utf8(path).unwrap_or("<invalid>");
        log::info!("[ntfs] mkdir: '{}'", path_str);

        let components = Self::split_path(path)?;
        if components.is_empty() {
            return Err(NtfsError::InvalidPath);
        }

        // Check if already exists
        if self.resolve_path(path).is_ok() {
            return Err(NtfsError::AlreadyExists);
        }

        // Find parent directory
        let parent_entry_number = if components.len() == 1 {
            MFT_ENTRY_ROOT
        } else {
            let parent_path: Vec<u8> = {
                let mut p = vec![b'/'];
                for (i, c) in components[..components.len() - 1].iter().enumerate() {
                    if i > 0 {
                        p.push(b'/');
                    }
                    p.extend_from_slice(c);
                }
                p
            };
            self.resolve_path(&parent_path)?
        };

        let dirname = core::str::from_utf8(components[components.len() - 1])
            .map_err(|_| NtfsError::InvalidPath)?;

        if dirname.len() > 255 {
            return Err(NtfsError::NameTooLong);
        }

        let txn = self.journal.borrow_mut().begin_transaction();

        // Allocate MFT entry
        let new_entry_number = self.allocate_mft_entry()?;
        log::debug!("[ntfs] allocated MFT entry #{} for directory '{}'", new_entry_number, dirname);

        // Build directory MFT entry (similar to file but with directory flag and $INDEX_ROOT)
        let parent_mft = self.read_mft_entry(parent_entry_number)?;
        let parent_ref = mft::make_mft_reference(parent_entry_number, parent_mft.header.sequence_number);

        let name_utf16: Vec<u16> = dirname.encode_utf16().collect();
        let now = 0u64; // TODO: current time

        let fn_attr = FileNameAttr {
            parent_reference: parent_ref,
            creation_time: now,
            modification_time: now,
            mft_modification_time: now,
            access_time: now,
            allocated_size: 0,
            real_size: 0,
            flags: crate::filename::FILE_ATTR_DIRECTORY,
            ea_reparse: 0,
            name_length: name_utf16.len() as u8,
            namespace: FileNamespace::Win32AndDos,
            name: String::from(dirname),
            name_utf16: name_utf16.clone(),
        };

        let record_size = self.mft_record_size as usize;
        let mut entry_data = vec![0u8; record_size];

        // FILE header
        entry_data[0..4].copy_from_slice(b"FILE");
        entry_data[0x04..0x06].copy_from_slice(&0x0030u16.to_le_bytes());
        let usa_count = (record_size / 512 + 1) as u16;
        entry_data[0x06..0x08].copy_from_slice(&usa_count.to_le_bytes());
        // LSN
        entry_data[0x08..0x10].copy_from_slice(&self.journal.borrow().current_lsn().to_le_bytes());
        entry_data[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
        entry_data[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
        let first_attr = ((0x30 + usa_count as usize * 2) + 7) & !7;
        entry_data[0x14..0x16].copy_from_slice(&(first_attr as u16).to_le_bytes());
        // Flags: in use + directory
        entry_data[0x16..0x18].copy_from_slice(&0x0003u16.to_le_bytes());
        entry_data[0x1C..0x20].copy_from_slice(&(record_size as u32).to_le_bytes());

        let mut attr_pos = first_attr;

        // $FILE_NAME attribute
        let fn_bytes = fn_attr.to_bytes();
        attr_pos = Self::write_resident_attribute(
            &mut entry_data, attr_pos, AttributeType::FileName, &fn_bytes, 0,
        );

        // $INDEX_ROOT attribute (empty directory index)
        let index_root_value = Self::build_empty_index_root();
        attr_pos = Self::write_resident_attribute(
            &mut entry_data, attr_pos, AttributeType::IndexRoot, &index_root_value, 1,
        );

        // End marker
        entry_data[attr_pos..attr_pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        attr_pos += 4;

        // Update used_size
        entry_data[0x18..0x1C].copy_from_slice(&(attr_pos as u32).to_le_bytes());

        // USA check value
        entry_data[0x30..0x32].copy_from_slice(&0x0001u16.to_le_bytes());

        let new_entry = MftEntry::from_bytes(&entry_data, record_size)
            .ok_or(NtfsError::Corrupt("failed to parse newly created directory MFT entry"))?;

        self.write_mft_entry(new_entry_number, &new_entry)?;

        // Insert index entry into parent directory
        self.insert_index_entry(parent_entry_number, new_entry_number, &fn_attr)?;

        self.journal.borrow_mut().commit_transaction(txn);
        self.flush_journal()?;

        log::info!("[ntfs] created directory '{}' as MFT#{}", dirname, new_entry_number);
        Ok(())
    }

    /// Build an empty $INDEX_ROOT value for a new directory.
    fn build_empty_index_root() -> Vec<u8> {
        let mut buf = vec![0u8; 32]; // 16 bytes header + 16 bytes index header

        // Indexed attribute type: $FILE_NAME (0x30)
        buf[0..4].copy_from_slice(&0x00000030u32.to_le_bytes());
        // Collation rule: COLLATION_FILENAME (0x01)
        buf[4..8].copy_from_slice(&0x00000001u32.to_le_bytes());
        // Index block size (4096 typical)
        buf[8..12].copy_from_slice(&4096u32.to_le_bytes());
        // Clusters per index block
        buf[12] = 1;
        // padding: buf[13..16] = 0

        // Index header (at offset 16)
        // entries_offset: offset to first entry from index header start
        let entries_off = 16u32; // index entries start right after the index header
        buf[16..20].copy_from_slice(&entries_off.to_le_bytes());
        // total_size: index header + last entry (16 bytes for sentinel)
        let sentinel_size = 16u32; // minimal last entry
        buf[20..24].copy_from_slice(&(entries_off + sentinel_size).to_le_bytes());
        // allocated_size
        buf[24..28].copy_from_slice(&(entries_off + sentinel_size).to_le_bytes());
        // flags = 0 (small index, no $INDEX_ALLOCATION needed)
        buf[28] = 0;

        // Append the sentinel (last) entry
        let mut sentinel = [0u8; 16];
        // MFT reference = 0
        // entry_length = 16
        sentinel[8..10].copy_from_slice(&16u16.to_le_bytes());
        // content_length = 0
        // flags = LAST_ENTRY
        sentinel[12..14].copy_from_slice(&index::INDEX_ENTRY_FLAG_LAST_ENTRY.to_le_bytes());

        buf.extend_from_slice(&sentinel);

        log::trace!("[ntfs] built empty INDEX_ROOT: {} bytes", buf.len());
        buf
    }

    // -----------------------------------------------------------------------
    // list_dir
    // -----------------------------------------------------------------------

    /// List the contents of a directory.
    pub fn list_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, NtfsError> {
        let path_str = core::str::from_utf8(path).unwrap_or("<invalid>");
        log::info!("[ntfs] list_dir: '{}'", path_str);

        let entry_number = self.resolve_path(path)?;
        let entries = self.read_directory_entries(entry_number)?;

        let mut results = Vec::new();
        for entry in &entries {
            if entry.is_last() {
                continue;
            }
            if let Some(ref fn_attr) = entry.filename {
                // Skip DOS-only names to avoid duplicates
                if fn_attr.namespace == FileNamespace::Dos {
                    continue;
                }
                results.push(DirEntry {
                    mft_entry: entry.entry_number(),
                    name: fn_attr.name.clone(),
                    is_directory: fn_attr.is_directory(),
                    size: fn_attr.real_size,
                    creation_time: fn_attr.creation_time,
                    modification_time: fn_attr.modification_time,
                    flags: fn_attr.flags,
                });
            }
        }

        log::info!("[ntfs] list_dir '{}': {} entries", path_str, results.len());
        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Shutdown
    // -----------------------------------------------------------------------

    /// Cleanly unmount the filesystem.
    ///
    /// Flushes the journal and marks it as clean.
    pub fn unmount(&self) -> Result<(), NtfsError> {
        self.journal.borrow_mut().checkpoint();
        self.journal.borrow_mut().mark_clean();
        self.flush_journal()?;
        self.device.flush()?;
        log::info!("[ntfs] filesystem unmounted cleanly");
        Ok(())
    }
}

impl<D: BlockDevice> fmt::Debug for NtfsFs<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NtfsFs")
            .field("boot_sector", &self.boot_sector)
            .field("cluster_size", &self.cluster_size)
            .field("mft_record_size", &self.mft_record_size)
            .field("mft_offset", &format_args!("0x{:X}", self.mft_offset))
            .field("mft_data_runs", &self.mft_data_runs.len())
            .finish()
    }
}
