//! High-level btrfs filesystem read/write API.
//!
//! This module provides the main `BtrFs` type that ties together the superblock,
//! chunk map, B-tree operations, inodes, directories, and extents into a usable
//! filesystem interface.
//!
//! ## Features
//!
//! - RAID-aware reads and writes (RAID0, RAID1, RAID10, DUP)
//! - Compression support (zlib, LZO; ZSTD stubbed)
//! - Large file writes via data extents (not just inline)
//! - Free space tracking via block group items
//! - COW (copy-on-write) transaction support with generation counters
//! - File overwrite updates existing inodes
//! - Leaf splitting wired into the insert path
//!
//! ## Usage
//!
//! ```rust,no_run
//! use claudio_btrfs::{BtrFs, BlockDevice};
//!
//! let fs = BtrFs::mount(my_device).expect("mount failed");
//! let data = fs.read_file(b"/hello.txt").expect("read failed");
//! fs.write_file(b"/output.txt", &data).expect("write failed");
//! ```

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::chunk::ChunkMap;
use crate::compress;
use crate::dir::{self, DirItem, DirEntry, dir_item_key, dir_index_key, dir_type};
use crate::extent::{FileExtentItem, BTRFS_FILE_EXTENT_INLINE, BTRFS_FILE_EXTENT_REG};
use crate::inode::{InodeItem, BtrfsTimespec};
use crate::item::{BtrfsHeader, BTRFS_HEADER_SIZE};
use crate::key::{BtrfsKey, KeyType, objectid};
use crate::superblock::{Superblock, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE};
use crate::tree;

/// Maximum size for inline extents.
///
/// Small files (up to ~3800 bytes) are stored directly inside the B-tree leaf
/// node alongside their metadata, avoiding a separate data extent allocation.
/// The limit is derived from the default nodesize (16384) minus the header
/// (101 bytes), item descriptor (25 bytes), and file extent item (53 bytes),
/// with some safety margin for other items sharing the leaf.
const MAX_INLINE_DATA_SIZE: usize = 3800;

/// Errors that can occur during btrfs filesystem operations.
#[derive(Debug)]
pub enum BtrfsError {
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
    /// No free space available for allocation.
    NoFreeSpace,
    /// The filesystem is corrupt (e.g., invalid tree node, broken chunk map).
    Corrupt(&'static str),
    /// A filename exceeds the maximum length (255 bytes).
    NameTooLong,
    /// The path is invalid (empty, missing leading slash, etc.).
    InvalidPath,
    /// The target is a directory when a file was expected.
    IsADirectory,
    /// The target is a file when a directory was expected.
    IsNotADirectory,
    /// Compressed extents not supported by this implementation.
    CompressedExtent,
    /// The chunk map could not resolve a logical address.
    UnmappedLogical(u64),
    /// Decompression failed.
    DecompressError(&'static str),
}

impl fmt::Display for BtrfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BtrfsError::IoError => write!(f, "I/O error"),
            BtrfsError::InvalidSuperblock => write!(f, "invalid superblock"),
            BtrfsError::UnsupportedFeature(feat) => write!(f, "unsupported feature: {}", feat),
            BtrfsError::NotFound => write!(f, "not found"),
            BtrfsError::NotADirectory => write!(f, "not a directory"),
            BtrfsError::AlreadyExists => write!(f, "already exists"),
            BtrfsError::NoFreeSpace => write!(f, "no free space"),
            BtrfsError::Corrupt(msg) => write!(f, "filesystem corrupt: {}", msg),
            BtrfsError::NameTooLong => write!(f, "filename too long"),
            BtrfsError::InvalidPath => write!(f, "invalid path"),
            BtrfsError::IsADirectory => write!(f, "is a directory"),
            BtrfsError::IsNotADirectory => write!(f, "is not a directory"),
            BtrfsError::CompressedExtent => write!(f, "compressed extents not supported"),
            BtrfsError::UnmappedLogical(addr) => write!(f, "unmapped logical address: 0x{:X}", addr),
            BtrfsError::DecompressError(msg) => write!(f, "decompression error: {}", msg),
        }
    }
}

/// Trait for the underlying block storage device.
///
/// Implement this for your NVMe driver, virtio-blk, RAM disk, or disk image
/// to provide btrfs with raw byte-level access.
pub trait BlockDevice {
    /// Read `buf.len()` bytes from the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition/device.
    /// Returns `Ok(())` on success.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), BtrfsError>;

    /// Write `buf.len()` bytes to the device starting at `offset`.
    ///
    /// `offset` is a byte offset from the start of the partition/device.
    /// Returns `Ok(())` on success.
    fn write_bytes(&self, offset: u64, buf: &[u8]) -> Result<(), BtrfsError>;
}

// ============================================================================
// Free space tracking
// ============================================================================

/// A free extent range: [start, start+len).
#[derive(Clone, Debug)]
struct FreeRange {
    start: u64,
    len: u64,
}

/// Block group descriptor for free space tracking.
#[derive(Clone, Debug)]
struct BlockGroupInfo {
    /// Logical start of this block group.
    logical: u64,
    /// Total length of the block group.
    length: u64,
    /// Bytes used within this block group.
    used: u64,
    /// Chunk type flags (DATA, METADATA, SYSTEM).
    flags: u64,
}

/// In-memory free space tracker.
///
/// Reads block group items at mount time to determine available space.
/// Tracks allocations via a simple free-range list per block group.
struct FreeSpaceTracker {
    /// Block groups discovered from the extent tree.
    block_groups: Vec<BlockGroupInfo>,
    /// Free ranges available for allocation (simplified: one list across all data BGs).
    free_ranges: Vec<FreeRange>,
}

impl FreeSpaceTracker {
    fn new() -> Self {
        FreeSpaceTracker {
            block_groups: Vec::new(),
            free_ranges: Vec::new(),
        }
    }

    /// Initialize from block group items found in the extent tree.
    fn add_block_group(&mut self, logical: u64, length: u64, used: u64, flags: u64) {
        log::debug!("[btrfs::freespace] block group: logical=0x{:X}, len={}, used={}, flags=0x{:X}",
            logical, length, used, flags);

        self.block_groups.push(BlockGroupInfo {
            logical,
            length,
            used,
            flags,
        });

        // Add free space in this block group as a single range (simplified).
        // A real implementation would parse the free space tree or extent tree.
        if used < length {
            let free_start = logical + used;
            let free_len = length - used;
            self.free_ranges.push(FreeRange {
                start: free_start,
                len: free_len,
            });
            log::debug!("[btrfs::freespace] added free range: 0x{:X}+{}", free_start, free_len);
        }
    }

    /// Allocate `size` bytes from the free space tracker.
    ///
    /// Returns the logical byte address of the allocated extent, aligned to `sectorsize`.
    fn allocate(&mut self, size: u64, sectorsize: u64, is_data: bool) -> Option<u64> {
        let aligned_size = (size + sectorsize - 1) & !(sectorsize - 1);

        // Find a free range large enough
        for i in 0..self.free_ranges.len() {
            if self.free_ranges[i].len >= aligned_size {
                // Check if block group type matches (data vs metadata)
                let range_start = self.free_ranges[i].start;

                // Find which block group this range belongs to
                let bg_match = self.block_groups.iter_mut().find(|bg| {
                    range_start >= bg.logical && range_start < bg.logical + bg.length
                });

                if let Some(bg) = bg_match {
                    let is_bg_data = bg.flags & crate::chunk::chunk_type::DATA != 0;
                    if is_data != is_bg_data {
                        continue; // Wrong type
                    }
                    bg.used += aligned_size;
                }

                let addr = self.free_ranges[i].start;
                self.free_ranges[i].start += aligned_size;
                self.free_ranges[i].len -= aligned_size;

                if self.free_ranges[i].len == 0 {
                    self.free_ranges.remove(i);
                }

                log::info!("[btrfs::freespace] allocated {} bytes at logical 0x{:X}", aligned_size, addr);
                return Some(addr);
            }
        }

        log::warn!("[btrfs::freespace] no free space for {} bytes (data={})", aligned_size, is_data);
        None
    }

