//! NVMe controller registers — BAR0 memory-mapped I/O.
//!
//! All register accesses use volatile reads/writes to prevent the compiler from
//! reordering or eliding MMIO operations. Register offsets and bitfield layouts
//! follow the NVMe 1.4 specification, section 3 (Controller Registers).

use core::ptr;

// ============================================================================
// Register offsets from BAR0
// ============================================================================

/// Controller Capabilities (64-bit, read-only)
pub const REG_CAP: usize = 0x00;
/// Version (32-bit, read-only)
pub const REG_VS: usize = 0x08;
/// Interrupt Mask Set (32-bit, read-write)
pub const REG_INTMS: usize = 0x0C;
/// Interrupt Mask Clear (32-bit, read-write)
pub const REG_INTMC: usize = 0x10;
/// Controller Configuration (32-bit, read-write)
pub const REG_CC: usize = 0x14;
/// Reserved
pub const REG_RESERVED: usize = 0x18;
/// Controller Status (32-bit, read-only)
pub const REG_CSTS: usize = 0x1C;
/// NVM Subsystem Reset (32-bit, read-write, optional)
pub const REG_NSSR: usize = 0x20;
/// Admin Queue Attributes (32-bit, read-write)
pub const REG_AQA: usize = 0x24;
/// Admin Submission Queue Base Address (64-bit, read-write)
pub const REG_ASQ: usize = 0x28;
/// Admin Completion Queue Base Address (64-bit, read-write)
pub const REG_ACQ: usize = 0x30;
/// Controller Memory Buffer Location (32-bit, read-only, optional)
pub const REG_CMBLOC: usize = 0x38;
/// Controller Memory Buffer Size (32-bit, read-only, optional)
pub const REG_CMBSZ: usize = 0x3C;
/// Boot Partition Information (32-bit, read-only, optional)
pub const REG_BPINFO: usize = 0x40;
/// Boot Partition Read Select (32-bit, read-write, optional)
pub const REG_BPRSEL: usize = 0x44;
/// Boot Partition Memory Buffer Location (64-bit, read-write, optional)
pub const REG_BPMBL: usize = 0x48;

/// Doorbell register base offset. Actual offset depends on DSTRD from CAP.
pub const REG_DOORBELL_BASE: usize = 0x1000;

// ============================================================================
// CAP — Controller Capabilities (offset 0x00, 64-bit)
// ============================================================================

/// Maximum Queue Entries Supported (0-based, bits 15:0)
pub const CAP_MQES_MASK: u64 = 0xFFFF;
pub const CAP_MQES_SHIFT: u64 = 0;

/// Contiguous Queues Required (bit 16)
pub const CAP_CQR_BIT: u64 = 1 << 16;

/// Arbitration Mechanism Supported (bits 18:17)
pub const CAP_AMS_MASK: u64 = 0x3;
pub const CAP_AMS_SHIFT: u64 = 17;

/// Timeout (bits 31:24) — in 500ms units, worst-case time for CSTS.RDY transition
pub const CAP_TO_MASK: u64 = 0xFF;
pub const CAP_TO_SHIFT: u64 = 24;

/// Doorbell Stride (bits 35:32) — stride is 2^(2 + DSTRD) bytes
pub const CAP_DSTRD_MASK: u64 = 0xF;
pub const CAP_DSTRD_SHIFT: u64 = 32;

/// NVM Subsystem Reset Supported (bit 36)
pub const CAP_NSSRS_BIT: u64 = 1 << 36;

/// Command Sets Supported (bits 44:37)
pub const CAP_CSS_MASK: u64 = 0xFF;
pub const CAP_CSS_SHIFT: u64 = 37;

/// NVM command set supported (bit 0 of CSS field)
pub const CAP_CSS_NVM: u64 = 1 << 37;

/// Boot Partition Support (bit 45)
pub const CAP_BPS_BIT: u64 = 1 << 45;

/// Memory Page Size Minimum (bits 51:48) — 2^(12 + MPSMIN) bytes
pub const CAP_MPSMIN_MASK: u64 = 0xF;
pub const CAP_MPSMIN_SHIFT: u64 = 48;

