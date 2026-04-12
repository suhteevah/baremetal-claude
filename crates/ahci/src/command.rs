//! AHCI command construction: command headers, command tables, FIS, PRDT.
//!
//! The HBA reads a Command List (array of Command Headers) from DMA memory.
//! Each Command Header points to a Command Table containing the FIS (Frame
//! Information Structure) and PRDT (Physical Region Descriptor Table) entries.
//!
//! ## Memory Layout
//!
//! ```text
//! Command List (1 KiB, 32 entries)
//! ┌────────────────────┐
//! │ Command Header [0] │──────> Command Table (128+ bytes)
//! │  - flags, PRDTL   │        ┌──────────────────────┐
//! │  - CTBA pointer   │        │ CFIS (64 bytes)      │ ← FIS Register H2D
//! ├────────────────────┤        │ ACMD (16 bytes)      │ ← ATAPI command (if any)
//! │ Command Header [1] │        │ Reserved (48 bytes)  │
//! │ ...               │        ├──────────────────────┤
//! │ Command Header [31]│        │ PRDT Entry [0]       │ ← data buffer pointer + size
//! └────────────────────┘        │ PRDT Entry [1]       │
//!                               │ ...                  │
//!                               └──────────────────────┘
//! ```
//!
//! Reference: AHCI 1.3.1 spec, Section 4 (Port System Memory Structures).

use alloc::alloc::{alloc_zeroed, Layout};
use core::ptr;

use crate::VirtToPhys;

// ---------------------------------------------------------------------------
// FIS types
// ---------------------------------------------------------------------------

/// Register FIS — Host to Device (H2D). Used for issuing ATA commands.
pub const FIS_TYPE_REG_H2D: u8 = 0x27;
/// Register FIS — Device to Host (D2H). Device sends status/completion.
pub const FIS_TYPE_REG_D2H: u8 = 0x34;
/// DMA Activate FIS — Device to Host.
pub const FIS_TYPE_DMA_ACT: u8 = 0x39;
/// DMA Setup FIS — bidirectional.
pub const FIS_TYPE_DMA_SETUP: u8 = 0x41;
/// Data FIS — bidirectional.
pub const FIS_TYPE_DATA: u8 = 0x46;
/// BIST Activate FIS.
pub const FIS_TYPE_BIST: u8 = 0x58;
/// PIO Setup FIS — Device to Host.
pub const FIS_TYPE_PIO_SETUP: u8 = 0x5F;
/// Set Device Bits FIS — Device to Host (for NCQ).
pub const FIS_TYPE_DEV_BITS: u8 = 0xA1;

// ---------------------------------------------------------------------------
// ATA commands
// ---------------------------------------------------------------------------

/// READ DMA EXT — read sectors using DMA, 48-bit LBA.
pub const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
/// WRITE DMA EXT — write sectors using DMA, 48-bit LBA.
pub const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
/// IDENTIFY DEVICE — returns 512 bytes of drive information.
pub const ATA_CMD_IDENTIFY: u8 = 0xEC;
/// FLUSH CACHE EXT — flush volatile write cache, 48-bit.
pub const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
/// SET FEATURES.
pub const ATA_CMD_SET_FEATURES: u8 = 0xEF;

// ---------------------------------------------------------------------------
// FIS Register H2D (20 bytes)
// ---------------------------------------------------------------------------

/// A Register FIS (Frame Information Structure) from Host to Device.
///
/// This is the primary mechanism for sending ATA commands to a SATA device.
/// The 20-byte structure is laid out in memory exactly as the HBA expects,
/// matching the SATA specification's Register FIS format.
///
/// The FIS encodes the ATA command register file: command opcode, LBA address
/// (up to 48-bit), sector count, device/head, and features. The C (Command) bit
/// in byte 1 distinguishes a command register write from a control register write.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FisRegH2d {
    /// FIS type — must be `FIS_TYPE_REG_H2D` (0x27).
    pub fis_type: u8,
    /// Bits 7:4 = PM port, bit 7 = C (command vs control).
    /// Set bit 7 to indicate this is a command (not control) register update.
    pub pm_port_c: u8,
    /// ATA command register.
    pub command: u8,
    /// Features register (low byte).
    pub feature_lo: u8,

    /// LBA bits 7:0.
    pub lba0: u8,
    /// LBA bits 15:8.
    pub lba1: u8,
    /// LBA bits 23:16.
    pub lba2: u8,
    /// Device register. Bit 6 = LBA mode.
    pub device: u8,

    /// LBA bits 31:24.
    pub lba3: u8,
    /// LBA bits 39:32.
    pub lba4: u8,
    /// LBA bits 47:40.
    pub lba5: u8,
    /// Features register (high byte).
    pub feature_hi: u8,

    /// Sector count (low byte).
    pub count_lo: u8,
    /// Sector count (high byte).
    pub count_hi: u8,
    /// Isochronous Command Completion.
    pub icc: u8,
    /// Control register.
    pub control: u8,

    /// Reserved (4 bytes to pad to 20 bytes total).
    pub _reserved: [u8; 4],
}

