//! High-level NVMe driver API — controller init, namespace access, BlockDevice.
//!
//! This module ties together registers, queues, admin commands, and I/O commands
//! into a usable driver. The initialization sequence follows the NVMe 1.4 spec
//! section 7.6.1 (Initialization).
//!
//! ## Controller Reset Sequence
//!
//! 1. Set CC.EN = 0 and wait for CSTS.RDY = 0
//! 2. Configure CC (MPS, AMS, CSS, IOSQES, IOCQES)
//! 3. Set AQA, ASQ, ACQ
//! 4. Set CC.EN = 1 and wait for CSTS.RDY = 1
//! 5. Identify Controller
//! 6. Set Number of Queues
//! 7. Create I/O Completion Queue(s)
//! 8. Create I/O Submission Queue(s)
//! 9. Identify Namespace(s)

use alloc::format;
use core::fmt;

use crate::admin::{self, IdentifyController, IdentifyNamespace};
use crate::io;
use crate::queue::{QueuePair, DEFAULT_QUEUE_DEPTH};
use crate::registers::{
    self, NvmeRegisters, CC_AMS_RR, CC_CSS_NVM, CC_IOCQES_16, CC_IOSQES_64, CC_EN_BIT,
};

// ============================================================================
// NVMe PCI class/subclass/progif for detection
// ============================================================================

/// PCI Class: Mass Storage Controller
pub const PCI_CLASS_STORAGE: u8 = 0x01;
/// PCI Subclass: Non-Volatile Memory Controller
pub const PCI_SUBCLASS_NVME: u8 = 0x08;
/// PCI Programming Interface: NVM Express
pub const PCI_PROGIF_NVME: u8 = 0x02;

// ============================================================================
// Error type
// ============================================================================

/// Errors returned by the NVMe driver.
#[derive(Debug)]
pub enum NvmeError {
    /// Controller reset timed out.
    ResetTimeout,
    /// Controller reported fatal status (CSTS.CFS = 1).
    ControllerFatal,
    /// Controller did not become ready after enable.
    ReadyTimeout,
    /// Identify Controller command failed.
    IdentifyControllerFailed,
    /// Identify Namespace command failed.
    IdentifyNamespaceFailed,
    /// Could not create I/O queues.
    CreateQueueFailed,
    /// Set Number of Queues failed.
    SetNumQueuesFailed,
    /// NVM command set not supported.
    NvmCssNotSupported,
    /// I/O read command failed (SCT, SC).
    ReadFailed(u8, u8),
    /// I/O write command failed (SCT, SC).
    WriteFailed(u8, u8),
    /// I/O flush command failed (SCT, SC).
    FlushFailed(u8, u8),
    /// The requested namespace does not exist.
    NamespaceNotFound(u32),
    /// Buffer size does not match the expected transfer size.
    BufferSizeMismatch {
        expected: usize,
        actual: usize,
    },
    /// LBA out of range for this namespace.
    LbaOutOfRange {
        lba: u64,
        count: u32,
        max_lba: u64,
    },
}

impl fmt::Display for NvmeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResetTimeout => write!(f, "NVMe controller reset timed out"),
            Self::ControllerFatal => write!(f, "NVMe controller fatal status"),
            Self::ReadyTimeout => write!(f, "NVMe controller ready timeout"),
            Self::IdentifyControllerFailed => write!(f, "Identify Controller failed"),
            Self::IdentifyNamespaceFailed => write!(f, "Identify Namespace failed"),
            Self::CreateQueueFailed => write!(f, "Create I/O queue failed"),
            Self::SetNumQueuesFailed => write!(f, "Set Number of Queues failed"),
            Self::NvmCssNotSupported => write!(f, "NVM command set not supported"),
            Self::ReadFailed(sct, sc) => write!(f, "NVMe read failed: SCT={} SC={:#x}", sct, sc),
            Self::WriteFailed(sct, sc) => write!(f, "NVMe write failed: SCT={} SC={:#x}", sct, sc),
            Self::FlushFailed(sct, sc) => write!(f, "NVMe flush failed: SCT={} SC={:#x}", sct, sc),
            Self::NamespaceNotFound(ns) => write!(f, "NVMe namespace {} not found", ns),
            Self::BufferSizeMismatch { expected, actual } => {
                write!(f, "buffer size mismatch: expected {} got {}", expected, actual)
            }
            Self::LbaOutOfRange { lba, count, max_lba } => {
                write!(f, "LBA out of range: lba={} count={} max={}", lba, count, max_lba)
            }
        }
    }
}

