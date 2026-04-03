//! ext4 journal (JBD2) support.
//!
//! The ext4 journal is stored in inode 8 (or the inode specified by
//! `s_journal_inum` in the superblock). It uses the JBD2 format:
//!
//! - **Journal superblock** at block 0 of the journal: contains magic, blocktype,
//!   sequence number, block count, and journal geometry.
//! - **Transactions** consist of:
//!   - Descriptor block (lists which filesystem blocks are journaled)
//!   - Data blocks (copies of the filesystem blocks)
//!   - Commit block (marks the transaction as complete)
//!
//! On mount, if `INCOMPAT_RECOVER` is set in the filesystem superblock, we must
//! replay committed transactions from the journal before the filesystem can be
//! used safely.
//!
//! Reference: <https://ext4.wiki.kernel.org/index.php/Ext4_Disk_Layout#Journal_(jbd2)>

use alloc::vec::Vec;
use core::fmt;

/// JBD2 journal magic number.
///
/// All JBD2 metadata blocks (superblock, descriptor, commit, revoke) begin
/// with this 4-byte big-endian magic value. It is used to distinguish
/// journal metadata from raw data blocks during log scanning.
///
/// Value: `0xC03B3998` (no ASCII meaning; chosen to be unlikely in user data).
pub const JBD2_MAGIC: u32 = 0xC03B3998;

/// Journal superblock (v2) block type.
pub const JBD2_SUPERBLOCK_V2: u32 = 4;
/// Journal superblock (v1) block type.
pub const JBD2_SUPERBLOCK_V1: u32 = 3;
/// Descriptor block type.
pub const JBD2_DESCRIPTOR_BLOCK: u32 = 1;
/// Commit block type.
pub const JBD2_COMMIT_BLOCK: u32 = 2;
/// Revocation block type.
pub const JBD2_REVOKE_BLOCK: u32 = 5;

/// Default journal inode number.
pub const JOURNAL_INODE: u32 = 8;

/// Journal block tag flag: same UUID as previous.
///
/// When set, the 16-byte UUID field is omitted from this tag because it
/// is the same as the previous tag's UUID. This saves 16 bytes per tag
/// in the common single-filesystem case.
pub const JBD2_FLAG_SAME_UUID: u32 = 0x02;

/// Journal block tag flag: last tag in descriptor block.
///
/// Marks the final tag in the descriptor block's tag array. The scanner
/// must stop parsing tags when it encounters a tag with this flag set.
pub const JBD2_FLAG_LAST_TAG: u32 = 0x08;

/// Journal block tag flag: block is escaped (magic number replaced).
///
/// If a journaled data block happens to start with the JBD2_MAGIC bytes,
/// those bytes are zeroed in the journal copy to avoid confusing the scanner.
/// On replay, the original magic bytes must be restored before writing the
/// block to its final filesystem location.
pub const JBD2_FLAG_ESCAPE: u32 = 0x01;

/// Parsed JBD2 journal superblock.
#[derive(Clone)]
pub struct JournalSuperblock {
    /// Magic number, must be JBD2_MAGIC.
    pub magic: u32,
    /// Block type (JBD2_SUPERBLOCK_V1 or JBD2_SUPERBLOCK_V2).
    pub blocktype: u32,
    /// Sequence number of the first transaction in the log.
    pub sequence: u32,
    /// Total number of blocks in the journal.
    pub blocksize: u32,
    /// Maximum number of blocks in the journal.
    pub maxlen: u32,
    /// First block of log information (after the journal superblock).
    pub first: u32,
    /// First commit ID expected in the log.
    pub first_commit_id: u32,
    /// Block number of the start of the log.
    pub log_start: u32,
    /// Error value from the journal.
    pub errno: u32,
    /// Compatible feature flags.
    pub feature_compat: u32,
    /// Incompatible feature flags.
    pub feature_incompat: u32,
    /// Read-only compatible feature flags.
    pub feature_ro_compat: u32,
    /// Journal UUID (16 bytes).
    pub uuid: [u8; 16],
    /// Number of filesystems sharing this journal.
    pub nr_users: u32,
}

