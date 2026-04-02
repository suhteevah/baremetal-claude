//! Transfer Request Block (TRB) Rings
//!
//! xHCI uses ring buffers of 16-byte TRBs for communication between software and
//! the host controller. There are three types:
//! - **Command Ring**: Software enqueues commands, HC dequeues and posts completion events
//! - **Event Ring**: HC enqueues events, software dequeues
//! - **Transfer Ring**: Software enqueues transfer TRBs for data movement on endpoints

use alloc::alloc::{alloc_zeroed, Layout};
use core::ptr;

// ---------------------------------------------------------------------------
// TRB Type codes (bits 15:10 of the control field, i.e., dword 3)
// ---------------------------------------------------------------------------

// Transfer TRB types
pub const TRB_TYPE_NORMAL: u32 = 1;
pub const TRB_TYPE_SETUP_STAGE: u32 = 2;
pub const TRB_TYPE_DATA_STAGE: u32 = 3;
pub const TRB_TYPE_STATUS_STAGE: u32 = 4;
pub const TRB_TYPE_ISOCH: u32 = 5;
pub const TRB_TYPE_LINK: u32 = 6;
pub const TRB_TYPE_EVENT_DATA: u32 = 7;
pub const TRB_TYPE_NO_OP: u32 = 8;

// Command TRB types
pub const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub const TRB_TYPE_DISABLE_SLOT: u32 = 10;
pub const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
pub const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
pub const TRB_TYPE_EVALUATE_CONTEXT: u32 = 13;
pub const TRB_TYPE_RESET_ENDPOINT: u32 = 14;
pub const TRB_TYPE_STOP_ENDPOINT: u32 = 15;
pub const TRB_TYPE_SET_TR_DEQUEUE: u32 = 16;
pub const TRB_TYPE_RESET_DEVICE: u32 = 17;
pub const TRB_TYPE_NO_OP_CMD: u32 = 23;

// Event TRB types
pub const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
pub const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
pub const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;
pub const TRB_TYPE_BANDWIDTH_REQUEST: u32 = 35;
pub const TRB_TYPE_HOST_CONTROLLER: u32 = 37;
pub const TRB_TYPE_DEVICE_NOTIFICATION: u32 = 38;
pub const TRB_TYPE_MFINDEX_WRAP: u32 = 39;

// ---------------------------------------------------------------------------
// TRB Completion Codes
// ---------------------------------------------------------------------------

pub const TRB_COMPLETION_INVALID: u8 = 0;
pub const TRB_COMPLETION_SUCCESS: u8 = 1;
pub const TRB_COMPLETION_DATA_BUFFER_ERROR: u8 = 2;
pub const TRB_COMPLETION_BABBLE: u8 = 3;
pub const TRB_COMPLETION_USB_TRANSACTION_ERROR: u8 = 4;
pub const TRB_COMPLETION_TRB_ERROR: u8 = 5;
pub const TRB_COMPLETION_STALL: u8 = 6;
pub const TRB_COMPLETION_SHORT_PACKET: u8 = 13;
pub const TRB_COMPLETION_RING_UNDERRUN: u8 = 14;
pub const TRB_COMPLETION_RING_OVERRUN: u8 = 15;
pub const TRB_COMPLETION_EVENT_RING_FULL: u8 = 21;
pub const TRB_COMPLETION_COMMAND_RING_STOPPED: u8 = 24;
pub const TRB_COMPLETION_COMMAND_ABORTED: u8 = 25;
pub const TRB_COMPLETION_STOPPED: u8 = 26;
pub const TRB_COMPLETION_STOPPED_LENGTH_INVALID: u8 = 27;

// ---------------------------------------------------------------------------
// TRB field masks and shifts
// ---------------------------------------------------------------------------

/// TRB type field: bits 15:10 of dword 3 (control)
pub const TRB_TYPE_SHIFT: u32 = 10;
pub const TRB_TYPE_MASK: u32 = 0x3F << TRB_TYPE_SHIFT;

/// Cycle bit: bit 0 of dword 3
pub const TRB_CYCLE_BIT: u32 = 1 << 0;