// ============================================================================
// NvmeController — owns BAR0, admin queue, controller identity
// ============================================================================

/// High-level NVMe controller handle.
///
/// Created by [`NvmeController::init`] which performs the full controller reset
/// and initialization sequence.
pub struct NvmeController {
    /// Memory-mapped register accessor.
    regs: NvmeRegisters,
    /// Admin queue pair (QID=0).
    admin_queue: QueuePair,
    /// I/O queue pair (QID=1). We create one I/O queue pair for simplicity.
    io_queue: QueuePair,
    /// Parsed Identify Controller data.
    pub identity: IdentifyController,
    /// Maximum data transfer size in bytes (0 = unlimited).
    pub max_transfer_size: usize,
}

impl NvmeController {
    /// Initialize an NVMe controller at the given BAR0 physical address.
    ///
    /// This performs the full NVMe initialization sequence:
    /// 1. Reset (CC.EN=0, wait CSTS.RDY=0)
    /// 2. Configure admin queues
    /// 3. Enable (CC.EN=1, wait CSTS.RDY=1)
    /// 4. Identify Controller
    /// 5. Create I/O queues
    ///
    /// # Safety
    ///
    /// `bar0_addr` must be the BAR0 physical address of an NVMe controller,
    /// identity-mapped into the virtual address space.
    pub unsafe fn init(bar0_addr: usize) -> Result<Self, NvmeError> {
        log::info!("[nvme:driver] initializing NVMe controller at BAR0={:#x}", bar0_addr);

        // ---- Step 0: Create register accessor ----
        let mut regs = unsafe { NvmeRegisters::new(bar0_addr) };
        regs.init_doorbell_stride();

        // Read and log version
        let (major, minor, tertiary) = regs.read_version();
        log::info!("[nvme:driver] NVMe version {}.{}.{}", major, minor, tertiary);

        // Read capabilities
        let cap = regs.read_cap();
        let mqes = (cap & registers::CAP_MQES_MASK) as u16;
        let timeout_500ms = ((cap >> registers::CAP_TO_SHIFT) & registers::CAP_TO_MASK) as u32;
        let mpsmin = regs.cap_mpsmin();
        let mpsmax = regs.cap_mpsmax();
        log::info!(
            "[nvme:driver] CAP: MQES={} TO={}*500ms MPSMIN={} MPSMAX={} CQR={} CSS_NVM={}",
            mqes + 1,
            timeout_500ms,
            mpsmin,
            mpsmax,
            regs.cap_cqr(),
            regs.cap_css_nvm()
        );

        if !regs.cap_css_nvm() {
            log::error!("[nvme:driver] NVM command set not supported!");
            return Err(NvmeError::NvmCssNotSupported);
        }

        // ---- Step 1: Disable controller (reset) ----
        // Per NVMe spec 7.6.1: Set CC.EN = 0 to disable the controller.
        // This causes the controller to complete or abort all outstanding commands
        // and reset internal state. We must wait for CSTS.RDY = 0 before proceeding.
        log::info!("[nvme:driver] disabling controller for reset...");
        regs.disable_controller();

        // Wait for CSTS.RDY = 0
        let max_wait = timeout_500ms * 500_000; // convert to rough iteration count
        let mut waited = 0u32;
        while regs.is_ready() {
            if waited > max_wait.max(2_000_000) {
                log::error!("[nvme:driver] timeout waiting for CSTS.RDY=0 after disable");
                return Err(NvmeError::ResetTimeout);
            }
            core::hint::spin_loop();
            waited += 1;
        }
        log::info!("[nvme:driver] controller disabled (CSTS.RDY=0) after {} spins", waited);

        // Check for fatal error
        if regs.is_fatal() {
            log::error!("[nvme:driver] controller fatal status after reset!");
            return Err(NvmeError::ControllerFatal);
        }

        // ---- Step 2: Allocate admin queues ----
        let queue_depth = DEFAULT_QUEUE_DEPTH.min(mqes + 1);
        log::info!("[nvme:driver] allocating admin queue pair, depth={}", queue_depth);
        let admin_queue = QueuePair::new(0, queue_depth);

        // ---- Step 3: Configure AQA, ASQ, ACQ ----
        regs.write_aqa(queue_depth - 1, queue_depth - 1);
        regs.write_asq(admin_queue.sq_phys_addr());
        regs.write_acq(admin_queue.cq_phys_addr());

        // ---- Step 4: Configure CC and enable ----
        // Configure the Controller Configuration register and set EN=1 to start:
        // - CSS=NVM (000): Use the NVM Command Set
        // - MPS=0: Memory Page Size = 2^(12+0) = 4 KiB
        // - AMS=RR: Round Robin arbitration
        // - SHN=none (00): No shutdown notification
        // - IOSQES=6: I/O SQ Entry Size = 2^6 = 64 bytes
        // - IOCQES=4: I/O CQ Entry Size = 2^4 = 16 bytes
        let cc = CC_EN_BIT | CC_CSS_NVM | CC_AMS_RR | CC_IOSQES_64 | CC_IOCQES_16;
        log::info!("[nvme:driver] writing CC={:#010x} (enable + configure)", cc);
        regs.write_cc(cc);

        // Wait for CSTS.RDY = 1
        waited = 0;
        while !regs.is_ready() {
            if regs.is_fatal() {
                log::error!("[nvme:driver] controller fatal during enable!");
                return Err(NvmeError::ControllerFatal);
            }
            if waited > max_wait.max(2_000_000) {
                log::error!("[nvme:driver] timeout waiting for CSTS.RDY=1 after enable");
                return Err(NvmeError::ReadyTimeout);
            }
            core::hint::spin_loop();
            waited += 1;
        }
        log::info!("[nvme:driver] controller enabled (CSTS.RDY=1) after {} spins", waited);

        // ---- Step 5: Identify Controller ----
        let mut admin_queue = admin_queue;
        let identity = admin::identify_controller(&mut admin_queue, &regs)
            .ok_or(NvmeError::IdentifyControllerFailed)?;

        // Calculate max transfer size
        let max_transfer_size = if identity.mdts == 0 {
            0 // no limit
        } else {
            // MDTS is in units of minimum memory page size (2^(12 + MPS))
            // With MPS=0 (4KiB pages): max_xfer = (2^MDTS) * 4096
            (1usize << identity.mdts) * mpsmin as usize
        };
        log::info!(
            "[nvme:driver] max transfer size = {} bytes (MDTS={})",
            if max_transfer_size == 0 { "unlimited".into() } else { format!("{}", max_transfer_size) },
            identity.mdts
        );

        // ---- Step 6: Set Number of Queues ----
        let (nsqa, ncqa) = admin::set_number_of_queues(&mut admin_queue, &regs, 1, 1)
            .ok_or(NvmeError::SetNumQueuesFailed)?;
        log::info!("[nvme:driver] allocated {} SQ(s) + {} CQ(s)", nsqa, ncqa);

        // ---- Step 7: Create I/O queue pair (QID=1) ----
        let io_queue = QueuePair::new(1, queue_depth);

        // Create I/O CQ first (must exist before SQ)
        admin::create_io_completion_queue(
            &mut admin_queue,
            &regs,
            1,
            io_queue.cq_phys_addr(),
            queue_depth,
            0, // interrupt vector 0 (polling)
        )
        .ok_or(NvmeError::CreateQueueFailed)?;

        // Create I/O SQ, associated with CQ 1
        admin::create_io_submission_queue(
            &mut admin_queue,
            &regs,
            1,
            io_queue.sq_phys_addr(),
            queue_depth,
            1, // CQID = 1
        )
        .ok_or(NvmeError::CreateQueueFailed)?;

        log::info!("[nvme:driver] I/O queue pair QID=1 created successfully");

        // ---- Done ----
        log::info!(
            "[nvme:driver] NVMe controller initialized: '{}' serial='{}' fw='{}'",
            identity.model,
            identity.serial,
            identity.firmware_rev
        );

        Ok(Self {
            regs,
            admin_queue,
            io_queue,
            identity,
            max_transfer_size,
        })
    }