impl JournalSuperblock {
    /// Parse a journal superblock from a block-sized buffer.
    ///
    /// JBD2 stores all multi-byte fields in big-endian format.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 1024 {
            log::error!("[ext4::journal] buffer too small for journal superblock: {}", buf.len());
            return None;
        }

        let magic = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != JBD2_MAGIC {
            log::error!(
                "[ext4::journal] invalid journal magic: 0x{:08X} (expected 0x{:08X})",
                magic, JBD2_MAGIC
            );
            return None;
        }

        let blocktype = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if blocktype != JBD2_SUPERBLOCK_V1 && blocktype != JBD2_SUPERBLOCK_V2 {
            log::error!(
                "[ext4::journal] unexpected journal superblock type: {}",
                blocktype
            );
            return None;
        }

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&buf[48..64]);

        let jsb = JournalSuperblock {
            magic,
            blocktype,
            sequence: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
            blocksize: u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]),
            maxlen: u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]),
            first: u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]),
            first_commit_id: u32::from_be_bytes([buf[24], buf[25], buf[26], buf[27]]),
            log_start: u32::from_be_bytes([buf[28], buf[29], buf[30], buf[31]]),
            errno: u32::from_be_bytes([buf[32], buf[33], buf[34], buf[35]]),
            feature_compat: u32::from_be_bytes([buf[36], buf[37], buf[38], buf[39]]),
            feature_incompat: u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]),
            feature_ro_compat: u32::from_be_bytes([buf[44], buf[45], buf[46], buf[47]]),
            uuid,
            nr_users: u32::from_be_bytes([buf[64], buf[65], buf[66], buf[67]]),
        };

        log::info!(
            "[ext4::journal] journal superblock: type={}, seq={}, blocksize={}, maxlen={}, first={}, log_start={}",
            jsb.blocktype, jsb.sequence, jsb.blocksize, jsb.maxlen, jsb.first, jsb.log_start
        );

        Some(jsb)
    }

    /// Whether the journal needs recovery (log_start != 0).
    #[inline]
    pub fn needs_recovery(&self) -> bool {
        self.log_start != 0
    }
}

impl fmt::Debug for JournalSuperblock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalSuperblock")
            .field("magic", &format_args!("0x{:08X}", self.magic))
            .field("blocktype", &self.blocktype)
            .field("sequence", &self.sequence)
            .field("blocksize", &self.blocksize)
            .field("maxlen", &self.maxlen)
            .field("log_start", &self.log_start)
            .field("needs_recovery", &self.needs_recovery())
            .finish()
    }
}

/// Parsed journal block header (common to descriptor, commit, and revoke blocks).
#[derive(Clone, Debug)]
pub struct JournalBlockHeader {
    /// Magic number (must be JBD2_MAGIC).
    pub magic: u32,
    /// Block type (descriptor, commit, revoke).
    pub blocktype: u32,
    /// Transaction sequence number.
    pub sequence: u32,
}

impl JournalBlockHeader {
    /// Parse a journal block header from the first 12 bytes.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 12 {
            return None;
        }
        let magic = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != JBD2_MAGIC {
            return None;
        }
        Some(JournalBlockHeader {
            magic,
            blocktype: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            sequence: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
        })
    }
}

/// A journal block tag in a descriptor block.
///
/// Tells us which filesystem block a journaled data block maps to.
#[derive(Clone, Debug)]
pub struct JournalBlockTag {
    /// Filesystem block number (low 32 bits).
    pub blocknr: u32,
    /// Flags (JBD2_FLAG_ESCAPE, JBD2_FLAG_SAME_UUID, JBD2_FLAG_LAST_TAG).
    pub flags: u32,
    /// Filesystem block number (high 32 bits), present if 64-bit journal.
    pub blocknr_hi: u32,
}

impl JournalBlockTag {
    /// Full 48-bit/64-bit block number.
    #[inline]
    pub fn block_number(&self) -> u64 {
        self.blocknr as u64 | ((self.blocknr_hi as u64) << 32)
    }

    /// Whether this is the last tag in the descriptor block.
    #[inline]
    pub fn is_last(&self) -> bool {
        self.flags & JBD2_FLAG_LAST_TAG != 0
    }

    /// Whether the data block was escaped (had the JBD2 magic replaced).
    #[inline]
    pub fn is_escaped(&self) -> bool {
        self.flags & JBD2_FLAG_ESCAPE != 0
    }
}

