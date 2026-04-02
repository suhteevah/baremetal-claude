//! Intel e1000/e1000e/igc register definitions.
//!
//! All registers are memory-mapped via PCI BAR0. Offsets are from the Intel
//! 82540EM (e1000) and I225 (igc) datasheets. Where I225 offsets differ from
//! e1000, both are provided with the I225 variant noted.

// =========================================================================
// General Control & Status
// =========================================================================

/// Device Control Register.
pub const CTRL: u32 = 0x0000;
/// Device Status Register (read-only).
pub const STATUS: u32 = 0x0008;
/// EEPROM/Flash Control & Data Register.
pub const EECD: u32 = 0x0010;
/// EEPROM Read Register.
pub const EERD: u32 = 0x0014;
/// Flash Access Register.
pub const FLA: u32 = 0x001C;
/// Extended Device Control Register.
pub const CTRL_EXT: u32 = 0x0018;
/// MDI Control Register (PHY access).
pub const MDIC: u32 = 0x0020;
/// Flow Control Address Low.
pub const FCAL: u32 = 0x0028;
/// Flow Control Address High.
pub const FCAH: u32 = 0x002C;
/// Flow Control Type.
pub const FCT: u32 = 0x0030;
/// Flow Control Transmit Timer Value.
pub const FCTTV: u32 = 0x0170;

// =========================================================================
// Interrupt Registers
// =========================================================================

/// Interrupt Cause Read (read clears).
pub const ICR: u32 = 0x00C0;
/// Interrupt Cause Set (write to trigger interrupt).
pub const ICS: u32 = 0x00C8;
/// Interrupt Mask Set/Read.
pub const IMS: u32 = 0x00D0;
/// Interrupt Mask Clear (write to disable interrupts).
pub const IMC: u32 = 0x00D8;

// =========================================================================
// Receive Registers
// =========================================================================

/// Receive Control Register.
pub const RCTL: u32 = 0x0100;
/// Receive Descriptor Base Address Low.
pub const RDBAL: u32 = 0x2800;
/// Receive Descriptor Base Address High.
pub const RDBAH: u32 = 0x2804;
/// Receive Descriptor Length (bytes, must be 128-byte aligned).
pub const RDLEN: u32 = 0x2808;
/// Receive Descriptor Head.
pub const RDH: u32 = 0x2810;
/// Receive Descriptor Tail.
pub const RDT: u32 = 0x2818;
/// Receive Delay Timer.
pub const RDTR: u32 = 0x2820;
/// Receive Interrupt Absolute Delay Timer.
pub const RADV: u32 = 0x282C;
/// Receive Small Packet Detect Interrupt (e1000e/igc).
pub const RSRPD: u32 = 0x2C00;

// =========================================================================
// Transmit Registers
// =========================================================================

/// Transmit Control Register.
pub const TCTL: u32 = 0x0400;
/// Transmit IPG (Inter-Packet Gap) Register.
pub const TIPG: u32 = 0x0410;
/// Transmit Descriptor Base Address Low.
pub const TDBAL: u32 = 0x3800;
/// Transmit Descriptor Base Address High.
pub const TDBAH: u32 = 0x3804;
/// Transmit Descriptor Length (bytes, must be 128-byte aligned).
pub const TDLEN: u32 = 0x3808;
/// Transmit Descriptor Head.
pub const TDH: u32 = 0x3810;
/// Transmit Descriptor Tail.
pub const TDT: u32 = 0x3818;
/// Transmit Interrupt Delay Value.
pub const TIDV: u32 = 0x3820;
/// Transmit Absolute Interrupt Delay Value.
pub const TADV: u32 = 0x382C;

// =========================================================================
// MAC Address Registers
// =========================================================================

/// Receive Address Low (bytes 0-3 of MAC, little-endian).
pub const RAL0: u32 = 0x5400;
/// Receive Address High (bytes 4-5 of MAC in bits [15:0], AV bit in [31]).
pub const RAH0: u32 = 0x5404;

