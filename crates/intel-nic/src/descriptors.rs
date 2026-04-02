//! RX and TX descriptor ring management for Intel e1000/e1000e/igc NICs.
//!
//! Both families use the "legacy" descriptor format. Each descriptor is 16 bytes.
//! The NIC reads/writes these via DMA, so they must be in physically contiguous
//! memory and we must use the physical address when programming RDBAL/TDBAL.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

// =========================================================================
// Legacy RX Descriptor (16 bytes)
// =========================================================================

/// Legacy receive descriptor as defined in the Intel 82540EM datasheet
/// (Section 3.2.3) and reused by e1000e and igc.
///
/// Layout (little-endian):
/// ```text
/// [0..8)   buffer_addr   — Physical address of the receive buffer
/// [8..10)  length         — Length of received data (written by hardware)
/// [10..12) checksum       — Packet checksum (written by hardware)
/// [12]     status         — Descriptor status bits (DD, EOP, etc.)
/// [13]     errors         — Error bits
/// [14..16) special        — VLAN tag (if applicable)
/// ```
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct RxDescriptor {
    /// Physical address of the receive buffer.
    pub buffer_addr: u64,
    /// Length of received data (set by hardware on completion).
    pub length: u16,
    /// Packet checksum (hardware-computed).
    pub checksum: u16,
    /// Descriptor status bits.
    pub status: u8,
    /// Error indication bits.
    pub errors: u8,
    /// Special field (VLAN tag).
    pub special: u16,
}

impl RxDescriptor {
    /// Create a zeroed RX descriptor.
    pub const fn zeroed() -> Self {
        Self {
            buffer_addr: 0,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        }
    }
}

// =========================================================================
// RX Status Bits (descriptor status field)
// =========================================================================

/// Descriptor Done — hardware has written data to this descriptor.
pub const RX_STATUS_DD: u8 = 1 << 0;
/// End of Packet — this descriptor contains the last fragment.
pub const RX_STATUS_EOP: u8 = 1 << 1;
/// Ignore Checksum Indication.
pub const RX_STATUS_IXSM: u8 = 1 << 2;
/// VLAN Packet — frame had a VLAN tag that was stripped.
pub const RX_STATUS_VP: u8 = 1 << 3;
/// TCP Checksum Calculated.
pub const RX_STATUS_TCPCS: u8 = 1 << 5;
/// IP Checksum Calculated.
pub const RX_STATUS_IPCS: u8 = 1 << 6;
/// Passed In-exact Filter.
pub const RX_STATUS_PIF: u8 = 1 << 7;

// =========================================================================
// RX Error Bits
// =========================================================================

/// CRC Error or Alignment Error.
pub const RX_ERROR_CE: u8 = 1 << 0;
/// Symbol Error (TBI mode).
pub const RX_ERROR_SE: u8 = 1 << 1;
/// Sequence Error.
pub const RX_ERROR_SEQ: u8 = 1 << 2;
/// Carrier Extension Error.
pub const RX_ERROR_CXE: u8 = 1 << 4;
/// TCP/UDP Checksum Error.
pub const RX_ERROR_TCPE: u8 = 1 << 5;
/// IP Checksum Error.
pub const RX_ERROR_IPE: u8 = 1 << 6;
/// RX Data Error.
pub const RX_ERROR_RXE: u8 = 1 << 7;

// =========================================================================
// Legacy TX Descriptor (16 bytes)
// =========================================================================

/// Legacy transmit descriptor as defined in the Intel 82540EM datasheet
/// (Section 3.3.3).
///
/// Layout (little-endian):
/// ```text
/// [0..8)   buffer_addr   — Physical address of the transmit data
/// [8..10)  length         — Length of data to transmit
/// [10]     cso            — Checksum Offset
/// [11]     cmd            — Command bits (EOP, IFCS, RS, etc.)
/// [12]     status         — Descriptor status bits (DD)
/// [13]     css            — Checksum Start
/// [14..16) special        — VLAN tag (if VLE set)
/// ```
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct TxDescriptor {
    /// Physical address of the transmit buffer.
    pub buffer_addr: u64,
    /// Length of data to transmit (bytes).
    pub length: u16,
    /// Checksum Offset — byte offset where checksum is inserted.
    pub cso: u8,
    /// Command bits.
    pub cmd: u8,
    /// Descriptor status (DD bit set by hardware on completion).
    pub status: u8,
    /// Checksum Start — byte offset where checksum computation begins.
    pub css: u8,
    /// Special field (VLAN tag).
    pub special: u16,
}

impl TxDescriptor {
    /// Create a zeroed TX descriptor.
    pub const fn zeroed() -> Self {
        Self {
            buffer_addr: 0,
            length: 0,
            cso: 0,
            cmd: 0,
            status: 0,
            css: 0,
            special: 0,
        }
    }
}

// =========================================================================
// TX Command Bits (cmd field)
// =========================================================================

