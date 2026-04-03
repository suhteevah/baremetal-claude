//! CRC32C (Castagnoli) implementation for ext4 metadata checksums.
//!
//! ext4 uses CRC32C for all metadata checksums when the
//! `RO_COMPAT_METADATA_CSUM` feature flag is set. This is the same
//! polynomial used by btrfs and SCTP: 0x82F63B78 (reflected).
//!
//! This is a table-based implementation suitable for `no_std` environments.

/// CRC32C polynomial (Castagnoli), reflected form.
const CRC32C_POLY: u32 = 0x82F63B78;

/// Pre-computed CRC32C lookup table (256 entries).
///
/// Generated at compile time from the CRC32C polynomial.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ CRC32C_POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Compute CRC32C of a byte slice, starting from the given initial value.
///
/// The initial CRC should be `0xFFFFFFFF` for a fresh calculation, or the
/// result of a previous `crc32c_update` call for incremental computation.
///
/// Returns the intermediate CRC value (NOT finalized -- do NOT XOR with 0xFFFFFFFF
/// until you are done feeding all data).
#[inline]
pub fn crc32c_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32C_TABLE[idx];
    }
    crc
}

/// Compute the finalized CRC32C of a byte slice.
///
/// Equivalent to `crc32c_update(0xFFFFFFFF, data) ^ 0xFFFFFFFF`.
#[inline]
pub fn crc32c(data: &[u8]) -> u32 {
    crc32c_update(0xFFFFFFFF, data) ^ 0xFFFFFFFF
}

/// Compute CRC32C incrementally from a seed, then finalize.
///
/// This is used by ext4 for seeded checksums (e.g., block group descriptor
/// checksums are seeded with the superblock UUID checksum).
#[inline]
pub fn crc32c_seed(seed: u32, data: &[u8]) -> u32 {
    crc32c_update(seed ^ 0xFFFFFFFF, data) ^ 0xFFFFFFFF
}

/// Compute the CRC32C of the superblock UUID, used as the seed for
/// all other metadata checksums.
///
/// `uuid` is the 16-byte filesystem UUID from the superblock.
#[inline]
pub fn crc32c_uuid_seed(uuid: &[u8; 16]) -> u32 {
    crc32c(uuid)
}

/// Verify a superblock checksum.
///
/// The superblock checksum is stored at offset 0x3FC (last 4 bytes).
/// It is the CRC32C of the entire 1024-byte superblock with the checksum
/// field itself zeroed out.
pub fn verify_superblock_checksum(sb_bytes: &[u8]) -> bool {
    if sb_bytes.len() < 1024 {
        return false;
    }
    let stored = u32::from_le_bytes([
        sb_bytes[0x3FC],
        sb_bytes[0x3FD],
        sb_bytes[0x3FE],
        sb_bytes[0x3FF],
    ]);

    // Compute CRC32C of the superblock with checksum field zeroed
    let mut crc = 0xFFFFFFFF_u32;
    crc = crc32c_update(crc, &sb_bytes[..0x3FC]);
    crc = crc32c_update(crc, &[0u8; 4]); // zeroed checksum field
    if sb_bytes.len() > 0x400 {
        // Shouldn't happen for a standard superblock, but be safe
    }
    let computed = crc ^ 0xFFFFFFFF;

    if stored != computed {
        log::warn!(
            "[ext4::crc32c] superblock checksum mismatch: stored=0x{:08X}, computed=0x{:08X}",
            stored, computed
        );
        return false;
    }
    log::trace!("[ext4::crc32c] superblock checksum OK: 0x{:08X}", stored);
    true
}

/// Compute the superblock checksum and write it to the buffer.
pub fn compute_superblock_checksum(sb_bytes: &mut [u8]) {
    if sb_bytes.len() < 1024 {
        return;
    }
    // Zero out the checksum field first
    sb_bytes[0x3FC..0x400].copy_from_slice(&[0u8; 4]);

    let mut crc = 0xFFFFFFFF_u32;
    crc = crc32c_update(crc, &sb_bytes[..0x3FC]);
    crc = crc32c_update(crc, &[0u8; 4]);
    let checksum = crc ^ 0xFFFFFFFF;

    sb_bytes[0x3FC..0x400].copy_from_slice(&checksum.to_le_bytes());
    log::trace!(
        "[ext4::crc32c] computed superblock checksum: 0x{:08X}",
        checksum
    );
}

