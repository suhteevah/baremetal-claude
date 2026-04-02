//! NVMe admin commands — Identify Controller, Identify Namespace, Create I/O Queue.
//!
//! Admin commands are submitted on queue pair 0 (the admin queue). They are used
//! during initialization to discover controller capabilities and to create the
//! I/O queues that will be used for actual read/write operations.

use alloc::string::String;
use alloc::vec;

use crate::queue::{CompletionQueueEntry, QueuePair, SubmissionQueueEntry};
use crate::registers::NvmeRegisters;

// ============================================================================
// Admin command opcodes (NVMe 1.4 Figure 138)
// ============================================================================

/// Delete I/O Submission Queue
pub const ADMIN_OPC_DELETE_IO_SQ: u8 = 0x00;
/// Create I/O Submission Queue
pub const ADMIN_OPC_CREATE_IO_SQ: u8 = 0x01;
/// Get Log Page
pub const ADMIN_OPC_GET_LOG_PAGE: u8 = 0x02;
/// Delete I/O Completion Queue
pub const ADMIN_OPC_DELETE_IO_CQ: u8 = 0x04;
/// Create I/O Completion Queue
pub const ADMIN_OPC_CREATE_IO_CQ: u8 = 0x05;
/// Identify
pub const ADMIN_OPC_IDENTIFY: u8 = 0x06;
/// Abort
pub const ADMIN_OPC_ABORT: u8 = 0x08;
/// Set Features
pub const ADMIN_OPC_SET_FEATURES: u8 = 0x09;
/// Get Features
pub const ADMIN_OPC_GET_FEATURES: u8 = 0x0A;
/// Async Event Request
pub const ADMIN_OPC_ASYNC_EVENT_REQ: u8 = 0x0C;
/// Namespace Management
pub const ADMIN_OPC_NS_MANAGEMENT: u8 = 0x0D;
/// Firmware Commit
pub const ADMIN_OPC_FW_COMMIT: u8 = 0x10;
/// Firmware Image Download
pub const ADMIN_OPC_FW_DOWNLOAD: u8 = 0x11;
/// Device Self-test
pub const ADMIN_OPC_DEVICE_SELF_TEST: u8 = 0x14;
/// Namespace Attachment
pub const ADMIN_OPC_NS_ATTACHMENT: u8 = 0x15;
/// Keep Alive
pub const ADMIN_OPC_KEEP_ALIVE: u8 = 0x18;
/// Format NVM
pub const ADMIN_OPC_FORMAT_NVM: u8 = 0x80;
/// Security Send
pub const ADMIN_OPC_SECURITY_SEND: u8 = 0x81;
/// Security Receive
pub const ADMIN_OPC_SECURITY_RECV: u8 = 0x82;

// ============================================================================
// Identify CNS values
// ============================================================================

/// Identify Namespace data structure (CNS=0x00)
pub const IDENTIFY_CNS_NAMESPACE: u32 = 0x00;
/// Identify Controller data structure (CNS=0x01)
pub const IDENTIFY_CNS_CONTROLLER: u32 = 0x01;
/// Active Namespace ID list (CNS=0x02)
pub const IDENTIFY_CNS_ACTIVE_NS_LIST: u32 = 0x02;
/// Namespace Identification Descriptor list (CNS=0x03)
pub const IDENTIFY_CNS_NS_DESC_LIST: u32 = 0x03;

// ============================================================================
// Set/Get Features — Feature Identifiers
// ============================================================================

/// Arbitration
pub const FEATURE_ARBITRATION: u32 = 0x01;
/// Power Management
pub const FEATURE_POWER_MANAGEMENT: u32 = 0x02;
/// LBA Range Type
pub const FEATURE_LBA_RANGE_TYPE: u32 = 0x03;
/// Temperature Threshold
pub const FEATURE_TEMP_THRESHOLD: u32 = 0x04;
/// Error Recovery
pub const FEATURE_ERROR_RECOVERY: u32 = 0x05;
/// Volatile Write Cache
pub const FEATURE_VOLATILE_WC: u32 = 0x06;
/// Number of Queues
pub const FEATURE_NUMBER_OF_QUEUES: u32 = 0x07;
/// Interrupt Coalescing
pub const FEATURE_INTERRUPT_COALESCING: u32 = 0x08;
/// Interrupt Vector Configuration
pub const FEATURE_INTERRUPT_VECTOR_CONFIG: u32 = 0x09;
/// Write Atomicity Normal
pub const FEATURE_WRITE_ATOMICITY: u32 = 0x0A;
/// Async Event Configuration
pub const FEATURE_ASYNC_EVENT_CONFIG: u32 = 0x0B;