/// Toggle Cycle bit (Link TRB): bit 1 of dword 3
pub const TRB_TOGGLE_CYCLE: u32 = 1 << 1;

/// Interrupt-on-Completion: bit 5 of dword 3
pub const TRB_IOC: u32 = 1 << 5;

/// Interrupt-on-Short-Packet: bit 2 of dword 3 (transfer TRBs)
pub const TRB_ISP: u32 = 1 << 2;

/// Chain bit: bit 4 of dword 3
pub const TRB_CHAIN: u32 = 1 << 4;

/// Immediate Data (IDT): bit 6 of dword 3 (Setup Stage TRB)
pub const TRB_IDT: u32 = 1 << 6;

/// Block Set Address Request (BSR): bit 9 of dword 3 (Address Device command)
pub const TRB_BSR: u32 = 1 << 9;

/// Transfer direction in Data Stage / Status Stage: bit 16 of dword 3
pub const TRB_DIR_IN: u32 = 1 << 16;

/// Completion code field: bits 31:24 of dword 2 in event TRBs
pub const TRB_COMPLETION_CODE_SHIFT: u32 = 24;
pub const TRB_COMPLETION_CODE_MASK: u32 = 0xFF << TRB_COMPLETION_CODE_SHIFT;

/// Slot ID field: bits 31:24 of dword 3 in command/event TRBs
pub const TRB_SLOT_ID_SHIFT: u32 = 24;
pub const TRB_SLOT_ID_MASK: u32 = 0xFF << TRB_SLOT_ID_SHIFT;

/// Endpoint ID field: bits 20:16 of dword 3 in some event TRBs
pub const TRB_ENDPOINT_ID_SHIFT: u32 = 16;
pub const TRB_ENDPOINT_ID_MASK: u32 = 0x1F << TRB_ENDPOINT_ID_SHIFT;

/// TRB Transfer Length field: bits 23:0 of dword 2 in transfer event TRBs
pub const TRB_TRANSFER_LENGTH_MASK: u32 = 0x00FF_FFFF;

/// Setup Stage TRB: Transfer Type (TRT) bits 17:16 of dword 3
pub const TRB_TRT_NO_DATA: u32 = 0 << 16;
pub const TRB_TRT_OUT: u32 = 2 << 16;
pub const TRB_TRT_IN: u32 = 3 << 16;

// ---------------------------------------------------------------------------
// TRB structure — 16 bytes, 4 DWORDs
// ---------------------------------------------------------------------------

/// A single Transfer Request Block (TRB). All TRBs are exactly 16 bytes.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct Trb {
    /// Parameter low (DW0) — meaning depends on TRB type
    pub parameter_lo: u32,
    /// Parameter high (DW1) — meaning depends on TRB type
    pub parameter_hi: u32,
    /// Status (DW2) — transfer length, completion code, interrupter target
    pub status: u32,
    /// Control (DW3) — TRB type, cycle bit, flags
    pub control: u32,
}

impl Trb {
    pub const SIZE: usize = 16;

    /// Create a zeroed TRB
    pub const fn zeroed() -> Self {
        Self {
            parameter_lo: 0,
            parameter_hi: 0,
            status: 0,
            control: 0,
        }
    }

    /// Get the 64-bit parameter field
    pub fn parameter(&self) -> u64 {
        (self.parameter_hi as u64) << 32 | (self.parameter_lo as u64)
    }

    /// Set the 64-bit parameter field
    pub fn set_parameter(&mut self, val: u64) {
        self.parameter_lo = val as u32;
        self.parameter_hi = (val >> 32) as u32;
    }

    /// Get the TRB type
    pub fn trb_type(&self) -> u32 {
        (self.control & TRB_TYPE_MASK) >> TRB_TYPE_SHIFT
    }

    /// Get the cycle bit
    pub fn cycle_bit(&self) -> bool {
        self.control & TRB_CYCLE_BIT != 0
    }

    /// Get completion code from an event TRB
    pub fn completion_code(&self) -> u8 {
        ((self.status & TRB_COMPLETION_CODE_MASK) >> TRB_COMPLETION_CODE_SHIFT) as u8
    }