impl FisRegH2d {
    /// Create a zeroed FIS with the type field set.
    pub fn new() -> Self {
        Self {
            fis_type: FIS_TYPE_REG_H2D,
            pm_port_c: 0,
            command: 0,
            feature_lo: 0,
            lba0: 0,
            lba1: 0,
            lba2: 0,
            device: 0,
            lba3: 0,
            lba4: 0,
            lba5: 0,
            feature_hi: 0,
            count_lo: 0,
            count_hi: 0,
            icc: 0,
            control: 0,
            _reserved: [0; 4],
        }
    }

    /// Set the C bit (command register update, not control).
    pub fn set_command_bit(&mut self) {
        self.pm_port_c |= 0x80; // bit 7 = C
    }

    /// Set a 48-bit LBA address.
    pub fn set_lba(&mut self, lba: u64) {
        self.lba0 = (lba & 0xFF) as u8;
        self.lba1 = ((lba >> 8) & 0xFF) as u8;
        self.lba2 = ((lba >> 16) & 0xFF) as u8;
        self.lba3 = ((lba >> 24) & 0xFF) as u8;
        self.lba4 = ((lba >> 32) & 0xFF) as u8;
        self.lba5 = ((lba >> 40) & 0xFF) as u8;
        self.device = 1 << 6; // LBA mode
    }

    /// Set the sector count (16-bit, for 48-bit commands).
    pub fn set_count(&mut self, count: u16) {
        self.count_lo = (count & 0xFF) as u8;
        self.count_hi = ((count >> 8) & 0xFF) as u8;
    }
}

// ---------------------------------------------------------------------------
// Command Header (32 bytes, in Command List)
// ---------------------------------------------------------------------------

/// A single Command Header in the Command List.
///
/// The Command List holds 32 of these. Each points to a Command Table
/// in system memory.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct CommandHeader {
    /// DW0: flags.
    /// Bits 4:0 = CFL (Command FIS Length in DWORDs, min 2).
    /// Bit 5 = A (ATAPI).
    /// Bit 6 = W (Write — direction: 1 = H2D, 0 = D2H).
    /// Bit 7 = P (Prefetchable).
    /// Bit 8 = R (Reset — SRST for this port).
    /// Bit 9 = B (BIST).
    /// Bit 10 = C (Clear Busy upon R_OK).
    /// Bits 15:12 = PMP (Port Multiplier Port).
    /// Bits 31:16 = PRDTL (Physical Region Descriptor Table Length).
    pub flags_prdtl: u32,
    /// DW1: PRDBC — Physical Region Descriptor Byte Count (status, set by HBA).
    pub prdbc: u32,
    /// DW2: CTBA — Command Table Descriptor Base Address (128-byte aligned).
    pub ctba: u32,
    /// DW3: CTBAU — Command Table Descriptor Base Address Upper 32 bits.
    pub ctbau: u32,
    /// DW4-7: Reserved.
    pub _reserved: [u32; 4],
}

// Command Header flag bits
pub const CMD_HDR_CFL_MASK: u32 = 0x1F;
pub const CMD_HDR_A: u32 = 1 << 5;   // ATAPI
pub const CMD_HDR_W: u32 = 1 << 6;   // Write direction
pub const CMD_HDR_P: u32 = 1 << 7;   // Prefetchable
pub const CMD_HDR_R: u32 = 1 << 8;   // Reset
pub const CMD_HDR_B: u32 = 1 << 9;   // BIST
pub const CMD_HDR_C: u32 = 1 << 10;  // Clear Busy upon R_OK
pub const CMD_HDR_PMP_SHIFT: u32 = 12;
pub const CMD_HDR_PRDTL_SHIFT: u32 = 16;

