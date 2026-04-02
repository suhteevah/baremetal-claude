//! NVMe I/O commands — Read, Write, Flush, and PRP list management.
//!
//! I/O commands are submitted on I/O queue pairs (QID >= 1). Each command
//! addresses data by namespace ID and logical block address (LBA).
//!
//! For transfers larger than one memory page, a PRP (Physical Region Page)
//! list is constructed in a separate heap-allocated page.

use alloc::vec;
use alloc::vec::Vec;

use crate::queue::{QueuePair, SubmissionQueueEntry};
use crate::registers::NvmeRegisters;

// ============================================================================
// I/O command opcodes (NVMe 1.4 Figure 346)
// ============================================================================

/// Flush
pub const IO_OPC_FLUSH: u8 = 0x00;
/// Write
pub const IO_OPC_WRITE: u8 = 0x01;
/// Read
pub const IO_OPC_READ: u8 = 0x02;
/// Write Uncorrectable
pub const IO_OPC_WRITE_UNCORRECTABLE: u8 = 0x04;
/// Compare
pub const IO_OPC_COMPARE: u8 = 0x05;
/// Write Zeroes
pub const IO_OPC_WRITE_ZEROES: u8 = 0x08;
/// Dataset Management
pub const IO_OPC_DATASET_MANAGEMENT: u8 = 0x09;
/// Reservation Register
pub const IO_OPC_RESERVATION_REGISTER: u8 = 0x0D;
/// Reservation Report
pub const IO_OPC_RESERVATION_REPORT: u8 = 0x0E;
/// Reservation Acquire
pub const IO_OPC_RESERVATION_ACQUIRE: u8 = 0x11;
/// Reservation Release
pub const IO_OPC_RESERVATION_RELEASE: u8 = 0x15;

/// Page size used for PRP calculations (4 KiB).
pub const PAGE_SIZE: usize = 4096;

// ============================================================================
// PRP list construction
// ============================================================================

/// Build a PRP list for a transfer that spans multiple pages.
///
/// NVMe uses Physical Region Pages (PRPs) to describe data buffers:
/// - PRP1: physical address of the first page (may be offset within a page)
/// - PRP2: if the transfer fits in 2 pages, PRP2 is the second page address.
///         if > 2 pages, PRP2 points to a PRP list (page of PRP entries).
///
/// This function returns `(prp1, prp2, prp_list)` where `prp_list` is a
/// heap-allocated Vec that must be kept alive until the command completes.
///
/// `buf_addr`: Physical/virtual address of the data buffer.
/// `len`: Transfer length in bytes.
pub fn build_prp_list(buf_addr: u64, len: usize) -> (u64, u64, Option<Vec<u64>>) {
    let prp1 = buf_addr;

    // How many bytes remain after the first page
    let first_page_offset = (buf_addr as usize) & (PAGE_SIZE - 1);
    let first_page_bytes = if first_page_offset == 0 {
        PAGE_SIZE
    } else {
        PAGE_SIZE - first_page_offset
    };

    if len <= first_page_bytes {
        // Entire transfer fits in one page
        log::trace!(
            "[nvme:io] PRP: single page, prp1={:#x} len={}",
            prp1,
            len
        );
        return (prp1, 0, None);
    }

    let remaining = len - first_page_bytes;
    let second_page_addr = (buf_addr & !(PAGE_SIZE as u64 - 1)) + PAGE_SIZE as u64;

    if remaining <= PAGE_SIZE {
        // Transfer spans exactly 2 pages — PRP2 is the second page address
        log::trace!(
            "[nvme:io] PRP: two pages, prp1={:#x} prp2={:#x} len={}",
            prp1,
            second_page_addr,
            len
        );
        return (prp1, second_page_addr, None);
    }

    // Transfer spans > 2 pages — need a PRP list
    let num_remaining_pages = (remaining + PAGE_SIZE - 1) / PAGE_SIZE;
    log::debug!(
        "[nvme:io] PRP: building PRP list for {} remaining pages ({} bytes)",
        num_remaining_pages,
        remaining
    );

    // Allocate a PRP list (each entry is 8 bytes = u64)
    // A single PRP list page can hold PAGE_SIZE/8 = 512 entries
    let mut prp_list: Vec<u64> = vec![0u64; num_remaining_pages];
    for i in 0..num_remaining_pages {
        prp_list[i] = second_page_addr + (i as u64) * PAGE_SIZE as u64;
        log::trace!(
            "[nvme:io] PRP list[{}] = {:#x}",
            i,
            prp_list[i]
        );
    }

    let prp2 = prp_list.as_ptr() as u64;
    log::debug!(
        "[nvme:io] PRP list at {:#x}, {} entries",
        prp2,
        num_remaining_pages
    );

    (prp1, prp2, Some(prp_list))
}

// ============================================================================
// I/O command submission
// ============================================================================