/// Memory Page Size Maximum (bits 55:52) — 2^(12 + MPSMAX) bytes
pub const CAP_MPSMAX_MASK: u64 = 0xF;
pub const CAP_MPSMAX_SHIFT: u64 = 52;

/// Persistent Memory Region Supported (bit 56)
pub const CAP_PMRS_BIT: u64 = 1 << 56;

// ============================================================================
// CC — Controller Configuration (offset 0x14, 32-bit)
// ============================================================================

/// Enable (bit 0)
pub const CC_EN_BIT: u32 = 1 << 0;

/// I/O Command Set Selected (bits 6:4)
pub const CC_CSS_MASK: u32 = 0x7;
pub const CC_CSS_SHIFT: u32 = 4;
/// NVM command set
pub const CC_CSS_NVM: u32 = 0 << 4;

/// Memory Page Size (bits 10:7) — host page size is 2^(12 + MPS)
pub const CC_MPS_MASK: u32 = 0xF;
pub const CC_MPS_SHIFT: u32 = 7;

/// Arbitration Mechanism Selected (bits 13:11)
pub const CC_AMS_MASK: u32 = 0x7;
pub const CC_AMS_SHIFT: u32 = 11;
/// Round Robin
pub const CC_AMS_RR: u32 = 0 << 11;

/// Shutdown Notification (bits 15:14)
pub const CC_SHN_MASK: u32 = 0x3;
pub const CC_SHN_SHIFT: u32 = 14;
/// No notification
pub const CC_SHN_NONE: u32 = 0 << 14;
/// Normal shutdown
pub const CC_SHN_NORMAL: u32 = 1 << 14;
/// Abrupt shutdown
pub const CC_SHN_ABRUPT: u32 = 2 << 14;

/// I/O Submission Queue Entry Size (bits 19:16) — 2^n bytes
pub const CC_IOSQES_MASK: u32 = 0xF;
pub const CC_IOSQES_SHIFT: u32 = 16;
/// 64-byte SQE (2^6 = 64)
pub const CC_IOSQES_64: u32 = 6 << 16;

/// I/O Completion Queue Entry Size (bits 23:20) — 2^n bytes
pub const CC_IOCQES_MASK: u32 = 0xF;
pub const CC_IOCQES_SHIFT: u32 = 20;
/// 16-byte CQE (2^4 = 16)
pub const CC_IOCQES_16: u32 = 4 << 20;

// ============================================================================
// CSTS — Controller Status (offset 0x1C, 32-bit)
// ============================================================================

/// Ready (bit 0) — controller is ready to process commands
pub const CSTS_RDY_BIT: u32 = 1 << 0;

/// Controller Fatal Status (bit 1)
pub const CSTS_CFS_BIT: u32 = 1 << 1;

/// Shutdown Status (bits 3:2)
pub const CSTS_SHST_MASK: u32 = 0x3;
pub const CSTS_SHST_SHIFT: u32 = 2;
/// Normal operation (not shut down)
pub const CSTS_SHST_NORMAL: u32 = 0;
/// Shutdown processing occurring
pub const CSTS_SHST_OCCURRING: u32 = 1;
/// Shutdown processing complete
pub const CSTS_SHST_COMPLETE: u32 = 2;

/// NVM Subsystem Reset Occurred (bit 4)
pub const CSTS_NSSRO_BIT: u32 = 1 << 4;

/// Processing Paused (bit 5)
pub const CSTS_PP_BIT: u32 = 1 << 5;

// ============================================================================
// AQA — Admin Queue Attributes (offset 0x24, 32-bit)
// ============================================================================

/// Admin Submission Queue Size (bits 11:0) — 0-based
pub const AQA_ASQS_MASK: u32 = 0xFFF;
pub const AQA_ASQS_SHIFT: u32 = 0;

/// Admin Completion Queue Size (bits 27:16) — 0-based
pub const AQA_ACQS_MASK: u32 = 0xFFF;
pub const AQA_ACQS_SHIFT: u32 = 16;

// ============================================================================
// NvmeRegisters — safe volatile wrapper over BAR0
// ============================================================================

