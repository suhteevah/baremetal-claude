//! NVMe submission and completion queues.
//!
//! Each queue pair consists of a Submission Queue (SQ) where the host posts
//! commands and a Completion Queue (CQ) where the controller posts results.
//! Queue memory must be physically contiguous and page-aligned.
//!
//! The phase bit in CQEs flips each time the controller wraps around the CQ,
//! allowing the host to distinguish new completions from stale entries without
//! an explicit interrupt.

use alloc::vec;
use alloc::vec::Vec;
use core::ptr;

use crate::registers::NvmeRegisters;

/// Size of a Submission Queue Entry in bytes (NVMe spec: 64 bytes).
pub const SQE_SIZE: usize = 64;

/// Size of a Completion Queue Entry in bytes (NVMe spec: 16 bytes).
pub const CQE_SIZE: usize = 16;

/// Default number of entries per queue (admin and I/O).
pub const DEFAULT_QUEUE_DEPTH: u16 = 64;

// ============================================================================
// Submission Queue Entry (64 bytes)
// ============================================================================

/// NVMe Submission Queue Entry — 64 bytes, laid out per NVMe 1.4 Figure 104.
///
/// ```text
/// Bytes   Field
/// 03:00   CDW0   — Opcode (7:0), FUSE (9:8), PSDT (15:14), CID (31:16)
/// 07:04   NSID   — Namespace Identifier
/// 15:08   CDW2-3 — Command-specific (reserved in many commands)
/// 23:16   MPTR   — Metadata Pointer
/// 31:24   PRP1   — PRP Entry 1 / SGL Segment
/// 39:32   PRP2   — PRP Entry 2 / SGL Last Segment
/// 43:40   CDW10
/// 47:44   CDW11
/// 51:48   CDW12
/// 55:52   CDW13
/// 59:56   CDW14
/// 63:60   CDW15
/// ```
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct SubmissionQueueEntry {
    /// Command Dword 0: opcode, fused op, PRP/SGL, command ID
    pub cdw0: u32,
    /// Namespace Identifier
    pub nsid: u32,
    /// Command Dwords 2-3 (reserved / command-specific)
    pub cdw2: u32,
    pub cdw3: u32,
    /// Metadata Pointer
    pub mptr: u64,
    /// PRP Entry 1 (data pointer)
    pub prp1: u64,
    /// PRP Entry 2 (data pointer or PRP list pointer)
    pub prp2: u64,
    /// Command Dwords 10-15 (command-specific)
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

impl SubmissionQueueEntry {
    /// Create a zeroed SQE.
    pub const fn zeroed() -> Self {
        Self {
            cdw0: 0,
            nsid: 0,
            cdw2: 0,
            cdw3: 0,
            mptr: 0,
            prp1: 0,
            prp2: 0,
            cdw10: 0,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    /// Build CDW0 from opcode and command ID.
    ///
    /// FUSE = 00 (normal), PSDT = 00 (PRP).
    pub fn set_opcode_cid(&mut self, opcode: u8, cid: u16) {
        self.cdw0 = (opcode as u32) | ((cid as u32) << 16);
    }
}

// ============================================================================
// Completion Queue Entry (16 bytes)
// ============================================================================

/// NVMe Completion Queue Entry — 16 bytes, per NVMe 1.4 Figure 126.
///
/// ```text
/// Bytes   Field
/// 03:00   DW0    — Command-specific result
/// 07:04   DW1    — Reserved
/// 09:08   SQHD   — SQ Head Pointer (tells host how far SQ was consumed)
/// 11:10   SQID   — SQ Identifier
/// 13:12   CID    — Command Identifier (matches SQE.CDW0.CID)
/// 15:14   Status — Phase (bit 0), Status Code (bits 8:1), SCT (bits 11:9),
///                   More (bit 14), Do Not Retry (bit 15)
/// ```
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct CompletionQueueEntry {
    /// Command-specific result (Dword 0)
    pub dw0: u32,
    /// Reserved (Dword 1)
    pub dw1: u32,
    /// SQ Head Pointer (bits 15:0) and SQ Identifier (bits 31:16)
    pub sqhd_sqid: u32,
    /// Command ID (bits 15:0) and Status field (bits 31:16)
    pub cid_status: u32,
}

impl CompletionQueueEntry {
    /// Create a zeroed CQE.
    pub const fn zeroed() -> Self {
        Self {
            dw0: 0,
            dw1: 0,
            sqhd_sqid: 0,
            cid_status: 0,
        }
    }

    /// Extract the SQ Head Pointer.
    pub fn sqhd(&self) -> u16 {
        self.sqhd_sqid as u16
    }

    /// Extract the SQ Identifier.
    pub fn sqid(&self) -> u16 {
        (self.sqhd_sqid >> 16) as u16
    }

    /// Extract the Command Identifier.
    pub fn cid(&self) -> u16 {
        self.cid_status as u16
    }

    /// Extract the full 16-bit status field (includes phase bit).
    pub fn status_raw(&self) -> u16 {
        (self.cid_status >> 16) as u16
    }

    /// Extract the Phase bit (bit 0 of the status field).
    pub fn phase(&self) -> bool {
        self.status_raw() & 1 != 0
    }

    /// Extract the Status Code (bits 8:1 of the status field).
    pub fn status_code(&self) -> u8 {
        ((self.status_raw() >> 1) & 0xFF) as u8
    }

    /// Extract the Status Code Type (bits 11:9 of the status field).
    pub fn status_code_type(&self) -> u8 {
        ((self.status_raw() >> 9) & 0x7) as u8
    }

    /// Check if the More bit is set (bit 14).
    pub fn more(&self) -> bool {
        self.status_raw() & (1 << 14) != 0
    }

    /// Check if the Do Not Retry bit is set (bit 15).
    pub fn do_not_retry(&self) -> bool {
        self.status_raw() & (1 << 15) != 0
    }

    /// Returns true if the completion indicates success (SC=0, SCT=0).
    pub fn is_success(&self) -> bool {
        self.status_code() == 0 && self.status_code_type() == 0
    }
}

// ============================================================================
// NVMe Status Code Types (SCT)
// ============================================================================

/// Generic Command Status
pub const SCT_GENERIC: u8 = 0x0;
/// Command Specific Status
pub const SCT_COMMAND_SPECIFIC: u8 = 0x1;
/// Media and Data Integrity Errors
pub const SCT_MEDIA: u8 = 0x2;
/// Path Related Status
pub const SCT_PATH: u8 = 0x3;
/// Vendor Specific
pub const SCT_VENDOR: u8 = 0x7;

// ============================================================================
// Generic Status Codes (SCT=0)
// ============================================================================

pub const SC_SUCCESS: u8 = 0x00;
pub const SC_INVALID_OPCODE: u8 = 0x01;
pub const SC_INVALID_FIELD: u8 = 0x02;
pub const SC_CMD_ID_CONFLICT: u8 = 0x03;
pub const SC_DATA_XFER_ERROR: u8 = 0x04;
pub const SC_POWER_LOSS_ABORT: u8 = 0x05;
pub const SC_INTERNAL_ERROR: u8 = 0x06;
pub const SC_CMD_ABORT_REQ: u8 = 0x07;
pub const SC_CMD_ABORT_SQ_DEL: u8 = 0x08;
pub const SC_CMD_ABORT_FUSE_FAIL: u8 = 0x09;
pub const SC_CMD_ABORT_FUSE_MISS: u8 = 0x0A;
pub const SC_INVALID_NS_OR_FMT: u8 = 0x0B;
pub const SC_CMD_SEQ_ERROR: u8 = 0x0C;
pub const SC_INVALID_SGL_SEG: u8 = 0x0D;
pub const SC_INVALID_SGL_COUNT: u8 = 0x0E;
pub const SC_INVALID_DATA_SGL_LEN: u8 = 0x0F;
pub const SC_INVALID_META_SGL_LEN: u8 = 0x10;
pub const SC_INVALID_SGL_TYPE: u8 = 0x11;
pub const SC_LBA_OUT_OF_RANGE: u8 = 0x80;
pub const SC_CAPACITY_EXCEEDED: u8 = 0x81;
pub const SC_NS_NOT_READY: u8 = 0x82;

// ============================================================================
// QueuePair — paired SQ + CQ with doorbell management
// ============================================================================

/// A paired Submission Queue and Completion Queue.
///
/// Manages the SQ tail pointer, CQ head pointer, and phase bit.
/// All queue memory is heap-allocated (page-aligned via overallocation).
pub struct QueuePair {
    /// Queue identifier (0 = admin, 1+ = I/O).
    pub qid: u16,
    /// Number of entries in each queue.
    pub depth: u16,
    /// Submission queue entry buffer (page-aligned allocation).
    sq_entries: Vec<SubmissionQueueEntry>,
    /// Completion queue entry buffer (page-aligned allocation).
    cq_entries: Vec<CompletionQueueEntry>,
    /// Current SQ tail index (next slot to write).
    sq_tail: u16,
    /// Current CQ head index (next slot to read).
    cq_head: u16,
    /// Expected phase bit for the next CQE.
    phase: bool,
    /// Monotonic command ID counter.
    next_cid: u16,
}

impl QueuePair {
    /// Allocate a new queue pair with the given depth.
    ///
    /// Memory is allocated on the heap. For bare-metal use, the heap must be
    /// backed by identity-mapped physical memory so that virtual addresses
    /// can be used as physical addresses for DMA.
    pub fn new(qid: u16, depth: u16) -> Self {
        log::info!(
            "[nvme:queue] allocating queue pair QID={} depth={}",
            qid,
            depth
        );

        let sq_entries = vec![SubmissionQueueEntry::zeroed(); depth as usize];
        let cq_entries = vec![CompletionQueueEntry::zeroed(); depth as usize];

        log::debug!(
            "[nvme:queue] QID={} SQ base={:#x} CQ base={:#x}",
            qid,
            sq_entries.as_ptr() as usize,
            cq_entries.as_ptr() as usize,
        );

        Self {
            qid,
            depth,
            sq_entries,
            cq_entries,
            sq_tail: 0,
            cq_head: 0,
            phase: true, // initial phase is 1
            next_cid: 0,
        }
    }

    /// Physical address of the Submission Queue (for register programming).
    pub fn sq_phys_addr(&self) -> u64 {
        self.sq_entries.as_ptr() as u64
    }

    /// Physical address of the Completion Queue (for register programming).
    pub fn cq_phys_addr(&self) -> u64 {
        self.cq_entries.as_ptr() as u64
    }

    /// Allocate the next command ID.
    pub fn alloc_cid(&mut self) -> u16 {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1);
        cid
    }

    /// Submit a command to the Submission Queue and ring the doorbell.
    ///
    /// Returns the command ID assigned to this submission.
    pub fn submit(&mut self, mut sqe: SubmissionQueueEntry, regs: &NvmeRegisters) -> u16 {
        let cid = self.alloc_cid();
        // Encode CID into CDW0 (preserve opcode in bits 7:0)
        sqe.cdw0 = (sqe.cdw0 & 0xFFFF) | ((cid as u32) << 16);

        let idx = self.sq_tail as usize;
        log::trace!(
            "[nvme:queue] QID={} submit SQE at index {} cid={} opcode={:#04x}",
            self.qid,
            idx,
            cid,
            sqe.cdw0 & 0xFF
        );

        // Write the SQE into the ring buffer
        unsafe {
            ptr::write_volatile(
                &mut self.sq_entries[idx] as *mut SubmissionQueueEntry,
                sqe,
            );
        }

        // Advance tail with wrap
        self.sq_tail = (self.sq_tail + 1) % self.depth;

        // Ring the doorbell
        regs.write_sq_tail_doorbell(self.qid, self.sq_tail);

        cid
    }

    /// Poll the Completion Queue for a completion matching the given CID.
    ///
    /// This spins until a matching CQE appears (phase bit matches expected).
    /// Returns the CQE on success.
    ///
    /// `max_spins`: Maximum number of poll iterations before giving up.
    /// Returns `None` if the timeout is exceeded.
    pub fn poll_completion(
        &mut self,
        expected_cid: u16,
        regs: &NvmeRegisters,
        max_spins: u32,
    ) -> Option<CompletionQueueEntry> {
        log::trace!(
            "[nvme:queue] QID={} polling for CID={} (max_spins={})",
            self.qid,
            expected_cid,
            max_spins
        );

        for spin in 0..max_spins {
            let idx = self.cq_head as usize;

            // Read the CQE with volatile to see controller writes
            let cqe = unsafe {
                ptr::read_volatile(&self.cq_entries[idx] as *const CompletionQueueEntry)
            };

            // Check if the phase bit matches our expected phase
            if cqe.phase() == self.phase {
                log::trace!(
                    "[nvme:queue] QID={} got CQE at index {} after {} spins: cid={} status={:#06x}",
                    self.qid,
                    idx,
                    spin,
                    cqe.cid(),
                    cqe.status_raw()
                );

                // Advance CQ head
                self.cq_head = (self.cq_head + 1) % self.depth;
                if self.cq_head == 0 {
                    // Wrapped around — flip expected phase
                    self.phase = !self.phase;
                    log::trace!(
                        "[nvme:queue] QID={} CQ wrapped, phase now {}",
                        self.qid,
                        self.phase as u8
                    );
                }

                // Signal the controller that we consumed this CQE
                regs.write_cq_head_doorbell(self.qid, self.cq_head);

                if cqe.cid() == expected_cid {
                    return Some(cqe);
                } else {
                    log::warn!(
                        "[nvme:queue] QID={} unexpected CID {} (wanted {}), discarding",
                        self.qid,
                        cqe.cid(),
                        expected_cid
                    );
                    // Continue polling — might get ours next
                }
            }

            // Spin hint for the CPU
            core::hint::spin_loop();
        }

        log::error!(
            "[nvme:queue] QID={} timeout waiting for CID={} after {} spins",
            self.qid,
            expected_cid,
            max_spins
        );
        None
    }
}