// ============================================================================
// Identify Controller data offsets (NVMe 1.4 Figure 247)
// ============================================================================

/// Identify Controller data structure is 4096 bytes.
pub const IDENTIFY_CTRL_SIZE: usize = 4096;

// Offsets within the 4096-byte Identify Controller structure
const CTRL_OFF_VID: usize = 0;      // PCI Vendor ID (16-bit)
const CTRL_OFF_SSVID: usize = 2;    // PCI Subsystem Vendor ID (16-bit)
const CTRL_OFF_SN: usize = 4;       // Serial Number (20 bytes ASCII)
const CTRL_OFF_MN: usize = 24;      // Model Number (40 bytes ASCII)
const CTRL_OFF_FR: usize = 64;      // Firmware Revision (8 bytes ASCII)
const CTRL_OFF_RAB: usize = 72;     // Recommended Arbitration Burst
const CTRL_OFF_IEEE: usize = 73;    // IEEE OUI Identifier (3 bytes)
const CTRL_OFF_CMIC: usize = 76;    // Controller Multi-Path I/O Capabilities
const CTRL_OFF_MDTS: usize = 77;    // Maximum Data Transfer Size (in MPS units)
const CTRL_OFF_CNTLID: usize = 78;  // Controller ID (16-bit)
const CTRL_OFF_VER: usize = 80;     // Version (32-bit)
const CTRL_OFF_OACS: usize = 256;   // Optional Admin Command Support (16-bit)
const CTRL_OFF_ACLS: usize = 258;   // Abort Command Limit (8-bit)
const CTRL_OFF_AERL: usize = 259;   // Async Event Request Limit (8-bit)
const CTRL_OFF_FRMW: usize = 260;   // Firmware Updates (8-bit)
const CTRL_OFF_LPA: usize = 261;    // Log Page Attributes (8-bit)
const CTRL_OFF_ELPE: usize = 262;   // Error Log Page Entries (8-bit)
const CTRL_OFF_NPSS: usize = 263;   // Number of Power States Support (8-bit)
const CTRL_OFF_SQES: usize = 512;   // SQ Entry Size (8-bit): min (3:0), max (7:4)
const CTRL_OFF_CQES: usize = 513;   // CQ Entry Size (8-bit): min (3:0), max (7:4)
const CTRL_OFF_MAXCMD: usize = 514; // Maximum Outstanding Commands (16-bit)
const CTRL_OFF_NN: usize = 516;     // Number of Namespaces (32-bit)
const CTRL_OFF_ONCS: usize = 520;   // Optional NVM Command Support (16-bit)

// ============================================================================
// Identify Namespace data offsets (NVMe 1.4 Figure 245)
// ============================================================================

/// Identify Namespace data structure is 4096 bytes.
pub const IDENTIFY_NS_SIZE: usize = 4096;

const NS_OFF_NSZE: usize = 0;       // Namespace Size (64-bit, in LBAs)
const NS_OFF_NCAP: usize = 8;       // Namespace Capacity (64-bit, in LBAs)
const NS_OFF_NUSE: usize = 16;      // Namespace Utilization (64-bit, in LBAs)
const NS_OFF_NSFEAT: usize = 24;    // Namespace Features (8-bit)
const NS_OFF_NLBAF: usize = 25;     // Number of LBA Formats (8-bit, 0-based)
const NS_OFF_FLBAS: usize = 26;     // Formatted LBA Size (8-bit)
const NS_OFF_MC: usize = 27;        // Metadata Capabilities (8-bit)
const NS_OFF_DPC: usize = 28;       // End-to-End Data Protection Capabilities (8-bit)
const NS_OFF_DPS: usize = 29;       // End-to-End Data Protection Type Settings (8-bit)
const NS_OFF_LBAF_BASE: usize = 128; // LBA Format 0 starts here (each is 4 bytes)