impl CommandHeader {
    /// Create a zeroed command header.
    pub fn zeroed() -> Self {
        Self {
            flags_prdtl: 0,
            prdbc: 0,
            ctba: 0,
            ctbau: 0,
            _reserved: [0; 4],
        }
    }

    /// Set the Command FIS Length (in DWORDs). For a FIS_REG_H2D, this is 5.
    pub fn set_cfl(&mut self, dwords: u8) {
        self.flags_prdtl = (self.flags_prdtl & !CMD_HDR_CFL_MASK) | (dwords as u32 & 0x1F);
    }

    /// Set the PRDT length (number of PRDT entries).
    pub fn set_prdtl(&mut self, count: u16) {
        self.flags_prdtl =
            (self.flags_prdtl & 0xFFFF) | ((count as u32) << CMD_HDR_PRDTL_SHIFT);
    }

    /// Set the write direction flag.
    pub fn set_write(&mut self, write: bool) {
        if write {
            self.flags_prdtl |= CMD_HDR_W;
        } else {
            self.flags_prdtl &= !CMD_HDR_W;
        }
    }

    /// Set the Command Table base address (128-byte aligned).
    pub fn set_ctba(&mut self, addr: u64) {
        self.ctba = addr as u32;
        self.ctbau = (addr >> 32) as u32;
    }
}

// ---------------------------------------------------------------------------
// PRDT Entry (16 bytes)
// ---------------------------------------------------------------------------

/// A Physical Region Descriptor Table (PRDT) entry.
///
/// Each PRDT entry describes one scatter/gather DMA region in system memory.
/// The HBA transfers data between the SATA device and the memory regions
/// described by the PRDT entries, in order. Maximum 4 MiB per entry (the byte
/// count field is 22 bits and stores count-1, so actual range is 1 byte to 4 MiB).
/// The data base address must be word-aligned (even byte boundary).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct PrdtEntry {
    /// Data Base Address (lower 32 bits, word-aligned).
    pub dba: u32,
    /// Data Base Address Upper 32 bits.
    pub dbau: u32,
    /// Reserved.
    pub _reserved: u32,
    /// Byte count (bits 21:0) and Interrupt on Completion (bit 31).
    /// Actual byte count = (dbc & 0x3FFFFF) + 1. Must be even (word-aligned).
    /// Bit 31: I — Interrupt on Completion.
    pub dbc_i: u32,
}

/// Bit 31 of PRDT DBC field: Interrupt on Completion.
pub const PRDT_IOC: u32 = 1 << 31;

impl PrdtEntry {
    /// Create a PRDT entry pointing to a buffer.
    ///
    /// `addr` is the physical address of the data buffer.
    /// `byte_count` is the number of bytes (will be stored as count - 1).
    /// `ioc` enables interrupt on completion for this entry.
    pub fn new(addr: u64, byte_count: u32, ioc: bool) -> Self {
        assert!(byte_count > 0, "PRDT byte count must be > 0");
        assert!(byte_count <= 4 * 1024 * 1024, "PRDT max 4 MiB per entry");
        let mut dbc_i = (byte_count - 1) & 0x003F_FFFF;
        if ioc {
            dbc_i |= PRDT_IOC;
        }
        Self {
            dba: addr as u32,
            dbau: (addr >> 32) as u32,
            _reserved: 0,
            dbc_i,
        }
    }
}

// ---------------------------------------------------------------------------
// Command Table (variable size: 128 bytes header + N*16 PRDT entries)
// ---------------------------------------------------------------------------

/// Size of the Command Table header (CFIS + ACMD + reserved, before PRDT).
/// CFIS = 64 bytes, ACMD = 16 bytes, reserved = 48 bytes = 128 bytes total.
pub const CMD_TABLE_HEADER_SIZE: usize = 128;

/// Offset of the CFIS (Command FIS) within the Command Table.
pub const CMD_TABLE_CFIS_OFFSET: usize = 0;
/// Offset of the ACMD (ATAPI Command) within the Command Table.
pub const CMD_TABLE_ACMD_OFFSET: usize = 64;
/// Offset of the PRDT array within the Command Table.
pub const CMD_TABLE_PRDT_OFFSET: usize = 128;