/// Memory-mapped NVMe controller register access.
///
/// All reads and writes go through `core::ptr::read_volatile` /
/// `core::ptr::write_volatile` to prevent compiler reordering.
pub struct NvmeRegisters {
    /// BAR0 base virtual address (must be identity-mapped or properly mapped).
    base: usize,
    /// Doorbell stride in bytes: 4 * 2^DSTRD (cached from CAP after first read).
    doorbell_stride: usize,
}

impl NvmeRegisters {
    /// Create a new register accessor for the given BAR0 base address.
    ///
    /// # Safety
    ///
    /// `bar0_base` must point to a valid, identity-mapped NVMe BAR0 region
    /// of at least 0x1000 + doorbell area bytes.
    pub unsafe fn new(bar0_base: usize) -> Self {
        let regs = Self {
            base: bar0_base,
            doorbell_stride: 4, // minimum, updated after reading CAP
        };
        log::trace!("[nvme:regs] created register accessor at {:#x}", bar0_base);
        regs
    }

    /// Initialize the doorbell stride from the CAP register.
    /// Must be called before using doorbell methods.
    pub fn init_doorbell_stride(&mut self) {
        let cap = self.read_cap();
        let dstrd = ((cap >> CAP_DSTRD_SHIFT) & CAP_DSTRD_MASK) as usize;
        self.doorbell_stride = 4 * (1 << dstrd);
        log::debug!(
            "[nvme:regs] doorbell stride = {} bytes (DSTRD={})",
            self.doorbell_stride,
            dstrd
        );
    }

    // ---- 32-bit register helpers ----

    fn read32(&self, offset: usize) -> u32 {
        unsafe {
            let val = ptr::read_volatile((self.base + offset) as *const u32);
            val
        }
    }

    fn write32(&self, offset: usize, val: u32) {
        unsafe {
            ptr::write_volatile((self.base + offset) as *mut u32, val);
        }
    }

    // ---- 64-bit register helpers ----

    fn read64(&self, offset: usize) -> u64 {
        // NVMe spec: 64-bit registers may be read as two 32-bit reads (low then high)
        let lo = self.read32(offset) as u64;
        let hi = self.read32(offset + 4) as u64;
        lo | (hi << 32)
    }

    fn write64(&self, offset: usize, val: u64) {
        // NVMe spec: write low 32 bits first, then high 32 bits
        self.write32(offset, val as u32);
        self.write32(offset + 4, (val >> 32) as u32);
    }

    // ========================================================================
    // CAP — Controller Capabilities (0x00, 64-bit, RO)
    // ========================================================================

    /// Read the full 64-bit Controller Capabilities register.
    pub fn read_cap(&self) -> u64 {
        let val = self.read64(REG_CAP);
        log::trace!("[nvme:regs] CAP = {:#018x}", val);
        val
    }

    /// Maximum Queue Entries Supported (0-based value; actual max = MQES + 1).
    pub fn cap_mqes(&self) -> u16 {
        let cap = self.read_cap();
        ((cap >> CAP_MQES_SHIFT) & CAP_MQES_MASK) as u16
    }

    /// Contiguous Queues Required.
    pub fn cap_cqr(&self) -> bool {
        self.read_cap() & CAP_CQR_BIT != 0
    }

    /// Timeout in 500ms units.
    pub fn cap_timeout(&self) -> u8 {
        ((self.read_cap() >> CAP_TO_SHIFT) & CAP_TO_MASK) as u8
    }

    /// Doorbell stride field (raw value).
    pub fn cap_dstrd(&self) -> u8 {
        ((self.read_cap() >> CAP_DSTRD_SHIFT) & CAP_DSTRD_MASK) as u8
    }

    /// Minimum Memory Page Size: 2^(12 + MPSMIN) bytes.
    pub fn cap_mpsmin(&self) -> u32 {
        let mpsmin = ((self.read_cap() >> CAP_MPSMIN_SHIFT) & CAP_MPSMIN_MASK) as u32;
        1 << (12 + mpsmin)
    }

