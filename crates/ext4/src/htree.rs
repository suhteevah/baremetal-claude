//! HTree (directory hash tree) implementation for ext4.
//!
//! ext4 directories can use a hash tree (HTree) for O(1) filename lookup
//! instead of linear scanning. The HTree is detected by the `EXT4_INDEX_FL`
//! flag (0x01000) on the directory inode.
//!
//! ## Structure
//!
//! - **dx_root** (first directory block): Fake "." and ".." entries followed
//!   by `dx_root_info` and an array of `dx_entry` (hash, block) pairs.
//! - **dx_node** (internal index blocks): Fake dirent header followed by
//!   `dx_entry` array.
//! - **Leaf blocks**: Standard directory entry blocks.
//!
//! The hash function is half-MD4 applied to the filename.
//!
//! Reference: <https://ext4.wiki.kernel.org/index.php/Ext4_Disk_Layout#Hash_Tree_Directories>

use alloc::vec::Vec;

use crate::dir::{self, DirEntry};

/// EXT4_INDEX_FL: inode flag indicating HTree-indexed directory.
pub const EXT4_INDEX_FL: u32 = 0x00001000;

/// Hash version: half-MD4 (legacy, most common).
pub const DX_HASH_HALF_MD4: u8 = 1;
/// Hash version: TEA.
pub const DX_HASH_TEA: u8 = 2;
/// Hash version: half-MD4 unsigned.
pub const DX_HASH_HALF_MD4_UNSIGNED: u8 = 3;
/// Hash version: TEA unsigned.
pub const DX_HASH_TEA_UNSIGNED: u8 = 4;
/// Hash version: siphash (ext4 with casefolding).
pub const DX_HASH_SIPHASH: u8 = 5;

/// Size of dx_root_info structure (after the fake ".." entry).
pub const DX_ROOT_INFO_SIZE: usize = 8;

/// A dx_entry: maps a hash value to a directory block number.
#[derive(Clone, Copy, Debug)]
pub struct DxEntry {
    /// Hash value (or 0 for the first entry which covers all hashes below the second entry).
    pub hash: u32,
    /// Block number within the directory's data blocks.
    pub block: u32,
}

impl DxEntry {
    /// Parse a dx_entry from 8 bytes.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 8 {
            return None;
        }
        Some(DxEntry {
            hash: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            block: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        })
    }
}

/// Parsed dx_root_info from the root block of an HTree directory.
#[derive(Clone, Debug)]
pub struct DxRootInfo {
    /// Hash version (DX_HASH_HALF_MD4, etc.).
    pub hash_version: u8,
    /// Length of the dx_root_info structure.
    pub info_length: u8,
    /// Indirect levels (0 = single level, 1 = two levels).
    pub indirect_levels: u8,
    /// Unused flags.
    pub unused_flags: u8,
}