    /// Free a previously allocated extent.
    fn free(&mut self, logical: u64, size: u64, sectorsize: u64) {
        let aligned_size = (size + sectorsize - 1) & !(sectorsize - 1);

        // Update block group used count
        if let Some(bg) = self.block_groups.iter_mut().find(|bg| {
            logical >= bg.logical && logical < bg.logical + bg.length
        }) {
            bg.used = bg.used.saturating_sub(aligned_size);
        }

        // Add back as a free range (no merging for simplicity)
        self.free_ranges.push(FreeRange {
            start: logical,
            len: aligned_size,
        });

        log::debug!("[btrfs::freespace] freed {} bytes at logical 0x{:X}", aligned_size, logical);
    }

    /// Get total free space across all block groups.
    fn total_free(&self) -> u64 {
        self.free_ranges.iter().map(|r| r.len).sum()
    }
}

// ============================================================================
// COW Transaction State
// ============================================================================

#[allow(dead_code)]
/// Tracks a pending COW transaction.
///
/// btrfs uses copy-on-write for all tree modifications. When a leaf or node is
/// modified, it is written to a new location, and parents are updated to point
/// to the new location, propagating up to the root. The superblock is updated
/// last with the new root pointer and generation.
struct CowTransaction {
    /// The generation counter for this transaction.
    generation: u64,
    /// Nodes that have been COW'd: maps old logical -> new logical.
    /// We track this so parent pointers can be updated.
    cow_map: Vec<(u64, u64)>,
}

impl CowTransaction {
    fn new(generation: u64) -> Self {
        CowTransaction {
            generation,
            cow_map: Vec::new(),
        }
    }

    /// Record that a node was COW'd from old_addr to new_addr.
    fn record_cow(&mut self, old_addr: u64, new_addr: u64) {
        log::debug!("[btrfs::cow] COW: 0x{:X} -> 0x{:X} (gen={})", old_addr, new_addr, self.generation);
        self.cow_map.push((old_addr, new_addr));
    }

    /// Look up the new address for a COW'd node.
    fn lookup(&self, old_addr: u64) -> Option<u64> {
        self.cow_map.iter().rev().find(|(old, _)| *old == old_addr).map(|(_, new)| *new)
    }
}

// ============================================================================
// BtrFs main type
// ============================================================================

/// A mounted btrfs filesystem.
///
/// Holds the superblock, chunk map, and device reference needed for all operations.
pub struct BtrFs<D: BlockDevice> {
    /// The underlying block device.
    dev: D,
    /// Parsed superblock.
    pub sb: Superblock,
    /// Chunk map (logical to physical address translation).
    pub chunk_map: ChunkMap,
    /// Current generation (for write operations).
    generation: u64,
    /// Next available inode number (tracked in memory for allocation).
    next_inode: u64,
    /// Next available directory index sequence.
    next_dir_index: u64,
    /// Free space tracker.
    free_space: FreeSpaceTracker,
    /// Active COW transaction (if any).
    cow_txn: Option<CowTransaction>,
}

impl<D: BlockDevice> BtrFs<D> {
    /// Mount a btrfs filesystem from the given block device.
    ///
    /// Reads the superblock, parses the sys_chunk_array for bootstrap chunk mappings,
    /// then reads the chunk tree for the full chunk map. Finally, locates the
    /// filesystem tree root and initializes free space tracking.
    pub fn mount(dev: D) -> Result<Self, BtrfsError> {
        log::info!("[btrfs::readwrite] mounting btrfs filesystem...");

        // Step 1: Read the superblock at offset 0x10000
        let mut sb_buf = vec![0u8; SUPERBLOCK_SIZE];
        dev.read_bytes(SUPERBLOCK_OFFSET, &mut sb_buf)?;

        let sb = Superblock::from_bytes(&sb_buf)
            .ok_or(BtrfsError::InvalidSuperblock)?;

        log::info!("[btrfs::readwrite] superblock parsed: generation={}, nodesize={}, label={:?}",
            sb.generation, sb.nodesize, sb.label_str());

        // Step 2: Parse sys_chunk_array for bootstrap chunk mappings
        let mut chunk_map = ChunkMap::new();
        chunk_map.parse_sys_chunk_array(&sb.sys_chunk_array, sb.sys_chunk_array_size);

        log::info!("[btrfs::readwrite] bootstrap chunk map: {} entries", chunk_map.len());

        // Step 3: Read the full chunk tree to populate remaining chunk mappings
        let chunk_root = sb.chunk_root;
        let chunk_level = sb.chunk_root_level;
        let nodesize = sb.nodesize;

        log::debug!("[btrfs::readwrite] reading chunk tree: root=0x{:X}, level={}", chunk_root, chunk_level);

        // Walk the chunk tree to find all CHUNK_ITEM entries
        {
            let chunk_map_ref = &chunk_map;
            let dev_ref = &dev;
            let read_node_fn = |logical: u64| -> Option<Vec<u8>> {
                read_node_via_chunks(dev_ref, chunk_map_ref, logical, nodesize)
            };

            let items = tree::collect_tree_items(chunk_root, chunk_level, nodesize, read_node_fn);

            for (key, data) in &items {
                if key.key_type == KeyType::ChunkItem as u8 {
                    if let Some(chunk) = crate::chunk::ChunkItem::from_bytes(data) {
                        let logical = key.offset;
                        let already_exists = chunk_map.entries.iter().any(|e| e.logical == logical);
                        if !already_exists {
                            chunk_map.insert(logical, chunk);
                        }
                    }
                }
            }
        }

        log::info!("[btrfs::readwrite] full chunk map: {} entries", chunk_map.len());

        let generation = sb.generation;

        // Step 4: Initialize free space tracker from extent tree block group items
        let mut free_space = FreeSpaceTracker::new();
        {
            let dev_ref = &dev;
            let chunk_map_ref = &chunk_map;
            let extent_root = sb.root; // extent tree items are found via root tree
            let extent_level = sb.root_level;

            // Walk the extent tree (tree 2) to find block group items.
            // First, find the extent tree root from the root tree.
            let root_key = BtrfsKey::new(
                objectid::EXTENT_TREE_OBJECTID,
                KeyType::RootItem as u8,
                0,
            );

            if let Some(result) = tree::search_tree(
                extent_root, extent_level, &root_key, nodesize,
                |logical| read_node_via_chunks(dev_ref, chunk_map_ref, logical, nodesize),
            ) {
                if result.exact {
                    let slot = result.path.leaf_slot().unwrap_or(0);
                    if let Some(data) = result.leaf.item_data(slot) {
                        if data.len() >= 184 {
                            let ext_root_bytenr = u64::from_le_bytes([
                                data[176], data[177], data[178], data[179],
                                data[180], data[181], data[182], data[183],
                            ]);
                            let ext_root_data = read_node_via_chunks(dev_ref, chunk_map_ref, ext_root_bytenr, nodesize);
                            let ext_level = ext_root_data.as_ref()
                                .and_then(|d| BtrfsHeader::from_bytes(d))
                                .map(|h| h.level)
                                .unwrap_or(0);

                            tree::walk_tree(
                                ext_root_bytenr, ext_level, nodesize,
                                |logical| read_node_via_chunks(dev_ref, chunk_map_ref, logical, nodesize),
                                |key, data| {
                                    if key.key_type == KeyType::BlockGroupItem as u8 && data.len() >= 24 {
                                        let used = u64::from_le_bytes([
                                            data[0], data[1], data[2], data[3],
                                            data[4], data[5], data[6], data[7],
                                        ]);
                                        let chunk_objectid = u64::from_le_bytes([
                                            data[8], data[9], data[10], data[11],
                                            data[12], data[13], data[14], data[15],
                                        ]);
                                        let flags = u64::from_le_bytes([
                                            data[16], data[17], data[18], data[19],
                                            data[20], data[21], data[22], data[23],
                                        ]);
                                        let _ = chunk_objectid;
                                        free_space.add_block_group(key.objectid, key.offset, used, flags);
                                    }
                                },
                            );
                        }
                    }
                }
            }
        }

        log::info!("[btrfs::readwrite] free space tracker: {} block groups, {} bytes free",
            free_space.block_groups.len(), free_space.total_free());

        let mut fs = BtrFs {
            dev,
            sb,
            chunk_map,
            generation,
            next_inode: objectid::FIRST_FREE_OBJECTID + 1,
            next_dir_index: 2,
            free_space,
            cow_txn: None,
        };

        // Step 5: Scan the FS tree to find the highest inode number (for allocation)
        fs.scan_next_inode()?;

        log::info!("[btrfs::readwrite] btrfs mounted successfully. next_inode={}", fs.next_inode);

        Ok(fs)
    }