    /// Retrieve information about a namespace and return an `NvmeDisk` handle.
    pub fn namespace(&mut self, nsid: u32) -> Result<NvmeDisk, NvmeError> {
        log::info!("[nvme:driver] probing namespace {}", nsid);

        if nsid == 0 || nsid > self.identity.num_namespaces {
            log::error!(
                "[nvme:driver] namespace {} out of range (controller has {})",
                nsid,
                self.identity.num_namespaces
            );
            return Err(NvmeError::NamespaceNotFound(nsid));
        }

        let ns_info = admin::identify_namespace(&mut self.admin_queue, &self.regs, nsid)
            .ok_or(NvmeError::IdentifyNamespaceFailed)?;

        let sector_size = ns_info.sector_size();
        let sector_count = ns_info.nsze;

        log::info!(
            "[nvme:driver] namespace {}: {} sectors x {} bytes = {} MiB",
            nsid,
            sector_count,
            sector_size,
            (sector_count * sector_size as u64) / (1024 * 1024)
        );

        Ok(NvmeDisk {
            nsid,
            sector_count,
            sector_size,
            ns_info,
        })
    }

    /// Read sectors from a namespace.
    pub fn read_sectors(
        &mut self,
        nsid: u32,
        lba: u64,
        count: u32,
        buf: &mut [u8],
        sector_size: u32,
    ) -> Result<(), NvmeError> {
        let expected = count as usize * sector_size as usize;
        if buf.len() < expected {
            return Err(NvmeError::BufferSizeMismatch {
                expected,
                actual: buf.len(),
            });
        }

        log::debug!(
            "[nvme:driver] read_sectors: NSID={} LBA={} count={} sector_size={}",
            nsid,
            lba,
            count,
            sector_size
        );

        // NVMe NLB (Number of Logical Blocks) is 0-based: a value of 0 means
        // 1 block, a value of N means N+1 blocks. This is different from ATA/AHCI
        // where the count field is 1-based.
        let nlb = (count - 1) as u16;
        io::read(&mut self.io_queue, &self.regs, nsid, lba, nlb, buf)
            .map_err(|(sct, sc)| NvmeError::ReadFailed(sct, sc))
    }