/// Allocate a Command Table with space for `prdt_count` PRDT entries.
///
/// The Command Table must be 128-byte aligned. Returns `(virt, phys)` —
/// the virtual address (for CPU writes) and the physical address (for CTBA).
pub fn allocate_cmd_table(prdt_count: usize, virt_to_phys: VirtToPhys) -> Option<(u64, u64)> {
    let size = CMD_TABLE_HEADER_SIZE + prdt_count * core::mem::size_of::<PrdtEntry>();
    let layout = match Layout::from_size_align(size, 128) {
        Ok(l) => l,
        Err(_) => {
            log::error!("[ahci] invalid command table layout (size={}, prdt={})", size, prdt_count);
            return None;
        }
    };
    let ptr = unsafe { alloc_zeroed(layout) };
    if ptr.is_null() {
        log::error!("[ahci] failed to allocate command table ({} bytes)", size);
        return None;
    }
    let virt = ptr as u64;
    let phys = virt_to_phys(ptr as usize);
    log::trace!(
        "[ahci] allocated command table virt={:#x} phys={:#x} ({} bytes, {} PRDT entries)",
        virt, phys, size, prdt_count
    );
    Some((virt, phys))
}

/// Write a FIS_REG_H2D into the CFIS area of a Command Table.
pub fn write_cfis(cmd_table_addr: u64, fis: &FisRegH2d) {
    unsafe {
        let dst = cmd_table_addr as *mut u8;
        ptr::copy_nonoverlapping(
            fis as *const FisRegH2d as *const u8,
            dst,
            core::mem::size_of::<FisRegH2d>(),
        );
    }
}

/// Write a PRDT entry at the given index within a Command Table.
pub fn write_prdt(cmd_table_addr: u64, index: usize, entry: &PrdtEntry) {
    let offset = CMD_TABLE_PRDT_OFFSET + index * core::mem::size_of::<PrdtEntry>();
    unsafe {
        let dst = (cmd_table_addr as usize + offset) as *mut u8;
        ptr::copy_nonoverlapping(
            entry as *const PrdtEntry as *const u8,
            dst,
            core::mem::size_of::<PrdtEntry>(),
        );
    }
}

// ---------------------------------------------------------------------------
// High-level command builders
// ---------------------------------------------------------------------------

/// Build a READ DMA EXT command (ATA command 0x25).
///
/// Reads `count` sectors starting at `lba` into the buffer at `buf_phys_addr`.
/// `buf_phys_addr` must already be a physical address (caller translates).
///
/// Returns `(cmd_header, cmd_table_virt)` — the command table virtual address
/// is returned so the caller can deallocate it later if needed.
pub fn build_read_dma_ext(
    lba: u64,
    count: u16,
    buf_phys_addr: u64,
    sector_size: u32,
    virt_to_phys: VirtToPhys,
) -> Option<(CommandHeader, u64)> {
    log::trace!(
        "[ahci] build READ DMA EXT: LBA={}, count={}, buf_phys={:#x}",
        lba, count, buf_phys_addr
    );

    let byte_count = count as u32 * sector_size;
    let prdt_count = prdt_entries_needed(byte_count);
    let (ct_virt, ct_phys) = allocate_cmd_table(prdt_count, virt_to_phys)?;

    // Build FIS — write to virtual address (CPU access)
    let mut fis = FisRegH2d::new();
    fis.set_command_bit();
    fis.command = ATA_CMD_READ_DMA_EXT;
    fis.set_lba(lba);
    fis.set_count(count);
    write_cfis(ct_virt, &fis);

    // Build PRDT entries (split across 4 MiB boundaries if needed)
    // PRDT entries contain physical addresses for the HBA to DMA into.
    let mut remaining = byte_count;
    let mut addr = buf_phys_addr;
    for i in 0..prdt_count {
        let chunk = core::cmp::min(remaining, 4 * 1024 * 1024);
        let ioc = i == prdt_count - 1; // IOC on last entry
        write_prdt(ct_virt, i, &PrdtEntry::new(addr, chunk, ioc));
        addr += chunk as u64;
        remaining -= chunk;
    }

    // Build Command Header — CTBA is physical (HBA fetches command table via DMA)
    let mut hdr = CommandHeader::zeroed();
    hdr.set_cfl(5); // FIS_REG_H2D is 5 DWORDs (20 bytes)
    hdr.set_prdtl(prdt_count as u16);
    hdr.set_write(false); // Read = D2H
    hdr.set_ctba(ct_phys);

    Some((hdr, ct_virt))
}

