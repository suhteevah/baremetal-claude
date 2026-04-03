//! High-level ext4 filesystem read/write API.
//!
//! This module provides the main `Ext4Fs` type that ties together superblock,
//! block groups, inodes, directories, extents, and bitmaps into a usable
//! filesystem interface.
//!
//! ## Usage
//!
//! Implement the `BlockDevice` trait for your storage backend, then:
//!
//! ```rust,no_run
//! use claudio_ext4::{Ext4Fs, BlockDevice};
//!
//! let fs = Ext4Fs::mount(my_device).expect("mount failed");
//! let data = fs.read_file(b"/hello.txt").expect("read failed");
//! fs.write_file(b"/output.txt", &data).expect("write failed");
//! ```

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::bitmap::BitmapAllocator;
use crate::block_group::{self, BlockGroupDesc};
use crate::block_map;
use crate::crc32c;
use crate::dir::{self, DirEntry, DirEntryIter, FT_DIR, FT_REG_FILE};
use crate::encrypt;
use crate::extent::{
    self, ExtentHeader, ExtentIndex, ExtentLeaf,
    EXTENT_HEADER_SIZE, EXTENT_INDEX_SIZE, EXTENT_LEAF_SIZE,
};
use crate::htree;
use crate::inode::{self, Inode, ROOT_INODE};
use crate::journal::{self, JournalSuperblock, JOURNAL_INODE};
use crate::superblock::{
    Superblock, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE,
    INCOMPAT_RECOVER, RO_COMPAT_METADATA_CSUM,
};

/// Errors that can occur during ext4 filesystem operations.
#[derive(Debug)]
pub enum Ext4Error {
    /// The device returned an I/O error.
    IoError,
    /// The superblock magic number is invalid or the superblock is corrupt.
    InvalidSuperblock,
    /// An unsupported feature flag was encountered.
    UnsupportedFeature(&'static str),
    /// The requested path was not found.
    NotFound,
    /// A path component is not a directory.
    NotADirectory,
    /// The target path already exists.
    AlreadyExists,
    /// No free blocks available for allocation.
    NoFreeBlocks,
    /// No free inodes available for allocation.
    NoFreeInodes,
    /// The filesystem is corrupt (e.g., invalid extent tree).
    Corrupt(&'static str),
    /// A filename exceeds the maximum length (255 bytes).
    NameTooLong,
    /// The path is invalid (empty, missing leading slash, etc.).
    InvalidPath,
    /// The target is a directory when a file was expected.
    IsADirectory,
    /// The target is a file when a directory was expected.
    IsNotADirectory,
    /// Directory is not empty (for rmdir).
    DirectoryNotEmpty,
}

impl fmt::Display for Ext4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ext4Error::IoError => write!(f, "I/O error"),
            Ext4Error::InvalidSuperblock => write!(f, "invalid superblock"),
            Ext4Error::UnsupportedFeature(feat) => write!(f, "unsupported feature: {}", feat),
            Ext4Error::NotFound => write!(f, "not found"),
            Ext4Error::NotADirectory => write!(f, "not a directory"),
            Ext4Error::AlreadyExists => write!(f, "already exists"),
            Ext4Error::NoFreeBlocks => write!(f, "no free blocks"),
            Ext4Error::NoFreeInodes => write!(f, "no free inodes"),
            Ext4Error::Corrupt(msg) => write!(f, "filesystem corrupt: {}", msg),
            Ext4Error::NameTooLong => write!(f, "filename too long"),
            Ext4Error::InvalidPath => write!(f, "invalid path"),
            Ext4Error::IsADirectory => write!(f, "is a directory"),
            Ext4Error::IsNotADirectory => write!(f, "is not a directory"),
            Ext4Error::DirectoryNotEmpty => write!(f, "directory not empty"),
        }
    }
}

/// Trait for the underlying block storage device.
///
/// Implement this for your NVMe driver, virtio-blk, RAM disk, or disk image
/// to provide ext4 with raw block access.
pub trait BlockDevice {
    /// Read `buf.len()` bytes from the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition.
    /// Returns `Ok(())` on success.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), Ext4Error>;

    /// Write `buf.len()` bytes to the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition.
    /// Returns `Ok(())` on success.
    fn write_bytes(&self, offset: u64, buf: &[u8]) -> Result<(), Ext4Error>;

    /// Flush any cached writes to the underlying storage.
    ///
    /// Called after metadata updates (superblock, bitmaps, etc.) to ensure durability.
    fn flush(&self) -> Result<(), Ext4Error> {
        Ok(())
    }
}

/// Main ext4 filesystem handle.
///
/// Holds the parsed superblock, block group descriptors, and a reference to the
/// block device. All operations go through this struct.
pub struct Ext4Fs<D: BlockDevice> {
    /// The underlying block device.
    pub device: D,
    /// The parsed superblock.
    pub sb: Superblock,
    /// Block group descriptors, one per block group.
    pub groups: Vec<BlockGroupDesc>,
    /// CRC32C seed computed from the superblock UUID.
    /// Only meaningful when `has_metadata_csum` is true.
    pub csum_seed: u32,
    /// Whether metadata checksums are enabled.
    pub has_metadata_csum: bool,
    /// Hash seed for HTree directory lookups (from superblock s_hash_seed).
    pub hash_seed: [u32; 4],
}