// =========================================================================
// Multicast Table Array — 128 entries of 32 bits each (4096 bit vector).
// =========================================================================

/// Base of the Multicast Table Array.
pub const MTA_BASE: u32 = 0x5200;
/// Number of MTA entries.
pub const MTA_COUNT: usize = 128;

// =========================================================================
// Statistics Registers (selected)
// =========================================================================

/// CRC Error Count.
pub const CRCERRS: u32 = 0x4000;
/// Missed Packets Count.
pub const MPC: u32 = 0x4010;
/// Good Packets Received Count.
pub const GPRC: u32 = 0x4074;
/// Good Packets Transmitted Count.
pub const GPTC: u32 = 0x4080;
/// Total Packets Received (low 32 bits).
pub const TPR: u32 = 0x40D0;
/// Total Packets Transmitted (low 32 bits).
pub const TPT: u32 = 0x40D4;

// =========================================================================
// CTRL Register Bit Definitions (offset 0x0000)
// =========================================================================

/// Full-Duplex mode.
pub const CTRL_FD: u32 = 1 << 0;
/// GIO Master Disable (I225).
pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
/// Link Reset (e1000).
pub const CTRL_LRST: u32 = 1 << 3;
/// Auto-Speed Detection Enable.
pub const CTRL_ASDE: u32 = 1 << 5;
/// Set Link Up.
pub const CTRL_SLU: u32 = 1 << 6;
/// Speed selection bits [9:8].
pub const CTRL_SPEED_SHIFT: u32 = 8;
pub const CTRL_SPEED_MASK: u32 = 0x3 << CTRL_SPEED_SHIFT;
pub const CTRL_SPEED_10: u32 = 0 << CTRL_SPEED_SHIFT;
pub const CTRL_SPEED_100: u32 = 1 << CTRL_SPEED_SHIFT;
pub const CTRL_SPEED_1000: u32 = 2 << CTRL_SPEED_SHIFT;
/// Force Speed.
pub const CTRL_FRCSPD: u32 = 1 << 11;
/// Force Duplex.
pub const CTRL_FRCDPLX: u32 = 1 << 12;
/// Software Reset — self-clearing.
pub const CTRL_RST: u32 = 1 << 26;
/// Receive Flow Control Enable.
pub const CTRL_RFCE: u32 = 1 << 27;
/// Transmit Flow Control Enable.
pub const CTRL_TFCE: u32 = 1 << 28;
/// VLAN Mode Enable.
pub const CTRL_VME: u32 = 1 << 30;
/// PHY Reset.
pub const CTRL_PHY_RST: u32 = 1 << 31;

// =========================================================================
// STATUS Register Bit Definitions (offset 0x0008)
// =========================================================================

/// Full Duplex indication.
pub const STATUS_FD: u32 = 1 << 0;
/// Link Up indication.
pub const STATUS_LU: u32 = 1 << 1;
/// Transmission Paused.
pub const STATUS_TXOFF: u32 = 1 << 4;
/// Speed indication bits [7:6].
pub const STATUS_SPEED_SHIFT: u32 = 6;
pub const STATUS_SPEED_MASK: u32 = 0x3 << STATUS_SPEED_SHIFT;
pub const STATUS_SPEED_10: u32 = 0 << STATUS_SPEED_SHIFT;
pub const STATUS_SPEED_100: u32 = 1 << STATUS_SPEED_SHIFT;
pub const STATUS_SPEED_1000: u32 = 2 << STATUS_SPEED_SHIFT;
/// GIO Master Enable Status (I225).
pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;

// =========================================================================
// RCTL Register Bit Definitions (offset 0x0100)
// =========================================================================