    /// Maximum Memory Page Size: 2^(12 + MPSMAX) bytes.
    pub fn cap_mpsmax(&self) -> u32 {
        let mpsmax = ((self.read_cap() >> CAP_MPSMAX_SHIFT) & CAP_MPSMAX_MASK) as u32;
        1 << (12 + mpsmax)
    }

    /// NVM command set supported?
    pub fn cap_css_nvm(&self) -> bool {
        self.read_cap() & CAP_CSS_NVM != 0
    }

    // ========================================================================
    // VS — Version (0x08, 32-bit, RO)
    // ========================================================================

    /// Read the Version register. Returns (major, minor, tertiary).
    pub fn read_version(&self) -> (u8, u8, u8) {
        let vs = self.read32(REG_VS);
        let major = (vs >> 16) as u8;
        let minor = (vs >> 8) as u8;
        let tertiary = vs as u8;
        log::debug!("[nvme:regs] version = {}.{}.{}", major, minor, tertiary);
        (major, minor, tertiary)
    }

    // ========================================================================
    // INTMS / INTMC — Interrupt Mask Set/Clear (0x0C / 0x10, 32-bit)
    // ========================================================================

    /// Set interrupt mask bits (write-only; bits set to 1 are masked).
    pub fn write_intms(&self, mask: u32) {
        log::trace!("[nvme:regs] INTMS <- {:#010x}", mask);
        self.write32(REG_INTMS, mask);
    }

    /// Clear interrupt mask bits (write-only; bits set to 1 are unmasked).
    pub fn write_intmc(&self, mask: u32) {
        log::trace!("[nvme:regs] INTMC <- {:#010x}", mask);
        self.write32(REG_INTMC, mask);
    }

    // ========================================================================
    // CC — Controller Configuration (0x14, 32-bit, RW)
    // ========================================================================

    /// Read the Controller Configuration register.
    pub fn read_cc(&self) -> u32 {
        let val = self.read32(REG_CC);
        log::trace!("[nvme:regs] CC = {:#010x}", val);
        val
    }

    /// Write the Controller Configuration register.
    pub fn write_cc(&self, val: u32) {
        log::trace!("[nvme:regs] CC <- {:#010x}", val);
        self.write32(REG_CC, val);
    }

    /// Set CC.EN = 1 (enable the controller).
    pub fn enable_controller(&self) {
        let cc = self.read_cc();
        log::info!("[nvme:regs] enabling controller (CC.EN=1)");
        self.write_cc(cc | CC_EN_BIT);
    }

    /// Set CC.EN = 0 (disable / reset the controller).
    pub fn disable_controller(&self) {
        let cc = self.read_cc();
        log::info!("[nvme:regs] disabling controller (CC.EN=0)");
        self.write_cc(cc & !CC_EN_BIT);
    }

    /// Check if CC.EN is set.
    pub fn is_enabled(&self) -> bool {
        self.read_cc() & CC_EN_BIT != 0
    }

    // ========================================================================
    // CSTS — Controller Status (0x1C, 32-bit, RO)
    // ========================================================================

    /// Read the Controller Status register.
    pub fn read_csts(&self) -> u32 {
        let val = self.read32(REG_CSTS);
        log::trace!("[nvme:regs] CSTS = {:#010x}", val);
        val
    }

    /// Check CSTS.RDY — controller ready.
    pub fn is_ready(&self) -> bool {
        self.read_csts() & CSTS_RDY_BIT != 0
    }

    /// Check CSTS.CFS — controller fatal status.
    pub fn is_fatal(&self) -> bool {
        self.read_csts() & CSTS_CFS_BIT != 0
    }

    /// Get shutdown status from CSTS.SHST.
    pub fn shutdown_status(&self) -> u32 {
        (self.read_csts() >> CSTS_SHST_SHIFT) & CSTS_SHST_MASK
    }

    // ========================================================================
    // NSSR — NVM Subsystem Reset (0x20, 32-bit, optional)
    // ========================================================================

    /// Write the NVM Subsystem Reset register (write 0x4E564D65 = "NVMe").
    pub fn subsystem_reset(&self) {
        log::warn!("[nvme:regs] issuing NVM subsystem reset");
        self.write32(REG_NSSR, 0x4E564D65);
    }