impl<D: BlockDevice> Ext4Fs<D> {
    /// Mount an ext4 filesystem from the given block device.
    ///
    /// Reads and validates the superblock, then loads all block group descriptors.
    /// If the journal has uncommitted transactions (INCOMPAT_RECOVER), replays them.
    pub fn mount(device: D) -> Result<Self, Ext4Error> {
        log::info!("[ext4::mount] mounting ext4 filesystem...");

        // Read superblock
        let mut sb_buf = vec![0u8; SUPERBLOCK_SIZE];
        device.read_bytes(SUPERBLOCK_OFFSET, &mut sb_buf)?;

        let sb = Superblock::from_bytes(&sb_buf).ok_or_else(|| {
            log::error!("[ext4::mount] failed to parse superblock");
            Ext4Error::InvalidSuperblock
        })?;

        let has_metadata_csum = sb.feature_ro_compat & RO_COMPAT_METADATA_CSUM != 0;
        let csum_seed = if has_metadata_csum {
            let seed = crc32c::crc32c_uuid_seed(&sb.uuid);
            log::debug!("[ext4::mount] metadata checksums enabled, UUID seed=0x{:08X}", seed);

            // Verify superblock checksum
            if !crc32c::verify_superblock_checksum(&sb_buf) {
                log::warn!("[ext4::mount] superblock checksum verification failed (continuing anyway)");
            }
            seed
        } else {
            0
        };

        // Read hash seed from superblock (at offset 0xEC, 4 x u32)
        let hash_seed = if sb_buf.len() >= 0xFC {
            [
                u32::from_le_bytes([sb_buf[0xEC], sb_buf[0xED], sb_buf[0xEE], sb_buf[0xEF]]),
                u32::from_le_bytes([sb_buf[0xF0], sb_buf[0xF1], sb_buf[0xF2], sb_buf[0xF3]]),
                u32::from_le_bytes([sb_buf[0xF4], sb_buf[0xF5], sb_buf[0xF6], sb_buf[0xF7]]),
                u32::from_le_bytes([sb_buf[0xF8], sb_buf[0xF9], sb_buf[0xFA], sb_buf[0xFB]]),
            ]
        } else {
            [0u32; 4]
        };

        log::info!("[ext4::mount] superblock valid: {} blocks, {} inodes, block_size={}, volume={:?}",
            sb.total_blocks(), sb.inodes_count, sb.block_size(), sb.volume_name_str());

        // Read block group descriptor table
        let bg_count = sb.block_group_count();
        let desc_size = sb.group_desc_size() as usize;
        let gdt_block = if sb.block_size() == 1024 { 2 } else { 1 };
        let gdt_offset = gdt_block as u64 * sb.block_size();
        let gdt_len = bg_count as usize * desc_size;

        log::debug!("[ext4::mount] reading {} block group descriptors from offset {} ({} bytes)",
            bg_count, gdt_offset, gdt_len);

        let mut gdt_buf = vec![0u8; gdt_len];
        device.read_bytes(gdt_offset, &mut gdt_buf)?;

        let groups = block_group::parse_block_group_table(&gdt_buf, bg_count, desc_size);
        if groups.len() != bg_count as usize {
            log::error!("[ext4::mount] expected {} block groups, parsed {}", bg_count, groups.len());
            return Err(Ext4Error::Corrupt("incomplete block group descriptor table"));
        }

        log::info!("[ext4::mount] mounted successfully: {} block groups", groups.len());

        let mut fs = Ext4Fs { device, sb, groups, csum_seed, has_metadata_csum, hash_seed };

        // Journal replay if needed
        if fs.sb.has_journal() && (fs.sb.feature_incompat & INCOMPAT_RECOVER != 0) {
            log::info!("[ext4::mount] filesystem needs journal recovery, replaying...");
            match fs.replay_journal() {
                Ok(count) => {
                    log::info!("[ext4::mount] journal replay complete: {} transactions replayed", count);
                }
                Err(e) => {
                    log::error!("[ext4::mount] journal replay failed: {}, continuing with caution", e);
                }
            }
        } else if fs.sb.has_journal() {
            log::debug!("[ext4::mount] journal present but clean, no replay needed");
        }

        Ok(fs)
    }