/// Parse journal block tags from a descriptor block.
///
/// Tags start at offset 12 (after the block header). Each tag is:
/// - 4 bytes: block number (low 32)
/// - 4 bytes: flags + checksum (we only use flags; bits 0..15 = flags, bits 16..31 = checksum)
///   Actually for JBD2 v3 tags, the layout is slightly different.
///   Standard JBD2 tag (without INCOMPAT_64BIT):
///     offset 0: blocknr (u32 BE)
///     offset 4: flags (u16 BE) -- only lower 16 bits
///     offset 6: if !SAME_UUID, 16 bytes of UUID follow
///   With INCOMPAT_64BIT set (JBD2_TAG_SIZE = 16):
///     offset 0: blocknr (u32 BE)
///     offset 4: flags (u16 BE)
///     offset 6: blocknr_hi (u32 BE) -- but actually at offset 8
///     Actually the v2/v3 tag format is:
///     - t_blocknr: u32 (BE) at +0
///     - t_checksum: u16 (BE) at +4 (v3 only)
///     - t_flags: u16 (BE) at +6
///     - t_blocknr_high: u32 (BE) at +8 (if 64-bit)
///
/// We parse both 32-bit and 64-bit tag formats.
pub fn parse_descriptor_tags(buf: &[u8], has_64bit: bool, has_csum_v3: bool) -> Vec<JournalBlockTag> {
    let mut tags = Vec::new();
    let header_size = 12; // JBD2 block header
    let tag_size = if has_csum_v3 {
        if has_64bit { 16 } else { 12 }
    } else {
        if has_64bit { 12 } else { 8 }
    };

    let mut offset = header_size;

    loop {
        if offset + tag_size > buf.len() {
            break;
        }

        let blocknr = u32::from_be_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]);

        let (flags, blocknr_hi) = if has_csum_v3 {
            // v3: checksum(u16) at +4, flags(u16) at +6, blocknr_hi(u32) at +8
            let flags = u16::from_be_bytes([buf[offset + 6], buf[offset + 7]]) as u32;
            let hi = if has_64bit {
                u32::from_be_bytes([buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11]])
            } else {
                0
            };
            (flags, hi)
        } else {
            // v1/v2: flags(u16) at +4, blocknr_hi at +8 if 64-bit
            // Actually: t_blocknr(u32) at +0, t_flags(u16) at +4, padding/uuid...
            let flags = u16::from_be_bytes([buf[offset + 4], buf[offset + 5]]) as u32;
            let hi = if has_64bit && offset + 12 <= buf.len() {
                u32::from_be_bytes([buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11]])
            } else {
                0
            };
            (flags, hi)
        };

        let tag = JournalBlockTag {
            blocknr,
            flags,
            blocknr_hi,
        };

        let is_last = tag.is_last();
        tags.push(tag);

        // If not SAME_UUID and not the last, there may be a 16-byte UUID after
        if flags & JBD2_FLAG_SAME_UUID as u32 == 0 && !is_last {
            offset += tag_size + 16; // skip UUID
        } else {
            offset += tag_size;
        }

        if is_last {
            break;
        }
    }

    log::debug!("[ext4::journal] parsed {} tags from descriptor block", tags.len());
    tags
}

/// A single committed transaction, ready to be replayed.
#[derive(Clone, Debug)]
pub struct JournalTransaction {
    /// Transaction sequence number.
    pub sequence: u32,
    /// List of (filesystem_block, journal_data_block_index) pairs.
    /// The data for each filesystem block is in the journal at the given
    /// journal-relative block index.
    pub mappings: Vec<TransactionMapping>,
}

/// A single block mapping within a transaction.
#[derive(Clone, Debug)]
pub struct TransactionMapping {
    /// Target filesystem block number.
    pub fs_block: u64,
    /// Whether the journal data block was escaped (magic number replaced).
    pub escaped: bool,
    /// Index of the data block within the journal (absolute journal block number).
    pub journal_block: u64,
}

