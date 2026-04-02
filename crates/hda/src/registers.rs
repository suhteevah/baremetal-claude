//! Intel HDA controller registers (PCI BAR0 memory-mapped I/O).
//!
//! All offsets and bit definitions per Intel High Definition Audio
//! Specification 1.0a (June 2010), Section 3 "Controller Register Set".

use core::ptr;

// =============================================================================
// Global register offsets (Section 3.3)
// =============================================================================

/// Global Capabilities (16-bit, offset 0x00)
pub const GCAP: usize = 0x00;
/// Minor Version (8-bit, offset 0x02)
pub const VMIN: usize = 0x02;
/// Major Version (8-bit, offset 0x03)
pub const VMAJ: usize = 0x03;
/// Output Payload Capability (16-bit, offset 0x04)
pub const OUTPAY: usize = 0x04;
/// Input Payload Capability (16-bit, offset 0x06)
pub const INPAY: usize = 0x06;
/// Global Control (32-bit, offset 0x08)
pub const GCTL: usize = 0x08;
/// Wake Enable (16-bit, offset 0x0C)
pub const WAKEEN: usize = 0x0C;
/// State Change Status (16-bit, offset 0x0E)
pub const STATESTS: usize = 0x0E;
/// Global Status (16-bit, offset 0x10)
pub const GSTS: usize = 0x10;
/// Interrupt Control (32-bit, offset 0x20)
pub const INTCTL: usize = 0x20;
/// Interrupt Status (32-bit, offset 0x24)
pub const INTSTS: usize = 0x24;
/// Wall Clock Counter (32-bit, offset 0x30)
pub const WALCLK: usize = 0x30;
/// Stream Synchronization (32-bit, offset 0x34)
pub const SSYNC: usize = 0x34;

// =============================================================================
// CORB registers (Section 3.3.2)
// =============================================================================

/// CORB Lower Base Address (32-bit, offset 0x40)
pub const CORBLBASE: usize = 0x40;
/// CORB Upper Base Address (32-bit, offset 0x44)
pub const CORBUBASE: usize = 0x44;
/// CORB Write Pointer (16-bit, offset 0x48)
pub const CORBWP: usize = 0x48;
/// CORB Read Pointer (16-bit, offset 0x4A)
pub const CORBRP: usize = 0x4A;
/// CORB Control (8-bit, offset 0x4C)
pub const CORBCTL: usize = 0x4C;
/// CORB Status (8-bit, offset 0x4D)
pub const CORBSTS: usize = 0x4D;
/// CORB Size (8-bit, offset 0x4E)
pub const CORBSIZE: usize = 0x4E;

// =============================================================================
// RIRB registers (Section 3.3.3)
// =============================================================================

/// RIRB Lower Base Address (32-bit, offset 0x50)
pub const RIRBLBASE: usize = 0x50;
/// RIRB Upper Base Address (32-bit, offset 0x54)
pub const RIRBUBASE: usize = 0x54;
/// RIRB Write Pointer (16-bit, offset 0x58)
pub const RIRBWP: usize = 0x58;
/// Response Interrupt Count (16-bit, offset 0x5A)
pub const RINTCNT: usize = 0x5A;
/// RIRB Control (8-bit, offset 0x5C)
pub const RIRBCTL: usize = 0x5C;
/// RIRB Status (8-bit, offset 0x5D)
pub const RIRBSTS: usize = 0x5D;
/// RIRB Size (8-bit, offset 0x5E)
pub const RIRBSIZE: usize = 0x5E;

// =============================================================================
// GCAP bit fields (Section 3.3.1.1)
// =============================================================================

/// 64-bit Address OK (bit 0)
pub const GCAP_64OK: u16 = 1 << 0;
/// Number of Serial Data Out signals (bits 2:1)
pub const GCAP_NSDO_SHIFT: u16 = 1;
pub const GCAP_NSDO_MASK: u16 = 0x03;
/// Number of Bidirectional Streams (bits 7:3)
pub const GCAP_BSS_SHIFT: u16 = 3;
pub const GCAP_BSS_MASK: u16 = 0x1F;
/// Number of Input Streams (bits 11:8)
pub const GCAP_ISS_SHIFT: u16 = 8;
pub const GCAP_ISS_MASK: u16 = 0x0F;
/// Number of Output Streams (bits 15:12)
pub const GCAP_OSS_SHIFT: u16 = 12;
pub const GCAP_OSS_MASK: u16 = 0x0F;