// ============================================================================
// Parsed structures
// ============================================================================

/// Parsed Identify Controller data.
#[derive(Debug)]
pub struct IdentifyController {
    /// PCI Vendor ID.
    pub vendor_id: u16,
    /// PCI Subsystem Vendor ID.
    pub subsystem_vendor_id: u16,
    /// Serial number (up to 20 chars, trimmed).
    pub serial: String,
    /// Model number (up to 40 chars, trimmed).
    pub model: String,
    /// Firmware revision (up to 8 chars, trimmed).
    pub firmware_rev: String,
    /// Maximum Data Transfer Size (in units of 2^(12 + MPS) bytes, 0 = no limit).
    pub mdts: u8,
    /// Controller ID.
    pub controller_id: u16,
    /// NVMe version from Identify (not the VS register).
    pub version: u32,
    /// Number of Namespaces.
    pub num_namespaces: u32,
    /// Recommended Arbitration Burst.
    pub rab: u8,
    /// Minimum SQ entry size (log2).
    pub sqes_min: u8,
    /// Maximum SQ entry size (log2).
    pub sqes_max: u8,
    /// Minimum CQ entry size (log2).
    pub cqes_min: u8,
    /// Maximum CQ entry size (log2).
    pub cqes_max: u8,
    /// Optional NVM Command Support bitmask.
    pub oncs: u16,
}

/// A single LBA format descriptor.
#[derive(Debug, Clone, Copy)]
pub struct LbaFormat {
    /// Metadata Size in bytes.
    pub metadata_size: u16,
    /// LBA Data Size as a power of 2 (e.g., 9 = 512 bytes, 12 = 4096 bytes).
    pub lba_data_size_log2: u8,
    /// Relative Performance (00=Best, 01=Better, 10=Good, 11=Degraded).
    pub relative_performance: u8,
}

impl LbaFormat {
    /// Actual sector size in bytes.
    pub fn sector_size(&self) -> u32 {
        1u32 << self.lba_data_size_log2
    }
}

/// Parsed Identify Namespace data.
#[derive(Debug)]
pub struct IdentifyNamespace {
    /// Namespace Size (total LBAs).
    pub nsze: u64,
    /// Namespace Capacity (usable LBAs).
    pub ncap: u64,
    /// Namespace Utilization (LBAs in use).
    pub nuse: u64,
    /// Number of LBA formats supported (0-based).
    pub num_lba_formats: u8,
    /// Index of the currently active LBA format (from FLBAS bits 3:0).
    pub active_lba_format: u8,
    /// LBA format descriptors (up to 16).
    pub lba_formats: [Option<LbaFormat>; 16],
}

impl IdentifyNamespace {
    /// Get the active LBA format.
    pub fn active_format(&self) -> Option<&LbaFormat> {
        self.lba_formats[self.active_lba_format as usize].as_ref()
    }

    /// Sector size in bytes for the active format.
    pub fn sector_size(&self) -> u32 {
        self.active_format()
            .map(|f| f.sector_size())
            .unwrap_or(512)
    }
}

// ============================================================================
// Parsing functions
// ============================================================================

/// Extract a trimmed ASCII string from a byte slice.
fn parse_ascii(data: &[u8], offset: usize, len: usize) -> String {
    let slice = &data[offset..offset + len];
    let s: String = slice
        .iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { ' ' })
        .collect();
    String::from(s.trim_end())
}