    /// Replay the journal (JBD2) if it has uncommitted transactions.
    ///
    /// Returns the number of transactions replayed.
    fn replay_journal(&mut self) -> Result<usize, Ext4Error> {
        // Read the journal inode (typically inode 8)
        let journal_ino = JOURNAL_INODE;
        let journal_inode = self.read_inode(journal_ino)?;

        if journal_inode.size() == 0 {
            log::warn!("[ext4::journal] journal inode {} has zero size", journal_ino);
            return Ok(0);
        }

        // Resolve journal data blocks (the journal itself uses extents or block map)
        let journal_blocks = if journal_inode.uses_extents() {
            self.resolve_extents(&journal_inode)?
        } else {
            // For block-map journals, we need a different approach.
            // Build a synthetic extent list by resolving each logical block.
            log::debug!("[ext4::journal] journal inode uses block map");
            Vec::new() // Will use block_map for individual reads below
        };

        let uses_block_map = journal_blocks.is_empty() && !journal_inode.uses_extents();
        let block_size = self.sb.block_size();

        // Helper: read a journal-relative block
        let read_journal_block = |journal_block_idx: u64| -> Option<Vec<u8>> {
            if uses_block_map {
                match block_map::read_block_map(
                    // We can't pass &self here in a closure, so we handle differently below.
                    // This path is unlikely for ext4 journals; they almost always use extents.
                    unsafe { &*(self as *const Self) },
                    &journal_inode,
                    journal_block_idx,
                ) {
                    Ok(Some(phys)) => {
                        let mut buf = vec![0u8; block_size as usize];
                        match self.device.read_bytes(phys * block_size, &mut buf) {
                            Ok(()) => Some(buf),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                }
            } else {
                // Find the physical block from extents
                for ext in &journal_blocks {
                    if let Some(phys) = ext.map_block(journal_block_idx as u32) {
                        let mut buf = vec![0u8; block_size as usize];
                        match self.device.read_bytes(phys * block_size, &mut buf) {
                            Ok(()) => return Some(buf),
                            Err(_) => return None,
                        }
                    }
                }
                None
            }
        };

        // Read journal superblock (block 0 of journal)
        let jsb_data = read_journal_block(0).ok_or(Ext4Error::Corrupt("failed to read journal superblock"))?;
        let jsb = JournalSuperblock::from_bytes(&jsb_data)
            .ok_or(Ext4Error::Corrupt("invalid journal superblock"))?;

        if !jsb.needs_recovery() {
            log::info!("[ext4::journal] journal is clean (log_start=0)");
            return Ok(0);
        }

        // Scan for committed transactions
        let transactions = journal::scan_journal(&jsb, read_journal_block);

        if transactions.is_empty() {
            log::info!("[ext4::journal] no committed transactions to replay");
            return Ok(0);
        }

        // Replay each transaction: copy journaled blocks to their final filesystem locations
        let mut replayed = 0;
        for txn in &transactions {
            log::info!("[ext4::journal] replaying transaction seq={} ({} blocks)",
                txn.sequence, txn.mappings.len());

            for mapping in &txn.mappings {
                // Read the journaled data block
                let jblock = if uses_block_map {
                    match block_map::read_block_map(
                        unsafe { &*(self as *const Self) },
                        &journal_inode,
                        mapping.journal_block,
                    ) {
                        Ok(Some(phys)) => {
                            let mut buf = vec![0u8; block_size as usize];
                            self.device.read_bytes(phys * block_size, &mut buf)?;
                            buf
                        }
                        _ => {
                            log::error!("[ext4::journal] failed to read journal block {}", mapping.journal_block);
                            continue;
                        }
                    }
                } else {
                    let mut found = None;
                    for ext in &journal_blocks {
                        if let Some(phys) = ext.map_block(mapping.journal_block as u32) {
                            let mut buf = vec![0u8; block_size as usize];
                            self.device.read_bytes(phys * block_size, &mut buf)?;
                            found = Some(buf);
                            break;
                        }
                    }
                    match found {
                        Some(b) => b,
                        None => {
                            log::error!("[ext4::journal] no extent for journal block {}", mapping.journal_block);
                            continue;
                        }
                    }
                };

                let mut data = jblock;

                // If the block was escaped, restore the original JBD2 magic
                if mapping.escaped {
                    let magic_bytes = journal::JBD2_MAGIC.to_be_bytes();
                    data[..4].copy_from_slice(&magic_bytes);
                }

                // Write to the final filesystem location
                let fs_offset = mapping.fs_block * block_size;
                self.device.write_bytes(fs_offset, &data)?;
                log::trace!("[ext4::journal] replayed block -> fs_block={}", mapping.fs_block);
            }
            replayed += 1;
        }

        // Clear the INCOMPAT_RECOVER flag and reset journal log_start
        self.sb.feature_incompat &= !INCOMPAT_RECOVER;
        let sb_bytes = self.sb.to_bytes();
        self.device.write_bytes(SUPERBLOCK_OFFSET, &sb_bytes)?;
        self.device.flush()?;

        log::info!("[ext4::journal] journal replay complete: {} transactions", replayed);
        Ok(replayed)
    }

    /// Read a block from the device.
    ///
    /// `block_num` is the absolute block number. Returns a Vec of `block_size` bytes.
    pub fn read_block(&self, block_num: u64) -> Result<Vec<u8>, Ext4Error> {
        let offset = block_num * self.sb.block_size();
        let mut buf = vec![0u8; self.sb.block_size() as usize];
        log::trace!("[ext4::io] reading block {} (offset={})", block_num, offset);
        self.device.read_bytes(offset, &mut buf)?;
        Ok(buf)
    }

    /// Write a block to the device.
    pub fn write_block(&self, block_num: u64, data: &[u8]) -> Result<(), Ext4Error> {
        let offset = block_num * self.sb.block_size();
        log::trace!("[ext4::io] writing block {} (offset={}, {} bytes)", block_num, offset, data.len());
        self.device.write_bytes(offset, data)?;
        Ok(())
    }

    /// Read an inode by its inode number (1-based).
    pub fn read_inode(&self, ino: u32) -> Result<Inode, Ext4Error> {
        if ino == 0 {
            log::error!("[ext4::inode] cannot read inode 0 (does not exist)");
            return Err(Ext4Error::NotFound);
        }

        let group = ((ino - 1) / self.sb.inodes_per_group) as usize;
        let index = ((ino - 1) % self.sb.inodes_per_group) as usize;

        if group >= self.groups.len() {
            log::error!("[ext4::inode] inode {} maps to group {} but only {} groups exist",
                ino, group, self.groups.len());
            return Err(Ext4Error::Corrupt("inode group out of range"));
        }

        let inode_table_block = self.groups[group].inode_table();
        let inode_size = self.sb.inode_size as u64;
        let offset = inode_table_block * self.sb.block_size() + index as u64 * inode_size;

        log::trace!("[ext4::inode] reading inode {}: group={}, index={}, offset={}",
            ino, group, index, offset);

        let mut buf = vec![0u8; inode_size as usize];
        self.device.read_bytes(offset, &mut buf)?;

        Inode::from_bytes(&buf, self.sb.inode_size as usize).ok_or_else(|| {
            log::error!("[ext4::inode] failed to parse inode {}", ino);
            Ext4Error::Corrupt("invalid inode data")
        })
    }

    /// Write an inode back to disk by its inode number (1-based).
    pub fn write_inode(&self, ino: u32, inode: &Inode) -> Result<(), Ext4Error> {
        if ino == 0 {
            return Err(Ext4Error::NotFound);
        }

        let group = ((ino - 1) / self.sb.inodes_per_group) as usize;
        let index = ((ino - 1) % self.sb.inodes_per_group) as usize;

        if group >= self.groups.len() {
            return Err(Ext4Error::Corrupt("inode group out of range"));
        }

        let inode_table_block = self.groups[group].inode_table();
        let inode_size = self.sb.inode_size as u64;
        let offset = inode_table_block * self.sb.block_size() + index as u64 * inode_size;

        log::trace!("[ext4::inode] writing inode {}: group={}, index={}, offset={}",
            ino, group, index, offset);

        let buf = inode.to_bytes(self.sb.inode_size as usize);
        self.device.write_bytes(offset, &buf)?;
        Ok(())
    }

    /// Resolve all data blocks for an inode using its extent tree.
    ///
    /// Returns a list of (logical_block, physical_block) mappings covering
    /// all data blocks of the file.
    pub fn resolve_extents(&self, inode: &Inode) -> Result<Vec<ExtentLeaf>, Ext4Error> {
        if !inode.uses_extents() {
            log::error!("[ext4::extent] inode does not use extents, use block_map instead");
            return Err(Ext4Error::UnsupportedFeature("legacy block map"));
        }

        let header = inode.extent_header().ok_or_else(|| {
            log::error!("[ext4::extent] failed to parse extent header from inode");
            Ext4Error::Corrupt("invalid extent header")
        })?;

        log::debug!("[ext4::extent] resolving extent tree: depth={}, entries={}",
            header.depth, header.entries);

        if header.is_leaf() {
            // Leaf node directly in the inode
            let leaves = extent::parse_leaves(&inode.i_block);
            log::debug!("[ext4::extent] resolved {} leaf extents from inode root", leaves.len());
            return Ok(leaves);
        }

        // Internal node: need to traverse the tree
        self.resolve_extent_tree_recursive(&inode.i_block, header.depth)
    }

    /// Recursively traverse the extent tree, reading child nodes from disk.
    fn resolve_extent_tree_recursive(&self, node_buf: &[u8], depth: u16) -> Result<Vec<ExtentLeaf>, Ext4Error> {
        if depth == 0 {
            return Ok(extent::parse_leaves(node_buf));
        }

        let indices = extent::parse_indices(node_buf);
        let mut all_leaves = Vec::new();

        for idx in &indices {
            let child_block = idx.physical_block();
            log::trace!("[ext4::extent] traversing index -> physical block {}", child_block);
            let child_data = self.read_block(child_block)?;
            let child_leaves = self.resolve_extent_tree_recursive(&child_data, depth - 1)?;
            all_leaves.extend(child_leaves);
        }

        log::debug!("[ext4::extent] resolved {} leaves from depth-{} subtree", all_leaves.len(), depth);
        Ok(all_leaves)
    }

    /// Read all data blocks of an inode into a contiguous Vec.
    ///
    /// The returned Vec is truncated to the inode's actual file size.
    /// Supports both extent-based and legacy block-map inodes.
    /// Returns an error for encrypted inodes.
    pub fn read_inode_data(&self, inode: &Inode) -> Result<Vec<u8>, Ext4Error> {
        // Check encryption before reading
        encrypt::check_encryption(inode.flags)?;

        let file_size = inode.size();
        if file_size == 0 {
            log::trace!("[ext4::read] inode has zero size");
            return Ok(Vec::new());
        }

        // Use legacy block map if the inode doesn't have the extents flag
        if !inode.uses_extents() {
            log::debug!("[ext4::read] inode uses legacy block map, reading via indirect pointers");
            return block_map::read_block_map_data(self, inode);
        }

        let extents = self.resolve_extents(inode)?;
        let block_size = self.sb.block_size();
        let total_blocks = ((file_size + block_size - 1) / block_size) as u32;

        log::debug!("[ext4::read] reading {} bytes ({} blocks, {} extents)",
            file_size, total_blocks, extents.len());

        let mut data = Vec::with_capacity(file_size as usize);

        for logical_block in 0..total_blocks {
            let phys = extent::find_leaf_for_block(&extents, logical_block)
                .and_then(|leaf| leaf.map_block(logical_block))
                .ok_or_else(|| {
                    log::error!("[ext4::read] no extent mapping for logical block {}", logical_block);
                    Ext4Error::Corrupt("missing extent for logical block")
                })?;

            let block_data = self.read_block(phys)?;
            data.extend_from_slice(&block_data);
        }

        // Truncate to actual file size
        data.truncate(file_size as usize);
        log::debug!("[ext4::read] read {} bytes of inode data", data.len());
        Ok(data)
    }

    /// Look up a path component by component, starting from the root inode.
    ///
    /// Returns the inode number and parsed Inode for the target.
    /// Path must start with '/'.
    pub fn lookup_path(&self, path: &[u8]) -> Result<(u32, Inode), Ext4Error> {
        if path.is_empty() || path[0] != b'/' {
            log::error!("[ext4::lookup] invalid path (must start with '/'): {:?}",
                core::str::from_utf8(path).unwrap_or("<invalid>"));
            return Err(Ext4Error::InvalidPath);
        }

        log::debug!("[ext4::lookup] resolving path: {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let mut current_ino = ROOT_INODE;
        let mut current_inode = self.read_inode(ROOT_INODE)?;

        // Split path and iterate components (skip leading '/' and empty segments)
        let components: Vec<&[u8]> = path[1..]
            .split(|&b| b == b'/')
            .filter(|c| !c.is_empty())
            .collect();

        if components.is_empty() {
            // Root directory
            log::debug!("[ext4::lookup] resolved to root inode {}", ROOT_INODE);
            return Ok((ROOT_INODE, current_inode));
        }

        for component in components.iter() {
            if !current_inode.is_dir() {
                log::error!("[ext4::lookup] inode {} is not a directory at component {:?}",
                    current_ino, core::str::from_utf8(component).unwrap_or("<invalid>"));
                return Err(Ext4Error::NotADirectory);
            }

            log::trace!("[ext4::lookup] searching directory inode {} for {:?}",
                current_ino, core::str::from_utf8(component).unwrap_or("<invalid>"));

            let (found_ino, found_inode) = self.lookup_in_dir(&current_inode, component)?;
            current_ino = found_ino;
            current_inode = found_inode;

            log::trace!("[ext4::lookup] component {:?} -> inode {}",
                core::str::from_utf8(component).unwrap_or("<invalid>"), current_ino);
        }

        log::debug!("[ext4::lookup] resolved path -> inode {}", current_ino);
        Ok((current_ino, current_inode))
    }

    /// Search a directory inode for a name.
    ///
    /// Uses HTree indexed lookup if the directory has the EXT4_INDEX_FL flag,
    /// otherwise falls back to linear scan.
    ///
    /// Returns the inode number and parsed Inode of the found entry.
    fn lookup_in_dir(&self, dir_inode: &Inode, name: &[u8]) -> Result<(u32, Inode), Ext4Error> {
        // Check if this directory uses HTree indexing
        if dir_inode.flags & htree::EXT4_INDEX_FL != 0 {
            log::debug!("[ext4::lookup] directory uses HTree, attempting indexed lookup");
            match self.htree_lookup_in_dir(dir_inode, name) {
                Ok(result) => return Ok(result),
                Err(e) => {
                    log::warn!("[ext4::lookup] HTree lookup failed: {}, falling back to linear scan", e);
                    // Fall through to linear scan
                }
            }
        }

        // Linear scan fallback
        self.linear_lookup_in_dir(dir_inode, name)
    }

    /// HTree-based directory lookup.
    fn htree_lookup_in_dir(&self, dir_inode: &Inode, name: &[u8]) -> Result<(u32, Inode), Ext4Error> {
        // Read the first block (dx_root) of the directory
        let dir_data_blocks = self.resolve_dir_data_blocks(dir_inode)?;
        if dir_data_blocks.is_empty() {
            return Err(Ext4Error::NotFound);
        }

        let root_block_data = self.read_block(dir_data_blocks[0])?;

        let entry = htree::htree_lookup(
            &root_block_data,
            name,
            &self.hash_seed,
            |block_num| {
                // block_num is a logical block index within the directory
                if (block_num as usize) < dir_data_blocks.len() {
                    self.read_block(dir_data_blocks[block_num as usize]).ok()
                } else {
                    None
                }
            },
        ).ok_or(Ext4Error::NotFound)?;

        let ino = entry.inode;
        let inode = self.read_inode(ino)?;
        Ok((ino, inode))
    }

    /// Resolve all physical block numbers for a directory's data blocks.
    fn resolve_dir_data_blocks(&self, dir_inode: &Inode) -> Result<Vec<u64>, Ext4Error> {
        let mut blocks = Vec::new();

        if dir_inode.uses_extents() {
            let extents = self.resolve_extents(dir_inode)?;
            for ext in &extents {
                for offset in 0..ext.block_count() {
                    blocks.push(ext.physical_start() + offset as u64);
                }
            }
        } else {
            // Legacy block map
            let block_size = self.sb.block_size();
            let total_blocks = (dir_inode.size() + block_size - 1) / block_size;
            for i in 0..total_blocks {
                match block_map::read_block_map(self, dir_inode, i)? {
                    Some(phys) => blocks.push(phys),
                    None => blocks.push(0), // hole
                }
            }
        }

        Ok(blocks)
    }

    /// Linear scan directory lookup (original implementation).
    fn linear_lookup_in_dir(&self, dir_inode: &Inode, name: &[u8]) -> Result<(u32, Inode), Ext4Error> {
        if dir_inode.uses_extents() {
            let extents = self.resolve_extents(dir_inode)?;
            for ext in &extents {
                for blk_offset in 0..ext.block_count() {
                    let phys = ext.physical_start() + blk_offset as u64;
                    let block_data = self.read_block(phys)?;

                    if let Some((_offset, entry)) = dir::lookup_in_block(&block_data, name) {
                        let ino = entry.inode;
                        let inode = self.read_inode(ino)?;
                        return Ok((ino, inode));
                    }
                }
            }
        } else {
            // Legacy block map
            let block_size = self.sb.block_size();
            let total_blocks = (dir_inode.size() + block_size - 1) / block_size;
            for logical in 0..total_blocks {
                if let Some(phys) = block_map::read_block_map(self, dir_inode, logical)? {
                    let block_data = self.read_block(phys)?;
                    if let Some((_offset, entry)) = dir::lookup_in_block(&block_data, name) {
                        let ino = entry.inode;
                        let inode = self.read_inode(ino)?;
                        return Ok((ino, inode));
                    }
                }
            }
        }

        log::trace!("[ext4::lookup] {:?} not found in directory",
            core::str::from_utf8(name).unwrap_or("<invalid>"));
        Err(Ext4Error::NotFound)
    }

    /// Read a file by its absolute path.
    ///
    /// Returns the file contents as a Vec<u8>.
    pub fn read_file(&self, path: &[u8]) -> Result<Vec<u8>, Ext4Error> {
        log::info!("[ext4::read_file] reading {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let (_ino, inode) = self.lookup_path(path)?;

        if inode.is_dir() {
            log::error!("[ext4::read_file] path is a directory, not a file");
            return Err(Ext4Error::IsADirectory);
        }

        let data = self.read_inode_data(&inode)?;
        log::info!("[ext4::read_file] read {} bytes from {:?}",
            data.len(), core::str::from_utf8(path).unwrap_or("<invalid>"));
        Ok(data)
    }

    /// List directory entries at the given path.
    pub fn list_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, Ext4Error> {
        log::info!("[ext4::list_dir] listing {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let (_ino, inode) = self.lookup_path(path)?;

        if !inode.is_dir() {
            return Err(Ext4Error::IsNotADirectory);
        }

        let mut entries = Vec::new();

        if inode.uses_extents() {
            let extents = self.resolve_extents(&inode)?;
            for ext in &extents {
                for blk_offset in 0..ext.block_count() {
                    let phys = ext.physical_start() + blk_offset as u64;
                    let block_data = self.read_block(phys)?;

                    for (_offset, entry) in DirEntryIter::new(&block_data) {
                        if !entry.is_deleted() {
                            entries.push(entry);
                        }
                    }
                }
            }
        } else {
            // Legacy block map
            let block_size = self.sb.block_size();
            let total_blocks = (inode.size() + block_size - 1) / block_size;
            for logical in 0..total_blocks {
                if let Some(phys) = block_map::read_block_map(self, &inode, logical)? {
                    let block_data = self.read_block(phys)?;
                    for (_offset, entry) in DirEntryIter::new(&block_data) {
                        if !entry.is_deleted() {
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        log::info!("[ext4::list_dir] found {} entries", entries.len());
        Ok(entries)
    }

    // --- Write operations ---

    /// Read the block bitmap for a block group.
    fn read_block_bitmap(&self, group: usize) -> Result<Vec<u8>, Ext4Error> {
        let bitmap_block = self.groups[group].block_bitmap();
        log::trace!("[ext4::bitmap] reading block bitmap for group {} (block {})", group, bitmap_block);
        self.read_block(bitmap_block)
    }

    /// Write the block bitmap for a block group.
    fn write_block_bitmap(&self, group: usize, bitmap: &[u8]) -> Result<(), Ext4Error> {
        let bitmap_block = self.groups[group].block_bitmap();
        log::trace!("[ext4::bitmap] writing block bitmap for group {} (block {})", group, bitmap_block);
        self.write_block(bitmap_block, bitmap)
    }

    /// Read the inode bitmap for a block group.
    fn read_inode_bitmap(&self, group: usize) -> Result<Vec<u8>, Ext4Error> {
        let bitmap_block = self.groups[group].inode_bitmap();
        log::trace!("[ext4::bitmap] reading inode bitmap for group {} (block {})", group, bitmap_block);
        self.read_block(bitmap_block)
    }

    /// Write the inode bitmap for a block group.
    fn write_inode_bitmap(&self, group: usize, bitmap: &[u8]) -> Result<(), Ext4Error> {
        let bitmap_block = self.groups[group].inode_bitmap();
        log::trace!("[ext4::bitmap] writing inode bitmap for group {} (block {})", group, bitmap_block);
        self.write_block(bitmap_block, bitmap)
    }

    /// Allocate a single block from any block group.
    ///
    /// Returns the absolute block number. Prefers the given `preferred_group` if it
    /// has free blocks.
    pub fn allocate_block(&mut self, preferred_group: usize) -> Result<u64, Ext4Error> {
        log::debug!("[ext4::alloc] allocating block (preferred group={})", preferred_group);

        let num_groups = self.groups.len();
        for offset in 0..num_groups {
            let group = (preferred_group + offset) % num_groups;

            if self.groups[group].free_blocks_count() == 0 {
                continue;
            }

            let mut bitmap = self.read_block_bitmap(group)?;
            let total_bits = self.sb.blocks_per_group;

            if let Some(bit) = BitmapAllocator::allocate_one(&mut bitmap, 0, total_bits) {
                self.write_block_bitmap(group, &bitmap)?;

                // Update block group free count
                let new_count = self.groups[group].free_blocks_count().saturating_sub(1);
                self.groups[group].set_free_blocks_count(new_count);

                // Update superblock free count
                let sb_free = self.sb.free_blocks().saturating_sub(1);
                self.sb.free_blocks_count_lo = sb_free as u32;
                self.sb.free_blocks_count_hi = (sb_free >> 32) as u32;

                let abs_block = group as u64 * self.sb.blocks_per_group as u64
                    + self.sb.first_data_block as u64
                    + bit as u64;

                log::info!("[ext4::alloc] allocated block {} (group={}, bit={})", abs_block, group, bit);
                return Ok(abs_block);
            }
        }

        log::error!("[ext4::alloc] no free blocks in any group");
        Err(Ext4Error::NoFreeBlocks)
    }

    /// Allocate a new inode from any block group.
    ///
    /// Returns the new inode number (1-based).
    pub fn allocate_inode(&mut self, preferred_group: usize) -> Result<u32, Ext4Error> {
        log::debug!("[ext4::alloc] allocating inode (preferred group={})", preferred_group);

        let num_groups = self.groups.len();
        for offset in 0..num_groups {
            let group = (preferred_group + offset) % num_groups;

            if self.groups[group].free_inodes_count() == 0 {
                continue;
            }

            let mut bitmap = self.read_inode_bitmap(group)?;
            let total_bits = self.sb.inodes_per_group;

            if let Some(bit) = BitmapAllocator::allocate_one(&mut bitmap, 0, total_bits) {
                self.write_inode_bitmap(group, &bitmap)?;

                // Update block group free count
                let new_count = self.groups[group].free_inodes_count().saturating_sub(1);
                self.groups[group].set_free_inodes_count(new_count);

                // Update superblock free count
                self.sb.free_inodes_count = self.sb.free_inodes_count.saturating_sub(1);

                let ino = group as u32 * self.sb.inodes_per_group + bit + 1;

                log::info!("[ext4::alloc] allocated inode {} (group={}, bit={})", ino, group, bit);
                return Ok(ino);
            }
        }

        log::error!("[ext4::alloc] no free inodes in any group");
        Err(Ext4Error::NoFreeInodes)
    }

    /// Add a directory entry to a directory inode.
    ///
    /// Searches existing blocks for free space; allocates a new block if needed.
    fn add_dir_entry(&mut self, dir_ino: u32, dir_inode: &mut Inode, entry: &DirEntry) -> Result<(), Ext4Error> {
        let needed = entry.actual_size();
        log::debug!("[ext4::dir] adding entry {:?} (inode={}) to dir inode {}, needs {} bytes",
            entry.name_str(), entry.inode, dir_ino, needed);

        let extents = self.resolve_extents(dir_inode)?;

        // Try to find space in existing blocks
        for ext in &extents {
            for blk_offset in 0..ext.block_count() {
                let phys = ext.physical_start() + blk_offset as u64;
                let block_data = self.read_block(phys)?;

                if let Some((insert_offset, available)) = dir::find_space_in_block(&block_data, needed) {
                    // Found space: insert the entry
                    let mut modified_block = block_data;

                    // If inserting at a split point (after an existing entry), update the
                    // previous entry's rec_len to its actual size first
                    if insert_offset > 0 {
                            // The previous entry's rec_len was already accounted for by
                        // find_space_in_block, which returns the offset after the
                        // previous entry's actual data.
                    }

                    let mut new_entry = entry.clone();
                    new_entry.rec_len = available;
                    let entry_bytes = new_entry.to_bytes();
                    let end = insert_offset + entry_bytes.len();
                    if end <= modified_block.len() {
                        modified_block[insert_offset..end].copy_from_slice(&entry_bytes);
                        self.write_block(phys, &modified_block)?;
                        log::info!("[ext4::dir] inserted entry at offset {} in block {}", insert_offset, phys);
                        return Ok(());
                    }
                }
            }
        }

        // No space in existing blocks: allocate a new block
        log::debug!("[ext4::dir] no space in existing blocks, allocating new block for directory");
        let dir_group = ((dir_ino - 1) / self.sb.inodes_per_group) as usize;
        let new_block = self.allocate_block(dir_group)?;

        // Create new block with just this entry, rec_len covering the whole block
        let block_size = self.sb.block_size() as u16;
        let mut new_entry = entry.clone();
        new_entry.rec_len = block_size;
        let entry_bytes = new_entry.to_bytes();

        let mut block_buf = vec![0u8; self.sb.block_size() as usize];
        block_buf[..entry_bytes.len()].copy_from_slice(&entry_bytes);
        self.write_block(new_block, &block_buf)?;

        // Update the directory inode's extent tree to include the new block
        self.append_extent(dir_ino, dir_inode, new_block)?;

        // Update directory size
        let new_size = dir_inode.size() + self.sb.block_size();
        dir_inode.set_size(new_size);
        self.write_inode(dir_ino, dir_inode)?;

        log::info!("[ext4::dir] added entry in new block {}", new_block);
        Ok(())
    }

    /// Append a new physical block to an inode's extent tree.
    ///
    /// Supports extent tree splitting when the root node is full:
    /// - Allocates a new block for leaf storage
    /// - Moves existing extents to the new leaf block
    /// - Converts root to an index node pointing to the leaf
    /// - Adds the new extent to the leaf
    fn append_extent(&mut self, ino: u32, inode: &mut Inode, phys_block: u64) -> Result<(), Ext4Error> {
        let header = inode.extent_header().ok_or(Ext4Error::Corrupt("no extent header"))?;

        // Calculate the next logical block number
        let next_logical = if header.is_leaf() {
            let leaves = extent::parse_leaves(&inode.i_block);
            leaves.iter()
                .map(|l| l.block + l.block_count())
                .max()
                .unwrap_or(0)
        } else {
            // For non-leaf root, we need to find the max across all leaves
            match self.resolve_extents(inode) {
                Ok(all_leaves) => all_leaves.iter()
                    .map(|l| l.block + l.block_count())
                    .max()
                    .unwrap_or(0),
                Err(_) => 0,
            }
        };

        let new_leaf = ExtentLeaf {
            block: next_logical,
            len: 1,
            start_hi: (phys_block >> 32) as u16,
            start_lo: phys_block as u32,
        };

        if header.is_leaf() {
            if header.entries < header.max {
                // Simple case: space in the root leaf node
                let leaf_bytes = new_leaf.to_bytes();
                let entry_offset = EXTENT_HEADER_SIZE + header.entries as usize * EXTENT_LEAF_SIZE;
                inode.i_block[entry_offset..entry_offset + EXTENT_LEAF_SIZE].copy_from_slice(&leaf_bytes);

                // Update header entries count
                let new_entries = header.entries + 1;
                inode.i_block[2] = new_entries as u8;
                inode.i_block[3] = (new_entries >> 8) as u8;

                self.write_inode(ino, inode)?;
                log::debug!("[ext4::extent] appended extent: logical={}, phys={}", next_logical, phys_block);
                Ok(())
            } else {
                // Root leaf node is full: split into index node + leaf block
                log::info!("[ext4::extent] root node full ({}/{}), splitting extent tree",
                    header.entries, header.max);
                self.split_root_extent_node(ino, inode, new_leaf)
            }
        } else {
            // Root is already an index node: find the right leaf and insert there
            self.insert_extent_into_tree(ino, inode, new_leaf)
        }
    }

    /// Split the root extent node when it's full.
    ///
    /// 1. Allocate a new block for the leaf node
    /// 2. Copy all existing leaf extents to the new block
    /// 3. Add the new extent to the new block
    /// 4. Convert the root to an index node with one entry pointing to the leaf
    fn split_root_extent_node(&mut self, ino: u32, inode: &mut Inode, new_extent: ExtentLeaf) -> Result<(), Ext4Error> {
        let block_size = self.sb.block_size() as usize;
        let group = ((ino - 1) / self.sb.inodes_per_group) as usize;

        // Parse existing leaves from root
        let existing_leaves = extent::parse_leaves(&inode.i_block);

        // Allocate a new block for the leaf node
        let leaf_block = self.allocate_block(group)?;

        // Build the new leaf block
        let max_entries_in_block = ((block_size - EXTENT_HEADER_SIZE) / EXTENT_LEAF_SIZE) as u16;
        let total_entries = existing_leaves.len() as u16 + 1;

        let mut leaf_buf = vec![0u8; block_size];

        // Write leaf header
        let leaf_header = ExtentHeader {
            magic: extent::EXT4_EXTENT_MAGIC,
            entries: total_entries,
            max: max_entries_in_block,
            depth: 0,
            generation: 0,
        };
        leaf_buf[..EXTENT_HEADER_SIZE].copy_from_slice(&leaf_header.to_bytes());

        // Write existing extents
        for (i, leaf) in existing_leaves.iter().enumerate() {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_LEAF_SIZE;
            leaf_buf[off..off + EXTENT_LEAF_SIZE].copy_from_slice(&leaf.to_bytes());
        }

        // Write the new extent
        let new_off = EXTENT_HEADER_SIZE + existing_leaves.len() * EXTENT_LEAF_SIZE;
        leaf_buf[new_off..new_off + EXTENT_LEAF_SIZE].copy_from_slice(&new_extent.to_bytes());

        // Write the leaf block to disk
        self.write_block(leaf_block, &leaf_buf)?;

        // Convert root to an index node with depth=1
        let first_logical = if existing_leaves.is_empty() {
            new_extent.block
        } else {
            existing_leaves[0].block
        };

        // Root index header: depth=1, entries=1, max stays at 4
        let root_header = ExtentHeader {
            magic: extent::EXT4_EXTENT_MAGIC,
            entries: 1,
            max: 4, // root can hold 4 index entries (60 - 12 = 48, / 12 = 4)
            depth: 1,
            generation: 0,
        };

        // Write root header
        inode.i_block = [0u8; inode::I_BLOCK_SIZE];
        inode.i_block[..EXTENT_HEADER_SIZE].copy_from_slice(&root_header.to_bytes());

        // Write the single index entry pointing to our leaf block
        let idx = ExtentIndex {
            block: first_logical,
            leaf_lo: leaf_block as u32,
            leaf_hi: (leaf_block >> 32) as u16,
            padding: 0,
        };
        inode.i_block[EXTENT_HEADER_SIZE..EXTENT_HEADER_SIZE + EXTENT_INDEX_SIZE]
            .copy_from_slice(&idx.to_bytes());

        self.write_inode(ino, inode)?;

        log::info!(
            "[ext4::extent] split root: {} leaves moved to leaf block {}, root is now depth-1 index",
            existing_leaves.len() + 1,
            leaf_block
        );
        Ok(())
    }

    /// Insert an extent into an existing multi-level extent tree.
    ///
    /// Walks the tree to find the correct leaf, splits if necessary.
    fn insert_extent_into_tree(&mut self, ino: u32, inode: &mut Inode, new_extent: ExtentLeaf) -> Result<(), Ext4Error> {
        let header = inode.extent_header().ok_or(Ext4Error::Corrupt("no extent header"))?;

        if header.depth == 0 {
            // Should not happen here, but handle gracefully
            return Err(Ext4Error::Corrupt("expected index node, found leaf"));
        }

        // Find the correct index entry for this logical block
        let indices = extent::parse_indices(&inode.i_block);
        if indices.is_empty() {
            return Err(Ext4Error::Corrupt("empty index node"));
        }

        // Find the right child: largest index.block <= new_extent.block
        let child_idx = {
            let target = find_index_for_insert(&indices, new_extent.block);
            if target < indices.len() { target } else { indices.len() - 1 }
        };

        let child_phys = indices[child_idx].physical_block();
        let child_data = self.read_block(child_phys)?;
        let child_header = ExtentHeader::from_bytes(&child_data)
            .ok_or(Ext4Error::Corrupt("invalid child extent header"))?;

        if child_header.is_leaf() {
            // Try to insert into this leaf
            if child_header.entries < child_header.max {
                // Space available in the leaf
                let mut modified = child_data;
                let entry_off = EXTENT_HEADER_SIZE + child_header.entries as usize * EXTENT_LEAF_SIZE;
                modified[entry_off..entry_off + EXTENT_LEAF_SIZE].copy_from_slice(&new_extent.to_bytes());

                // Update entries count
                let new_count = child_header.entries + 1;
                modified[2] = new_count as u8;
                modified[3] = (new_count >> 8) as u8;

                self.write_block(child_phys, &modified)?;
                log::debug!("[ext4::extent] inserted extent into existing leaf at block {}", child_phys);
                Ok(())
            } else {
                // Leaf is full: split it
                log::info!("[ext4::extent] leaf block {} full ({}/{}), splitting",
                    child_phys, child_header.entries, child_header.max);
                self.split_leaf_extent_node(ino, inode, child_phys, &child_data, new_extent)
            }
        } else {
            // Multi-level tree: recurse deeper (simplified -- only handle 2 levels for now)
            log::warn!("[ext4::extent] deep extent tree (depth>1) insertion not fully supported");
            Err(Ext4Error::UnsupportedFeature("extent tree depth > 2 for insertion"))
        }
    }

    /// Split a full leaf extent node.
    ///
    /// 1. Allocate a new block for the second leaf
    /// 2. Move the upper half of extents to the new block
    /// 3. Add the new extent to the appropriate half
    /// 4. Add a new index entry in the parent (root) pointing to the new leaf
    fn split_leaf_extent_node(
        &mut self,
        ino: u32,
        inode: &mut Inode,
        old_leaf_phys: u64,
        old_leaf_data: &[u8],
        new_extent: ExtentLeaf,
    ) -> Result<(), Ext4Error> {
        let block_size = self.sb.block_size() as usize;
        let group = ((ino - 1) / self.sb.inodes_per_group) as usize;

        // Parse all leaves from the old block
        let mut all_leaves = extent::parse_leaves(old_leaf_data);
        all_leaves.push(new_extent);
        // Sort by logical block number
        all_leaves.sort_by_key(|l| l.block);

        let mid = all_leaves.len() / 2;
        let left_leaves = &all_leaves[..mid];
        let right_leaves = &all_leaves[mid..];

        let max_entries = ((block_size - EXTENT_HEADER_SIZE) / EXTENT_LEAF_SIZE) as u16;

        // Rewrite the old leaf block with left half
        let mut left_buf = vec![0u8; block_size];
        let left_header = ExtentHeader {
            magic: extent::EXT4_EXTENT_MAGIC,
            entries: left_leaves.len() as u16,
            max: max_entries,
            depth: 0,
            generation: 0,
        };
        left_buf[..EXTENT_HEADER_SIZE].copy_from_slice(&left_header.to_bytes());
        for (i, leaf) in left_leaves.iter().enumerate() {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_LEAF_SIZE;
            left_buf[off..off + EXTENT_LEAF_SIZE].copy_from_slice(&leaf.to_bytes());
        }
        self.write_block(old_leaf_phys, &left_buf)?;

        // Allocate new block for right half
        let new_leaf_phys = self.allocate_block(group)?;
        let mut right_buf = vec![0u8; block_size];
        let right_header = ExtentHeader {
            magic: extent::EXT4_EXTENT_MAGIC,
            entries: right_leaves.len() as u16,
            max: max_entries,
            depth: 0,
            generation: 0,
        };
        right_buf[..EXTENT_HEADER_SIZE].copy_from_slice(&right_header.to_bytes());
        for (i, leaf) in right_leaves.iter().enumerate() {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_LEAF_SIZE;
            right_buf[off..off + EXTENT_LEAF_SIZE].copy_from_slice(&leaf.to_bytes());
        }
        self.write_block(new_leaf_phys, &right_buf)?;

        // Add a new index entry in the root pointing to the new leaf
        let root_header = inode.extent_header().ok_or(Ext4Error::Corrupt("no extent header"))?;
        if root_header.entries >= root_header.max {
            log::error!("[ext4::extent] root index node is also full, cannot add new index entry");
            return Err(Ext4Error::UnsupportedFeature("root index node overflow (3+ level tree)"));
        }

        // First logical block of the right half
        let right_first_logical = right_leaves[0].block;

        let new_idx = ExtentIndex {
            block: right_first_logical,
            leaf_lo: new_leaf_phys as u32,
            leaf_hi: (new_leaf_phys >> 32) as u16,
            padding: 0,
        };

        // Append the new index entry
        let entry_off = EXTENT_HEADER_SIZE + root_header.entries as usize * EXTENT_INDEX_SIZE;
        inode.i_block[entry_off..entry_off + EXTENT_INDEX_SIZE].copy_from_slice(&new_idx.to_bytes());

        // Update root entries count
        let new_count = root_header.entries + 1;
        inode.i_block[2] = new_count as u8;
        inode.i_block[3] = (new_count >> 8) as u8;

        self.write_inode(ino, inode)?;

        log::info!(
            "[ext4::extent] split leaf: left={} extents at block {}, right={} extents at block {}",
            left_leaves.len(),
            old_leaf_phys,
            right_leaves.len(),
            new_leaf_phys
        );
        Ok(())
    }

    /// Write a file at the given absolute path.
    ///
    /// If the file exists, its contents are replaced. If it does not exist, it is created.
    /// Parent directories must already exist.
    pub fn write_file(&mut self, path: &[u8], data: &[u8]) -> Result<(), Ext4Error> {
        log::info!("[ext4::write_file] writing {} bytes to {:?}",
            data.len(), core::str::from_utf8(path).unwrap_or("<invalid>"));

        // Split into parent path and filename
        let (parent_path, filename) = split_path(path)?;

        // Resolve parent directory
        let (parent_ino, mut parent_inode) = self.lookup_path(parent_path)?;
        if !parent_inode.is_dir() {
            return Err(Ext4Error::NotADirectory);
        }

        // Check if file already exists
        let existing = self.lookup_in_dir(&parent_inode, filename);
        let (file_ino, mut file_inode) = match existing {
            Ok((ino, inode)) => {
                if inode.is_dir() {
                    return Err(Ext4Error::IsADirectory);
                }
                // Check encryption
                encrypt::check_encryption(inode.flags)?;
                log::debug!("[ext4::write_file] overwriting existing file at inode {}", ino);
                // TODO: free old blocks before rewriting
                (ino, inode)
            }
            Err(Ext4Error::NotFound) => {
                // Create new inode
                let group = ((parent_ino - 1) / self.sb.inodes_per_group) as usize;
                let new_ino = self.allocate_inode(group)?;
                let new_inode = Inode::new_file(0o644, 0, 0, 0);
                self.write_inode(new_ino, &new_inode)?;

                // Add directory entry
                let entry = DirEntry::new(new_ino, filename, FT_REG_FILE, 0);
                self.add_dir_entry(parent_ino, &mut parent_inode, &entry)?;

                log::info!("[ext4::write_file] created new file inode {}", new_ino);
                (new_ino, new_inode)
            }
            Err(e) => return Err(e),
        };

        // Allocate blocks and write data
        let block_size = self.sb.block_size() as usize;
        let blocks_needed = (data.len() + block_size - 1) / block_size;
        let group = ((file_ino - 1) / self.sb.inodes_per_group) as usize;

        // Reset the inode's extent tree for fresh write
        file_inode.flags |= inode::EXT4_EXTENTS_FL;
        file_inode.i_block = [0u8; inode::I_BLOCK_SIZE];
        let header = ExtentHeader {
            magic: extent::EXT4_EXTENT_MAGIC,
            entries: 0,
            max: extent::ROOT_MAX_ENTRIES,
            depth: 0,
            generation: 0,
        };
        let hdr_bytes = header.to_bytes();
        file_inode.i_block[..EXTENT_HEADER_SIZE].copy_from_slice(&hdr_bytes);

        log::debug!("[ext4::write_file] writing {} blocks of data", blocks_needed);

        for i in 0..blocks_needed {
            let phys_block = self.allocate_block(group)?;
            let start = i * block_size;
            let end = core::cmp::min(start + block_size, data.len());

            let mut block_buf = vec![0u8; block_size];
            block_buf[..end - start].copy_from_slice(&data[start..end]);
            self.write_block(phys_block, &block_buf)?;

            self.append_extent(file_ino, &mut file_inode, phys_block)?;
        }

        // Update file size and write inode
        file_inode.set_size(data.len() as u64);
        file_inode.blocks_lo = (blocks_needed as u32) * (self.sb.block_size() as u32 / 512);
        self.write_inode(file_ino, &file_inode)?;

        // Flush to ensure durability
        self.device.flush()?;

        log::info!("[ext4::write_file] wrote {} bytes to inode {} ({} blocks)",
            data.len(), file_ino, blocks_needed);
        Ok(())
    }

    /// Create a new directory at the given absolute path.
    ///
    /// Parent directories must already exist. The new directory is created with
    /// "." and ".." entries.
    pub fn mkdir(&mut self, path: &[u8]) -> Result<u32, Ext4Error> {
        log::info!("[ext4::mkdir] creating directory {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let (parent_path, dirname) = split_path(path)?;

        // Resolve parent
        let (parent_ino, mut parent_inode) = self.lookup_path(parent_path)?;
        if !parent_inode.is_dir() {
            return Err(Ext4Error::NotADirectory);
        }

        // Check for existing entry
        if self.lookup_in_dir(&parent_inode, dirname).is_ok() {
            return Err(Ext4Error::AlreadyExists);
        }

        // Allocate inode
        let group = ((parent_ino - 1) / self.sb.inodes_per_group) as usize;
        let new_ino = self.allocate_inode(group)?;

        // Create directory inode
        let mut new_inode = Inode::new_dir(0o755, 0, 0, 0);

        // Allocate a block for . and .. entries
        let data_block = self.allocate_block(group)?;
        let dot_data = dir::create_dot_entries(new_ino, parent_ino, self.sb.block_size() as u32);
        self.write_block(data_block, &dot_data)?;

        // Set up the extent tree with one extent pointing to the dot-entries block
        self.append_extent(new_ino, &mut new_inode, data_block)?;
        new_inode.set_size(self.sb.block_size());
        new_inode.blocks_lo = (self.sb.block_size() / 512) as u32;
        self.write_inode(new_ino, &new_inode)?;

        // Add entry in parent directory
        let entry = DirEntry::new(new_ino, dirname, FT_DIR, 0);
        self.add_dir_entry(parent_ino, &mut parent_inode, &entry)?;

        // Increment parent link count (for the ".." entry pointing back)
        parent_inode.links_count = parent_inode.links_count.saturating_add(1);
        self.write_inode(parent_ino, &parent_inode)?;

        // Update block group used_dirs count
        if group < self.groups.len() {
            let old = self.groups[group].used_dirs_count();
            self.groups[group].used_dirs_count_lo = (old + 1) as u16;
            self.groups[group].used_dirs_count_hi = ((old + 1) >> 16) as u16;
        }

        self.device.flush()?;

        log::info!("[ext4::mkdir] created directory inode {} in parent {}", new_ino, parent_ino);
        Ok(new_ino)
    }

    /// Write the superblock and block group descriptors back to disk.
    ///
    /// Call this after any metadata-modifying operation to persist the state.
    pub fn sync_metadata(&mut self) -> Result<(), Ext4Error> {
        log::info!("[ext4::sync] writing superblock and block group descriptors to disk");

        // Write superblock (with checksum if enabled)
        let mut sb_bytes = self.sb.to_bytes();
        if self.has_metadata_csum {
            crc32c::compute_superblock_checksum(&mut sb_bytes);
        }
        self.device.write_bytes(SUPERBLOCK_OFFSET, &sb_bytes)?;

        // Write block group descriptor table
        let desc_size = self.sb.group_desc_size() as usize;
        let gdt_block = if self.sb.block_size() == 1024 { 2 } else { 1 };
        let gdt_offset = gdt_block as u64 * self.sb.block_size();

        for (i, group) in self.groups.iter().enumerate() {
            let offset = gdt_offset + (i * desc_size) as u64;
            let mut bytes = group.to_bytes(desc_size);

            // Compute and set bgd checksum if metadata checksums are enabled
            if self.has_metadata_csum {
                let csum = crc32c::compute_bgd_checksum(self.csum_seed, i as u32, &bytes);
                // Write checksum at offset 0x1E
                if bytes.len() >= 0x20 {
                    bytes[0x1E] = csum as u8;
                    bytes[0x1F] = (csum >> 8) as u8;
                }
            }

            self.device.write_bytes(offset, &bytes)?;
        }

        self.device.flush()?;
        log::info!("[ext4::sync] metadata sync complete");
        Ok(())
    }
}

/// Find the index entry position for inserting a new extent with the given logical block.
fn find_index_for_insert(indices: &[ExtentIndex], logical_block: u32) -> usize {
    let mut lo = 0usize;
    let mut hi = indices.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if indices[mid].block <= logical_block {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 { 0 } else { lo - 1 }
}

/// Split an absolute path into (parent_directory, filename).
///
/// e.g., b"/foo/bar/baz.txt" -> (b"/foo/bar", b"baz.txt")
/// e.g., b"/file.txt" -> (b"/", b"file.txt")
fn split_path(path: &[u8]) -> Result<(&[u8], &[u8]), Ext4Error> {
    if path.is_empty() || path[0] != b'/' {
        return Err(Ext4Error::InvalidPath);
    }

    // Find last '/'
    let last_slash = path.iter().rposition(|&b| b == b'/').ok_or(Ext4Error::InvalidPath)?;
    let filename = &path[last_slash + 1..];
    if filename.is_empty() {
        return Err(Ext4Error::InvalidPath);
    }
    if filename.len() > dir::EXT4_NAME_LEN {
        return Err(Ext4Error::NameTooLong);
    }

    let parent = if last_slash == 0 { &path[..1] } else { &path[..last_slash] };

    log::trace!("[ext4::path] split: parent={:?}, name={:?}",
        core::str::from_utf8(parent).unwrap_or("<invalid>"),
        core::str::from_utf8(filename).unwrap_or("<invalid>"));

    Ok((parent, filename))
}