/// Parse the dx_root block (the first block of an HTree directory).
///
/// Returns (dx_root_info, Vec<DxEntry>).
///
/// Layout of the root block:
/// - offset 0: fake "." dirent (12 bytes)
/// - offset 12: fake ".." dirent (rec_len covers rest up to the dx_root_info)
///   The ".." dirent has rec_len = block_size - 12
///   But the actual ".." entry data ends after its name, and the dx_root_info
///   starts at a fixed offset.
///
/// Actually, the standard layout is:
/// - 0x00: "." dirent (inode=self, rec_len=12, name_len=1, type=2, name=".")
/// - 0x0C: ".." dirent (inode=parent, rec_len=block_size-12, name_len=2, type=2, name="..")
/// - 0x18: dx_root_info (4 bytes: reserved=0, hash_version, info_length=8, indirect_levels, unused_flags)
///   Wait, actually at 0x14 is another 4 bytes of zero (the "reserved" field).
///
/// Real layout:
/// - 0x00..0x0C: "." entry (rec_len=12)
/// - 0x0C..0x18: ".." entry (rec_len=block_size-12), but only 12 bytes of actual data
///   The ".." rec_len is set to (block_size - 12) to consume the rest of the block.
///   After the ".." entry at offset 0x14 (or 0x18 on 4-byte aligned name), we have:
///
/// The actual position: after the ".." entry's name and padding:
/// - "." at 0x00: 8 header + 1 name + 3 pad = 12 bytes total (rec_len=12)
/// - ".." at 0x0C: 8 header + 2 name + 2 pad = 12 bytes actual data
///   But rec_len = block_size - 12.
///   The dx_root_info starts at offset 0x0C + actual_dotdot_size(12) = 0x18
///
/// So:
/// 0x18: reserved (u32) = 0
/// 0x1C: dx_root_info (4 bytes): hash_version(u8), info_length(u8), indirect_levels(u8), unused_flags(u8)
/// 0x20: limit (u16) -- max dx_entry count
/// 0x22: count (u16) -- actual dx_entry count
/// 0x24: block (u32) -- block for first entry (hash=0)
/// Then dx_entries at 0x28, 0x30, ...
pub fn parse_dx_root(block_data: &[u8]) -> Option<(DxRootInfo, Vec<DxEntry>)> {
    if block_data.len() < 0x28 {
        log::error!("[ext4::htree] block too small for dx_root: {} bytes", block_data.len());
        return None;
    }

    // Verify "." entry at offset 0
    let dot_rec_len = u16::from_le_bytes([block_data[4], block_data[5]]);
    if dot_rec_len != 12 {
        log::warn!("[ext4::htree] unexpected dot rec_len: {} (expected 12)", dot_rec_len);
    }

    // dx_root_info at offset 0x1C
    let info = DxRootInfo {
        hash_version: block_data[0x1C],
        info_length: block_data[0x1D],
        indirect_levels: block_data[0x1E],
        unused_flags: block_data[0x1F],
    };

    log::debug!(
        "[ext4::htree] dx_root_info: hash_version={}, info_length={}, indirect_levels={}",
        info.hash_version, info.info_length, info.indirect_levels
    );

    // Count and limit at 0x20
    let limit = u16::from_le_bytes([block_data[0x20], block_data[0x21]]) as usize;
    let count = u16::from_le_bytes([block_data[0x22], block_data[0x23]]) as usize;

    log::debug!("[ext4::htree] dx_root: count={}, limit={}", count, limit);

    // dx_entries start at 0x24 (the first is the "dot" entry covering hash=0)
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 0x24 + i * 8;
        if off + 8 > block_data.len() {
            log::warn!("[ext4::htree] truncated dx_entry at index {}", i);
            break;
        }
        if let Some(entry) = DxEntry::from_bytes(&block_data[off..]) {
            entries.push(entry);
        }
    }

    Some((info, entries))
}

/// Parse a dx_node block (internal HTree node, not the root).
///
/// Layout:
/// - 0x00: fake dirent (inode=0, rec_len=block_size, name_len=0, type=0)
///   This is 8 bytes.
/// - 0x08: limit (u16), count (u16)
/// - 0x0C: first dx_entry ...
pub fn parse_dx_node(block_data: &[u8]) -> Option<Vec<DxEntry>> {
    if block_data.len() < 0x14 {
        log::error!("[ext4::htree] block too small for dx_node: {} bytes", block_data.len());
        return None;
    }

    let limit = u16::from_le_bytes([block_data[0x08], block_data[0x09]]) as usize;
    let count = u16::from_le_bytes([block_data[0x0A], block_data[0x0B]]) as usize;

    log::debug!("[ext4::htree] dx_node: count={}, limit={}", count, limit);

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 0x0C + i * 8;
        if off + 8 > block_data.len() {
            break;
        }
        if let Some(entry) = DxEntry::from_bytes(&block_data[off..]) {
            entries.push(entry);
        }
    }

    Some(entries)
}