    /// Write sectors to a namespace.
    pub fn write_sectors(
        &mut self,
        nsid: u32,
        lba: u64,
        count: u32,
        buf: &[u8],
        sector_size: u32,
    ) -> Result<(), NvmeError> {
        let expected = count as usize * sector_size as usize;
        if buf.len() < expected {
            return Err(NvmeError::BufferSizeMismatch {
                expected,
                actual: buf.len(),
            });
        }

        log::debug!(
            "[nvme:driver] write_sectors: NSID={} LBA={} count={} sector_size={}",
            nsid,
            lba,
            count,
            sector_size
        );

        let nlb = (count - 1) as u16;
        io::write(&mut self.io_queue, &self.regs, nsid, lba, nlb, buf)
            .map_err(|(sct, sc)| NvmeError::WriteFailed(sct, sc))
    }

    /// Flush volatile data on a namespace.
    pub fn flush_namespace(&mut self, nsid: u32) -> Result<(), NvmeError> {
        log::debug!("[nvme:driver] flush: NSID={}", nsid);
        io::flush(&mut self.io_queue, &self.regs, nsid)
            .map_err(|(sct, sc)| NvmeError::FlushFailed(sct, sc))
    }
}

// ============================================================================
// NvmeDisk — namespace handle for sector I/O
// ============================================================================