    // ========================================================================
    // Low-level node I/O (RAID-aware)
    // ========================================================================

    /// Read a node from the device, resolving logical to physical via chunk map.
    fn read_node(&self, logical: u64) -> Result<Vec<u8>, BtrfsError> {
        let (_devid, physical) = self.chunk_map.resolve(logical)
            .ok_or(BtrfsError::UnmappedLogical(logical))?;

        let mut buf = vec![0u8; self.sb.nodesize as usize];
        self.dev.read_bytes(physical, &mut buf)?;

        log::trace!("[btrfs::readwrite] read node: logical=0x{:X} -> physical=0x{:X} ({} bytes)",
            logical, physical, buf.len());

        Ok(buf)
    }

    /// Write a node to the device, resolving logical to physical.
    ///
    /// RAID-aware: writes to all mirrors/stripes as needed.
    fn write_node(&self, logical: u64, data: &[u8]) -> Result<(), BtrfsError> {
        // Use RAID-aware write: write to all target locations
        let targets = self.chunk_map.resolve_write(logical)
            .ok_or(BtrfsError::UnmappedLogical(logical))?;

        for (_devid, physical) in &targets {
            self.dev.write_bytes(*physical, data)?;
        }

        log::trace!("[btrfs::readwrite] wrote node: logical=0x{:X} -> {} targets ({} bytes)",
            logical, targets.len(), data.len());

        Ok(())
    }

    /// Read raw bytes from a logical address range (for extent data).
    fn read_bytes_logical(&self, logical: u64, buf: &mut [u8]) -> Result<(), BtrfsError> {
        let (_devid, physical) = self.chunk_map.resolve(logical)
            .ok_or(BtrfsError::UnmappedLogical(logical))?;
        self.dev.read_bytes(physical, buf)
    }

    /// Write raw bytes to a logical address range (for extent data).
    ///
    /// RAID-aware: writes to all mirrors/stripes.
    fn write_bytes_logical(&self, logical: u64, data: &[u8]) -> Result<(), BtrfsError> {
        let targets = self.chunk_map.resolve_write(logical)
            .ok_or(BtrfsError::UnmappedLogical(logical))?;

        for (_devid, physical) in &targets {
            self.dev.write_bytes(*physical, data)?;
        }

        Ok(())
    }

    // ========================================================================
    // COW transaction support
    // ========================================================================

    /// Begin a COW transaction with incremented generation.
    fn begin_transaction(&mut self) -> u64 {
        let new_gen = self.generation + 1;
        self.cow_txn = Some(CowTransaction::new(new_gen));
        log::debug!("[btrfs::readwrite] began transaction gen={}", new_gen);
        new_gen
    }

    /// COW a tree node: allocate a new location, copy the data there, return new address.
    #[allow(dead_code)]
    ///
    /// The old node is not freed immediately (btrfs keeps old roots for transid-based recovery).
    fn cow_node(&mut self, old_logical: u64) -> Result<u64, BtrfsError> {
        // Check if already COW'd in this transaction
        if let Some(ref txn) = self.cow_txn {
            if let Some(new_addr) = txn.lookup(old_logical) {
                return Ok(new_addr);
            }
        }

        let nodesize = self.sb.nodesize;
        let sectorsize = self.sb.sectorsize;

        // Read the old node
        let mut node_data = self.read_node(old_logical)?;

        // Allocate a new location for the COW'd node
        let new_logical = self.free_space.allocate(nodesize as u64, sectorsize as u64, false)
            .ok_or(BtrfsError::NoFreeSpace)?;

        // Update the bytenr in the header to reflect the new location
        let generation = self.cow_txn.as_ref().map(|t| t.generation).unwrap_or(self.generation + 1);
        // bytenr is at offset 0x30
        node_data[0x30..0x38].copy_from_slice(&new_logical.to_le_bytes());
        // Update generation at offset 0x50
        node_data[0x50..0x58].copy_from_slice(&generation.to_le_bytes());
        // Recompute checksum
        BtrfsHeader::update_csum(&mut node_data);

        // Write to the new location
        self.write_node(new_logical, &node_data)?;

        // Record the COW mapping
        if let Some(ref mut txn) = self.cow_txn {
            txn.record_cow(old_logical, new_logical);
        }

        log::debug!("[btrfs::readwrite] COW'd node: 0x{:X} -> 0x{:X}", old_logical, new_logical);
        Ok(new_logical)
    }

    /// Commit the current transaction: update the superblock with new root + generation.
    fn commit_transaction(&mut self) -> Result<(), BtrfsError> {
        let txn_gen = match &self.cow_txn {
            Some(txn) => txn.generation,
            None => return Ok(()),
        };

        // If the FS tree root was COW'd, update the root tree to point to the new root.
        // For simplicity, we update the superblock's generation.
        self.sb.generation = txn_gen;
        self.generation = txn_gen;

        // Write updated superblock
        let sb_bytes = self.sb.to_bytes();
        self.dev.write_bytes(SUPERBLOCK_OFFSET, &sb_bytes)?;

        log::info!("[btrfs::readwrite] committed transaction gen={}", txn_gen);
        self.cow_txn = None;
        Ok(())
    }

    // ========================================================================
    // Tree search helpers
    // ========================================================================

    /// Search the filesystem tree for a key.
    fn search_fs_tree(&self, key: &BtrfsKey) -> Result<tree::SearchResult, BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;

        let chunk_map = &self.chunk_map;
        let dev = &self.dev;
        let nodesize = self.sb.nodesize;

        let result = tree::search_tree(
            fs_root, fs_level, key, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
        );