/// Find the dx_entry whose hash range contains the target hash.
///
/// dx_entries are sorted by hash. We find the entry with the largest hash
/// that is <= target_hash (binary search).
pub fn find_dx_entry_for_hash(entries: &[DxEntry], target_hash: u32) -> Option<&DxEntry> {
    if entries.is_empty() {
        return None;
    }

    // The first entry always has hash=0 and covers everything below entries[1].hash
    let mut lo = 0usize;
    let mut hi = entries.len();

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if entries[mid].hash <= target_hash {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }

    if lo == 0 {
        // target_hash is less than all entries; shouldn't happen if entries[0].hash == 0
        Some(&entries[0])
    } else {
        Some(&entries[lo - 1])
    }
}

/// Half-MD4 hash function used by ext4 for HTree directory lookups.
///
/// This is NOT the full MD4 hash -- it's a simplified version that ext4 uses.
/// The input is the filename bytes. The output is a 32-bit hash.
///
/// The algorithm processes the name in 32-byte chunks through a half-MD4 round.
pub fn half_md4_hash(name: &[u8], seed: &[u32; 4]) -> u32 {
    let mut a = seed[0];
    let mut b = seed[1];
    let mut c = seed[2];
    let mut d = seed[3];

    let len = name.len();
    let mut offset = 0;

    while offset < len || offset == 0 {
        // Read up to 32 bytes as 8 u32 values (little-endian, zero-padded)
        let mut input = [0u32; 8];
        for i in 0..8 {
            let base = offset + i * 4;
            if base < len {
                let b0 = if base < len { name[base] as u32 } else { 0 };
                let b1 = if base + 1 < len { name[base + 1] as u32 } else { 0 };
                let b2 = if base + 2 < len { name[base + 2] as u32 } else { 0 };
                let b3 = if base + 3 < len { name[base + 3] as u32 } else { 0 };
                input[i] = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
            }
        }

        // Half-MD4 transform (three rounds)
        half_md4_round1(&mut a, &mut b, &mut c, &mut d, &input);
        half_md4_round2(&mut a, &mut b, &mut c, &mut d, &input);
        half_md4_round3(&mut a, &mut b, &mut c, &mut d, &input);

        offset += 32;
        if offset > 0 && offset >= len {
            break;
        }
    }

    // Return the lower 32 bits of the hash, masking out the sign bit (bit 31)
    // to ensure the hash is always a non-negative value. The ext4 HTree uses
    // unsigned comparison for hash ordering, so this keeps things consistent.
    (b.wrapping_add(a)) & 0x7FFFFFFF
}

/// TEA (Tiny Encryption Algorithm) hash, alternative to half-MD4.
pub fn tea_hash(name: &[u8], seed: &[u32; 4]) -> u32 {
    let mut a = seed[0];
    let mut b = seed[1];
    let mut c = seed[2];
    let mut d = seed[3];
    let len = name.len();
    let mut offset = 0;

    while offset < len {
        let mut input = [0u32; 4];
        for i in 0..4 {
            let base = offset + i * 4;
            if base < len {
                let b0 = if base < len { name[base] as u32 } else { 0 };
                let b1 = if base + 1 < len { name[base + 1] as u32 } else { 0 };
                let b2 = if base + 2 < len { name[base + 2] as u32 } else { 0 };
                let b3 = if base + 3 < len { name[base + 3] as u32 } else { 0 };
                input[i] = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
            }
        }

        // TEA-like scramble using the golden ratio constant.
        // delta = 0x9E3779B9 = floor(2^32 / golden_ratio) is the standard
        // TEA constant that ensures each round produces a different mixing.
        let mut sum = 0u32;
        let delta: u32 = 0x9E3779B9;
        for _ in 0..16 {
            sum = sum.wrapping_add(delta);
            a = a.wrapping_add(
                (b.wrapping_shl(4).wrapping_add(input[0]))
                    ^ (b.wrapping_add(sum))
                    ^ (b.wrapping_shr(5).wrapping_add(input[1])),
            );
            b = b.wrapping_add(
                (a.wrapping_shl(4).wrapping_add(input[2]))
                    ^ (a.wrapping_add(sum))
                    ^ (a.wrapping_shr(5).wrapping_add(input[3])),
            );
        }

        // Fold the result
        c = c.wrapping_add(a);
        d = d.wrapping_add(b);

        offset += 16;
    }

    (c.wrapping_add(d)) & 0x7FFFFFFF
}