/// A handle to a specific NVMe namespace, providing sector-level I/O.
///
/// Created by [`NvmeController::namespace`]. Holds namespace metadata
/// (sector count, sector size) and delegates I/O back to the controller.
pub struct NvmeDisk {
    /// Namespace ID (1-based).
    pub nsid: u32,
    /// Total number of sectors (from Identify Namespace NSZE).
    pub sector_count: u64,
    /// Sector size in bytes (from the active LBA format).
    pub sector_size: u32,
    /// Full namespace identity info.
    pub ns_info: IdentifyNamespace,
}

impl NvmeDisk {
    /// Read `count` sectors starting at `lba` into `buf`.
    ///
    /// `buf` must be at least `count * sector_size` bytes.
    pub fn read_sectors(
        &self,
        lba: u64,
        count: u32,
        buf: &mut [u8],
        ctrl: &mut NvmeController,
    ) -> Result<(), NvmeError> {
        // Range check
        if lba + count as u64 > self.sector_count {
            return Err(NvmeError::LbaOutOfRange {
                lba,
                count,
                max_lba: self.sector_count,
            });
        }
        ctrl.read_sectors(self.nsid, lba, count, buf, self.sector_size)
    }

    /// Write `count` sectors starting at `lba` from `buf`.
    ///
    /// `buf` must be at least `count * sector_size` bytes.
    pub fn write_sectors(
        &self,
        lba: u64,
        count: u32,
        buf: &[u8],
        ctrl: &mut NvmeController,
    ) -> Result<(), NvmeError> {
        if lba + count as u64 > self.sector_count {
            return Err(NvmeError::LbaOutOfRange {
                lba,
                count,
                max_lba: self.sector_count,
            });
        }
        ctrl.write_sectors(self.nsid, lba, count, buf, self.sector_size)
    }

    /// Flush volatile data to non-volatile storage.
    pub fn flush(&self, ctrl: &mut NvmeController) -> Result<(), NvmeError> {
        ctrl.flush_namespace(self.nsid)
    }

    /// Total capacity in bytes.
    pub fn capacity_bytes(&self) -> u64 {
        self.sector_count * self.sector_size as u64
    }
}

// ============================================================================
// BlockDevice trait implementation
// ============================================================================

/// BlockDevice-compatible wrapper around an NVMe namespace.
///
/// This struct holds a mutable reference to the controller and a namespace,
/// matching the `BlockDevice` trait pattern used by claudio-ext4.
///
/// ```rust,no_run
/// use claudio_nvme::driver::NvmeBlockDevice;
///
/// // After init:
/// let block_dev = NvmeBlockDevice::new(&mut ctrl, 1, sector_count, sector_size);
/// // block_dev.read_bytes(offset, &mut buf)?;
/// // block_dev.write_bytes(offset, &buf)?;
/// // block_dev.flush()?;
/// ```
pub struct NvmeBlockDevice<'a> {
    ctrl: &'a mut NvmeController,
    nsid: u32,
    sector_count: u64,
    sector_size: u32,
}

