//! xHCI Memory-Mapped I/O Register Definitions
//!
//! Per xHCI specification 1.2, the register space is divided into:
//! - Capability registers (base + 0x00): read-only hardware parameters
//! - Operational registers (base + CAPLENGTH): controller command/status
//! - Port registers (operational + 0x400): per-port status/control
//! - Runtime registers (base + RTSOFF): interrupt and microframe index
//! - Doorbell registers (base + DBOFF): per-slot doorbell array

use core::ptr;

// ---------------------------------------------------------------------------
// Capability Register Offsets (from BAR0 base)
// ---------------------------------------------------------------------------

/// Capability register length + HC interface version (8-bit + 16-bit)
pub const CAP_CAPLENGTH: usize = 0x00;
/// Host Controller Interface Version (16-bit at offset 0x02)
pub const CAP_HCIVERSION: usize = 0x02;
/// Structural Parameters 1
pub const CAP_HCSPARAMS1: usize = 0x04;
/// Structural Parameters 2
pub const CAP_HCSPARAMS2: usize = 0x08;
/// Structural Parameters 3
pub const CAP_HCSPARAMS3: usize = 0x0C;
/// Capability Parameters 1
pub const CAP_HCCPARAMS1: usize = 0x10;
/// Doorbell Offset
pub const CAP_DBOFF: usize = 0x14;
/// Runtime Register Space Offset
pub const CAP_RTSOFF: usize = 0x18;
/// Capability Parameters 2
pub const CAP_HCCPARAMS2: usize = 0x1C;

// ---------------------------------------------------------------------------
// Operational Register Offsets (from operational base = BAR0 + CAPLENGTH)
// ---------------------------------------------------------------------------

/// USB Command Register
pub const OP_USBCMD: usize = 0x00;
/// USB Status Register
pub const OP_USBSTS: usize = 0x04;
/// Page Size Register
pub const OP_PAGESIZE: usize = 0x08;
/// Device Notification Control
pub const OP_DNCTRL: usize = 0x14;
/// Command Ring Control Register (64-bit)
pub const OP_CRCR: usize = 0x18;
/// Device Context Base Address Array Pointer (64-bit)
pub const OP_DCBAAP: usize = 0x30;
/// Configure Register
pub const OP_CONFIG: usize = 0x38;

// ---------------------------------------------------------------------------
// USBCMD bits
// ---------------------------------------------------------------------------

/// Run/Stop — 1 = Run, 0 = Stop
pub const USBCMD_RS: u32 = 1 << 0;
/// Host Controller Reset
pub const USBCMD_HCRST: u32 = 1 << 1;
/// Interrupter Enable
pub const USBCMD_INTE: u32 = 1 << 2;
/// Host System Error Enable
pub const USBCMD_HSEE: u32 = 1 << 3;
/// Light Host Controller Reset
pub const USBCMD_LHCRST: u32 = 1 << 7;
/// Controller Save State
pub const USBCMD_CSS: u32 = 1 << 8;
/// Controller Restore State
pub const USBCMD_CRS: u32 = 1 << 9;
/// Enable Wrap Event
pub const USBCMD_EWE: u32 = 1 << 10;

// ---------------------------------------------------------------------------
// USBSTS bits
// ---------------------------------------------------------------------------

/// HC Halted — 1 when Run/Stop = 0 and controller has stopped
pub const USBSTS_HCH: u32 = 1 << 0;
/// Host System Error
pub const USBSTS_HSE: u32 = 1 << 2;
/// Event Interrupt
pub const USBSTS_EINT: u32 = 1 << 3;
/// Port Change Detect
pub const USBSTS_PCD: u32 = 1 << 4;
/// Save State Status
pub const USBSTS_SSS: u32 = 1 << 8;
/// Restore State Status
pub const USBSTS_RSS: u32 = 1 << 9;
/// Save/Restore Error
pub const USBSTS_SRE: u32 = 1 << 10;
/// Controller Not Ready — 1 during reset, 0 when ready
pub const USBSTS_CNR: u32 = 1 << 11;
/// Host Controller Error
pub const USBSTS_HCE: u32 = 1 << 12;

// ---------------------------------------------------------------------------
// CRCR (Command Ring Control Register) bits
// ---------------------------------------------------------------------------