    /// Get slot ID from a command completion event or command TRB
    pub fn slot_id(&self) -> u8 {
        ((self.control & TRB_SLOT_ID_MASK) >> TRB_SLOT_ID_SHIFT) as u8
    }

    /// Get endpoint ID from transfer event
    pub fn endpoint_id(&self) -> u8 {
        ((self.control & TRB_ENDPOINT_ID_MASK) >> TRB_ENDPOINT_ID_SHIFT) as u8
    }

    /// Get transfer length residual from transfer event
    pub fn transfer_length(&self) -> u32 {
        self.status & TRB_TRANSFER_LENGTH_MASK
    }

    /// Build a No-Op command TRB
    pub fn no_op_cmd(cycle: bool) -> Self {
        Self {
            parameter_lo: 0,
            parameter_hi: 0,
            status: 0,
            control: (TRB_TYPE_NO_OP_CMD << TRB_TYPE_SHIFT)
                | if cycle { TRB_CYCLE_BIT } else { 0 },
        }
    }

    /// Build an Enable Slot command TRB
    pub fn enable_slot(cycle: bool) -> Self {
        Self {
            parameter_lo: 0,
            parameter_hi: 0,
            status: 0,
            control: (TRB_TYPE_ENABLE_SLOT << TRB_TYPE_SHIFT)
                | if cycle { TRB_CYCLE_BIT } else { 0 },
        }
    }

    /// Build an Address Device command TRB
    pub fn address_device(input_ctx_phys: u64, slot_id: u8, bsr: bool, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(input_ctx_phys);
        trb.control = (TRB_TYPE_ADDRESS_DEVICE << TRB_TYPE_SHIFT)
            | ((slot_id as u32) << TRB_SLOT_ID_SHIFT)
            | if bsr { TRB_BSR } else { 0 }
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }

    /// Build a Configure Endpoint command TRB
    pub fn configure_endpoint(input_ctx_phys: u64, slot_id: u8, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(input_ctx_phys);
        trb.control = (TRB_TYPE_CONFIGURE_ENDPOINT << TRB_TYPE_SHIFT)
            | ((slot_id as u32) << TRB_SLOT_ID_SHIFT)
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }

    /// Build an Evaluate Context command TRB
    pub fn evaluate_context(input_ctx_phys: u64, slot_id: u8, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(input_ctx_phys);
        trb.control = (TRB_TYPE_EVALUATE_CONTEXT << TRB_TYPE_SHIFT)
            | ((slot_id as u32) << TRB_SLOT_ID_SHIFT)
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }

    /// Build a Reset Endpoint command TRB
    pub fn reset_endpoint(slot_id: u8, endpoint_id: u8, cycle: bool) -> Self {
        Self {
            parameter_lo: 0,
            parameter_hi: 0,
            status: 0,
            control: (TRB_TYPE_RESET_ENDPOINT << TRB_TYPE_SHIFT)
                | ((slot_id as u32) << TRB_SLOT_ID_SHIFT)
                | ((endpoint_id as u32) << TRB_ENDPOINT_ID_SHIFT)
                | if cycle { TRB_CYCLE_BIT } else { 0 },
        }
    }

    /// Build a Link TRB pointing to `next_segment_phys`, optionally toggling cycle
    pub fn link(next_segment_phys: u64, toggle_cycle: bool, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(next_segment_phys);
        trb.control = (TRB_TYPE_LINK << TRB_TYPE_SHIFT)
            | if toggle_cycle { TRB_TOGGLE_CYCLE } else { 0 }
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }

    /// Build a Setup Stage TRB for a control transfer.
    /// `request_type`, `request`, `value`, `index`, `length` are the USB setup packet fields.
    /// `trt` is the transfer type (TRB_TRT_NO_DATA, TRB_TRT_IN, TRB_TRT_OUT).
    pub fn setup_stage(
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        trt: u32,
        cycle: bool,
    ) -> Self {
        Self {
            parameter_lo: (request_type as u32) | ((request as u32) << 8)
                | ((value as u32) << 16),
            parameter_hi: (index as u32) | ((length as u32) << 16),
            status: 8, // always 8 bytes for setup packet
            control: (TRB_TYPE_SETUP_STAGE << TRB_TYPE_SHIFT)
                | TRB_IDT  // Immediate Data — setup data is in the TRB
                | trt
                | if cycle { TRB_CYCLE_BIT } else { 0 },
        }
    }

