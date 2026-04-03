//! Simplified NTFS journal ($LogFile) support.
//!
//! This module implements a basic redo/undo transaction log for NTFS write
//! operations. Before any write, the operation is logged with old data (undo)
//! and new data (redo), along with the target offset on disk.
//!
//! On mount, the journal is checked for uncommitted entries and can replay
//! (redo) or rollback (undo) as needed.
//!
//! This is a simplified version of the real NTFS $LogFile format — sufficient
//! for correctness but not binary-compatible with Windows' full implementation.

use alloc::vec;
use alloc::vec::Vec;

/// Log Sequence Number — monotonically increasing identifier for journal entries.
pub type Lsn = u64;

/// Journal operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JournalOp {
    /// Write data to MFT entry.
    MftWrite = 1,
    /// Write data to cluster(s).
    ClusterWrite = 2,
    /// Update volume bitmap (cluster allocation).
    BitmapUpdate = 3,
    /// Update MFT bitmap (entry allocation).
    MftBitmapUpdate = 4,
    /// Index insertion into a directory.
    IndexInsert = 5,
    /// Compound transaction commit marker.
    Commit = 0xFE,
    /// Checkpoint (all prior entries are durable).
    Checkpoint = 0xFF,
}

impl JournalOp {
    /// Convert from raw byte.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(Self::MftWrite),
            2 => Some(Self::ClusterWrite),
            3 => Some(Self::BitmapUpdate),
            4 => Some(Self::MftBitmapUpdate),
            5 => Some(Self::IndexInsert),
            0xFE => Some(Self::Commit),
            0xFF => Some(Self::Checkpoint),
            _ => None,
        }
    }
}

/// A single journal entry describing one atomic write operation.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    /// Log Sequence Number for this entry.
    pub lsn: Lsn,
    /// Transaction ID grouping related entries.
    pub transaction_id: u64,
    /// Operation type.
    pub op: JournalOp,
    /// Target byte offset on disk.
    pub target_offset: u64,
    /// Length of the data being written.
    pub data_length: u32,
    /// Redo data (new data to write for replay).
    pub redo_data: Vec<u8>,
    /// Undo data (old data to restore for rollback).
    pub undo_data: Vec<u8>,
    /// Whether this entry has been committed (part of a completed transaction).
    pub committed: bool,
}

/// Entry header size on disk.
///
/// Layout (40 bytes total):
///   Offset 0:  LSN (u64 LE) -- Log Sequence Number
///   Offset 8:  transaction_id (u64 LE) -- groups related entries
///   Offset 16: op (u8) -- operation type (see JournalOp)
///   Offset 17: committed (u8) -- 0 = uncommitted, 1 = committed
///   Offset 18: padding (2 bytes)
///   Offset 20: target_offset (u64 LE) -- byte offset on disk
///   Offset 28: data_length (u32 LE) -- length of write data
///   Offset 32: redo_len (u32 LE) -- bytes of redo data following header
///   Offset 36: undo_len (u32 LE) -- bytes of undo data following redo
///
/// After the header: redo_data[redo_len] followed by undo_data[undo_len].
const JOURNAL_ENTRY_HEADER_SIZE: usize = 40;

/// Magic for the journal header block: ASCII "JRNL" (0x4A=J, 0x52=R, 0x4E=N, 0x4C=L).
/// Used to identify a valid ClaudioOS NTFS journal region on disk.
const JOURNAL_MAGIC: u32 = 0x4A524E4C;

/// Journal header (first 64 bytes of the journal area).
#[derive(Debug, Clone)]
pub struct JournalHeader {
    /// Magic identifier.
    pub magic: u32,
    /// Current (next) LSN to assign.
    pub current_lsn: Lsn,
    /// LSN of the last checkpoint.
    pub checkpoint_lsn: Lsn,
    /// Whether the journal was cleanly closed.
    pub clean_shutdown: bool,
    /// Number of entries in the journal.
    pub entry_count: u32,
}

const JOURNAL_HEADER_SIZE: usize = 64;

