//! Legacy indirect block map support for ext2/ext3 inodes.
//!
//! Inodes that do NOT have the `EXT4_EXTENTS_FL` flag use the traditional
//! block mapping scheme from ext2/ext3:
//!
//! - `i_block[0..11]` : 12 direct block pointers
//! - `i_block[12]`    : single indirect (pointer to a block of pointers)
//! - `i_block[13]`    : double indirect (pointer to block of indirect pointers)
//! - `i_block[14]`    : triple indirect
//!
//! Each block pointer is a 32-bit little-endian block number. A value of 0
//! means "not allocated" (hole).
//!
//! For a 4 KiB block size, each indirect block holds 1024 pointers.
//! Maximum file size with triple indirect: ~4 TiB.

use alloc::vec;
use alloc::vec::Vec;

use crate::inode::{Inode, DIRECT_BLOCKS};
use crate::readwrite::{BlockDevice, Ext4Error, Ext4Fs};

/// Number of 32-bit block pointers that fit in one indirect block.
///
/// Each indirect block is one filesystem block filled entirely with 32-bit
/// (4-byte) little-endian block numbers. For a 4 KiB block size, this is
/// 1024 pointers. This value determines the fan-out at each level of
/// indirection and thus the maximum file size:
///   - Single indirect: `ppb` blocks = 4 MiB
///   - Double indirect: `ppb^2` blocks = 4 GiB
///   - Triple indirect: `ppb^3` blocks = 4 TiB
#[inline]
fn ptrs_per_block(block_size: u64) -> u64 {
    block_size / 4
}

/// Resolve a logical block number to a physical block number using the
/// legacy indirect block map.
///
/// Returns `Ok(Some(physical_block))` if the block is mapped,
/// `Ok(None)` if the logical block falls in a hole (sparse file),
/// or `Err(...)` on I/O error.
pub fn read_block_map<D: BlockDevice>(
    fs: &Ext4Fs<D>,
    inode: &Inode,
    logical_block: u64,
) -> Result<Option<u64>, Ext4Error> {
    let block_size = fs.sb.block_size();
    let ppb = ptrs_per_block(block_size);

    // Direct blocks: 0..11
    if logical_block < DIRECT_BLOCKS as u64 {
        let ptr = inode.direct_block(logical_block as usize);
        log::trace!(
            "[ext4::block_map] direct block {}: phys={}",
            logical_block,
            ptr
        );
        return Ok(if ptr == 0 { None } else { Some(ptr as u64) });
    }

    let remaining = logical_block - DIRECT_BLOCKS as u64;

    // Single indirect: covers ppb blocks
    if remaining < ppb {
        let ind_block = inode.indirect_block();
        if ind_block == 0 {
            return Ok(None);
        }
        let ptr = read_indirect_entry(fs, ind_block as u64, remaining as usize)?;
        log::trace!(
            "[ext4::block_map] single indirect block {}: phys={}",
            logical_block,
            ptr
        );
        return Ok(if ptr == 0 { None } else { Some(ptr as u64) });
    }

    let remaining = remaining - ppb;

    // Double indirect: covers ppb * ppb blocks
    if remaining < ppb * ppb {
        let dind_block = inode.double_indirect_block();
        if dind_block == 0 {
            return Ok(None);
        }
        let index1 = (remaining / ppb) as usize;
        let index2 = (remaining % ppb) as usize;

        let ind_block = read_indirect_entry(fs, dind_block as u64, index1)?;
        if ind_block == 0 {
            return Ok(None);
        }
        let ptr = read_indirect_entry(fs, ind_block as u64, index2)?;
        log::trace!(
            "[ext4::block_map] double indirect block {}: phys={}",
            logical_block,
            ptr
        );
        return Ok(if ptr == 0 { None } else { Some(ptr as u64) });
    }

    let remaining = remaining - ppb * ppb;

    // Triple indirect: covers ppb * ppb * ppb blocks
    if remaining < ppb * ppb * ppb {
        let tind_block = inode.triple_indirect_block();
        if tind_block == 0 {
            return Ok(None);
        }
        let index1 = (remaining / (ppb * ppb)) as usize;
        let index2 = ((remaining / ppb) % ppb) as usize;
        let index3 = (remaining % ppb) as usize;

        let dind_block = read_indirect_entry(fs, tind_block as u64, index1)?;
        if dind_block == 0 {
            return Ok(None);
        }
        let ind_block = read_indirect_entry(fs, dind_block as u64, index2)?;
        if ind_block == 0 {
            return Ok(None);
        }
        let ptr = read_indirect_entry(fs, ind_block as u64, index3)?;
        log::trace!(
            "[ext4::block_map] triple indirect block {}: phys={}",
            logical_block,
            ptr
        );
        return Ok(if ptr == 0 { None } else { Some(ptr as u64) });
    }

    log::error!(
        "[ext4::block_map] logical block {} exceeds maximum for indirect mapping",
        logical_block
    );
    Err(Ext4Error::Corrupt("logical block out of range for indirect block map"))
}

/// Read a single 32-bit block pointer from an indirect block.
///
/// `indirect_block` is the physical block number of the indirect block.
/// `index` is the index of the pointer within the block (0-based).
fn read_indirect_entry<D: BlockDevice>(
    fs: &Ext4Fs<D>,
    indirect_block: u64,
    index: usize,
) -> Result<u64, Ext4Error> {
    let block_data = fs.read_block(indirect_block)?;
    let offset = index * 4;
    if offset + 4 > block_data.len() {
        log::error!(
            "[ext4::block_map] indirect entry index {} out of range (block size {})",
            index,
            block_data.len()
        );
        return Err(Ext4Error::Corrupt("indirect block entry out of range"));
    }
    let ptr = u32::from_le_bytes([
        block_data[offset],
        block_data[offset + 1],
        block_data[offset + 2],
        block_data[offset + 3],
    ]);
    Ok(ptr as u64)
}

/// Read all data of an inode that uses the legacy block map.
///
/// Returns the complete file data, truncated to the inode's size.
/// Holes (unmapped blocks) are filled with zeros.
pub fn read_block_map_data<D: BlockDevice>(
    fs: &Ext4Fs<D>,
    inode: &Inode,
) -> Result<Vec<u8>, Ext4Error> {
    let file_size = inode.size();
    if file_size == 0 {
        return Ok(Vec::new());
    }

    let block_size = fs.sb.block_size();
    let total_blocks = (file_size + block_size - 1) / block_size;

    log::debug!(
        "[ext4::block_map] reading {} bytes ({} blocks) via indirect block map",
        file_size,
        total_blocks
    );

    let mut data = Vec::with_capacity(file_size as usize);

    for logical_block in 0..total_blocks {
        match read_block_map(fs, inode, logical_block)? {
            Some(phys) => {
                let block_data = fs.read_block(phys)?;
                data.extend_from_slice(&block_data);
            }
            None => {
                // Hole -- fill with zeros
                let zeros = vec![0u8; block_size as usize];
                data.extend_from_slice(&zeros);
            }
        }
    }

    data.truncate(file_size as usize);
    log::debug!(
        "[ext4::block_map] read {} bytes via block map",
        data.len()
    );
    Ok(data)
}