    /// Build a Data Stage TRB.
    /// `data_phys` is the physical address of the data buffer.
    /// `length` is the transfer length in bytes.
    /// `dir_in` is true for IN (device to host), false for OUT.
    pub fn data_stage(data_phys: u64, length: u32, dir_in: bool, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(data_phys);
        trb.status = length & 0x1FFFF; // bits 16:0 = TRB transfer length (max 64K)
        trb.control = (TRB_TYPE_DATA_STAGE << TRB_TYPE_SHIFT)
            | if dir_in { TRB_DIR_IN } else { 0 }
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }

    /// Build a Status Stage TRB.
    /// `dir_in` = true means status stage direction is IN (used for OUT data transfers).
    pub fn status_stage(dir_in: bool, cycle: bool) -> Self {
        Self {
            parameter_lo: 0,
            parameter_hi: 0,
            status: 0,
            control: (TRB_TYPE_STATUS_STAGE << TRB_TYPE_SHIFT)
                | TRB_IOC  // Interrupt on Completion
                | if dir_in { TRB_DIR_IN } else { 0 }
                | if cycle { TRB_CYCLE_BIT } else { 0 },
        }
    }

    /// Build a Normal TRB for bulk/interrupt transfers.
    pub fn normal(data_phys: u64, length: u32, ioc: bool, cycle: bool) -> Self {
        let mut trb = Self::zeroed();
        trb.set_parameter(data_phys);
        trb.status = length & 0x1FFFF;
        trb.control = (TRB_TYPE_NORMAL << TRB_TYPE_SHIFT)
            | if ioc { TRB_IOC } else { 0 }
            | TRB_ISP
            | if cycle { TRB_CYCLE_BIT } else { 0 };
        trb
    }
}

// ---------------------------------------------------------------------------
// Ring allocation constants
// ---------------------------------------------------------------------------

/// Default number of TRBs per ring segment (including the Link TRB at the end)
pub const RING_SEGMENT_TRBS: usize = 256;

/// Total size of a ring segment in bytes
pub const RING_SEGMENT_SIZE: usize = RING_SEGMENT_TRBS * Trb::SIZE;

// ---------------------------------------------------------------------------
// Command Ring
// ---------------------------------------------------------------------------

/// Producer ring for software-initiated commands to the host controller.
pub struct CommandRing {
    /// Virtual address of the ring segment
    ring_va: *mut Trb,
    /// Physical address of the ring segment (for CRCR register)
    ring_phys: u64,
    /// Number of TRBs in the segment (including Link TRB)
    segment_len: usize,
    /// Current enqueue index
    enqueue_idx: usize,
    /// Producer Cycle State (PCS)
    cycle: bool,
}

impl CommandRing {
    /// Allocate and initialize a command ring.
    ///
    /// # Safety
    /// The returned physical address must be identity-mapped or the caller must
    /// ensure the xHCI controller can DMA to it.
    pub unsafe fn new() -> Self {
        let layout = Layout::from_size_align(RING_SEGMENT_SIZE, 64)
            .expect("xhci: command ring layout");
        let ptr = alloc_zeroed(layout);
        assert!(!ptr.is_null(), "xhci: command ring allocation failed");

        let ring_va = ptr as *mut Trb;
        // In ClaudioOS, virtual == physical (identity mapped)
        let ring_phys = ptr as u64;

        log::info!(
            "xhci: command ring allocated at VA={:#x} PA={:#x} ({} TRBs)",
            ring_va as usize,
            ring_phys,
            RING_SEGMENT_TRBS
        );

        let mut ring = Self {
            ring_va,
            ring_phys,
            segment_len: RING_SEGMENT_TRBS,
            enqueue_idx: 0,
            cycle: true,
        };

        // Write the Link TRB at the last slot, pointing back to start with toggle
        let link_idx = RING_SEGMENT_TRBS - 1;
        let link_trb = Trb::link(ring_phys, true, ring.cycle);
        ring.write_trb(link_idx, &link_trb);

        log::debug!("xhci: command ring link TRB at index {}", link_idx);

        ring
    }