/// Ring Cycle State
pub const CRCR_RCS: u64 = 1 << 0;
/// Command Stop
pub const CRCR_CS: u64 = 1 << 1;
/// Command Abort
pub const CRCR_CA: u64 = 1 << 2;
/// Command Ring Running
pub const CRCR_CRR: u64 = 1 << 3;

// ---------------------------------------------------------------------------
// Port Status and Control Register offsets (from operational base + 0x400)
// Each port register set is 0x10 bytes
// ---------------------------------------------------------------------------

/// Port register set base offset from operational registers
pub const PORT_REGISTER_BASE: usize = 0x400;
/// Size of each port register set
pub const PORT_REGISTER_SET_SIZE: usize = 0x10;

/// Port Status and Control (offset 0x00 within port register set)
pub const PORTSC_OFFSET: usize = 0x00;
/// Port Power Management Status and Control
pub const PORTPMSC_OFFSET: usize = 0x04;
/// Port Link Info
pub const PORTLI_OFFSET: usize = 0x08;
/// Port Hardware LPM Control
pub const PORTHLPMC_OFFSET: usize = 0x0C;

// ---------------------------------------------------------------------------
// PORTSC bits
// ---------------------------------------------------------------------------

/// Current Connect Status
pub const PORTSC_CCS: u32 = 1 << 0;
/// Port Enabled/Disabled
pub const PORTSC_PED: u32 = 1 << 1;
/// Over-current Active
pub const PORTSC_OCA: u32 = 1 << 3;
/// Port Reset
pub const PORTSC_PR: u32 = 1 << 4;
/// Port Link State (bits 8:5) — mask
pub const PORTSC_PLS_MASK: u32 = 0xF << 5;
/// Port Link State shift
pub const PORTSC_PLS_SHIFT: u32 = 5;
/// Port Power
pub const PORTSC_PP: u32 = 1 << 9;
/// Port Speed (bits 13:10) — mask
pub const PORTSC_SPEED_MASK: u32 = 0xF << 10;
/// Port Speed shift
pub const PORTSC_SPEED_SHIFT: u32 = 10;
/// Port Link State Write Strobe
pub const PORTSC_LWS: u32 = 1 << 16;
/// Connect Status Change
pub const PORTSC_CSC: u32 = 1 << 17;
/// Port Enabled/Disabled Change
pub const PORTSC_PEC: u32 = 1 << 18;
/// Warm Port Reset Change
pub const PORTSC_WRC: u32 = 1 << 19;
/// Over-Current Change
pub const PORTSC_OCC: u32 = 1 << 20;
/// Port Reset Change
pub const PORTSC_PRC: u32 = 1 << 21;
/// Port Link State Change
pub const PORTSC_PLC: u32 = 1 << 22;
/// Port Config Error Change
pub const PORTSC_CEC: u32 = 1 << 23;
/// Wake on Connect Enable
pub const PORTSC_WCE: u32 = 1 << 25;
/// Wake on Disconnect Enable
pub const PORTSC_WDE: u32 = 1 << 26;
/// Wake on Over-Current Enable
pub const PORTSC_WOE: u32 = 1 << 27;
/// Device Removable
pub const PORTSC_DR: u32 = 1 << 30;
/// Warm Port Reset
pub const PORTSC_WPR: u32 = 1 << 31;

/// All RW1C status change bits in PORTSC — must be preserved when writing
pub const PORTSC_CHANGE_BITS: u32 =
    PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;

// ---------------------------------------------------------------------------
// Port Speed values (from PORTSC bits 13:10)
// ---------------------------------------------------------------------------

/// Full Speed (USB 1.1, 12 Mb/s)
pub const PORT_SPEED_FULL: u32 = 1;
/// Low Speed (USB 1.0, 1.5 Mb/s)
pub const PORT_SPEED_LOW: u32 = 2;
/// High Speed (USB 2.0, 480 Mb/s)
pub const PORT_SPEED_HIGH: u32 = 3;
/// SuperSpeed (USB 3.0, 5 Gb/s)
pub const PORT_SPEED_SUPER: u32 = 4;
/// SuperSpeedPlus (USB 3.1, 10 Gb/s)
pub const PORT_SPEED_SUPER_PLUS: u32 = 5;

// ---------------------------------------------------------------------------
// Port Link State values (PORTSC bits 8:5)
// ---------------------------------------------------------------------------