/// Extract a little-endian u16 from a byte slice.
fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Extract a little-endian u32 from a byte slice.
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Extract a little-endian u64 from a byte slice.
fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Parse the 4096-byte Identify Controller data structure.
pub fn parse_identify_controller(data: &[u8; IDENTIFY_CTRL_SIZE]) -> IdentifyController {
    let serial = parse_ascii(data, CTRL_OFF_SN, 20);
    let model = parse_ascii(data, CTRL_OFF_MN, 40);
    let firmware_rev = parse_ascii(data, CTRL_OFF_FR, 8);

    let vendor_id = read_u16_le(data, CTRL_OFF_VID);
    let subsystem_vendor_id = read_u16_le(data, CTRL_OFF_SSVID);
    let mdts = data[CTRL_OFF_MDTS];
    let controller_id = read_u16_le(data, CTRL_OFF_CNTLID);
    let version = read_u32_le(data, CTRL_OFF_VER);
    let num_namespaces = read_u32_le(data, CTRL_OFF_NN);
    let rab = data[CTRL_OFF_RAB];
    let sqes = data[CTRL_OFF_SQES];
    let cqes = data[CTRL_OFF_CQES];
    let oncs = read_u16_le(data, CTRL_OFF_ONCS);

    let id = IdentifyController {
        vendor_id,
        subsystem_vendor_id,
        serial,
        model,
        firmware_rev,
        mdts,
        controller_id,
        version,
        num_namespaces,
        rab,
        sqes_min: sqes & 0x0F,
        sqes_max: (sqes >> 4) & 0x0F,
        cqes_min: cqes & 0x0F,
        cqes_max: (cqes >> 4) & 0x0F,
        oncs,
    };

    log::info!(
        "[nvme:admin] Identify Controller: vendor={:#06x} model='{}' serial='{}' fw='{}' MDTS={} CNTLID={} NN={}",
        id.vendor_id, id.model, id.serial, id.firmware_rev, id.mdts, id.controller_id, id.num_namespaces
    );
    log::debug!(
        "[nvme:admin]   SQES min/max={}/{} CQES min/max={}/{} ONCS={:#06x}",
        id.sqes_min, id.sqes_max, id.cqes_min, id.cqes_max, id.oncs
    );

    id
}

/// Parse the 4096-byte Identify Namespace data structure.
pub fn parse_identify_namespace(data: &[u8; IDENTIFY_NS_SIZE]) -> IdentifyNamespace {
    let nsze = read_u64_le(data, NS_OFF_NSZE);
    let ncap = read_u64_le(data, NS_OFF_NCAP);
    let nuse = read_u64_le(data, NS_OFF_NUSE);
    let nlbaf = data[NS_OFF_NLBAF];
    let flbas = data[NS_OFF_FLBAS];
    let active_lba_format = flbas & 0x0F;

    let mut lba_formats = [None; 16];
    for i in 0..=nlbaf as usize {
        if i >= 16 {
            break;
        }
        let off = NS_OFF_LBAF_BASE + i * 4;
        let dword = read_u32_le(data, off);
        let metadata_size = (dword & 0xFFFF) as u16;
        let lba_data_size_log2 = ((dword >> 16) & 0xFF) as u8;
        let relative_performance = ((dword >> 24) & 0x3) as u8;

        lba_formats[i] = Some(LbaFormat {
            metadata_size,
            lba_data_size_log2,
            relative_performance,
        });
    }

    let ns = IdentifyNamespace {
        nsze,
        ncap,
        nuse,
        num_lba_formats: nlbaf,
        active_lba_format,
        lba_formats,
    };

    log::info!(
        "[nvme:admin] Identify Namespace: NSZE={} NCAP={} NUSE={} NLBAF={} active_fmt={}",
        ns.nsze, ns.ncap, ns.nuse, ns.num_lba_formats, ns.active_lba_format
    );
    if let Some(fmt) = ns.active_format() {
        log::info!(
            "[nvme:admin]   Active LBA format: sector_size={} metadata_size={} rp={}",
            fmt.sector_size(),
            fmt.metadata_size,
            fmt.relative_performance
        );
    }

    ns
}

// ============================================================================
// Admin command construction and submission
// ============================================================================