/// End of Packet — marks the last descriptor for a frame.
pub const TX_CMD_EOP: u8 = 1 << 0;
/// Insert FCS/CRC — hardware appends the Ethernet CRC.
pub const TX_CMD_IFCS: u8 = 1 << 1;
/// Insert Checksum (offload).
pub const TX_CMD_IC: u8 = 1 << 2;
/// Report Status — hardware sets DD in status field on completion.
pub const TX_CMD_RS: u8 = 1 << 3;
/// Extension (use extended descriptor format, not legacy).
pub const TX_CMD_DEXT: u8 = 1 << 5;
/// VLAN Packet Enable.
pub const TX_CMD_VLE: u8 = 1 << 6;
/// Interrupt Delay Enable.
pub const TX_CMD_IDE: u8 = 1 << 7;

// =========================================================================
// TX Status Bits
// =========================================================================

/// Descriptor Done — hardware has completed transmitting this descriptor.
pub const TX_STATUS_DD: u8 = 1 << 0;
/// Excess Collisions.
pub const TX_STATUS_EC: u8 = 1 << 1;
/// Late Collision.
pub const TX_STATUS_LC: u8 = 1 << 2;

// =========================================================================
// Descriptor Ring
// =========================================================================

/// Number of descriptors per ring. Must be a multiple of 8 and fit in a
/// 128-byte aligned region. 256 is a good default for both performance and
/// memory usage.
pub const RING_SIZE: usize = 256;

/// Size of each DMA buffer. 2048 bytes accommodates standard Ethernet MTU
/// (1518 bytes) with room for headers and alignment.
pub const BUF_SIZE: usize = 2048;

/// A ring of RX or TX descriptors with associated DMA buffers.
///
/// The descriptors and buffers are heap-allocated. The caller must translate
/// virtual addresses to physical addresses before programming the NIC
/// (RDBAL/TDBAL registers).
pub struct DescriptorRing<D: Copy> {
    /// The descriptor array. Must be 128-byte aligned for the NIC.
    /// Boxed to ensure stable address.
    pub descriptors: Box<[D; RING_SIZE]>,
    /// DMA buffers, one per descriptor.
    pub buffers: Vec<Box<[u8; BUF_SIZE]>>,
    /// Current head index (next descriptor to process/reclaim).
    pub head: usize,
    /// Current tail index (next descriptor to fill/submit).
    pub tail: usize,
}

/// RX descriptor ring.
pub type RxRing = DescriptorRing<RxDescriptor>;
/// TX descriptor ring.
pub type TxRing = DescriptorRing<TxDescriptor>;

impl<D: Copy> DescriptorRing<D> {
    /// Advance an index with wrap-around.
    #[inline]
    pub fn wrap_next(idx: usize) -> usize {
        (idx + 1) % RING_SIZE
    }

    /// Number of descriptors currently in use (between head and tail).
    pub fn in_use(&self) -> usize {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            RING_SIZE - self.head + self.tail
        }
    }

    /// Number of free descriptor slots.
    pub fn free_count(&self) -> usize {
        // Keep one slot empty to distinguish full from empty.
        RING_SIZE - 1 - self.in_use()
    }

    /// Check if the ring is full.
    pub fn is_full(&self) -> bool {
        Self::wrap_next(self.tail) == self.head
    }
}

/// Allocate a new RX descriptor ring with zeroed descriptors and buffers.
pub fn alloc_rx_ring() -> RxRing {
    log::debug!("[intel-nic] allocating RX descriptor ring ({} entries, {} bytes each)",
        RING_SIZE, BUF_SIZE);

    // Allocate the descriptor array (128-byte aligned for NIC DMA).
    // Box<[RxDescriptor; 256]> on heap.
    let descriptors = Box::new([RxDescriptor::zeroed(); RING_SIZE]);

    // Allocate DMA buffers.
    let mut buffers = Vec::with_capacity(RING_SIZE);
    for _ in 0..RING_SIZE {
        buffers.push(Box::new([0u8; BUF_SIZE]));
    }

    log::debug!("[intel-nic] RX ring allocated: descriptors at {:p}", &*descriptors as *const _);

    DescriptorRing {
        descriptors,
        buffers,
        head: 0,
        tail: 0,
    }
}

/// Allocate a new TX descriptor ring with zeroed descriptors and buffers.
pub fn alloc_tx_ring() -> TxRing {
    log::debug!("[intel-nic] allocating TX descriptor ring ({} entries, {} bytes each)",
        RING_SIZE, BUF_SIZE);

    let descriptors = Box::new([TxDescriptor::zeroed(); RING_SIZE]);

    let mut buffers = Vec::with_capacity(RING_SIZE);
    for _ in 0..RING_SIZE {
        buffers.push(Box::new([0u8; BUF_SIZE]));
    }

    log::debug!("[intel-nic] TX ring allocated: descriptors at {:p}", &*descriptors as *const _);

    DescriptorRing {
        descriptors,
        buffers,
        head: 0,
        tail: 0,
    }
}