/// Scan the journal for committed transactions and return them in order.
///
/// `journal_blocks` is a closure that reads a journal-relative block number.
/// `journal_sb` is the parsed journal superblock.
///
/// This function walks the journal log from `log_start` looking for
/// descriptor+commit pairs. Transactions that have a descriptor but no
/// commit block are considered incomplete and are skipped.
pub fn scan_journal<F>(
    journal_sb: &JournalSuperblock,
    mut read_journal_block: F,
) -> Vec<JournalTransaction>
where
    F: FnMut(u64) -> Option<Vec<u8>>,
{
    let mut transactions = Vec::new();

    if !journal_sb.needs_recovery() {
        log::info!("[ext4::journal] journal is clean, no recovery needed");
        return transactions;
    }

    let maxlen = journal_sb.maxlen as u64;
    let first = journal_sb.first as u64;
    let mut block_idx = journal_sb.log_start as u64;
    let mut expected_seq = journal_sb.sequence;
    // JBD2_FEATURE_INCOMPAT_64BIT (bit 0): block tags include a high-32-bit blocknr field,
    // allowing the journal to address blocks beyond the 2^32 limit.
    let has_64bit = journal_sb.feature_incompat & 0x01 != 0;
    // JBD2_FEATURE_INCOMPAT_CSUM_V3 (bit 4): tags use the v3 format with a per-tag
    // CRC32C checksum field (2 bytes) inserted before the flags field.
    let has_csum_v3 = journal_sb.feature_incompat & 0x10 != 0;

    log::info!(
        "[ext4::journal] scanning journal: start_block={}, expected_seq={}, maxlen={}",
        block_idx, expected_seq, maxlen
    );

    // Limit scan to maxlen blocks to prevent infinite loops
    let mut blocks_scanned = 0u64;
    let scan_limit = maxlen;

    loop {
        if blocks_scanned >= scan_limit {
            log::warn!("[ext4::journal] reached scan limit, stopping");
            break;
        }

        let data = match read_journal_block(block_idx) {
            Some(d) => d,
            None => {
                log::warn!("[ext4::journal] failed to read journal block {}", block_idx);
                break;
            }
        };

        let header = match JournalBlockHeader::from_bytes(&data) {
            Some(h) => h,
            None => {
                log::debug!("[ext4::journal] no valid header at block {}, end of journal", block_idx);
                break;
            }
        };

        if header.sequence != expected_seq {
            log::debug!(
                "[ext4::journal] sequence mismatch at block {}: expected {}, got {}",
                block_idx, expected_seq, header.sequence
            );
            break;
        }

        match header.blocktype {
            JBD2_DESCRIPTOR_BLOCK => {
                log::debug!(
                    "[ext4::journal] descriptor block at journal block {}, seq={}",
                    block_idx, header.sequence
                );

                let tags = parse_descriptor_tags(&data, has_64bit, has_csum_v3);
                let mut mappings = Vec::new();
                let mut data_block = block_idx + 1;

                for tag in &tags {
                    // Wrap around journal
                    if data_block >= maxlen {
                        data_block = first;
                    }

                    mappings.push(TransactionMapping {
                        fs_block: tag.block_number(),
                        escaped: tag.is_escaped(),
                        journal_block: data_block,
                    });
                    data_block += 1;
                    blocks_scanned += 1;
                }

                // Now look for the commit block
                if data_block >= maxlen {
                    data_block = first;
                }

                let commit_data = match read_journal_block(data_block) {
                    Some(d) => d,
                    None => {
                        log::warn!("[ext4::journal] failed to read expected commit block at {}", data_block);
                        break;
                    }
                };

                let commit_header = match JournalBlockHeader::from_bytes(&commit_data) {
                    Some(h) => h,
                    None => {
                        log::warn!("[ext4::journal] no valid header at expected commit block {}", data_block);
                        break;
                    }
                };

                if commit_header.blocktype == JBD2_COMMIT_BLOCK
                    && commit_header.sequence == expected_seq
                {
                    log::info!(
                        "[ext4::journal] committed transaction seq={} with {} block mappings",
                        expected_seq, mappings.len()
                    );
                    transactions.push(JournalTransaction {
                        sequence: expected_seq,
                        mappings,
                    });
                    block_idx = data_block + 1;
                    if block_idx >= maxlen {
                        block_idx = first;
                    }
                    expected_seq = expected_seq.wrapping_add(1);
                    blocks_scanned += 2; // descriptor + commit
                } else {
                    log::warn!(
                        "[ext4::journal] incomplete transaction seq={}: expected commit at block {}, got type={}",
                        expected_seq, data_block, commit_header.blocktype
                    );
                    break;
                }
            }
            JBD2_REVOKE_BLOCK => {
                // Revoke blocks mark filesystem blocks that should NOT be replayed
                // from earlier transactions. They exist to prevent stale data from
                // an older transaction from overwriting newer data written directly
                // to the filesystem (bypassing the journal). A full implementation
                // would collect revoked block numbers and skip them during replay.
                // For simplicity, we skip revoke blocks entirely here.
                log::debug!(
                    "[ext4::journal] revoke block at journal block {}, seq={}",
                    block_idx, header.sequence
                );
                block_idx += 1;
                if block_idx >= maxlen {
                    block_idx = first;
                }
                blocks_scanned += 1;
            }
            JBD2_COMMIT_BLOCK => {
                // Stray commit without descriptor -- skip
                log::debug!(
                    "[ext4::journal] stray commit block at journal block {}, seq={}",
                    block_idx, header.sequence
                );
                block_idx += 1;
                if block_idx >= maxlen {
                    block_idx = first;
                }
                expected_seq = expected_seq.wrapping_add(1);
                blocks_scanned += 1;
            }
            _ => {
                log::warn!(
                    "[ext4::journal] unknown block type {} at journal block {}",
                    header.blocktype, block_idx
                );
                break;
            }
        }
    }

    log::info!(
        "[ext4::journal] scan complete: found {} committed transactions",
        transactions.len()
    );
    transactions
}