impl JournalHeader {
    /// Parse from bytes.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < JOURNAL_HEADER_SIZE {
            return None;
        }
        let magic = read_u32(buf, 0);
        if magic != JOURNAL_MAGIC {
            log::warn!("[ntfs::journal] invalid journal magic: 0x{:08X}", magic);
            return None;
        }
        Some(JournalHeader {
            magic,
            current_lsn: read_u64(buf, 4),
            checkpoint_lsn: read_u64(buf, 12),
            clean_shutdown: buf[20] != 0,
            entry_count: read_u32(buf, 24),
        })
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = vec![0u8; JOURNAL_HEADER_SIZE];
        write_u32(&mut buf, 0, self.magic);
        write_u64(&mut buf, 4, self.current_lsn);
        write_u64(&mut buf, 12, self.checkpoint_lsn);
        buf[20] = if self.clean_shutdown { 1 } else { 0 };
        write_u32(&mut buf, 24, self.entry_count);
        buf
    }

    /// Create a fresh journal header.
    pub fn new() -> Self {
        JournalHeader {
            magic: JOURNAL_MAGIC,
            current_lsn: 1,
            checkpoint_lsn: 0,
            clean_shutdown: true,
            entry_count: 0,
        }
    }
}

impl JournalEntry {
    /// Serialize a journal entry to bytes for writing to the journal area.
    pub fn to_bytes(&self) -> Vec<u8> {
        let redo_len = self.redo_data.len() as u32;
        let undo_len = self.undo_data.len() as u32;
        let total = JOURNAL_ENTRY_HEADER_SIZE + redo_len as usize + undo_len as usize;
        let mut buf = vec![0u8; total];

        write_u64(&mut buf, 0, self.lsn);
        write_u64(&mut buf, 8, self.transaction_id);
        buf[16] = self.op as u8;
        buf[17] = if self.committed { 1 } else { 0 };
        // 18..20 padding
        write_u64(&mut buf, 20, self.target_offset);
        write_u32(&mut buf, 28, self.data_length);
        write_u32(&mut buf, 32, redo_len);
        write_u32(&mut buf, 36, undo_len);

        buf[JOURNAL_ENTRY_HEADER_SIZE..JOURNAL_ENTRY_HEADER_SIZE + redo_len as usize]
            .copy_from_slice(&self.redo_data);
        buf[JOURNAL_ENTRY_HEADER_SIZE + redo_len as usize..]
            .copy_from_slice(&self.undo_data);

        buf
    }

    /// Parse a journal entry from bytes.
    pub fn from_bytes(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < JOURNAL_ENTRY_HEADER_SIZE {
            return None;
        }

        let lsn = read_u64(buf, 0);
        let transaction_id = read_u64(buf, 8);
        let op = JournalOp::from_u8(buf[16])?;
        let committed = buf[17] != 0;
        let target_offset = read_u64(buf, 20);
        let data_length = read_u32(buf, 28);
        let redo_len = read_u32(buf, 32) as usize;
        let undo_len = read_u32(buf, 36) as usize;

        let total = JOURNAL_ENTRY_HEADER_SIZE + redo_len + undo_len;
        if buf.len() < total {
            log::error!("[ntfs::journal] entry truncated: need {} bytes, have {}", total, buf.len());
            return None;
        }

        let redo_data = buf[JOURNAL_ENTRY_HEADER_SIZE..JOURNAL_ENTRY_HEADER_SIZE + redo_len].to_vec();
        let undo_data = buf[JOURNAL_ENTRY_HEADER_SIZE + redo_len..total].to_vec();

        Some((JournalEntry {
            lsn,
            transaction_id,
            op,
            target_offset,
            data_length,
            redo_data,
            undo_data,
            committed,
        }, total))
    }
}

/// In-memory journal that buffers entries before flushing.
///
/// The journal is stored as a contiguous region on disk (in the $LogFile area
/// or a reserved region). For our simplified implementation, we keep entries
/// in memory and flush them to a designated area.
#[derive(Debug)]
pub struct Journal {
    /// The journal header.
    pub header: JournalHeader,
    /// Buffered journal entries (unflushed).
    pub entries: Vec<JournalEntry>,
    /// Current transaction ID.
    pub current_transaction: u64,
    /// Next transaction ID to assign.
    next_transaction_id: u64,
    /// Byte offset on disk where the journal is stored.
    pub journal_offset: u64,
    /// Maximum journal size in bytes.
    pub journal_capacity: u64,
}