/// Submit an Identify Controller command and return the parsed result.
///
/// Allocates a 4096-byte buffer, submits the Identify command with CNS=0x01,
/// polls for completion, and parses the returned data.
pub fn identify_controller(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
) -> Option<IdentifyController> {
    log::info!("[nvme:admin] submitting Identify Controller (CNS=0x01)");

    let mut buf = vec![0u8; IDENTIFY_CTRL_SIZE];
    let buf_phys = buf.as_mut_ptr() as u64;

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_IDENTIFY, 0);
    sqe.nsid = 0;
    sqe.prp1 = buf_phys;
    sqe.prp2 = 0;
    sqe.cdw10 = IDENTIFY_CNS_CONTROLLER;

    let cid = admin_queue.submit(sqe, regs);
    log::debug!("[nvme:admin] Identify Controller submitted, CID={}", cid);

    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Identify Controller FAILED: SCT={} SC={:#04x}",
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!("[nvme:admin] Identify Controller completed successfully");
    let data: &[u8; IDENTIFY_CTRL_SIZE] = buf[..IDENTIFY_CTRL_SIZE].try_into().unwrap();
    Some(parse_identify_controller(data))
}

/// Submit an Identify Namespace command and return the parsed result.
pub fn identify_namespace(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    nsid: u32,
) -> Option<IdentifyNamespace> {
    log::info!(
        "[nvme:admin] submitting Identify Namespace (CNS=0x00, NSID={})",
        nsid
    );

    let mut buf = vec![0u8; IDENTIFY_NS_SIZE];
    let buf_phys = buf.as_mut_ptr() as u64;

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_IDENTIFY, 0);
    sqe.nsid = nsid;
    sqe.prp1 = buf_phys;
    sqe.prp2 = 0;
    sqe.cdw10 = IDENTIFY_CNS_NAMESPACE;

    let cid = admin_queue.submit(sqe, regs);
    log::debug!(
        "[nvme:admin] Identify Namespace submitted, CID={} NSID={}",
        cid,
        nsid
    );

    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Identify Namespace FAILED: SCT={} SC={:#04x}",
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!(
        "[nvme:admin] Identify Namespace {} completed successfully",
        nsid
    );
    let data: &[u8; IDENTIFY_NS_SIZE] = buf[..IDENTIFY_NS_SIZE].try_into().unwrap();
    Some(parse_identify_namespace(data))
}

/// Create an I/O Completion Queue.
///
/// `qid`: Queue identifier (must be >= 1).
/// `queue_pair`: The queue pair whose CQ physical address will be used.
/// `vector`: Interrupt vector (0 for polling mode).
pub fn create_io_completion_queue(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    qid: u16,
    cq_phys_addr: u64,
    depth: u16,
    vector: u16,
) -> Option<CompletionQueueEntry> {
    log::info!(
        "[nvme:admin] Create I/O CQ: QID={} depth={} phys={:#x} vector={}",
        qid,
        depth,
        cq_phys_addr,
        vector
    );

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_CREATE_IO_CQ, 0);
    sqe.prp1 = cq_phys_addr;
    // CDW10: Queue Size (31:16, 0-based) | Queue Identifier (15:0)
    sqe.cdw10 = ((depth as u32 - 1) << 16) | (qid as u32);
    // CDW11: Interrupt Vector (31:16) | IEN (1) | PC (0) — physically contiguous, interrupts enabled
    sqe.cdw11 = ((vector as u32) << 16) | (1 << 1) | 1; // PC=1 (physically contiguous), IEN=1

    let cid = admin_queue.submit(sqe, regs);
    log::debug!("[nvme:admin] Create I/O CQ submitted, CID={}", cid);

    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Create I/O CQ FAILED: SCT={} SC={:#04x}",
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!("[nvme:admin] I/O CQ {} created successfully", qid);
    Some(cqe)
}