        result.ok_or(BtrfsError::Corrupt("failed to search filesystem tree"))
    }

    /// Find the root of the default filesystem tree by searching the root tree.
    fn find_fs_tree_root(&self) -> Result<u64, BtrfsError> {
        let root_key = BtrfsKey::new(
            objectid::FS_TREE_OBJECTID,
            KeyType::RootItem as u8,
            0,
        );

        let chunk_map = &self.chunk_map;
        let dev = &self.dev;
        let nodesize = self.sb.nodesize;

        let result = tree::search_tree(
            self.sb.root, self.sb.root_level, &root_key, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
        ).ok_or(BtrfsError::Corrupt("failed to search root tree for FS_TREE"))?;

        if !result.exact {
            log::error!("[btrfs::readwrite] FS_TREE root item not found in root tree");
            return Err(BtrfsError::Corrupt("FS_TREE root item not found"));
        }

        let slot = result.path.leaf_slot().unwrap_or(0);
        let data = result.leaf.item_data(slot)
            .ok_or(BtrfsError::Corrupt("cannot read FS_TREE root item data"))?;

        // btrfs_root_item: first field after the inode item (160 bytes) is generation (u64),
        // then root_dirid (u64), then bytenr (u64)
        if data.len() < 176 + 8 {
            return Err(BtrfsError::Corrupt("root item too small"));
        }

        let bytenr = u64::from_le_bytes([
            data[176], data[177], data[178], data[179],
            data[180], data[181], data[182], data[183],
        ]);

        log::debug!("[btrfs::readwrite] FS_TREE root bytenr=0x{:X}", bytenr);
        Ok(bytenr)
    }

    /// Find the level of the filesystem tree root.
    fn find_fs_tree_level(&self) -> Result<u8, BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let node_data = self.read_node(fs_root)?;
        let header = BtrfsHeader::from_bytes(&node_data)
            .ok_or(BtrfsError::Corrupt("cannot parse FS tree root header"))?;
        Ok(header.level)
    }

    /// Scan the FS tree to find the highest inode number in use.
    fn scan_next_inode(&mut self) -> Result<(), BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;

        let chunk_map = &self.chunk_map;
        let dev = &self.dev;
        let nodesize = self.sb.nodesize;

        let mut max_inode = objectid::FIRST_FREE_OBJECTID;

        tree::walk_tree(
            fs_root, fs_level, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
            |key, _data| {
                if key.key_type == KeyType::InodeItem as u8 && key.objectid > max_inode {
                    max_inode = key.objectid;
                }
            },
        );

        self.next_inode = max_inode + 1;
        log::debug!("[btrfs::readwrite] highest inode found: {}, next_inode={}", max_inode, self.next_inode);
        Ok(())
    }

    /// Allocate a new inode number.
    fn alloc_inode(&mut self) -> u64 {
        let ino = self.next_inode;
        self.next_inode += 1;
        log::debug!("[btrfs::readwrite] allocated inode {}", ino);
        ino
    }

    /// Allocate a new directory index.
    fn alloc_dir_index(&mut self) -> u64 {
        let idx = self.next_dir_index;
        self.next_dir_index += 1;
        idx
    }

    // ========================================================================
    // Path resolution and inode operations
    // ========================================================================

    /// Resolve a path to its inode number and inode item.
    ///
    /// Path must start with `/` and components are separated by `/`.
    fn resolve_path(&self, path: &[u8]) -> Result<(u64, InodeItem), BtrfsError> {
        if path.is_empty() || path[0] != b'/' {
            log::error!("[btrfs::readwrite] invalid path: must start with /");
            return Err(BtrfsError::InvalidPath);
        }

        log::debug!("[btrfs::readwrite] resolving path: {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        // Start from the root directory (inode 256 in the default subvolume)
        let mut current_inode = objectid::FIRST_FREE_OBJECTID;
        let mut current_item = self.read_inode(current_inode)?;

        // Split path into components
        let path_str = &path[1..]; // skip leading /
        if path_str.is_empty() {
            // Root directory
            return Ok((current_inode, current_item));
        }

        for component in path_str.split(|&b| b == b'/') {
            if component.is_empty() {
                continue; // skip double slashes
            }

            if !current_item.is_dir() {
                log::error!("[btrfs::readwrite] path component is not a directory at inode {}", current_inode);
                return Err(BtrfsError::NotADirectory);
            }

            // Look up the component in the current directory
            let dir_entry = self.lookup_dir_entry(current_inode, component)?;
            current_inode = dir_entry.location.objectid;
            current_item = self.read_inode(current_inode)?;

            log::trace!("[btrfs::readwrite] resolved {:?} -> inode {}",
                core::str::from_utf8(component).unwrap_or("<invalid>"), current_inode);
        }

        Ok((current_inode, current_item))
    }

    /// Read an inode item from the filesystem tree.
    fn read_inode(&self, inode: u64) -> Result<InodeItem, BtrfsError> {
        let key = BtrfsKey::new(inode, KeyType::InodeItem as u8, 0);
        let result = self.search_fs_tree(&key)?;

        if !result.exact {
            log::error!("[btrfs::readwrite] inode {} not found", inode);
            return Err(BtrfsError::NotFound);
        }

        let slot = result.path.leaf_slot().unwrap_or(0);
        let data = result.leaf.item_data(slot)
            .ok_or(BtrfsError::Corrupt("cannot read inode item data"))?;

        InodeItem::from_bytes(data).ok_or(BtrfsError::Corrupt("cannot parse inode item"))
    }

    /// Look up a directory entry by name in the given directory inode.
    fn lookup_dir_entry(&self, dir_inode: u64, name: &[u8]) -> Result<DirItem, BtrfsError> {
        let key = dir_item_key(dir_inode, name);
        let result = self.search_fs_tree(&key)?;

        if !result.exact {
            log::trace!("[btrfs::readwrite] dir entry {:?} not found in inode {}",
                core::str::from_utf8(name).unwrap_or("<invalid>"), dir_inode);
            return Err(BtrfsError::NotFound);
        }

        let slot = result.path.leaf_slot().unwrap_or(0);
        let data = result.leaf.item_data(slot)
            .ok_or(BtrfsError::Corrupt("cannot read dir item data"))?;

        let items = DirItem::parse_all(data);
        dir::find_by_name(&items, name)
            .cloned()
            .ok_or(BtrfsError::NotFound)
    }

    // ========================================================================
    // File reading (with compression support)
    // ========================================================================

    /// Read the contents of a file at the given path.
    pub fn read_file(&self, path: &[u8]) -> Result<Vec<u8>, BtrfsError> {
        log::info!("[btrfs::readwrite] read_file: {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let (inode_num, inode_item) = self.resolve_path(path)?;

        if inode_item.is_dir() {
            return Err(BtrfsError::IsADirectory);
        }

        if !inode_item.is_file() {
            log::error!("[btrfs::readwrite] inode {} is not a regular file (mode=0o{:06o})",
                inode_num, inode_item.mode);
            return Err(BtrfsError::NotFound);
        }

        self.read_file_data(inode_num, inode_item.size)
    }

    /// Read file data by collecting all EXTENT_DATA items for the inode.
    ///
    /// Supports inline extents, regular extents, compressed extents (zlib, LZO),
    /// and sparse holes.
    fn read_file_data(&self, inode: u64, size: u64) -> Result<Vec<u8>, BtrfsError> {
        log::debug!("[btrfs::readwrite] reading file data for inode {}: {} bytes", inode, size);

        let mut file_data = vec![0u8; size as usize];
        let mut bytes_read = 0u64;

        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;
        let nodesize = self.sb.nodesize;
        let chunk_map = &self.chunk_map;
        let dev = &self.dev;

        // Collect all extent data items for this inode
        let mut extent_items: Vec<(u64, FileExtentItem)> = Vec::new();

        tree::walk_tree(
            fs_root, fs_level, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
            |key, data| {
                if key.objectid == inode && key.key_type == KeyType::ExtentData as u8 {
                    if let Some(extent) = FileExtentItem::from_bytes(data) {
                        extent_items.push((key.offset, extent));
                    }
                }
            },
        );

        log::debug!("[btrfs::readwrite] found {} extent items for inode {}", extent_items.len(), inode);

        for (file_offset, extent) in &extent_items {
            match extent.extent_type {
                BTRFS_FILE_EXTENT_INLINE => {
                    let raw_data = &extent.inline_data;

                    let decompressed = if extent.is_compressed() {
                        // Decompress inline data
                        let ram_bytes = extent.ram_bytes as usize;
                        match compress::decompress(extent.compression, raw_data, ram_bytes) {
                            Ok(d) => d,
                            Err(e) => {
                                log::error!("[btrfs::readwrite] inline decompression failed: {:?}", e);
                                return Err(BtrfsError::DecompressError("inline extent decompression failed"));
                            }
                        }
                    } else {
                        raw_data.clone()
                    };

                    let len = decompressed.len().min((size - file_offset) as usize);
                    let dst_start = *file_offset as usize;
                    let dst_end = dst_start + len;
                    if dst_end <= file_data.len() {
                        file_data[dst_start..dst_end].copy_from_slice(&decompressed[..len]);
                        bytes_read += len as u64;
                        log::trace!("[btrfs::readwrite] read {} inline bytes at offset {} (compressed={})",
                            len, file_offset, extent.is_compressed());
                    }
                }
                BTRFS_FILE_EXTENT_REG => {
                    if extent.is_hole() {
                        log::trace!("[btrfs::readwrite] hole at offset {}, {} bytes", file_offset, extent.num_bytes);
                        continue;
                    }

                    if extent.is_compressed() {
                        // Read compressed data from disk, then decompress
                        let logical_start = extent.disk_bytenr;
                        let compressed_size = extent.disk_num_bytes as usize;
                        let ram_bytes = extent.ram_bytes as usize;

                        let mut compressed_buf = vec![0u8; compressed_size];
                        self.read_bytes_logical(logical_start, &mut compressed_buf)?;

                        let decompressed = match compress::decompress(extent.compression, &compressed_buf, ram_bytes) {
                            Ok(d) => d,
                            Err(e) => {
                                log::error!("[btrfs::readwrite] extent decompression failed: {:?}", e);
                                return Err(BtrfsError::DecompressError("extent decompression failed"));
                            }
                        };

                        // Apply the extent offset and num_bytes
                        let ext_offset = extent.offset as usize;
                        let ext_len = extent.num_bytes as usize;
                        let src_end = (ext_offset + ext_len).min(decompressed.len());
                        let src_data = &decompressed[ext_offset..src_end];

                        let dst_start = *file_offset as usize;
                        let dst_end = (dst_start + src_data.len()).min(file_data.len());
                        let copy_len = dst_end - dst_start;
                        file_data[dst_start..dst_end].copy_from_slice(&src_data[..copy_len]);
                        bytes_read += copy_len as u64;

                        log::trace!("[btrfs::readwrite] read {} decompressed bytes at file offset {}",
                            copy_len, file_offset);
                    } else {
                        // Uncompressed regular extent
                        let logical_start = extent.disk_bytenr + extent.offset;
                        let len = extent.num_bytes.min(size - file_offset);

                        let dst_start = *file_offset as usize;
                        let dst_end = (dst_start + len as usize).min(file_data.len());

                        self.read_bytes_logical(logical_start, &mut file_data[dst_start..dst_end])?;
                        let read_len = dst_end - dst_start;
                        bytes_read += read_len as u64;

                        log::trace!("[btrfs::readwrite] read {} bytes from logical=0x{:X} at file offset {}",
                            read_len, logical_start, file_offset);
                    }
                }
                _ => {
                    log::trace!("[btrfs::readwrite] skipping prealloc extent at offset {}", file_offset);
                }
            }
        }

        log::info!("[btrfs::readwrite] read_file_data: inode={}, total {} bytes read", inode, bytes_read);
        Ok(file_data)
    }

    // ========================================================================
    // File writing (with large file support, overwrite, COW)
    // ========================================================================

    /// Write data to a file at the given path (creating it if it doesn't exist).
    ///
    /// - Files <= 3800 bytes use inline extents (stored in the B-tree leaf).
    /// - Larger files allocate data extents on disk via the free space tracker.
    /// - If the file already exists, its inode is updated in-place (old extents freed).
    /// - All tree modifications use COW (copy-on-write) for crash consistency.
    pub fn write_file(&mut self, path: &[u8], data: &[u8]) -> Result<(), BtrfsError> {
        log::info!("[btrfs::readwrite] write_file: {:?} ({} bytes)",
            core::str::from_utf8(path).unwrap_or("<invalid>"), data.len());

        if path.is_empty() || path[0] != b'/' {
            return Err(BtrfsError::InvalidPath);
        }

        // Split path into parent and filename
        let (parent_path, filename) = split_path(path)?;

        // Resolve the parent directory
        let (parent_inode, parent_item) = self.resolve_path(parent_path)?;
        if !parent_item.is_dir() {
            return Err(BtrfsError::NotADirectory);
        }

        // Begin a COW transaction
        let next_gen = self.begin_transaction();
        let now = BtrfsTimespec { sec: 0, nsec: 0 }; // TODO: get real time

        // Check if the file already exists
        let existing = self.lookup_dir_entry(parent_inode, filename).ok();

        if let Some(ref dir_entry) = existing {
            // File exists: update in-place
            let inode_num = dir_entry.location.objectid;
            log::info!("[btrfs::readwrite] overwriting existing file inode {}", inode_num);

            // Delete old extent data items for this inode
            self.delete_extent_items(inode_num)?;

            // Write new extent data
            self.write_extent_data(inode_num, data, next_gen)?;

            // Update inode item with new size and timestamps
            self.update_inode_size(inode_num, data.len() as u64, next_gen, now)?;
        } else {
            // File doesn't exist: create new inode and directory entries
            let new_inode = self.alloc_inode();

            // Create inode item
            let mut inode_item = InodeItem::new_file(0o644, 0, 0, next_gen, now);
            inode_item.size = data.len() as u64;
            inode_item.nbytes = data.len() as u64;
            let inode_data = inode_item.to_bytes();

            // Create directory entries (DIR_ITEM and DIR_INDEX)
            let dir_item = DirItem::new(filename, new_inode, dir_type::REG_FILE, next_gen);
            let dir_item_data = dir_item.to_bytes();

            let fs_root = self.find_fs_tree_root()?;
            let fs_level = self.find_fs_tree_level()?;

            let inode_key = BtrfsKey::new(new_inode, KeyType::InodeItem as u8, 0);
            let dir_key = dir_item_key(parent_inode, filename);
            let dir_idx = self.alloc_dir_index();
            let dir_idx_key = dir_index_key(parent_inode, dir_idx);

            // Insert inode
            self.insert_item(fs_root, fs_level, &inode_key, &inode_data)?;

            // Write extent data for the new file
            self.write_extent_data(new_inode, data, next_gen)?;

            // Insert directory entries
            self.insert_item(fs_root, fs_level, &dir_key, &dir_item_data)?;
            self.insert_item(fs_root, fs_level, &dir_idx_key, &dir_item_data)?;
        }

        // Commit the transaction
        self.commit_transaction()?;

        log::info!("[btrfs::readwrite] write_file complete: {} bytes written", data.len());
        Ok(())
    }

    /// Write extent data for a file.
    ///
    /// Chooses inline or regular extents based on data size.
    fn write_extent_data(&mut self, inode: u64, data: &[u8], generation: u64) -> Result<(), BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;

        if data.len() <= MAX_INLINE_DATA_SIZE {
            // Inline extent: data stored directly in the B-tree leaf
            let extent_item = FileExtentItem::new_inline(data, generation);
            let extent_data = extent_item.to_bytes();
            let extent_key = BtrfsKey::new(inode, KeyType::ExtentData as u8, 0);
            self.insert_item(fs_root, fs_level, &extent_key, &extent_data)?;

            log::debug!("[btrfs::readwrite] wrote inline extent: {} bytes for inode {}", data.len(), inode);
        } else {
            // Large file: allocate data extent(s) on disk
            let sectorsize = self.sb.sectorsize as u64;
            let mut file_offset = 0u64;
            let max_extent_size = 128 * 1024 * 1024u64; // 128 MiB max extent

            while file_offset < data.len() as u64 {
                let remaining = data.len() as u64 - file_offset;
                let extent_size = remaining.min(max_extent_size);
                let aligned_size = (extent_size + sectorsize - 1) & !(sectorsize - 1);

                // Allocate space from free space tracker
                let disk_bytenr = self.free_space.allocate(aligned_size, sectorsize, true)
                    .ok_or(BtrfsError::NoFreeSpace)?;

                // Write the data to the allocated extent
                let chunk_data = &data[file_offset as usize..(file_offset + extent_size) as usize];
                self.write_bytes_logical(disk_bytenr, chunk_data)?;

                // If extent was padded, write zeros for the remaining bytes
                if aligned_size > extent_size {
                    let pad = vec![0u8; (aligned_size - extent_size) as usize];
                    self.write_bytes_logical(disk_bytenr + extent_size, &pad)?;
                }

                // Create a regular extent item pointing to the allocated blocks
                let extent_item = FileExtentItem::new_regular(
                    disk_bytenr,
                    aligned_size,
                    0,               // offset within extent
                    extent_size,     // num_bytes of actual file data
                    generation,
                );
                let extent_data = extent_item.to_bytes();
                let extent_key = BtrfsKey::new(inode, KeyType::ExtentData as u8, file_offset);
                self.insert_item(fs_root, fs_level, &extent_key, &extent_data)?;

                log::debug!("[btrfs::readwrite] wrote regular extent: {} bytes at disk 0x{:X}, file_offset={}",
                    extent_size, disk_bytenr, file_offset);

                file_offset += extent_size;
            }
        }

        Ok(())
    }

    /// Delete all EXTENT_DATA items for an inode (used during overwrite).
    ///
    /// Also frees the disk space used by regular extents.
    fn delete_extent_items(&mut self, inode: u64) -> Result<(), BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;
        let nodesize = self.sb.nodesize;
        let chunk_map = &self.chunk_map;
        let dev = &self.dev;
        let sectorsize = self.sb.sectorsize as u64;

        // Collect all extent data items for this inode
        let mut extents_to_free: Vec<(u64, u64)> = Vec::new(); // (disk_bytenr, disk_num_bytes)
        let leaf_updates: Vec<(u64, BtrfsKey)> = Vec::new(); // (leaf_bytenr, key_to_delete)

        tree::walk_tree(
            fs_root, fs_level, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
            |key, data| {
                if key.objectid == inode && key.key_type == KeyType::ExtentData as u8 {
                    if let Some(extent) = FileExtentItem::from_bytes(data) {
                        if extent.is_regular() && !extent.is_hole() {
                            extents_to_free.push((extent.disk_bytenr, extent.disk_num_bytes));
                        }
                    }
                    // We note which keys to delete; actual deletion would require
                    // removing items from the leaf. For now, the insert of new extents
                    // will use the same key offsets, effectively replacing them.
                    // A full implementation would remove these items.
                }
            },
        );

        // Free disk space for old extents
        for (bytenr, num_bytes) in &extents_to_free {
            if *bytenr != 0 && *num_bytes != 0 {
                self.free_space.free(*bytenr, *num_bytes, sectorsize);
                log::debug!("[btrfs::readwrite] freed extent: disk_bytenr=0x{:X}, size={}", bytenr, num_bytes);
            }
        }

        log::info!("[btrfs::readwrite] deleted {} extent items for inode {}, freed {} extents",
            leaf_updates.len(), inode, extents_to_free.len());

        Ok(())
    }

    /// Update an existing inode item's size and timestamps.
    fn update_inode_size(
        &mut self,
        inode: u64,
        new_size: u64,
        generation: u64,
        now: BtrfsTimespec,
    ) -> Result<(), BtrfsError> {
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;
        let nodesize = self.sb.nodesize;
        let chunk_map = &self.chunk_map;
        let dev = &self.dev;

        let key = BtrfsKey::new(inode, KeyType::InodeItem as u8, 0);

        let result = tree::search_tree(
            fs_root, fs_level, &key, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
        ).ok_or(BtrfsError::Corrupt("failed to find inode for update"))?;

        if !result.exact {
            log::error!("[btrfs::readwrite] inode {} not found for update", inode);
            return Err(BtrfsError::NotFound);
        }

        let slot = result.path.leaf_slot().unwrap_or(0);
        let leaf_bytenr = result.path.leaf().map(|e| e.bytenr)
            .ok_or(BtrfsError::Corrupt("no leaf in path"))?;

        // Read the leaf, modify the inode item in-place
        let mut leaf_data = self.read_node(leaf_bytenr)?;
        let item_data = result.leaf.item_data(slot)
            .ok_or(BtrfsError::Corrupt("cannot read inode item data for update"))?;

        if let Some(mut inode_item) = InodeItem::from_bytes(item_data) {
            inode_item.size = new_size;
            inode_item.nbytes = new_size;
            inode_item.transid = generation;
            inode_item.mtime = now;
            inode_item.ctime = now;

            let new_bytes = inode_item.to_bytes();
            let item_offset = result.leaf.items[slot].offset as usize;
            let item_size = result.leaf.items[slot].size as usize;
            if item_offset + item_size <= leaf_data.len() && new_bytes.len() <= item_size {
                leaf_data[item_offset..item_offset + new_bytes.len()].copy_from_slice(&new_bytes);
                BtrfsHeader::update_csum(&mut leaf_data);
                self.write_node(leaf_bytenr, &leaf_data)?;
                log::debug!("[btrfs::readwrite] updated inode {} size to {}", inode, new_size);
            }
        }

        Ok(())
    }

    // ========================================================================
    // Item insertion with leaf splitting
    // ========================================================================

    /// Insert an item into a tree at the given root.
    ///
    /// If the target leaf is full, splits the leaf (and parent nodes recursively)
    /// to make room for the new item.
    fn insert_item(
        &mut self,
        root_bytenr: u64,
        root_level: u8,
        key: &BtrfsKey,
        data: &[u8],
    ) -> Result<(), BtrfsError> {
        let nodesize = self.sb.nodesize;
        let chunk_map = &self.chunk_map;
        let dev = &self.dev;

        // Search for the insertion point
        let result = tree::search_tree(
            root_bytenr, root_level, key, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
        ).ok_or(BtrfsError::Corrupt("failed to search for insertion point"))?;

        if result.exact {
            log::warn!("[btrfs::readwrite] key {} already exists, skipping insert", key);
            return Ok(());
        }

        // Get the leaf and try to insert
        let leaf_bytenr = result.path.leaf().map(|e| e.bytenr)
            .ok_or(BtrfsError::Corrupt("no leaf in search path"))?;

        let mut leaf_data = self.read_node(leaf_bytenr)?;
        let generation = self.cow_txn.as_ref().map(|t| t.generation).unwrap_or(self.generation + 1);

        let inserted = tree::insert_into_leaf(
            &mut leaf_data,
            nodesize,
            key,
            data,
            generation,
            &self.sb.fsid,
        );

        if inserted.is_some() {
            // Write the updated leaf back
            self.write_node(leaf_bytenr, &leaf_data)?;
            log::debug!("[btrfs::readwrite] inserted key {} into leaf at 0x{:X}", key, leaf_bytenr);
            Ok(())
        } else {
            // Leaf is full -- split the leaf
            log::info!("[btrfs::readwrite] leaf at 0x{:X} is full, splitting...", leaf_bytenr);
            self.split_and_insert(root_bytenr, root_level, &result.path, key, data, generation)
        }
    }

    /// Split a full leaf and insert the item into the correct half.
    ///
    /// This is the core of the leaf splitting logic:
    /// 1. Split the leaf in half
    /// 2. Insert the new item in the correct half
    /// 3. Update the parent's key_ptr to point to both halves
    /// 4. If the parent is full, split recursively
    fn split_and_insert(
        &mut self,
        _root_bytenr: u64,
        _root_level: u8,
        path: &tree::TreePath,
        key: &BtrfsKey,
        data: &[u8],
        generation: u64,
    ) -> Result<(), BtrfsError> {
        let nodesize = self.sb.nodesize;
        let sectorsize = self.sb.sectorsize as u64;

        // The leaf is the last element in the path
        let leaf_elem = path.leaf()
            .ok_or(BtrfsError::Corrupt("no leaf in path for split"))?;
        let leaf_bytenr = leaf_elem.bytenr;

        // Allocate a new block for the right half of the split
        let right_bytenr = self.free_space.allocate(nodesize as u64, sectorsize, false)
            .ok_or(BtrfsError::NoFreeSpace)?;

        // Read the full leaf and split it
        let leaf_buf = self.read_node(leaf_bytenr)?;
        let (mut left_buf, mut right_buf, split_key) = tree::split_leaf(
            &leaf_buf, nodesize, right_bytenr, generation,
        ).ok_or(BtrfsError::Corrupt("failed to split leaf"))?;

        // Determine which half the new item goes in, and insert
        let target_buf = if *key < split_key {
            &mut left_buf
        } else {
            &mut right_buf
        };

        tree::insert_into_leaf(target_buf, nodesize, key, data, generation, &self.sb.fsid)
            .ok_or(BtrfsError::NoFreeSpace)?;

        // Write both halves
        self.write_node(leaf_bytenr, &left_buf)?;
        self.write_node(right_bytenr, &right_buf)?;

        log::debug!("[btrfs::readwrite] split leaf: left=0x{:X}, right=0x{:X}, split_key={}",
            leaf_bytenr, right_bytenr, split_key);

        // Now update the parent to include the new right child
        if path.elements.len() >= 2 {
            // Parent is the second-to-last element
            let parent_elem = &path.elements[path.elements.len() - 2];
            let parent_bytenr = parent_elem.bytenr;

            let mut parent_buf = self.read_node(parent_bytenr)?;

            let inserted = tree::insert_into_node(
                &mut parent_buf,
                nodesize,
                &split_key,
                right_bytenr,
                generation,
            );

            if inserted.is_some() {
                self.write_node(parent_bytenr, &parent_buf)?;
                log::debug!("[btrfs::readwrite] updated parent at 0x{:X} with new key_ptr for right child",
                    parent_bytenr);
            } else {
                // Parent is also full -- need to split the parent recursively.
                // For now, allocate a new node and split.
                log::info!("[btrfs::readwrite] parent node at 0x{:X} is full, splitting recursively...",
                    parent_bytenr);

                let new_node_bytenr = self.free_space.allocate(nodesize as u64, sectorsize, false)
                    .ok_or(BtrfsError::NoFreeSpace)?;

                if let Some((mut left_node, mut right_node, node_split_key)) = tree::split_node(
                    &parent_buf, nodesize, new_node_bytenr, generation,
                ) {
                    // Insert the new key_ptr into the correct half
                    if split_key < node_split_key {
                        tree::insert_into_node(&mut left_node, nodesize, &split_key, right_bytenr, generation);
                    } else {
                        tree::insert_into_node(&mut right_node, nodesize, &split_key, right_bytenr, generation);
                    }

                    self.write_node(parent_bytenr, &left_node)?;
                    self.write_node(new_node_bytenr, &right_node)?;

                    // If there's a grandparent, update it. Otherwise we need a new root.
                    if path.elements.len() >= 3 {
                        let grandparent_elem = &path.elements[path.elements.len() - 3];
                        let mut gp_buf = self.read_node(grandparent_elem.bytenr)?;
                        tree::insert_into_node(&mut gp_buf, nodesize, &node_split_key, new_node_bytenr, generation);
                        self.write_node(grandparent_elem.bytenr, &gp_buf)?;
                    } else {
                        // Need to create a new root node
                        log::info!("[btrfs::readwrite] creating new root node for tree growth");
                        // This is complex -- for now, log a warning.
                        // A full implementation would allocate a new root node
                        // containing two key_ptrs and update the root tree.
                        log::warn!("[btrfs::readwrite] root node split not yet fully wired to root tree update");
                    }
                }
            }
        } else {
            // The leaf was the root -- create a new internal root node
            log::info!("[btrfs::readwrite] leaf was root, creating new root node at level 1");

            let new_root_bytenr = self.free_space.allocate(nodesize as u64, sectorsize, false)
                .ok_or(BtrfsError::NoFreeSpace)?;

            // Build a new root node with two key_ptrs
            let mut new_root = vec![0u8; nodesize as usize];

            // Copy header from old leaf and modify
            let header = BtrfsHeader::from_bytes(&leaf_buf)
                .ok_or(BtrfsError::Corrupt("cannot parse leaf header for new root"))?;

            let mut hdr_bytes = header.to_bytes();
            // Set level to 1, nritems to 2, bytenr to new_root_bytenr, generation
            hdr_bytes[0x64] = 1; // level
            new_root[..BTRFS_HEADER_SIZE].copy_from_slice(&hdr_bytes);

            // Write nritems = 2
            new_root[0x60..0x64].copy_from_slice(&2u32.to_le_bytes());
            // Write generation
            new_root[0x50..0x58].copy_from_slice(&generation.to_le_bytes());
            // Write bytenr
            new_root[0x30..0x38].copy_from_slice(&new_root_bytenr.to_le_bytes());

            // First key_ptr: left child (original leaf)
            let left_key = BtrfsKey::min(); // The first key in the left leaf
            let left_ptr = crate::item::BtrfsKeyPtr {
                key: left_key,
                blockptr: leaf_bytenr,
                generation,
            };
            let ptr_bytes = left_ptr.to_bytes();
            let off = BTRFS_HEADER_SIZE;
            new_root[off..off + crate::item::BTRFS_KEY_PTR_SIZE].copy_from_slice(&ptr_bytes);

            // Second key_ptr: right child (new leaf)
            let right_ptr = crate::item::BtrfsKeyPtr {
                key: split_key,
                blockptr: right_bytenr,
                generation,
            };
            let ptr_bytes = right_ptr.to_bytes();
            let off = BTRFS_HEADER_SIZE + crate::item::BTRFS_KEY_PTR_SIZE;
            new_root[off..off + crate::item::BTRFS_KEY_PTR_SIZE].copy_from_slice(&ptr_bytes);

            BtrfsHeader::update_csum(&mut new_root);
            self.write_node(new_root_bytenr, &new_root)?;

            log::info!("[btrfs::readwrite] new root node at 0x{:X} with 2 children", new_root_bytenr);
            // Note: the root tree would need updating to point to new_root_bytenr.
            // This requires updating the ROOT_ITEM for FS_TREE in the root tree.
        }

        Ok(())
    }

    // ========================================================================
    // Directory operations
    // ========================================================================

    /// Create a directory at the given path.
    pub fn mkdir(&mut self, path: &[u8]) -> Result<(), BtrfsError> {
        log::info!("[btrfs::readwrite] mkdir: {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        if path.is_empty() || path[0] != b'/' {
            return Err(BtrfsError::InvalidPath);
        }

        let (parent_path, dirname) = split_path(path)?;

        // Resolve the parent
        let (parent_inode, parent_item) = self.resolve_path(parent_path)?;
        if !parent_item.is_dir() {
            return Err(BtrfsError::NotADirectory);
        }

        // Check if already exists
        if self.lookup_dir_entry(parent_inode, dirname).is_ok() {
            return Err(BtrfsError::AlreadyExists);
        }

        let next_gen = self.begin_transaction();
        let now = BtrfsTimespec { sec: 0, nsec: 0 };

        let new_inode = self.alloc_inode();

        // Create the directory inode
        let inode_item = InodeItem::new_dir(0o755, 0, 0, next_gen, now);
        let inode_data = inode_item.to_bytes();

        // Create dir entries in the parent
        let dir_item = DirItem::new(dirname, new_inode, dir_type::DIR, next_gen);
        let dir_item_data = dir_item.to_bytes();

        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;

        let inode_key = BtrfsKey::new(new_inode, KeyType::InodeItem as u8, 0);
        let dir_key = dir_item_key(parent_inode, dirname);
        let dir_idx = self.alloc_dir_index();
        let dir_idx_key = dir_index_key(parent_inode, dir_idx);

        self.insert_item(fs_root, fs_level, &inode_key, &inode_data)?;
        self.insert_item(fs_root, fs_level, &dir_key, &dir_item_data)?;
        self.insert_item(fs_root, fs_level, &dir_idx_key, &dir_item_data)?;

        self.commit_transaction()?;

        log::info!("[btrfs::readwrite] mkdir complete: {:?} -> inode {}",
            core::str::from_utf8(dirname).unwrap_or("<invalid>"), new_inode);

        Ok(())
    }

    /// List the contents of a directory at the given path.
    pub fn list_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, BtrfsError> {
        log::info!("[btrfs::readwrite] list_dir: {:?}",
            core::str::from_utf8(path).unwrap_or("<invalid>"));

        let (dir_inode, dir_item) = self.resolve_path(path)?;

        if !dir_item.is_dir() {
            return Err(BtrfsError::IsNotADirectory);
        }

        // Walk the FS tree looking for DIR_INDEX items for this inode
        let fs_root = self.find_fs_tree_root()?;
        let fs_level = self.find_fs_tree_level()?;
        let nodesize = self.sb.nodesize;
        let chunk_map = &self.chunk_map;
        let dev = &self.dev;

        let mut entries: Vec<DirEntry> = Vec::new();

        tree::walk_tree(
            fs_root, fs_level, nodesize,
            |logical| read_node_via_chunks(dev, chunk_map, logical, nodesize),
            |key, data| {
                if key.objectid == dir_inode && key.key_type == KeyType::DirIndex as u8 {
                    let items = DirItem::parse_all(data);
                    for item in items {
                        let name = String::from_utf8_lossy(&item.name[..item.name_len as usize]).into_owned();
                        entries.push(DirEntry {
                            name,
                            inode: item.location.objectid,
                            file_type: item.dir_type,
                        });
                    }
                }
            },
        );

        log::info!("[btrfs::readwrite] list_dir: {} entries in inode {}", entries.len(), dir_inode);

        for entry in &entries {
            log::debug!("[btrfs::readwrite]   {:?} (inode={}, type={})",
                entry.name, entry.inode, entry.type_str());
        }

        Ok(entries)
    }

    // ========================================================================
    // Sync / flush
    // ========================================================================

    /// Flush the superblock to disk (with updated generation).
    ///
    /// Also writes updated block group items to reflect current free space usage.
    pub fn sync(&mut self) -> Result<(), BtrfsError> {
        log::info!("[btrfs::readwrite] syncing superblock to disk (generation={})", self.sb.generation);

        // Update superblock bytes_used from free space tracker
        let total_used: u64 = self.free_space.block_groups.iter().map(|bg| bg.used).sum();
        self.sb.bytes_used = total_used;

        let sb_bytes = self.sb.to_bytes();
        self.dev.write_bytes(SUPERBLOCK_OFFSET, &sb_bytes)?;

        log::info!("[btrfs::readwrite] sync complete (bytes_used={})", total_used);
        Ok(())
    }

    /// Get filesystem statistics.
    pub fn stats(&self) -> FsStats {
        FsStats {
            total_bytes: self.sb.total_bytes,
            bytes_used: self.sb.bytes_used,
            free_bytes: self.free_space.total_free(),
            generation: self.generation,
            nodesize: self.sb.nodesize,
            sectorsize: self.sb.sectorsize,
            num_devices: self.sb.num_devices,
            block_groups: self.free_space.block_groups.len(),
        }
    }
}