/// Build a WRITE DMA EXT command (ATA command 0x35).
///
/// Writes `count` sectors starting at `lba` from the buffer at `buf_phys_addr`.
/// `buf_phys_addr` must already be a physical address (caller translates).
///
/// Returns `(cmd_header, cmd_table_virt)`.
pub fn build_write_dma_ext(
    lba: u64,
    count: u16,
    buf_phys_addr: u64,
    sector_size: u32,
    virt_to_phys: VirtToPhys,
) -> Option<(CommandHeader, u64)> {
    log::trace!(
        "[ahci] build WRITE DMA EXT: LBA={}, count={}, buf_phys={:#x}",
        lba, count, buf_phys_addr
    );

    let byte_count = count as u32 * sector_size;
    let prdt_count = prdt_entries_needed(byte_count);
    let (ct_virt, ct_phys) = allocate_cmd_table(prdt_count, virt_to_phys)?;

    // Build FIS — write to virtual address (CPU access)
    let mut fis = FisRegH2d::new();
    fis.set_command_bit();
    fis.command = ATA_CMD_WRITE_DMA_EXT;
    fis.set_lba(lba);
    fis.set_count(count);
    write_cfis(ct_virt, &fis);

    // Build PRDT entries — physical addresses for HBA DMA
    let mut remaining = byte_count;
    let mut addr = buf_phys_addr;
    for i in 0..prdt_count {
        let chunk = core::cmp::min(remaining, 4 * 1024 * 1024);
        let ioc = i == prdt_count - 1;
        write_prdt(ct_virt, i, &PrdtEntry::new(addr, chunk, ioc));
        addr += chunk as u64;
        remaining -= chunk;
    }

    // Build Command Header — CTBA is physical (HBA fetches command table via DMA)
    let mut hdr = CommandHeader::zeroed();
    hdr.set_cfl(5);
    hdr.set_prdtl(prdt_count as u16);
    hdr.set_write(true); // Write = H2D
    hdr.set_ctba(ct_phys);

    Some((hdr, ct_virt))
}

/// Build an ATA IDENTIFY DEVICE command (0xEC).
///
/// `identify_buf_phys` must be the physical address of the 512-byte
/// IDENTIFY receive buffer (caller translates).
///
/// Returns `(cmd_header, cmd_table_virt)`.
pub fn build_identify(identify_buf_phys: u64, virt_to_phys: VirtToPhys) -> Option<(CommandHeader, u64)> {
    log::trace!(
        "[ahci] build IDENTIFY DEVICE: buf_phys={:#x}",
        identify_buf_phys
    );

    let (ct_virt, ct_phys) = allocate_cmd_table(1, virt_to_phys)?; // 1 PRDT entry for 512 bytes

    // Build FIS — write to virtual address (CPU access)
    let mut fis = FisRegH2d::new();
    fis.set_command_bit();
    fis.command = ATA_CMD_IDENTIFY;
    fis.device = 0; // No LBA for IDENTIFY
    write_cfis(ct_virt, &fis);

    // Single PRDT entry: 512 bytes of IDENTIFY data — physical address for HBA
    write_prdt(ct_virt, 0, &PrdtEntry::new(identify_buf_phys, 512, true));

    // Build Command Header — CTBA is physical
    let mut hdr = CommandHeader::zeroed();
    hdr.set_cfl(5);
    hdr.set_prdtl(1);
    hdr.set_write(false); // IDENTIFY is D2H
    hdr.set_ctba(ct_phys);

    Some((hdr, ct_virt))
}

/// Build a FLUSH CACHE EXT command (0xEA).
///
/// Returns `(cmd_header, cmd_table_virt)`.
pub fn build_flush_cache(virt_to_phys: VirtToPhys) -> Option<(CommandHeader, u64)> {
    log::trace!("[ahci] build FLUSH CACHE EXT");

    let (ct_virt, ct_phys) = allocate_cmd_table(0, virt_to_phys)?; // No data transfer

    let mut fis = FisRegH2d::new();
    fis.set_command_bit();
    fis.command = ATA_CMD_FLUSH_CACHE_EXT;
    write_cfis(ct_virt, &fis);

    let mut hdr = CommandHeader::zeroed();
    hdr.set_cfl(5);
    hdr.set_prdtl(0);
    hdr.set_write(false);
    hdr.set_ctba(ct_phys);

    Some((hdr, ct_virt))
}

/// Calculate how many PRDT entries are needed for a given byte count.
/// Each PRDT entry can cover up to 4 MiB.
fn prdt_entries_needed(byte_count: u32) -> usize {
    let max_per_entry = 4 * 1024 * 1024u32;
    ((byte_count + max_per_entry - 1) / max_per_entry) as usize
}