pub const PLS_U0: u32 = 0;
pub const PLS_U1: u32 = 1;
pub const PLS_U2: u32 = 2;
pub const PLS_U3: u32 = 3;
pub const PLS_DISABLED: u32 = 4;
pub const PLS_RX_DETECT: u32 = 5;
pub const PLS_INACTIVE: u32 = 6;
pub const PLS_POLLING: u32 = 7;
pub const PLS_RECOVERY: u32 = 8;
pub const PLS_HOT_RESET: u32 = 9;
pub const PLS_COMPLIANCE: u32 = 10;
pub const PLS_TEST: u32 = 11;
pub const PLS_RESUME: u32 = 15;

// ---------------------------------------------------------------------------
// Runtime Register Offsets (from runtime base = BAR0 + RTSOFF)
// ---------------------------------------------------------------------------

/// Microframe Index Register
pub const RT_MFINDEX: usize = 0x00;

/// Interrupter Register Set base (offset 0x20 from runtime base)
pub const RT_IR_BASE: usize = 0x20;
/// Size of each interrupter register set
pub const RT_IR_SET_SIZE: usize = 0x20;

/// Interrupter Management Register (within interrupter set)
pub const IR_IMAN: usize = 0x00;
/// Interrupter Moderation Register
pub const IR_IMOD: usize = 0x04;
/// Event Ring Segment Table Size
pub const IR_ERSTSZ: usize = 0x08;
/// Event Ring Segment Table Base Address (64-bit)
pub const IR_ERSTBA: usize = 0x10;
/// Event Ring Dequeue Pointer (64-bit)
pub const IR_ERDP: usize = 0x18;

// ---------------------------------------------------------------------------
// IMAN bits
// ---------------------------------------------------------------------------

/// Interrupt Pending
pub const IMAN_IP: u32 = 1 << 0;
/// Interrupt Enable
pub const IMAN_IE: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// HCSPARAMS1 field extraction
// ---------------------------------------------------------------------------

/// Extract MaxSlots (bits 7:0)
pub fn hcsparams1_max_slots(val: u32) -> u8 {
    (val & 0xFF) as u8
}

/// Extract MaxIntrs (bits 18:8)
pub fn hcsparams1_max_intrs(val: u32) -> u16 {
    ((val >> 8) & 0x7FF) as u16
}

/// Extract MaxPorts (bits 31:24)
pub fn hcsparams1_max_ports(val: u32) -> u8 {
    ((val >> 24) & 0xFF) as u8
}

// ---------------------------------------------------------------------------
// HCSPARAMS2 field extraction
// ---------------------------------------------------------------------------

/// Extract Max Scratchpad Buffers High (bits 25:21)
pub fn hcsparams2_max_scratchpad_hi(val: u32) -> u8 {
    ((val >> 21) & 0x1F) as u8
}

/// Extract Max Scratchpad Buffers Low (bits 31:27)
pub fn hcsparams2_max_scratchpad_lo(val: u32) -> u8 {
    ((val >> 27) & 0x1F) as u8
}

/// Total scratchpad buffers = (hi << 5) | lo
pub fn hcsparams2_max_scratchpad(val: u32) -> u32 {
    let hi = hcsparams2_max_scratchpad_hi(val) as u32;
    let lo = hcsparams2_max_scratchpad_lo(val) as u32;
    (hi << 5) | lo
}

// ---------------------------------------------------------------------------
// HCCPARAMS1 field extraction
// ---------------------------------------------------------------------------

/// 64-bit Addressing Capability
pub fn hccparams1_ac64(val: u32) -> bool {
    val & (1 << 0) != 0
}

/// Context Size — 0 = 32 bytes, 1 = 64 bytes
pub fn hccparams1_csz(val: u32) -> bool {
    val & (1 << 2) != 0
}

/// xHCI Extended Capabilities Pointer (bits 31:16), offset in DWORDs from BAR0
pub fn hccparams1_xecp(val: u32) -> u16 {
    ((val >> 16) & 0xFFFF) as u16
}

// ---------------------------------------------------------------------------
// Volatile MMIO access helpers
// ---------------------------------------------------------------------------

/// Read a 32-bit register at the given MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO address.
#[inline]
pub unsafe fn mmio_read32(addr: usize) -> u32 {
    ptr::read_volatile(addr as *const u32)
}

/// Write a 32-bit register at the given MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO address.
#[inline]
pub unsafe fn mmio_write32(addr: usize, val: u32) {
    ptr::write_volatile(addr as *mut u32, val);
}