/// Create an I/O Submission Queue.
///
/// `qid`: Queue identifier (must be >= 1).
/// `cqid`: Associated Completion Queue identifier.
pub fn create_io_submission_queue(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    qid: u16,
    sq_phys_addr: u64,
    depth: u16,
    cqid: u16,
) -> Option<CompletionQueueEntry> {
    log::info!(
        "[nvme:admin] Create I/O SQ: QID={} depth={} phys={:#x} CQID={}",
        qid,
        depth,
        sq_phys_addr,
        cqid
    );

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_CREATE_IO_SQ, 0);
    sqe.prp1 = sq_phys_addr;
    // CDW10: Queue Size (31:16, 0-based) | Queue Identifier (15:0)
    sqe.cdw10 = ((depth as u32 - 1) << 16) | (qid as u32);
    // CDW11: CQID (31:16) | QPRIO (2:1) | PC (0) — physically contiguous, medium priority
    sqe.cdw11 = ((cqid as u32) << 16) | 1; // PC=1, QPRIO=00 (urgent)

    let cid = admin_queue.submit(sqe, regs);
    log::debug!("[nvme:admin] Create I/O SQ submitted, CID={}", cid);

    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Create I/O SQ FAILED: SCT={} SC={:#04x}",
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!("[nvme:admin] I/O SQ {} created successfully", qid);
    Some(cqe)
}

/// Set Features command.
///
/// `fid`: Feature Identifier.
/// `cdw11`: Feature-specific value.
/// Returns DW0 of the completion (feature-specific result).
pub fn set_features(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    fid: u32,
    cdw11: u32,
) -> Option<u32> {
    log::info!(
        "[nvme:admin] Set Features: FID={:#x} CDW11={:#010x}",
        fid,
        cdw11
    );

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_SET_FEATURES, 0);
    sqe.cdw10 = fid;
    sqe.cdw11 = cdw11;

    let cid = admin_queue.submit(sqe, regs);
    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Set Features FAILED: FID={:#x} SCT={} SC={:#04x}",
            fid,
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!(
        "[nvme:admin] Set Features FID={:#x} success, DW0={:#010x}",
        fid,
        cqe.dw0
    );
    Some(cqe.dw0)
}

/// Get Features command.
///
/// `fid`: Feature Identifier.
/// Returns DW0 of the completion (feature-specific result).
pub fn get_features(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    fid: u32,
) -> Option<u32> {
    log::info!("[nvme:admin] Get Features: FID={:#x}", fid);

    let mut sqe = SubmissionQueueEntry::zeroed();
    sqe.set_opcode_cid(ADMIN_OPC_GET_FEATURES, 0);
    sqe.cdw10 = fid;

    let cid = admin_queue.submit(sqe, regs);
    let cqe = admin_queue.poll_completion(cid, regs, 1_000_000)?;
    if !cqe.is_success() {
        log::error!(
            "[nvme:admin] Get Features FAILED: FID={:#x} SCT={} SC={:#04x}",
            fid,
            cqe.status_code_type(),
            cqe.status_code()
        );
        return None;
    }

    log::info!(
        "[nvme:admin] Get Features FID={:#x} success, DW0={:#010x}",
        fid,
        cqe.dw0
    );
    Some(cqe.dw0)
}

/// Query the controller for how many I/O queues it supports via Set Features (Number of Queues).
///
/// Returns `(num_sq_allocated, num_cq_allocated)`, both 1-based.
pub fn set_number_of_queues(
    admin_queue: &mut QueuePair,
    regs: &NvmeRegisters,
    desired_sq: u16,
    desired_cq: u16,
) -> Option<(u16, u16)> {
    log::info!(
        "[nvme:admin] requesting {} SQs + {} CQs",
        desired_sq,
        desired_cq
    );

    // CDW11: NCQR (31:16) | NSQR (15:0), both 0-based
    let cdw11 = (((desired_cq - 1) as u32) << 16) | ((desired_sq - 1) as u32);
    let dw0 = set_features(admin_queue, regs, FEATURE_NUMBER_OF_QUEUES, cdw11)?;

    let nsqa = (dw0 & 0xFFFF) as u16 + 1;
    let ncqa = ((dw0 >> 16) & 0xFFFF) as u16 + 1;
    log::info!(
        "[nvme:admin] controller allocated {} SQs + {} CQs",
        nsqa,
        ncqa
    );
    Some((nsqa, ncqa))
}