impl Journal {
    /// Create a new empty journal.
    pub fn new(journal_offset: u64, journal_capacity: u64) -> Self {
        Journal {
            header: JournalHeader::new(),
            entries: Vec::new(),
            current_transaction: 1,
            next_transaction_id: 2,
            journal_offset,
            journal_capacity,
        }
    }

    /// Load an existing journal from disk bytes.
    pub fn from_bytes(buf: &[u8], journal_offset: u64, journal_capacity: u64) -> Option<Self> {
        let header = JournalHeader::from_bytes(buf)?;
        let mut entries = Vec::new();
        let mut pos = JOURNAL_HEADER_SIZE;

        for _ in 0..header.entry_count {
            if pos >= buf.len() {
                break;
            }
            match JournalEntry::from_bytes(&buf[pos..]) {
                Some((entry, consumed)) => {
                    entries.push(entry);
                    pos += consumed;
                }
                None => break,
            }
        }

        log::info!("[ntfs::journal] loaded journal: lsn={}, entries={}, clean={}",
            header.current_lsn, entries.len(), header.clean_shutdown);

        let next_txn = entries.iter().map(|e| e.transaction_id).max().unwrap_or(0) + 1;

        Some(Journal {
            header,
            entries,
            current_transaction: next_txn,
            next_transaction_id: next_txn + 1,
            journal_offset,
            journal_capacity,
        })
    }

    /// Serialize the entire journal (header + entries) to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = self.header.to_bytes();
        for entry in &self.entries {
            buf.extend_from_slice(&entry.to_bytes());
        }
        buf
    }

    /// Begin a new transaction. Returns the transaction ID.
    pub fn begin_transaction(&mut self) -> u64 {
        let txn_id = self.next_transaction_id;
        self.next_transaction_id += 1;
        self.current_transaction = txn_id;
        log::debug!("[ntfs::journal] begin transaction {}", txn_id);
        txn_id
    }

    /// Log a write operation. Call this BEFORE performing the actual write.
    ///
    /// `old_data` is read from disk before the write (for undo).
    /// `new_data` is what will be written (for redo).
    pub fn log_write(
        &mut self,
        op: JournalOp,
        target_offset: u64,
        old_data: &[u8],
        new_data: &[u8],
    ) -> Lsn {
        let lsn = self.header.current_lsn;
        self.header.current_lsn += 1;
        self.header.entry_count += 1;

        let entry = JournalEntry {
            lsn,
            transaction_id: self.current_transaction,
            op,
            target_offset,
            data_length: new_data.len() as u32,
            redo_data: new_data.to_vec(),
            undo_data: old_data.to_vec(),
            committed: false,
        };

        log::trace!("[ntfs::journal] log_write: lsn={}, txn={}, op={:?}, offset=0x{:X}, len={}",
            lsn, self.current_transaction, op, target_offset, new_data.len());

        self.entries.push(entry);
        lsn
    }

    /// Mark all entries in the current transaction as committed.
    pub fn commit_transaction(&mut self, txn_id: u64) {
        for entry in &mut self.entries {
            if entry.transaction_id == txn_id {
                entry.committed = true;
            }
        }

        // Add a commit marker
        let lsn = self.header.current_lsn;
        self.header.current_lsn += 1;
        self.header.entry_count += 1;

        self.entries.push(JournalEntry {
            lsn,
            transaction_id: txn_id,
            op: JournalOp::Commit,
            target_offset: 0,
            data_length: 0,
            redo_data: Vec::new(),
            undo_data: Vec::new(),
            committed: true,
        });

        log::debug!("[ntfs::journal] committed transaction {} at lsn={}", txn_id, lsn);
    }

    /// Get uncommitted entries (for rollback on mount after crash).
    pub fn uncommitted_entries(&self) -> Vec<&JournalEntry> {
        // Find transactions that have entries but no commit marker
        let committed_txns: Vec<u64> = self.entries.iter()
            .filter(|e| e.op == JournalOp::Commit)
            .map(|e| e.transaction_id)
            .collect();

        self.entries.iter()
            .filter(|e| !committed_txns.contains(&e.transaction_id) && e.op != JournalOp::Commit && e.op != JournalOp::Checkpoint)
            .collect()
    }

    /// Get committed but not yet checkpointed entries (for redo replay).
    pub fn entries_after_checkpoint(&self) -> Vec<&JournalEntry> {
        self.entries.iter()
            .filter(|e| e.lsn > self.header.checkpoint_lsn && e.committed)
            .collect()
    }

    /// Set a checkpoint at the current LSN, clearing old entries.
    pub fn checkpoint(&mut self) {
        self.header.checkpoint_lsn = self.header.current_lsn;

        // Remove all entries before the checkpoint
        self.entries.retain(|e| e.lsn >= self.header.checkpoint_lsn);
        self.header.entry_count = self.entries.len() as u32;

        log::info!("[ntfs::journal] checkpoint at lsn={}", self.header.checkpoint_lsn);
    }

    /// Mark the journal as cleanly shut down.
    pub fn mark_clean(&mut self) {
        self.header.clean_shutdown = true;
    }

    /// Mark the journal as dirty (unclean state, e.g., during writes).
    pub fn mark_dirty(&mut self) {
        self.header.clean_shutdown = false;
    }

    /// Get the current LSN (for updating MFT entry headers).
    pub fn current_lsn(&self) -> Lsn {
        self.header.current_lsn
    }
}