/// Receiver Enable.
pub const RCTL_EN: u32 = 1 << 1;
/// Store Bad Packets.
pub const RCTL_SBP: u32 = 1 << 2;
/// Unicast Promiscuous Enable.
pub const RCTL_UPE: u32 = 1 << 3;
/// Multicast Promiscuous Enable.
pub const RCTL_MPE: u32 = 1 << 4;
/// Long Packet Enable (>1522 bytes).
pub const RCTL_LPE: u32 = 1 << 5;
/// Loopback Mode bits [7:6].
pub const RCTL_LBM_SHIFT: u32 = 6;
pub const RCTL_LBM_NONE: u32 = 0 << RCTL_LBM_SHIFT;
/// Receive Descriptor Minimum Threshold Size [9:8].
pub const RCTL_RDMTS_SHIFT: u32 = 8;
pub const RCTL_RDMTS_HALF: u32 = 0 << RCTL_RDMTS_SHIFT;
pub const RCTL_RDMTS_QUARTER: u32 = 1 << RCTL_RDMTS_SHIFT;
pub const RCTL_RDMTS_EIGHTH: u32 = 2 << RCTL_RDMTS_SHIFT;
/// Multicast Offset bits [13:12] — selects which bits of multicast addr to use.
pub const RCTL_MO_SHIFT: u32 = 12;
/// Broadcast Accept Mode.
pub const RCTL_BAM: u32 = 1 << 15;
/// Receive Buffer Size bits [17:16] (when BSEX=0).
pub const RCTL_BSIZE_SHIFT: u32 = 16;
/// 2048-byte receive buffers (BSIZE=00, BSEX=0).
pub const RCTL_BSIZE_2048: u32 = 0 << RCTL_BSIZE_SHIFT;
/// 1024-byte receive buffers.
pub const RCTL_BSIZE_1024: u32 = 1 << RCTL_BSIZE_SHIFT;
/// 512-byte receive buffers.
pub const RCTL_BSIZE_512: u32 = 2 << RCTL_BSIZE_SHIFT;
/// 256-byte receive buffers.
pub const RCTL_BSIZE_256: u32 = 3 << RCTL_BSIZE_SHIFT;
/// Buffer Size Extension (when set, BSIZE values are 16x larger).
pub const RCTL_BSEX: u32 = 1 << 25;
/// Strip Ethernet CRC from received frames.
pub const RCTL_SECRC: u32 = 1 << 26;

// =========================================================================
// TCTL Register Bit Definitions (offset 0x0400)
// =========================================================================

/// Transmitter Enable.
pub const TCTL_EN: u32 = 1 << 1;
/// Pad Short Packets (to 64 bytes).
pub const TCTL_PSP: u32 = 1 << 3;
/// Collision Threshold bits [11:4] — number of retransmission attempts.
pub const TCTL_CT_SHIFT: u32 = 4;
/// Cold (Collision Distance) bits [21:12] — byte times to back off.
pub const TCTL_COLD_SHIFT: u32 = 12;
/// Full-duplex collision distance (standard value: 64 byte times for GbE).
pub const TCTL_COLD_FD: u32 = 0x40 << TCTL_COLD_SHIFT;
/// Half-duplex collision distance (standard value: 512 byte times).
pub const TCTL_COLD_HD: u32 = 0x200 << TCTL_COLD_SHIFT;
/// Re-transmit on Late Collision.
pub const TCTL_RTLC: u32 = 1 << 24;

// =========================================================================
// Interrupt Cause Bit Definitions (ICR/ICS/IMS/IMC)
// =========================================================================

/// Transmit Descriptor Written Back.
pub const ICR_TXDW: u32 = 1 << 0;
/// Transmit Queue Empty.
pub const ICR_TXQE: u32 = 1 << 1;
/// Link Status Change.
pub const ICR_LSC: u32 = 1 << 2;
/// Receive Sequence Error (e1000).
pub const ICR_RXSEQ: u32 = 1 << 3;
/// Receive Descriptor Minimum Threshold Reached.
pub const ICR_RXDMT0: u32 = 1 << 4;
/// Receiver Overrun.
pub const ICR_RXO: u32 = 1 << 6;
/// Receive Timer Interrupt (packet received).
pub const ICR_RXT0: u32 = 1 << 7;
/// MDIO Access Complete.
pub const ICR_MDAC: u32 = 1 << 9;
/// PHY Interrupt.
pub const ICR_PHYINT: u32 = 1 << 12;