// =============================================================================
// GCTL bit fields (Section 3.3.1.5)
// =============================================================================

/// Controller Reset (bit 0) — 0 = enter reset, 1 = exit reset
pub const GCTL_CRST: u32 = 1 << 0;
/// Flush Control (bit 1)
pub const GCTL_FCNTRL: u32 = 1 << 1;
/// Accept Unsolicited Responses (bit 8)
pub const GCTL_UNSOL: u32 = 1 << 8;

// =============================================================================
// INTCTL bit fields (Section 3.3.1.9)
// =============================================================================

/// Controller Interrupt Enable (bit 30)
pub const INTCTL_CIE: u32 = 1 << 30;
/// Global Interrupt Enable (bit 31)
pub const INTCTL_GIE: u32 = 1 << 31;

// =============================================================================
// CORBCTL bit fields (Section 3.3.2.4)
// =============================================================================

/// CORB Memory Error Interrupt Enable (bit 0)
pub const CORBCTL_MEIE: u8 = 1 << 0;
/// CORB DMA Engine Run (bit 1)
pub const CORBCTL_RUN: u8 = 1 << 1;

// =============================================================================
// CORBRP bit fields (Section 3.3.2.3)
// =============================================================================

/// CORB Read Pointer Reset (bit 15)
pub const CORBRP_RST: u16 = 1 << 15;

// =============================================================================
// RIRBCTL bit fields (Section 3.3.3.4)
// =============================================================================

/// RIRB Response Interrupt Control (bit 0)
pub const RIRBCTL_RINTCTL: u8 = 1 << 0;
/// RIRB DMA Engine Run (bit 1)
pub const RIRBCTL_RUN: u8 = 1 << 1;
/// RIRB Response Overrun Interrupt Control (bit 2)
pub const RIRBCTL_ROIC: u8 = 1 << 2;

// =============================================================================
// RIRBSTS bit fields (Section 3.3.3.5)
// =============================================================================

/// Response Interrupt (bit 0)
pub const RIRBSTS_RINTFL: u8 = 1 << 0;
/// Response Overrun Interrupt Status (bit 2)
pub const RIRBSTS_RIRBOIS: u8 = 1 << 2;

// =============================================================================
// RIRBWP bit fields (Section 3.3.3.2)
// =============================================================================

/// RIRB Write Pointer Reset (bit 15)
pub const RIRBWP_RST: u16 = 1 << 15;

// =============================================================================
// CORBSIZE / RIRBSIZE capability encoding (Section 3.3.2.6 / 3.3.3.6)
// =============================================================================

/// Size capability: 2 entries
pub const RINGBUF_SIZE_2: u8 = 0x00;
/// Size capability: 16 entries
pub const RINGBUF_SIZE_16: u8 = 0x01;
/// Size capability: 256 entries
pub const RINGBUF_SIZE_256: u8 = 0x02;

// =============================================================================
// Stream Descriptor registers (Section 3.3.4)
// Each stream descriptor block is 0x20 (32) bytes.
// Output streams start after input streams in the register space.
// Base offset: 0x80 + (stream_index * 0x20)
// =============================================================================

/// Stream Descriptor base offset
pub const SD_BASE: usize = 0x80;
/// Stream Descriptor stride
pub const SD_STRIDE: usize = 0x20;