    // ========================================================================
    // AQA — Admin Queue Attributes (0x24, 32-bit, RW)
    // ========================================================================

    /// Read the Admin Queue Attributes register.
    pub fn read_aqa(&self) -> u32 {
        let val = self.read32(REG_AQA);
        log::trace!("[nvme:regs] AQA = {:#010x}", val);
        val
    }

    /// Write the Admin Queue Attributes register.
    ///
    /// `asqs` and `acqs` are 0-based (e.g., 31 means 32 entries).
    pub fn write_aqa(&self, asqs: u16, acqs: u16) {
        let val = ((acqs as u32) & AQA_ACQS_MASK) << AQA_ACQS_SHIFT
            | ((asqs as u32) & AQA_ASQS_MASK) << AQA_ASQS_SHIFT;
        log::debug!(
            "[nvme:regs] AQA <- {:#010x} (ASQS={}, ACQS={})",
            val,
            asqs,
            acqs
        );
        self.write32(REG_AQA, val);
    }

    // ========================================================================
    // ASQ — Admin Submission Queue Base Address (0x28, 64-bit, RW)
    // ========================================================================

    /// Write the Admin Submission Queue base physical address.
    /// Must be page-aligned.
    pub fn write_asq(&self, phys_addr: u64) {
        log::debug!("[nvme:regs] ASQ <- {:#018x}", phys_addr);
        debug_assert!(phys_addr & 0xFFF == 0, "ASQ must be page-aligned");
        self.write64(REG_ASQ, phys_addr);
    }

    /// Read the Admin Submission Queue base address.
    pub fn read_asq(&self) -> u64 {
        self.read64(REG_ASQ)
    }

    // ========================================================================
    // ACQ — Admin Completion Queue Base Address (0x30, 64-bit, RW)
    // ========================================================================

    /// Write the Admin Completion Queue base physical address.
    /// Must be page-aligned.
    pub fn write_acq(&self, phys_addr: u64) {
        log::debug!("[nvme:regs] ACQ <- {:#018x}", phys_addr);
        debug_assert!(phys_addr & 0xFFF == 0, "ACQ must be page-aligned");
        self.write64(REG_ACQ, phys_addr);
    }

    /// Read the Admin Completion Queue base address.
    pub fn read_acq(&self) -> u64 {
        self.read64(REG_ACQ)
    }

    // ========================================================================
    // Doorbells — Submission Queue y Tail / Completion Queue y Head
    // ========================================================================

    /// Calculate the byte offset of the Submission Queue y Tail Doorbell.
    ///
    /// Per NVMe spec: 0x1000 + (2y * doorbell_stride)
    fn sq_tail_doorbell_offset(&self, qid: u16) -> usize {
        REG_DOORBELL_BASE + (2 * qid as usize) * self.doorbell_stride
    }

    /// Calculate the byte offset of the Completion Queue y Head Doorbell.
    ///
    /// Per NVMe spec: 0x1000 + ((2y + 1) * doorbell_stride)
    fn cq_head_doorbell_offset(&self, qid: u16) -> usize {
        REG_DOORBELL_BASE + (2 * qid as usize + 1) * self.doorbell_stride
    }

    /// Ring the Submission Queue Tail Doorbell for queue `qid`.
    pub fn write_sq_tail_doorbell(&self, qid: u16, tail: u16) {
        let offset = self.sq_tail_doorbell_offset(qid);
        log::trace!(
            "[nvme:regs] SQ{} tail doorbell <- {} (offset {:#x})",
            qid,
            tail,
            offset
        );
        self.write32(offset, tail as u32);
    }

    /// Ring the Completion Queue Head Doorbell for queue `qid`.
    pub fn write_cq_head_doorbell(&self, qid: u16, head: u16) {
        let offset = self.cq_head_doorbell_offset(qid);
        log::trace!(
            "[nvme:regs] CQ{} head doorbell <- {} (offset {:#x})",
            qid,
            head,
            offset
        );
        self.write32(offset, head as u32);
    }
}