/// Compute the directory hash for a filename using the specified hash version.
///
/// `hash_seed` comes from the superblock's `s_hash_seed` field (4 u32 values).
/// If the superblock doesn't have a seed, use [0; 4].
pub fn dx_hash(name: &[u8], hash_version: u8, hash_seed: &[u32; 4]) -> u32 {
    match hash_version {
        DX_HASH_HALF_MD4 => half_md4_hash(name, hash_seed),
        DX_HASH_HALF_MD4_UNSIGNED => {
            // Same as half-MD4 but treats name bytes as unsigned (they already are in Rust)
            half_md4_hash(name, hash_seed)
        }
        DX_HASH_TEA | DX_HASH_TEA_UNSIGNED => tea_hash(name, hash_seed),
        _ => {
            log::warn!(
                "[ext4::htree] unknown hash version {}, falling back to half-MD4",
                hash_version
            );
            half_md4_hash(name, hash_seed)
        }
    }
}

/// Look up a name in an HTree-indexed directory.
///
/// `read_dir_block` is a closure that reads the Nth data block of the directory.
/// `root_block` is the data of the first directory block (the dx_root).
///
/// Returns the DirEntry if found.
pub fn htree_lookup<F>(
    root_block: &[u8],
    name: &[u8],
    hash_seed: &[u32; 4],
    mut read_dir_block: F,
) -> Option<DirEntry>
where
    F: FnMut(u32) -> Option<Vec<u8>>,
{
    let (info, root_entries) = parse_dx_root(root_block)?;

    let hash = dx_hash(name, info.hash_version, hash_seed);
    log::debug!(
        "[ext4::htree] looking up {:?} with hash=0x{:08X}",
        core::str::from_utf8(name).unwrap_or("<invalid>"),
        hash
    );

    // Find the correct dx_entry in the root
    let target_entry = find_dx_entry_for_hash(&root_entries, hash)?;
    let target_block_num = target_entry.block;

    if info.indirect_levels == 0 {
        // Single-level: target_block_num is a leaf directory block
        let leaf_data = read_dir_block(target_block_num)?;
        if let Some((_off, entry)) = dir::lookup_in_block(&leaf_data, name) {
            return Some(entry);
        }
    } else {
        // Two-level: target_block_num is a dx_node
        let node_data = read_dir_block(target_block_num)?;
        let node_entries = parse_dx_node(&node_data)?;
        let leaf_entry = find_dx_entry_for_hash(&node_entries, hash)?;
        let leaf_data = read_dir_block(leaf_entry.block)?;
        if let Some((_off, entry)) = dir::lookup_in_block(&leaf_data, name) {
            return Some(entry);
        }
    }

    log::trace!(
        "[ext4::htree] {:?} not found via htree",
        core::str::from_utf8(name).unwrap_or("<invalid>")
    );
    None
}

// --- Half-MD4 internal round functions ---
//
// These are the three auxiliary boolean functions from the MD4 specification
// (RFC 1320). Each round of the half-MD4 transform uses one of these to mix
// the four state variables (a, b, c, d) together with input words.

/// Round 1 function F(X,Y,Z) = (X AND Y) OR (NOT X AND Z).
/// Acts as a bitwise multiplexer: selects Y bits where X=1, Z bits where X=0.
#[inline]
fn f(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (!x & z)
}

/// Round 2 function G(X,Y,Z) = (X AND Y) OR (X AND Z) OR (Y AND Z).
/// Majority function: output bit is 1 if at least two of the three input bits are 1.
#[inline]
fn g(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (x & z) | (y & z)
}

/// Round 3 function H(X,Y,Z) = X XOR Y XOR Z.
/// Parity function: output bit is 1 if an odd number of input bits are 1.
#[inline]
fn h(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}