/// Read a 64-bit register at the given MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO address.
#[inline]
pub unsafe fn mmio_read64(addr: usize) -> u64 {
    ptr::read_volatile(addr as *const u64)
}

/// Write a 64-bit register at the given MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO address.
#[inline]
pub unsafe fn mmio_write64(addr: usize, val: u64) {
    ptr::write_volatile(addr as *mut u64, val);
}

// ---------------------------------------------------------------------------
// Register access structs
// ---------------------------------------------------------------------------

/// Provides structured access to xHCI capability registers.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityRegs {
    pub base: usize,
}

impl CapabilityRegs {
    /// Create a new capability register accessor.
    ///
    /// # Safety
    /// `base` must point to the xHCI capability register space (PCI BAR0).
    pub unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// CAPLENGTH — Capability Register Length (8-bit)
    pub fn caplength(&self) -> u8 {
        unsafe { (mmio_read32(self.base + CAP_CAPLENGTH) & 0xFF) as u8 }
    }

    /// HCIVERSION — Host Controller Interface Version Number (16-bit)
    pub fn hciversion(&self) -> u16 {
        unsafe { (mmio_read32(self.base + CAP_CAPLENGTH) >> 16) as u16 }
    }

    /// HCSPARAMS1 — Structural Parameters 1
    pub fn hcsparams1(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_HCSPARAMS1) }
    }

    /// HCSPARAMS2 — Structural Parameters 2
    pub fn hcsparams2(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_HCSPARAMS2) }
    }

    /// HCSPARAMS3 — Structural Parameters 3
    pub fn hcsparams3(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_HCSPARAMS3) }
    }

    /// HCCPARAMS1 — Capability Parameters 1
    pub fn hccparams1(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_HCCPARAMS1) }
    }

    /// DBOFF — Doorbell Offset (bits 31:2 are the offset, bits 1:0 reserved)
    pub fn dboff(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_DBOFF) & !0x3 }
    }

    /// RTSOFF — Runtime Register Space Offset (bits 31:5 are offset, bits 4:0 reserved)
    pub fn rtsoff(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_RTSOFF) & !0x1F }
    }

    /// HCCPARAMS2 — Capability Parameters 2
    pub fn hccparams2(&self) -> u32 {
        unsafe { mmio_read32(self.base + CAP_HCCPARAMS2) }
    }

    /// Maximum number of device slots
    pub fn max_slots(&self) -> u8 {
        hcsparams1_max_slots(self.hcsparams1())
    }

    /// Maximum number of interrupters
    pub fn max_intrs(&self) -> u16 {
        hcsparams1_max_intrs(self.hcsparams1())
    }

    /// Maximum number of ports
    pub fn max_ports(&self) -> u8 {
        hcsparams1_max_ports(self.hcsparams1())
    }

    /// Context size in bytes (32 or 64)
    pub fn context_size(&self) -> usize {
        if hccparams1_csz(self.hccparams1()) {
            64
        } else {
            32
        }
    }

    /// Number of scratchpad buffers required
    pub fn max_scratchpad_buffers(&self) -> u32 {
        hcsparams2_max_scratchpad(self.hcsparams2())
    }

    /// Base address of operational registers
    pub fn operational_base(&self) -> usize {
        self.base + self.caplength() as usize
    }

    /// Base address of runtime registers
    pub fn runtime_base(&self) -> usize {
        self.base + self.rtsoff() as usize
    }

    /// Base address of doorbell registers
    pub fn doorbell_base(&self) -> usize {
        self.base + self.dboff() as usize
    }
}

/// Provides structured access to xHCI operational registers.
#[derive(Debug, Clone, Copy)]
pub struct OperationalRegs {
    pub base: usize,
}

impl OperationalRegs {
    /// Create a new operational register accessor.
    ///
    /// # Safety
    /// `base` must point to the xHCI operational register space.
    pub unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// Read USBCMD
    pub fn usbcmd(&self) -> u32 {
        unsafe { mmio_read32(self.base + OP_USBCMD) }
    }

    /// Write USBCMD
    pub fn set_usbcmd(&self, val: u32) {
        unsafe { mmio_write32(self.base + OP_USBCMD, val) }
    }

    /// Read USBSTS
    pub fn usbsts(&self) -> u32 {
        unsafe { mmio_read32(self.base + OP_USBSTS) }
    }

    /// Write USBSTS (to clear status bits — write-1-to-clear)
    pub fn set_usbsts(&self, val: u32) {
        unsafe { mmio_write32(self.base + OP_USBSTS, val) }
    }