/// Stream Descriptor Control (24-bit, 3 bytes at offset +0x00)
/// Byte 0: CTL0 — SRST(0), RUN(1), IOCE(2), FEIE(3), DEIE(4)
/// Byte 2: CTL2 — STRIPE(1:0), TP(2), DIR(3), STRM(7:4)
pub const SD_CTL: usize = 0x00;
/// Stream Descriptor Status (8-bit, offset +0x03)
pub const SD_STS: usize = 0x03;
/// Stream Descriptor Link Position in Buffer (32-bit, offset +0x04)
pub const SD_LPIB: usize = 0x04;
/// Stream Descriptor Cyclic Buffer Length (32-bit, offset +0x08)
pub const SD_CBL: usize = 0x08;
/// Stream Descriptor Last Valid Index (16-bit, offset +0x0C)
pub const SD_LVI: usize = 0x0C;
/// Stream Descriptor FIFO Size (16-bit, offset +0x10)
pub const SD_FIFOS: usize = 0x10;
/// Stream Descriptor Format (16-bit, offset +0x12)
pub const SD_FMT: usize = 0x12;
/// Stream Descriptor BDL Pointer Lower (32-bit, offset +0x18)
pub const SD_BDPL: usize = 0x18;
/// Stream Descriptor BDL Pointer Upper (32-bit, offset +0x1C)
pub const SD_BDPU: usize = 0x1C;

// =============================================================================
// SD_CTL bit fields
// =============================================================================

/// Stream Reset (bit 0 of CTL byte 0)
pub const SD_CTL_SRST: u8 = 1 << 0;
/// Stream Run (bit 1 of CTL byte 0)
pub const SD_CTL_RUN: u8 = 1 << 1;
/// Interrupt on Completion Enable (bit 2 of CTL byte 0)
pub const SD_CTL_IOCE: u8 = 1 << 2;
/// FIFO Error Interrupt Enable (bit 3 of CTL byte 0)
pub const SD_CTL_FEIE: u8 = 1 << 3;
/// Descriptor Error Interrupt Enable (bit 4 of CTL byte 0)
pub const SD_CTL_DEIE: u8 = 1 << 4;

// CTL byte 2 (offset +0x02): stream number in bits 7:4, direction in bit 3
/// Stream Number shift (bits 7:4 of CTL byte 2)
pub const SD_CTL2_STRM_SHIFT: u8 = 4;
/// Stream Number mask
pub const SD_CTL2_STRM_MASK: u8 = 0x0F;
/// Bidirectional Direction Control (bit 3 of CTL byte 2): 1 = output
pub const SD_CTL2_DIR: u8 = 1 << 3;

// =============================================================================
// SD_STS bit fields
// =============================================================================

/// Buffer Completion Interrupt Status (bit 2)
pub const SD_STS_BCIS: u8 = 1 << 2;
/// FIFO Error (bit 3)
pub const SD_STS_FIFOE: u8 = 1 << 3;
/// Descriptor Error (bit 4)
pub const SD_STS_DESE: u8 = 1 << 4;
/// FIFO Ready (bit 5)
pub const SD_STS_FIFORDY: u8 = 1 << 5;

// =============================================================================
// Volatile MMIO accessors
// =============================================================================

/// Read a 8-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn read8(base: usize, offset: usize) -> u8 {
    let addr = (base + offset) as *const u8;
    unsafe { ptr::read_volatile(addr) }
}

/// Write a 8-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn write8(base: usize, offset: usize, val: u8) {
    let addr = (base + offset) as *mut u8;
    unsafe { ptr::write_volatile(addr, val) }
}

/// Read a 16-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn read16(base: usize, offset: usize) -> u16 {
    let addr = (base + offset) as *const u16;
    unsafe { ptr::read_volatile(addr) }
}

/// Write a 16-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn write16(base: usize, offset: usize, val: u16) {
    let addr = (base + offset) as *mut u16;
    unsafe { ptr::write_volatile(addr, val) }
}

/// Read a 32-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn read32(base: usize, offset: usize) -> u32 {
    let addr = (base + offset) as *const u32;
    unsafe { ptr::read_volatile(addr) }
}

/// Write a 32-bit register at `base + offset`.
///
/// # Safety
/// `base` must be a valid MMIO base address mapped into the address space.
#[inline]
pub unsafe fn write32(base: usize, offset: usize, val: u32) {
    let addr = (base + offset) as *mut u32;
    unsafe { ptr::write_volatile(addr, val) }
}

/// Compute the base offset of stream descriptor `index`.
///
/// Stream descriptors start at 0x80 with a stride of 0x20 bytes each.
/// Input streams come first, then output streams.
#[inline]
pub const fn stream_desc_offset(index: usize) -> usize {
    SD_BASE + index * SD_STRIDE
}
