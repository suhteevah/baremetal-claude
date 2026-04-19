//! RTL8168 MMIO register offsets and bit masks.
//! Source: Realtek RTL8168 datasheet rev 1.3 §5. All offsets from BAR2 base.

pub const IDR0: u32 = 0x0000;
pub const IDR4: u32 = 0x0004;

pub const CR:   u32 = 0x0037;
pub const CR_RST: u8 = 1 << 4;
pub const CR_RE:  u8 = 1 << 3;
pub const CR_TE:  u8 = 1 << 2;

pub const TPPOLL: u32 = 0x003D;
pub const TPPOLL_NPQ: u8 = 1 << 6;

pub const IMR: u32 = 0x003C;
pub const ISR: u32 = 0x003E;
pub const INT_ROK:  u16 = 1 << 0;
pub const INT_RER:  u16 = 1 << 1;
pub const INT_TOK:  u16 = 1 << 2;
pub const INT_TER:  u16 = 1 << 3;
pub const INT_LINK: u16 = 1 << 5;

pub const TXCR: u32 = 0x0040;
pub const TXCR_DEFAULT: u32 = (0b11 << 24) | (0b111 << 8);

pub const RXCR: u32 = 0x0044;
/// AAP (bit 0, promiscuous) + AM (bit 2, multicast) + APM (bit 1, unicast-match)
/// + AB (bit 3, broadcast) + DMA burst 1024 (bits 8-10 = 0b110).
/// Promiscuous is enabled for diagnostics — we want to see if *any* packets
/// make it past the filter. Can be tightened later once RX path is proven.
pub const RXCR_DEFAULT: u32 = (1 << 3) | (1 << 2) | (1 << 1) | (1 << 0) | (0b110 << 8);

pub const CR9346: u32 = 0x0050;
pub const CR9346_UNLOCK: u8 = 0xC0;
pub const CR9346_LOCK:   u8 = 0x00;

pub const RDSAR_LOW:  u32 = 0x00E4;
pub const RDSAR_HIGH: u32 = 0x00E8;
pub const TNPDS_LOW:  u32 = 0x0020;
pub const TNPDS_HIGH: u32 = 0x0024;
pub const THPDS_LOW:  u32 = 0x0028;
pub const THPDS_HIGH: u32 = 0x002C;

pub const RXMPS: u32 = 0x00DA;
pub const RXMPS_DEFAULT: u16 = 1536;

/// C+ Command Register (u16, offset 0xE0). Controls RX checksum offload,
/// VLAN tagging, etc. Linux r8169 writes this early during init; leaving it
/// at reset default (garbage in some chip variants) can cause silent TX
/// corruption or RX never-fires.
pub const CPLUS_CMD: u32 = 0x00E0;

/// Max TX packet size (u8, offset 0xEC). Unit = 128 bytes. 0x3F = 8064 bytes,
/// sufficient for any standard Ethernet frame. If this is 0 after reset the
/// TX path may stall.
pub const MAX_TX_PKT_SIZE: u32 = 0x00EC;

/// PHY Status Register (u8, offset 0x6C). Bit 1 = LINKOK, bit 2 = 10M,
/// bit 3 = 100M, bit 4 = 1000M, bit 6 = FULL_DUP.
pub const PHY_STATUS: u32 = 0x006C;
pub const PHY_STATUS_LINKOK:  u8 = 1 << 1;
pub const PHY_STATUS_FULLDUP: u8 = 1 << 0;