impl<'a> NvmeBlockDevice<'a> {
    /// Create a new BlockDevice wrapper.
    pub fn new(
        ctrl: &'a mut NvmeController,
        nsid: u32,
        sector_count: u64,
        sector_size: u32,
    ) -> Self {
        log::info!(
            "[nvme:blockdev] wrapping NSID={} as BlockDevice ({} sectors x {} bytes)",
            nsid,
            sector_count,
            sector_size
        );
        Self {
            ctrl,
            nsid,
            sector_count,
            sector_size,
        }
    }

    /// Read `buf.len()` bytes from the device starting at byte `offset`.
    ///
    /// Matches the ext4 BlockDevice::read_bytes signature.
    pub fn read_bytes(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), NvmeError> {
        if buf.is_empty() {
            return Ok(());
        }

        let ss = self.sector_size as u64;
        let start_lba = offset / ss;
        let end_byte = offset + buf.len() as u64;
        let end_lba = (end_byte + ss - 1) / ss;
        let total_sectors = (end_lba - start_lba) as u32;

        log::debug!(
            "[nvme:blockdev] read_bytes: offset={} len={} -> LBA {}..{} ({} sectors)",
            offset,
            buf.len(),
            start_lba,
            end_lba,
            total_sectors
        );

        // If the read is sector-aligned and sector-sized, read directly
        let sector_offset = (offset % ss) as usize;
        if sector_offset == 0 && buf.len() % self.sector_size as usize == 0 {
            return self.ctrl.read_sectors(
                self.nsid,
                start_lba,
                total_sectors,
                buf,
                self.sector_size,
            );
        }

        // Non-aligned read: read into a temp buffer and copy the relevant portion
        let tmp_size = total_sectors as usize * self.sector_size as usize;
        let mut tmp = alloc::vec![0u8; tmp_size];
        self.ctrl.read_sectors(
            self.nsid,
            start_lba,
            total_sectors,
            &mut tmp,
            self.sector_size,
        )?;
        buf.copy_from_slice(&tmp[sector_offset..sector_offset + buf.len()]);
        Ok(())
    }

    /// Write `buf.len()` bytes to the device starting at byte `offset`.
    ///
    /// Matches the ext4 BlockDevice::write_bytes signature.
    pub fn write_bytes(&mut self, offset: u64, buf: &[u8]) -> Result<(), NvmeError> {
        if buf.is_empty() {
            return Ok(());
        }

        let ss = self.sector_size as u64;
        let start_lba = offset / ss;
        let end_byte = offset + buf.len() as u64;
        let end_lba = (end_byte + ss - 1) / ss;
        let total_sectors = (end_lba - start_lba) as u32;

        log::debug!(
            "[nvme:blockdev] write_bytes: offset={} len={} -> LBA {}..{} ({} sectors)",
            offset,
            buf.len(),
            start_lba,
            end_lba,
            total_sectors
        );

        let sector_offset = (offset % ss) as usize;
        if sector_offset == 0 && buf.len() % self.sector_size as usize == 0 {
            return self.ctrl.write_sectors(
                self.nsid,
                start_lba,
                total_sectors,
                buf,
                self.sector_size,
            );
        }

        // Non-aligned write: read-modify-write
        let tmp_size = total_sectors as usize * self.sector_size as usize;
        let mut tmp = alloc::vec![0u8; tmp_size];

        log::debug!(
            "[nvme:blockdev] non-aligned write, doing read-modify-write for {} sectors",
            total_sectors
        );

        // Read existing data
        self.ctrl.read_sectors(
            self.nsid,
            start_lba,
            total_sectors,
            &mut tmp,
            self.sector_size,
        )?;

        // Modify
        tmp[sector_offset..sector_offset + buf.len()].copy_from_slice(buf);

        // Write back
        self.ctrl.write_sectors(
            self.nsid,
            start_lba,
            total_sectors,
            &tmp,
            self.sector_size,
        )
    }

    /// Flush volatile writes to non-volatile storage.
    pub fn flush(&mut self) -> Result<(), NvmeError> {
        self.ctrl.flush_namespace(self.nsid)
    }
}