    /// Read PAGESIZE register. Bit N set means (2^(N+12)) page size supported.
    pub fn pagesize(&self) -> u32 {
        unsafe { mmio_read32(self.base + OP_PAGESIZE) }
    }

    /// Read DNCTRL
    pub fn dnctrl(&self) -> u32 {
        unsafe { mmio_read32(self.base + OP_DNCTRL) }
    }

    /// Write DNCTRL
    pub fn set_dnctrl(&self, val: u32) {
        unsafe { mmio_write32(self.base + OP_DNCTRL, val) }
    }

    /// Read CRCR (64-bit Command Ring Control Register)
    pub fn crcr(&self) -> u64 {
        unsafe { mmio_read64(self.base + OP_CRCR) }
    }

    /// Write CRCR (64-bit)
    pub fn set_crcr(&self, val: u64) {
        unsafe { mmio_write64(self.base + OP_CRCR, val) }
    }

    /// Read DCBAAP (64-bit Device Context Base Address Array Pointer)
    pub fn dcbaap(&self) -> u64 {
        unsafe { mmio_read64(self.base + OP_DCBAAP) }
    }

    /// Write DCBAAP (64-bit)
    pub fn set_dcbaap(&self, val: u64) {
        unsafe { mmio_write64(self.base + OP_DCBAAP, val) }
    }

    /// Read CONFIG
    pub fn config(&self) -> u32 {
        unsafe { mmio_read32(self.base + OP_CONFIG) }
    }

    /// Write CONFIG — MaxSlotsEn in bits 7:0
    pub fn set_config(&self, val: u32) {
        unsafe { mmio_write32(self.base + OP_CONFIG, val) }
    }

    /// Read a port's PORTSC register. `port` is 1-indexed per xHCI spec.
    pub fn portsc(&self, port: u8) -> u32 {
        assert!(port >= 1, "xHCI ports are 1-indexed");
        let offset = PORT_REGISTER_BASE + (port as usize - 1) * PORT_REGISTER_SET_SIZE + PORTSC_OFFSET;
        unsafe { mmio_read32(self.base + offset) }
    }

    /// Write a port's PORTSC register. Caller must preserve RW1C bits.
    /// `port` is 1-indexed.
    pub fn set_portsc(&self, port: u8, val: u32) {
        assert!(port >= 1, "xHCI ports are 1-indexed");
        let offset = PORT_REGISTER_BASE + (port as usize - 1) * PORT_REGISTER_SET_SIZE + PORTSC_OFFSET;
        unsafe { mmio_write32(self.base + offset, val) }
    }

    /// Read a port's PORTPMSC register. `port` is 1-indexed.
    pub fn portpmsc(&self, port: u8) -> u32 {
        assert!(port >= 1, "xHCI ports are 1-indexed");
        let offset = PORT_REGISTER_BASE + (port as usize - 1) * PORT_REGISTER_SET_SIZE + PORTPMSC_OFFSET;
        unsafe { mmio_read32(self.base + offset) }
    }

    /// Read a port's PORTLI register. `port` is 1-indexed.
    pub fn portli(&self, port: u8) -> u32 {
        assert!(port >= 1, "xHCI ports are 1-indexed");
        let offset = PORT_REGISTER_BASE + (port as usize - 1) * PORT_REGISTER_SET_SIZE + PORTLI_OFFSET;
        unsafe { mmio_read32(self.base + offset) }
    }

    /// Read a port's PORTHLPMC register. `port` is 1-indexed.
    pub fn porthlpmc(&self, port: u8) -> u32 {
        assert!(port >= 1, "xHCI ports are 1-indexed");
        let offset = PORT_REGISTER_BASE + (port as usize - 1) * PORT_REGISTER_SET_SIZE + PORTHLPMC_OFFSET;
        unsafe { mmio_read32(self.base + offset) }
    }

    /// Check if the controller is halted
    pub fn is_halted(&self) -> bool {
        self.usbsts() & USBSTS_HCH != 0
    }

    /// Check if the controller is ready (CNR = 0)
    pub fn is_ready(&self) -> bool {
        self.usbsts() & USBSTS_CNR == 0
    }