    /// Physical address of the ring (for programming into CRCR).
    /// Includes the initial cycle bit in bit 0.
    pub fn phys_addr_with_cycle(&self) -> u64 {
        self.ring_phys | if self.cycle { 1 } else { 0 }
    }

    /// Enqueue a TRB onto the command ring. Returns the physical address of
    /// the enqueued TRB.
    pub fn enqueue(&mut self, mut trb: Trb) -> u64 {
        // Set/clear the cycle bit to match our PCS
        if self.cycle {
            trb.control |= TRB_CYCLE_BIT;
        } else {
            trb.control &= !TRB_CYCLE_BIT;
        }

        let phys = self.ring_phys + (self.enqueue_idx * Trb::SIZE) as u64;

        log::trace!(
            "xhci: cmd ring enqueue idx={} type={} phys={:#x} cycle={}",
            self.enqueue_idx,
            trb.trb_type(),
            phys,
            self.cycle
        );

        unsafe { self.write_trb(self.enqueue_idx, &trb) };

        self.advance_enqueue();
        phys
    }

    /// Write a TRB at a given index using volatile write.
    unsafe fn write_trb(&self, idx: usize, trb: &Trb) {
        let dest = self.ring_va.add(idx);
        ptr::write_volatile(dest, *trb);
    }

    /// Advance the enqueue pointer, wrapping at the Link TRB.
    fn advance_enqueue(&mut self) {
        self.enqueue_idx += 1;

        // If we hit the Link TRB slot, wrap around and toggle cycle
        if self.enqueue_idx >= self.segment_len - 1 {
            log::trace!("xhci: cmd ring wrap, toggling cycle {} -> {}", self.cycle, !self.cycle);

            // Update the Link TRB's cycle bit before wrapping
            unsafe {
                let link = self.ring_va.add(self.segment_len - 1);
                let mut link_trb = ptr::read_volatile(link);
                if self.cycle {
                    link_trb.control |= TRB_CYCLE_BIT;
                } else {
                    link_trb.control &= !TRB_CYCLE_BIT;
                }
                ptr::write_volatile(link, link_trb);
            }

            self.enqueue_idx = 0;
            self.cycle = !self.cycle;
        }
    }
}

// ---------------------------------------------------------------------------
// Event Ring Segment Table Entry
// ---------------------------------------------------------------------------

/// Event Ring Segment Table Entry (ERSTE) — 16 bytes per xHCI spec 6.5
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct EventRingSegmentTableEntry {
    /// Ring Segment Base Address (physical, 64-byte aligned)
    pub ring_segment_base: u64,
    /// Ring Segment Size (number of TRBs in this segment)
    pub ring_segment_size: u16,
    /// Reserved
    _reserved1: u16,
    /// Reserved
    _reserved2: u32,
}

// ---------------------------------------------------------------------------
// Event Ring
// ---------------------------------------------------------------------------

/// Consumer ring for events posted by the host controller.
pub struct EventRing {
    /// Virtual address of the event ring segment
    ring_va: *const Trb,
    /// Physical address of the event ring segment
    ring_phys: u64,
    /// Virtual address of the segment table
    erst_va: *mut EventRingSegmentTableEntry,
    /// Physical address of the segment table (for ERSTBA)
    erst_phys: u64,
    /// Number of TRBs in the segment
    segment_len: usize,
    /// Current dequeue index
    dequeue_idx: usize,
    /// Consumer Cycle State (CCS)
    cycle: bool,
}