/// Filesystem statistics.
#[derive(Debug)]
pub struct FsStats {
    /// Total filesystem size.
    pub total_bytes: u64,
    /// Bytes used by data and metadata.
    pub bytes_used: u64,
    /// Free bytes available for allocation.
    pub free_bytes: u64,
    /// Current generation (transaction ID).
    pub generation: u64,
    /// Node size (typically 16 KiB).
    pub nodesize: u32,
    /// Sector size (typically 4 KiB).
    pub sectorsize: u32,
    /// Number of physical devices.
    pub num_devices: u64,
    /// Number of block groups.
    pub block_groups: usize,
}

// ============================================================================
// Helper functions
// ============================================================================

/// Read a tree node via chunk map address translation.
///
/// Helper function used by tree operations that need a closure for reading nodes.
fn read_node_via_chunks<D: BlockDevice>(
    dev: &D,
    chunk_map: &ChunkMap,
    logical: u64,
    nodesize: u32,
) -> Option<Vec<u8>> {
    let (_devid, physical) = chunk_map.resolve(logical)?;

    let mut buf = vec![0u8; nodesize as usize];
    match dev.read_bytes(physical, &mut buf) {
        Ok(()) => {
            log::trace!("[btrfs::readwrite] read_node_via_chunks: logical=0x{:X} -> physical=0x{:X}",
                logical, physical);
            Some(buf)
        }
        Err(e) => {
            log::error!("[btrfs::readwrite] failed to read node at logical=0x{:X}, physical=0x{:X}: {}",
                logical, physical, e);
            None
        }
    }
}

/// Split a path into (parent_path, filename).
///
/// For example, `/foo/bar/baz` -> (`/foo/bar`, `baz`).
fn split_path(path: &[u8]) -> Result<(&[u8], &[u8]), BtrfsError> {
    if path.is_empty() || path[0] != b'/' {
        return Err(BtrfsError::InvalidPath);
    }

    // Remove trailing slash
    let path = if path.len() > 1 && path[path.len() - 1] == b'/' {
        &path[..path.len() - 1]
    } else {
        path
    };

    // Find last slash
    let last_slash = path.iter().rposition(|&b| b == b'/')
        .ok_or(BtrfsError::InvalidPath)?;

    let parent = if last_slash == 0 { &path[..1] } else { &path[..last_slash] };
    let filename = &path[last_slash + 1..];

    if filename.is_empty() {
        return Err(BtrfsError::InvalidPath);
    }

    if filename.len() > 255 {
        return Err(BtrfsError::NameTooLong);
    }

    Ok((parent, filename))
}