    /// Extract port speed from PORTSC
    pub fn port_speed(&self, port: u8) -> u32 {
        (self.portsc(port) & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT
    }

    /// Extract port link state from PORTSC
    pub fn port_link_state(&self, port: u8) -> u32 {
        (self.portsc(port) & PORTSC_PLS_MASK) >> PORTSC_PLS_SHIFT
    }
}

/// Provides structured access to xHCI runtime registers.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeRegs {
    pub base: usize,
}

impl RuntimeRegs {
    /// Create a new runtime register accessor.
    ///
    /// # Safety
    /// `base` must point to the xHCI runtime register space.
    pub unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// Read MFINDEX (Microframe Index)
    pub fn mfindex(&self) -> u32 {
        unsafe { mmio_read32(self.base + RT_MFINDEX) }
    }

    /// Base address of interrupter register set `n` (0-indexed)
    fn ir_base(&self, n: u16) -> usize {
        self.base + RT_IR_BASE + (n as usize) * RT_IR_SET_SIZE
    }

    /// Read IMAN for interrupter `n`
    pub fn iman(&self, n: u16) -> u32 {
        unsafe { mmio_read32(self.ir_base(n) + IR_IMAN) }
    }

    /// Write IMAN for interrupter `n`
    pub fn set_iman(&self, n: u16, val: u32) {
        unsafe { mmio_write32(self.ir_base(n) + IR_IMAN, val) }
    }

    /// Read IMOD for interrupter `n`
    pub fn imod(&self, n: u16) -> u32 {
        unsafe { mmio_read32(self.ir_base(n) + IR_IMOD) }
    }

    /// Write IMOD for interrupter `n`
    pub fn set_imod(&self, n: u16, val: u32) {
        unsafe { mmio_write32(self.ir_base(n) + IR_IMOD, val) }
    }

    /// Read ERSTSZ (Event Ring Segment Table Size) for interrupter `n`
    pub fn erstsz(&self, n: u16) -> u32 {
        unsafe { mmio_read32(self.ir_base(n) + IR_ERSTSZ) }
    }

    /// Write ERSTSZ for interrupter `n`
    pub fn set_erstsz(&self, n: u16, val: u32) {
        unsafe { mmio_write32(self.ir_base(n) + IR_ERSTSZ, val) }
    }

    /// Read ERSTBA (64-bit Event Ring Segment Table Base Address) for interrupter `n`
    pub fn erstba(&self, n: u16) -> u64 {
        unsafe { mmio_read64(self.ir_base(n) + IR_ERSTBA) }
    }

    /// Write ERSTBA for interrupter `n`
    pub fn set_erstba(&self, n: u16, val: u64) {
        unsafe { mmio_write64(self.ir_base(n) + IR_ERSTBA, val) }
    }

    /// Read ERDP (64-bit Event Ring Dequeue Pointer) for interrupter `n`
    pub fn erdp(&self, n: u16) -> u64 {
        unsafe { mmio_read64(self.ir_base(n) + IR_ERDP) }
    }

    /// Write ERDP for interrupter `n`
    pub fn set_erdp(&self, n: u16, val: u64) {
        unsafe { mmio_write64(self.ir_base(n) + IR_ERDP, val) }
    }
}

/// Provides access to xHCI doorbell registers.
#[derive(Debug, Clone, Copy)]
pub struct DoorbellRegs {
    pub base: usize,
}

impl DoorbellRegs {
    /// Create a new doorbell register accessor.
    ///
    /// # Safety
    /// `base` must point to the xHCI doorbell register array.
    pub unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// Ring doorbell for slot `slot` (0 = host controller, 1..N = device slots).
    /// `target` is the doorbell target value (endpoint index or 0 for command ring).
    /// `stream_id` is the stream ID (0 if not using streams).
    pub fn ring(&self, slot: u8, target: u8, stream_id: u16) {
        let val = (stream_id as u32) << 16 | (target as u32);
        log::trace!("xhci: ringing doorbell slot={} target={} stream_id={}", slot, target, stream_id);
        unsafe { mmio_write32(self.base + (slot as usize) * 4, val) }
    }

    /// Ring the host controller doorbell (slot 0) for command ring
    pub fn ring_command(&self) {
        self.ring(0, 0, 0);
    }

    /// Ring doorbell for a device endpoint.
    /// `slot` is 1-indexed device slot, `dci` is device context index (1 = EP0, 2 = EP1 OUT, 3 = EP1 IN, ...).
    pub fn ring_endpoint(&self, slot: u8, dci: u8) {
        self.ring(slot, dci, 0);
    }
}