impl EventRing {
    /// Allocate and initialize an event ring with one segment.
    ///
    /// # Safety
    /// Memory must be identity-mapped for DMA.
    pub unsafe fn new() -> Self {
        // Allocate the event ring segment
        let ring_layout = Layout::from_size_align(RING_SEGMENT_SIZE, 64)
            .expect("xhci: event ring layout");
        let ring_ptr = alloc_zeroed(ring_layout);
        assert!(!ring_ptr.is_null(), "xhci: event ring allocation failed");

        let ring_va = ring_ptr as *const Trb;
        let ring_phys = ring_ptr as u64;

        // Allocate the Event Ring Segment Table (one entry)
        let erst_layout = Layout::from_size_align(
            core::mem::size_of::<EventRingSegmentTableEntry>(),
            64,
        )
        .expect("xhci: ERST layout");
        let erst_ptr = alloc_zeroed(erst_layout);
        assert!(!erst_ptr.is_null(), "xhci: ERST allocation failed");

        let erst_va = erst_ptr as *mut EventRingSegmentTableEntry;
        let erst_phys = erst_ptr as u64;

        // Fill in the segment table entry
        ptr::write_volatile(
            erst_va,
            EventRingSegmentTableEntry {
                ring_segment_base: ring_phys,
                ring_segment_size: RING_SEGMENT_TRBS as u16,
                _reserved1: 0,
                _reserved2: 0,
            },
        );

        log::info!(
            "xhci: event ring allocated at VA={:#x} PA={:#x} ({} TRBs), ERST at PA={:#x}",
            ring_va as usize,
            ring_phys,
            RING_SEGMENT_TRBS,
            erst_phys,
        );

        Self {
            ring_va,
            ring_phys,
            erst_va,
            erst_phys,
            segment_len: RING_SEGMENT_TRBS,
            dequeue_idx: 0,
            cycle: true,
        }
    }

    /// Physical address of the ERST (for programming ERSTBA)
    pub fn erst_phys(&self) -> u64 {
        self.erst_phys
    }

    /// Number of segments (always 1 for now)
    pub fn erst_size(&self) -> u32 {
        1
    }

    /// Current dequeue pointer physical address (for programming ERDP)
    pub fn dequeue_phys(&self) -> u64 {
        self.ring_phys + (self.dequeue_idx * Trb::SIZE) as u64
    }

    /// Try to dequeue an event TRB. Returns `Some(trb)` if an event is pending,
    /// `None` if the ring is empty (cycle bit mismatch).
    pub fn dequeue(&mut self) -> Option<Trb> {
        let trb = unsafe {
            ptr::read_volatile(self.ring_va.add(self.dequeue_idx))
        };

        // Check if this TRB's cycle bit matches our expected CCS
        if trb.cycle_bit() != self.cycle {
            return None;
        }

        log::trace!(
            "xhci: event ring dequeue idx={} type={} completion={} slot={}",
            self.dequeue_idx,
            trb.trb_type(),
            trb.completion_code(),
            trb.slot_id(),
        );

        self.dequeue_idx += 1;
        if self.dequeue_idx >= self.segment_len {
            self.dequeue_idx = 0;
            self.cycle = !self.cycle;
            log::trace!("xhci: event ring wrap, cycle toggled to {}", self.cycle);
        }

        Some(trb)
    }

    /// Check if there is a pending event without consuming it.
    pub fn has_pending(&self) -> bool {
        let trb = unsafe {
            ptr::read_volatile(self.ring_va.add(self.dequeue_idx))
        };
        trb.cycle_bit() == self.cycle
    }
}

// ---------------------------------------------------------------------------
// Transfer Ring
// ---------------------------------------------------------------------------

/// Producer ring for endpoint data transfers (control, bulk, interrupt).
pub struct TransferRing {
    /// Virtual address of the ring segment
    ring_va: *mut Trb,
    /// Physical address of the ring segment
    ring_phys: u64,
    /// Number of TRBs in the segment (including Link TRB)
    segment_len: usize,
    /// Current enqueue index
    enqueue_idx: usize,
    /// Producer Cycle State
    cycle: bool,
}