/// Submit a Read command.
///
/// `io_queue`: The I/O queue pair to submit on.
/// `regs`: Controller registers for doorbell access.
/// `nsid`: Namespace ID.
/// `slba`: Starting LBA.
/// `nlb`: Number of Logical Blocks (0-based, i.e., 0 = 1 block).
/// `buf`: Buffer to read data into (must be large enough for (nlb+1) * sector_size).
///
/// Returns `Ok(())` on success, or the NVMe status code on failure.
pub fn read(
    io_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    nsid: u32,
    slba: u64,
    nlb: u16,
    buf: &mut [u8],
) -> Result<(), (u8, u8)> {
    log::info!(
        "[nvme:io] Read: NSID={} SLBA={} NLB={} buf_len={}",
        nsid,
        slba,
        nlb + 1,
        buf.len()
    );

    let buf_addr = buf.as_mut_ptr() as u64;
    let (prp1, prp2, _prp_list) = build_prp_list(buf_addr, buf.len());

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(IO_OPC_READ, 0);
    sqe.nsid = nsid;
    sqe.prp1 = prp1;
    sqe.prp2 = prp2;
    // CDW10: Starting LBA (low 32 bits)
    sqe.cdw10 = slba as u32;
    // CDW11: Starting LBA (high 32 bits)
    sqe.cdw11 = (slba >> 32) as u32;
    // CDW12: NLB (15:0, 0-based) | other flags
    sqe.cdw12 = nlb as u32;

    let cid = io_queue.submit(sqe, regs);
    log::debug!("[nvme:io] Read submitted, CID={}", cid);

    let cqe = io_queue
        .poll_completion(cid, regs, 5_000_000)
        .ok_or((0xFF, 0xFF))?;

    if !cqe.is_success() {
        log::error!(
            "[nvme:io] Read FAILED: SLBA={} NLB={} SCT={} SC={:#04x}",
            slba,
            nlb + 1,
            cqe.status_code_type(),
            cqe.status_code()
        );
        return Err((cqe.status_code_type(), cqe.status_code()));
    }

    log::debug!(
        "[nvme:io] Read complete: SLBA={} NLB={} OK",
        slba,
        nlb + 1
    );
    // _prp_list is dropped here after the command completes
    Ok(())
}

/// Submit a Write command.
///
/// `io_queue`: The I/O queue pair to submit on.
/// `regs`: Controller registers for doorbell access.
/// `nsid`: Namespace ID.
/// `slba`: Starting LBA.
/// `nlb`: Number of Logical Blocks (0-based, i.e., 0 = 1 block).
/// `buf`: Buffer containing data to write.
///
/// Returns `Ok(())` on success, or the NVMe status code on failure.
pub fn write(
    io_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    nsid: u32,
    slba: u64,
    nlb: u16,
    buf: &[u8],
) -> Result<(), (u8, u8)> {
    log::info!(
        "[nvme:io] Write: NSID={} SLBA={} NLB={} buf_len={}",
        nsid,
        slba,
        nlb + 1,
        buf.len()
    );

    let buf_addr = buf.as_ptr() as u64;
    let (prp1, prp2, _prp_list) = build_prp_list(buf_addr, buf.len());

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(IO_OPC_WRITE, 0);
    sqe.nsid = nsid;
    sqe.prp1 = prp1;
    sqe.prp2 = prp2;
    // CDW10: Starting LBA (low 32 bits)
    sqe.cdw10 = slba as u32;
    // CDW11: Starting LBA (high 32 bits)
    sqe.cdw11 = (slba >> 32) as u32;
    // CDW12: NLB (15:0, 0-based)
    sqe.cdw12 = nlb as u32;

    let cid = io_queue.submit(sqe, regs);
    log::debug!("[nvme:io] Write submitted, CID={}", cid);

    let cqe = io_queue
        .poll_completion(cid, regs, 5_000_000)
        .ok_or((0xFF, 0xFF))?;

    if !cqe.is_success() {
        log::error!(
            "[nvme:io] Write FAILED: SLBA={} NLB={} SCT={} SC={:#04x}",
            slba,
            nlb + 1,
            cqe.status_code_type(),
            cqe.status_code()
        );
        return Err((cqe.status_code_type(), cqe.status_code()));
    }

    log::debug!(
        "[nvme:io] Write complete: SLBA={} NLB={} OK",
        slba,
        nlb + 1
    );
    Ok(())
}

/// Submit a Flush command for the given namespace.
///
/// Forces all volatile data and metadata to non-volatile storage.
pub fn flush(
    io_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    nsid: u32,
) -> Result<(), (u8, u8)> {
    log::info!("[nvme:io] Flush: NSID={}", nsid);

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(IO_OPC_FLUSH, 0);
    sqe.nsid = nsid;

    let cid = io_queue.submit(sqe, regs);
    log::debug!("[nvme:io] Flush submitted, CID={}", cid);

    let cqe = io_queue
        .poll_completion(cid, regs, 5_000_000)
        .ok_or((0xFF, 0xFF))?;

    if !cqe.is_success() {
        log::error!(
            "[nvme:io] Flush FAILED: NSID={} SCT={} SC={:#04x}",
            nsid,
            cqe.status_code_type(),
            cqe.status_code()
        );
        return Err((cqe.status_code_type(), cqe.status_code()));
    }

    log::info!("[nvme:io] Flush NSID={} complete", nsid);
    Ok(())
}