// --- Little-endian byte helpers ---

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

#[inline]
fn write_u32(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

#[inline]
fn write_u64(buf: &mut [u8], offset: usize, val: u64) {
    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_entry_roundtrip() {
        let entry = JournalEntry {
            lsn: 42,
            transaction_id: 7,
            op: JournalOp::MftWrite,
            target_offset: 0x1000,
            data_length: 4,
            redo_data: vec![1, 2, 3, 4],
            undo_data: vec![5, 6, 7, 8],
            committed: false,
        };
        let bytes = entry.to_bytes();
        let (parsed, consumed) = JournalEntry::from_bytes(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.lsn, 42);
        assert_eq!(parsed.transaction_id, 7);
        assert_eq!(parsed.op, JournalOp::MftWrite);
        assert_eq!(parsed.target_offset, 0x1000);
        assert_eq!(parsed.redo_data, vec![1, 2, 3, 4]);
        assert_eq!(parsed.undo_data, vec![5, 6, 7, 8]);
    }

    #[test]
    fn test_journal_header_roundtrip() {
        let hdr = JournalHeader {
            magic: JOURNAL_MAGIC,
            current_lsn: 100,
            checkpoint_lsn: 50,
            clean_shutdown: true,
            entry_count: 3,
        };
        let bytes = hdr.to_bytes();
        let parsed = JournalHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.current_lsn, 100);
        assert_eq!(parsed.checkpoint_lsn, 50);
        assert!(parsed.clean_shutdown);
        assert_eq!(parsed.entry_count, 3);
    }

    #[test]
    fn test_journal_transaction_flow() {
        let mut journal = Journal::new(0, 65536);
        let txn = journal.begin_transaction();
        journal.log_write(JournalOp::MftWrite, 0x1000, &[0; 4], &[1, 2, 3, 4]);
        journal.log_write(JournalOp::ClusterWrite, 0x2000, &[0; 8], &[5; 8]);

        // Before commit, entries should be uncommitted
        assert_eq!(journal.uncommitted_entries().len(), 2);

        journal.commit_transaction(txn);

        // After commit, no uncommitted entries
        assert_eq!(journal.uncommitted_entries().len(), 0);
    }

    #[test]
    fn test_journal_full_roundtrip() {
        let mut journal = Journal::new(0, 65536);
        let txn = journal.begin_transaction();
        journal.log_write(JournalOp::MftWrite, 0x1000, &[0xAA; 4], &[0xBB; 4]);
        journal.commit_transaction(txn);

        let bytes = journal.to_bytes();
        let loaded = Journal::from_bytes(&bytes, 0, 65536).unwrap();
        assert_eq!(loaded.entries.len(), 2); // 1 write + 1 commit
        assert_eq!(loaded.entries[0].redo_data, vec![0xBB; 4]);
        assert_eq!(loaded.entries[0].undo_data, vec![0xAA; 4]);
    }
}