/// Compute a block group descriptor checksum.
///
/// The checksum covers: UUID seed + group_number(LE32) + descriptor bytes
/// (with checksum field at offset 0x1E zeroed).
///
/// `uuid_seed` is `crc32c(superblock.uuid)`.
/// `group_num` is the 0-based block group index.
/// `desc_bytes` is the raw descriptor bytes (32 or 64).
pub fn compute_bgd_checksum(uuid_seed: u32, group_num: u32, desc_bytes: &[u8]) -> u16 {
    let mut crc = uuid_seed ^ 0xFFFFFFFF;
    crc = crc32c_update(crc, &group_num.to_le_bytes());

    // Feed descriptor bytes, but zero out the checksum field at offset 0x1E..0x20
    if desc_bytes.len() >= 0x20 {
        crc = crc32c_update(crc, &desc_bytes[..0x1E]);
        crc = crc32c_update(crc, &[0u8; 2]); // zeroed checksum
        crc = crc32c_update(crc, &desc_bytes[0x20..]);
    } else {
        crc = crc32c_update(crc, desc_bytes);
    }

    let checksum = crc ^ 0xFFFFFFFF;
    // ext4 uses the lower 16 bits for the bgd checksum field
    checksum as u16
}

/// Compute an inode checksum.
///
/// `uuid_seed` is `crc32c(superblock.uuid)`.
/// `ino` is the inode number.
/// `generation` is the inode generation.
/// `inode_bytes` is the raw inode bytes with checksum fields zeroed.
pub fn compute_inode_checksum(
    uuid_seed: u32,
    ino: u32,
    generation: u32,
    inode_bytes: &[u8],
) -> u32 {
    let mut crc = uuid_seed ^ 0xFFFFFFFF;
    crc = crc32c_update(crc, &ino.to_le_bytes());
    crc = crc32c_update(crc, &generation.to_le_bytes());
    crc = crc32c_update(crc, inode_bytes);
    crc ^ 0xFFFFFFFF
}

/// Compute an extent tree tail checksum.
///
/// The extent tail is 4 bytes at the end of an extent tree block,
/// containing the CRC32C of: UUID seed + inode_number(LE32) + generation(LE32)
/// + extent_block_data (excluding the tail itself).
pub fn compute_extent_checksum(
    uuid_seed: u32,
    ino: u32,
    generation: u32,
    extent_block: &[u8],
) -> u32 {
    let mut crc = uuid_seed ^ 0xFFFFFFFF;
    crc = crc32c_update(crc, &ino.to_le_bytes());
    crc = crc32c_update(crc, &generation.to_le_bytes());
    crc = crc32c_update(crc, extent_block);
    crc ^ 0xFFFFFFFF
}

/// Compute a directory entry block tail checksum (dx_tail).
///
/// `uuid_seed` is `crc32c(superblock.uuid)`.
/// `ino` is the directory inode number.
/// `generation` is the inode generation.
/// `dir_block` is the directory block data (excluding the tail).
pub fn compute_dir_checksum(
    uuid_seed: u32,
    ino: u32,
    generation: u32,
    dir_block: &[u8],
) -> u32 {
    let mut crc = uuid_seed ^ 0xFFFFFFFF;
    crc = crc32c_update(crc, &ino.to_le_bytes());
    crc = crc32c_update(crc, &generation.to_le_bytes());
    crc = crc32c_update(crc, dir_block);
    crc ^ 0xFFFFFFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32c_empty() {
        assert_eq!(crc32c(&[]), 0x00000000);
    }

    #[test]
    fn test_crc32c_known_vectors() {
        // Known test vector: CRC32C of "123456789" = 0xE3069283
        assert_eq!(crc32c(b"123456789"), 0xE3069283);
    }

    #[test]
    fn test_crc32c_incremental() {
        let full = crc32c(b"hello world");
        let mut partial = 0xFFFFFFFF;
        partial = crc32c_update(partial, b"hello ");
        partial = crc32c_update(partial, b"world");
        let incremental = partial ^ 0xFFFFFFFF;
        assert_eq!(full, incremental);
    }
}