fn half_md4_round1(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32, input: &[u32; 8]) {
    *a = a.wrapping_add(f(*b, *c, *d)).wrapping_add(input[0]).rotate_left(3);
    *d = d.wrapping_add(f(*a, *b, *c)).wrapping_add(input[1]).rotate_left(7);
    *c = c.wrapping_add(f(*d, *a, *b)).wrapping_add(input[2]).rotate_left(11);
    *b = b.wrapping_add(f(*c, *d, *a)).wrapping_add(input[3]).rotate_left(19);
    *a = a.wrapping_add(f(*b, *c, *d)).wrapping_add(input[4]).rotate_left(3);
    *d = d.wrapping_add(f(*a, *b, *c)).wrapping_add(input[5]).rotate_left(7);
    *c = c.wrapping_add(f(*d, *a, *b)).wrapping_add(input[6]).rotate_left(11);
    *b = b.wrapping_add(f(*c, *d, *a)).wrapping_add(input[7]).rotate_left(19);
}

/// Round 2 additive constant: floor(2^30 * sqrt(2)) = 0x5A827999.
/// This is the same constant used in the original MD4 specification.
const MD4_ROUND2_CONST: u32 = 0x5A827999;

fn half_md4_round2(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32, input: &[u32; 8]) {
    *a = a.wrapping_add(g(*b, *c, *d)).wrapping_add(input[1]).wrapping_add(MD4_ROUND2_CONST).rotate_left(3);
    *d = d.wrapping_add(g(*a, *b, *c)).wrapping_add(input[3]).wrapping_add(MD4_ROUND2_CONST).rotate_left(5);
    *c = c.wrapping_add(g(*d, *a, *b)).wrapping_add(input[5]).wrapping_add(MD4_ROUND2_CONST).rotate_left(9);
    *b = b.wrapping_add(g(*c, *d, *a)).wrapping_add(input[7]).wrapping_add(MD4_ROUND2_CONST).rotate_left(13);
    *a = a.wrapping_add(g(*b, *c, *d)).wrapping_add(input[0]).wrapping_add(MD4_ROUND2_CONST).rotate_left(3);
    *d = d.wrapping_add(g(*a, *b, *c)).wrapping_add(input[2]).wrapping_add(MD4_ROUND2_CONST).rotate_left(5);
    *c = c.wrapping_add(g(*d, *a, *b)).wrapping_add(input[4]).wrapping_add(MD4_ROUND2_CONST).rotate_left(9);
    *b = b.wrapping_add(g(*c, *d, *a)).wrapping_add(input[6]).wrapping_add(MD4_ROUND2_CONST).rotate_left(13);
}

/// Round 3 additive constant: floor(2^30 * sqrt(3)) = 0x6ED9EBA1.
/// This is the same constant used in the original MD4 specification.
const MD4_ROUND3_CONST: u32 = 0x6ED9EBA1;

fn half_md4_round3(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32, input: &[u32; 8]) {
    *a = a.wrapping_add(h(*b, *c, *d)).wrapping_add(input[0]).wrapping_add(MD4_ROUND3_CONST).rotate_left(3);
    *d = d.wrapping_add(h(*a, *b, *c)).wrapping_add(input[4]).wrapping_add(MD4_ROUND3_CONST).rotate_left(9);
    *c = c.wrapping_add(h(*d, *a, *b)).wrapping_add(input[2]).wrapping_add(MD4_ROUND3_CONST).rotate_left(11);
    *b = b.wrapping_add(h(*c, *d, *a)).wrapping_add(input[6]).wrapping_add(MD4_ROUND3_CONST).rotate_left(15);
    *a = a.wrapping_add(h(*b, *c, *d)).wrapping_add(input[1]).wrapping_add(MD4_ROUND3_CONST).rotate_left(3);
    *d = d.wrapping_add(h(*a, *b, *c)).wrapping_add(input[5]).wrapping_add(MD4_ROUND3_CONST).rotate_left(9);
    *c = c.wrapping_add(h(*d, *a, *b)).wrapping_add(input[3]).wrapping_add(MD4_ROUND3_CONST).rotate_left(11);
    *b = b.wrapping_add(h(*c, *d, *a)).wrapping_add(input[7]).wrapping_add(MD4_ROUND3_CONST).rotate_left(15);
}