// =========================================================================
// EERD (EEPROM Read) Register Bit Definitions
// =========================================================================

/// Start EEPROM read.
pub const EERD_START: u32 = 1 << 0;
/// EEPROM read done (e1000: bit 4, e1000e/igc: bit 1).
pub const EERD_DONE_E1000: u32 = 1 << 4;
pub const EERD_DONE_E1000E: u32 = 1 << 1;
/// Address shift (e1000: 8, e1000e/igc: 2).
pub const EERD_ADDR_SHIFT_E1000: u32 = 8;
pub const EERD_ADDR_SHIFT_E1000E: u32 = 2;
/// Data is always in bits [31:16].
pub const EERD_DATA_SHIFT: u32 = 16;
pub const EERD_DATA_MASK: u32 = 0xFFFF << EERD_DATA_SHIFT;

// =========================================================================
// MDIC Register Bit Definitions (PHY access via MDI)
// =========================================================================

/// Data mask [15:0].
pub const MDIC_DATA_MASK: u32 = 0xFFFF;
/// PHY register address [20:16].
pub const MDIC_REGADD_SHIFT: u32 = 16;
/// PHY address [25:21].
pub const MDIC_PHYADD_SHIFT: u32 = 21;
/// Opcode [27:26]: 01 = write, 10 = read.
pub const MDIC_OP_WRITE: u32 = 1 << 26;
pub const MDIC_OP_READ: u32 = 2 << 26;
/// Ready bit — set by hardware when operation completes.
pub const MDIC_READY: u32 = 1 << 28;
/// Interrupt Enable for MDIO completion.
pub const MDIC_INT_EN: u32 = 1 << 29;
/// Error bit.
pub const MDIC_ERROR: u32 = 1 << 30;

// =========================================================================
// TIPG (Transmit Inter-Packet Gap) recommended values
// =========================================================================

/// Standard TIPG value for e1000 (IPGT=10, IPGR1=10, IPGR2=10).
/// IEEE 802.3 standard: IPGT=10 for full duplex.
pub const TIPG_DEFAULT: u32 = 10 | (10 << 10) | (10 << 20);

// =========================================================================
// MMIO helpers
// =========================================================================

/// Read a 32-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into virtual memory.
/// `offset` must be a valid register offset.
#[inline]
pub unsafe fn read_reg(base: *mut u8, offset: u32) -> u32 {
    let ptr = base.add(offset as usize) as *const u32;
    let val = unsafe { core::ptr::read_volatile(ptr) };
    val
}

/// Write a 32-bit value to a register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into virtual memory.
/// `offset` must be a valid register offset.
#[inline]
pub unsafe fn write_reg(base: *mut u8, offset: u32, val: u32) {
    let ptr = base.add(offset as usize) as *mut u32;
    unsafe { core::ptr::write_volatile(ptr, val) };
}

/// Set specific bits in a register (read-modify-write).
///
/// # Safety
/// Same as `read_reg` / `write_reg`.
#[inline]
pub unsafe fn set_reg_bits(base: *mut u8, offset: u32, bits: u32) {
    let val = unsafe { read_reg(base, offset) };
    unsafe { write_reg(base, offset, val | bits) };
}

/// Clear specific bits in a register (read-modify-write).
///
/// # Safety
/// Same as `read_reg` / `write_reg`.
#[inline]
pub unsafe fn clear_reg_bits(base: *mut u8, offset: u32, bits: u32) {
    let val = unsafe { read_reg(base, offset) };
    unsafe { write_reg(base, offset, val & !bits) };
}