impl TransferRing {
    /// Allocate and initialize a transfer ring.
    ///
    /// # Safety
    /// Memory must be identity-mapped for DMA.
    pub unsafe fn new() -> Self {
        let layout = Layout::from_size_align(RING_SEGMENT_SIZE, 64)
            .expect("xhci: transfer ring layout");
        let ptr = alloc_zeroed(layout);
        assert!(!ptr.is_null(), "xhci: transfer ring allocation failed");

        let ring_va = ptr as *mut Trb;
        let ring_phys = ptr as u64;

        log::debug!(
            "xhci: transfer ring allocated at VA={:#x} PA={:#x} ({} TRBs)",
            ring_va as usize,
            ring_phys,
            RING_SEGMENT_TRBS
        );

        let mut ring = Self {
            ring_va,
            ring_phys,
            segment_len: RING_SEGMENT_TRBS,
            enqueue_idx: 0,
            cycle: true,
        };

        // Write Link TRB at end
        let link_idx = RING_SEGMENT_TRBS - 1;
        let link_trb = Trb::link(ring_phys, true, ring.cycle);
        ptr::write_volatile(ring_va.add(link_idx), link_trb);

        ring
    }

    /// Physical address of the ring start (for TR Dequeue Pointer in endpoint context).
    /// Includes cycle bit in bit 0 and DCS (Dequeue Cycle State).
    pub fn phys_addr_with_dcs(&self) -> u64 {
        self.ring_phys | if self.cycle { 1 } else { 0 }
    }

    /// Enqueue a TRB onto the transfer ring. Sets the cycle bit appropriately.
    pub fn enqueue(&mut self, mut trb: Trb) -> u64 {
        if self.cycle {
            trb.control |= TRB_CYCLE_BIT;
        } else {
            trb.control &= !TRB_CYCLE_BIT;
        }

        let phys = self.ring_phys + (self.enqueue_idx * Trb::SIZE) as u64;

        log::trace!(
            "xhci: transfer ring enqueue idx={} type={} phys={:#x}",
            self.enqueue_idx,
            trb.trb_type(),
            phys,
        );

        unsafe {
            ptr::write_volatile(self.ring_va.add(self.enqueue_idx), trb);
        }

        self.advance_enqueue();
        phys
    }

    /// Enqueue a control transfer (Setup + optional Data + Status).
    /// Returns the physical address of the Status Stage TRB.
    pub fn enqueue_control_transfer(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data_phys: u64,
        data_len: u16,
    ) -> u64 {
        let dir_in = request_type & 0x80 != 0;

        // Determine Transfer Type for Setup Stage
        let trt = if data_len == 0 {
            TRB_TRT_NO_DATA
        } else if dir_in {
            TRB_TRT_IN
        } else {
            TRB_TRT_OUT
        };

        log::debug!(
            "xhci: control transfer reqtype={:#x} req={:#x} val={:#x} idx={:#x} len={} dir_in={}",
            request_type, request, value, index, data_len, dir_in
        );

        // Setup Stage TRB
        let setup = Trb::setup_stage(request_type, request, value, index, data_len, trt, false);
        self.enqueue(setup);

        // Data Stage TRB (if needed)
        if data_len > 0 {
            let data = Trb::data_stage(data_phys, data_len as u32, dir_in, false);
            self.enqueue(data);
        }

        // Status Stage TRB — direction is opposite of data stage
        // (or IN if no data stage)
        let status_dir = if data_len == 0 { true } else { !dir_in };
        let status = Trb::status_stage(status_dir, false);
        self.enqueue(status)
    }

    /// Enqueue an interrupt IN transfer (e.g., for HID keyboard polling).
    /// `data_phys` is the buffer physical address, `length` is the max bytes to read.
    pub fn enqueue_interrupt_in(&mut self, data_phys: u64, length: u32) -> u64 {
        let trb = Trb::normal(data_phys, length, true, false);
        self.enqueue(trb)
    }

    /// Advance the enqueue pointer, handling Link TRB wrap.
    fn advance_enqueue(&mut self) {
        self.enqueue_idx += 1;
        if self.enqueue_idx >= self.segment_len - 1 {
            log::trace!("xhci: transfer ring wrap at idx {}", self.enqueue_idx);

            // Update Link TRB cycle bit
            unsafe {
                let link = self.ring_va.add(self.segment_len - 1);
                let mut link_trb = ptr::read_volatile(link);
                if self.cycle {
                    link_trb.control |= TRB_CYCLE_BIT;
                } else {
                    link_trb.control &= !TRB_CYCLE_BIT;
                }
                ptr::write_volatile(link, link_trb);
            }

            self.enqueue_idx = 0;
            self.cycle = !self.cycle;
        }
    }
}
