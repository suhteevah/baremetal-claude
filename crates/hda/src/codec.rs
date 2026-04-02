//! HDA codec communication via CORB/RIRB ring buffers.
//!
//! The Command Output Ring Buffer (CORB) sends 32-bit verbs to codecs.
//! The Response Input Ring Buffer (RIRB) receives 64-bit responses.
//! Per Intel HDA spec 1.0a, Sections 4.4 and 4.5.

use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use log::{debug, error, info, trace, warn};

use crate::registers::*;

/// Maximum number of CORB/RIRB entries (256 is the largest supported size).
const RING_BUF_MAX_ENTRIES: usize = 256;

/// CORB entry size: 4 bytes (one 32-bit verb).
const CORB_ENTRY_SIZE: usize = 4;

/// RIRB entry size: 8 bytes (32-bit response + 32-bit response extended).
const RIRB_ENTRY_SIZE: usize = 8;

/// Maximum poll iterations waiting for a RIRB response before timeout.
const RIRB_POLL_TIMEOUT: u32 = 100_000;

// =============================================================================
// Verb encoding helpers (Section 7.1)
// =============================================================================

/// Encode a 12-bit verb (4-bit payload): `codec(4) | nid(8) | verb(12) | payload(8)`
/// Total = 32 bits. Used for "Set" and short "Get" verbs.
#[inline]
pub const fn encode_verb_short(codec: u8, nid: u8, verb: u16, payload: u8) -> u32 {
    ((codec as u32 & 0x0F) << 28)
        | ((nid as u32) << 20)
        | ((verb as u32 & 0xFFF) << 8)
        | (payload as u32)
}

/// Encode a 4-bit verb with 16-bit payload: `codec(4) | nid(8) | verb(4) | payload(16)`
/// Used for verbs like SET_STREAM_FORMAT, SET_AMP_GAIN, SET_CHANNEL_STREAMID.
#[inline]
pub const fn encode_verb_long(codec: u8, nid: u8, verb: u8, payload: u16) -> u32 {
    ((codec as u32 & 0x0F) << 28)
        | ((nid as u32) << 20)
        | ((verb as u32 & 0x0F) << 16)
        | (payload as u32)
}

// =============================================================================
// Common verb IDs (Section 7.3)
// =============================================================================

/// GET_PARAMETER (verb 0xF00, 8-bit parameter ID)
pub const VERB_GET_PARAMETER: u16 = 0xF00;

/// GET_CONNECTION_LIST_ENTRY (verb 0xF02)
pub const VERB_GET_CONN_LIST: u16 = 0xF02;

/// GET_CONNECTION_SELECT (verb 0xF01)
pub const VERB_GET_CONN_SELECT: u16 = 0xF01;

/// SET_CONNECTION_SELECT (verb 0x701)
pub const VERB_SET_CONN_SELECT: u16 = 0x701;

/// GET_PIN_WIDGET_CONTROL (verb 0xF07)
pub const VERB_GET_PIN_CTRL: u16 = 0xF07;

/// SET_PIN_WIDGET_CONTROL (verb 0x707)
pub const VERB_SET_PIN_CTRL: u16 = 0x707;

/// GET_EAPD/BTL_ENABLE (verb 0xF0C)
pub const VERB_GET_EAPDBTL: u16 = 0xF0C;

/// SET_EAPD/BTL_ENABLE (verb 0x70C)
pub const VERB_SET_EAPDBTL: u16 = 0x70C;

/// GET_POWER_STATE (verb 0xF05)
pub const VERB_GET_POWER_STATE: u16 = 0xF05;

/// SET_POWER_STATE (verb 0x705)
pub const VERB_SET_POWER_STATE: u16 = 0x705;

/// GET_STREAM_FORMAT (4-bit verb 0xA)
pub const VERB_GET_STREAM_FORMAT: u8 = 0x0A;

/// SET_STREAM_FORMAT (4-bit verb 0x2)
pub const VERB_SET_STREAM_FORMAT: u8 = 0x02;

/// GET_AMP_GAIN_MUTE (verb 0xB)
pub const VERB_GET_AMP_GAIN: u8 = 0x0B;

/// SET_AMP_GAIN_MUTE (verb 0x3)
pub const VERB_SET_AMP_GAIN: u8 = 0x03;

/// SET_CHANNEL/STREAM_ID (4-bit verb 0x6)
pub const VERB_SET_CHANNEL_STREAMID: u8 = 0x06;

/// GET_CHANNEL/STREAM_ID (verb 0xF06)
pub const VERB_GET_CHANNEL_STREAMID: u16 = 0xF06;

/// GET_CONFIGURATION_DEFAULT (verb 0xF1C)
pub const VERB_GET_CONFIG_DEFAULT: u16 = 0xF1C;

// =============================================================================
// Pin Widget Control bits (Section 7.3.3.13)
// =============================================================================

/// Headphone Enable (bit 7)
pub const PIN_CTRL_HP_ENABLE: u8 = 1 << 7;
/// Output Enable (bit 6)
pub const PIN_CTRL_OUT_ENABLE: u8 = 1 << 6;
/// Input Enable (bit 5)
pub const PIN_CTRL_IN_ENABLE: u8 = 1 << 5;

// =============================================================================
// Amp Gain/Mute payload encoding (Section 7.3.3.7)
// =============================================================================

/// Set Output Amp (bit 15)
pub const AMP_SET_OUTPUT: u16 = 1 << 15;
/// Set Input Amp (bit 14)
pub const AMP_SET_INPUT: u16 = 1 << 14;
/// Set Left Channel (bit 13)
pub const AMP_SET_LEFT: u16 = 1 << 13;
/// Set Right Channel (bit 12)
pub const AMP_SET_RIGHT: u16 = 1 << 12;
/// Mute (bit 7)
pub const AMP_MUTE: u16 = 1 << 7;
/// Gain index mask (bits 6:0)
pub const AMP_GAIN_MASK: u16 = 0x7F;

// =============================================================================
// CORB/RIRB ring buffer management
// =============================================================================

/// CORB/RIRB state for communicating with codecs.
pub struct CorbRirb {
    /// BAR0 MMIO base address.
    bar0: usize,
    /// CORB DMA buffer (aligned, pinned in physical memory).
    /// Each entry is 4 bytes (u32 verb).
    corb_buf: Vec<u32>,
    /// Physical address of the CORB buffer.
    corb_phys: u64,
    /// Number of CORB entries (2, 16, or 256).
    corb_entries: usize,
    /// RIRB DMA buffer (aligned, pinned in physical memory).
    /// Each entry is 8 bytes: u32 response + u32 response_ex.
    rirb_buf: Vec<u64>,
    /// Physical address of the RIRB buffer.
    rirb_phys: u64,
    /// Number of RIRB entries (2, 16, or 256).
    rirb_entries: usize,
    /// Current RIRB read pointer (software-maintained).
    rirb_rp: usize,
}

impl CorbRirb {
    /// Create and initialize CORB/RIRB for the controller at `bar0`.
    ///
    /// This allocates DMA buffers, configures CORB/RIRB size and base addresses,
    /// resets read/write pointers, and starts both DMA engines.
    ///
    /// # Safety
    /// `bar0` must be a valid, mapped HDA controller BAR0 address.
    pub unsafe fn init(bar0: usize) -> Self {
        info!("hda: initializing CORB/RIRB ring buffers");

        // --- Determine CORB size ---
        let corbsize_reg = unsafe { read8(bar0, CORBSIZE) };
        let corb_size_cap = (corbsize_reg >> 4) & 0x0F;
        let (corb_entries, corb_size_val) = Self::pick_ring_size(corb_size_cap);
        debug!(
            "hda: CORB size capability=0x{:02x}, selecting {} entries (val={})",
            corb_size_cap, corb_entries, corb_size_val
        );

        // --- Determine RIRB size ---
        let rirbsize_reg = unsafe { read8(bar0, RIRBSIZE) };
        let rirb_size_cap = (rirbsize_reg >> 4) & 0x0F;
        let (rirb_entries, rirb_size_val) = Self::pick_ring_size(rirb_size_cap);
        debug!(
            "hda: RIRB size capability=0x{:02x}, selecting {} entries (val={})",
            rirb_size_cap, rirb_entries, rirb_size_val
        );

        // --- Stop CORB/RIRB if running ---
        unsafe {
            write8(bar0, CORBCTL, 0);
            write8(bar0, RIRBCTL, 0);
        }
        trace!("hda: stopped CORB/RIRB DMA engines");

        // --- Allocate CORB buffer ---
        // Must be physically contiguous and aligned. In our single-address-space
        // kernel, heap addresses ARE physical addresses (identity-mapped).
        let corb_buf = vec![0u32; corb_entries];
        let corb_phys = corb_buf.as_ptr() as u64;
        info!(
            "hda: CORB buffer allocated at phys=0x{:016x}, {} entries",
            corb_phys, corb_entries
        );

        // --- Allocate RIRB buffer ---
        let rirb_buf = vec![0u64; rirb_entries];
        let rirb_phys = rirb_buf.as_ptr() as u64;
        info!(
            "hda: RIRB buffer allocated at phys=0x{:016x}, {} entries",
            rirb_phys, rirb_entries
        );

        // --- Configure CORB ---
        unsafe {
            // Set CORB base address
            write32(bar0, CORBLBASE, corb_phys as u32);
            write32(bar0, CORBUBASE, (corb_phys >> 32) as u32);
            trace!(
                "hda: CORB base address set to 0x{:016x}",
                corb_phys
            );

            // Set CORB size
            write8(bar0, CORBSIZE, (corbsize_reg & 0xFC) | corb_size_val);

            // Reset CORB read pointer
            write16(bar0, CORBRP, CORBRP_RST);
            // Wait for hardware to acknowledge reset
            let mut attempts = 0;
            while read16(bar0, CORBRP) & CORBRP_RST == 0 {
                attempts += 1;
                if attempts > 10_000 {
                    warn!("hda: CORB read pointer reset not acknowledged after {} attempts", attempts);
                    break;
                }
            }
            // Clear reset bit
            write16(bar0, CORBRP, 0);
            let mut attempts = 0;
            while read16(bar0, CORBRP) & CORBRP_RST != 0 {
                attempts += 1;
                if attempts > 10_000 {
                    warn!("hda: CORB read pointer reset clear not acknowledged");
                    break;
                }
            }
            trace!("hda: CORB read pointer reset complete");

            // Set CORB write pointer to 0
            write16(bar0, CORBWP, 0);
        }

        // --- Configure RIRB ---
        unsafe {
            // Set RIRB base address
            write32(bar0, RIRBLBASE, rirb_phys as u32);
            write32(bar0, RIRBUBASE, (rirb_phys >> 32) as u32);
            trace!(
                "hda: RIRB base address set to 0x{:016x}",
                rirb_phys
            );

            // Set RIRB size
            write8(bar0, RIRBSIZE, (rirbsize_reg & 0xFC) | rirb_size_val);

            // Reset RIRB write pointer
            write16(bar0, RIRBWP, RIRBWP_RST);
            trace!("hda: RIRB write pointer reset");

            // Set response interrupt count (1 = interrupt after every response)
            write16(bar0, RINTCNT, 1);
        }

        // --- Start CORB/RIRB DMA engines ---
        unsafe {
            write8(bar0, CORBCTL, CORBCTL_RUN | CORBCTL_MEIE);
            write8(bar0, RIRBCTL, RIRBCTL_RUN | RIRBCTL_RINTCTL);
        }
        info!("hda: CORB/RIRB DMA engines started");

        CorbRirb {
            bar0,
            corb_buf,
            corb_phys,
            corb_entries,
            rirb_buf,
            rirb_phys,
            rirb_entries,
            rirb_rp: 0,
        }
    }

    /// Pick the largest supported ring buffer size from the capability bits.
    ///
    /// Returns (number_of_entries, size_register_value).
    fn pick_ring_size(cap: u8) -> (usize, u8) {
        if cap & (1 << 2) != 0 {
            (256, RINGBUF_SIZE_256)
        } else if cap & (1 << 1) != 0 {
            (16, RINGBUF_SIZE_16)
        } else {
            (2, RINGBUF_SIZE_2)
        }
    }

    /// Send a 32-bit verb to a codec via CORB and wait for the RIRB response.
    ///
    /// Returns the 32-bit response on success, or `None` on timeout.
    ///
    /// # Safety
    /// BAR0 must remain valid.
    pub unsafe fn send_verb(&mut self, verb: u32) -> Option<u32> {
        trace!("hda: sending verb 0x{:08x}", verb);

        // Read current CORB write pointer
        let wp = unsafe { read16(self.bar0, CORBWP) } as usize;
        let new_wp = (wp + 1) % self.corb_entries;

        // Write verb to CORB buffer
        unsafe {
            let entry_ptr = (self.corb_phys as usize + new_wp * CORB_ENTRY_SIZE) as *mut u32;
            ptr::write_volatile(entry_ptr, verb);
        }

        // Bump CORB write pointer to submit the command
        unsafe {
            write16(self.bar0, CORBWP, new_wp as u16);
        }
        trace!(
            "hda: CORB write pointer advanced {} -> {}",
            wp,
            new_wp
        );

        // Poll RIRB for response
        let expected_rp = (self.rirb_rp + 1) % self.rirb_entries;
        let mut poll_count: u32 = 0;

        loop {
            let rirb_wp = unsafe { read16(self.bar0, RIRBWP) } as usize;

            if rirb_wp != self.rirb_rp {
                // New response available
                let response_ptr =
                    (self.rirb_phys as usize + expected_rp * RIRB_ENTRY_SIZE) as *const u64;
                let raw = unsafe { ptr::read_volatile(response_ptr) };

                let response = raw as u32;
                let response_ex = (raw >> 32) as u32;
                let solicited = (response_ex & (1 << 4)) == 0;

                self.rirb_rp = expected_rp;

                // Clear RIRB interrupt status
                unsafe {
                    write8(self.bar0, RIRBSTS, RIRBSTS_RINTFL);
                }

                if solicited {
                    trace!(
                        "hda: RIRB response: 0x{:08x} (ex=0x{:08x}, solicited)",
                        response,
                        response_ex
                    );
                    return Some(response);
                } else {
                    debug!(
                        "hda: unsolicited response 0x{:08x} (ex=0x{:08x}), skipping",
                        response, response_ex
                    );
                    // Continue polling for the solicited response
                }
            }

            poll_count += 1;
            if poll_count >= RIRB_POLL_TIMEOUT {
                error!(
                    "hda: RIRB poll timeout after {} iterations for verb 0x{:08x}",
                    poll_count, verb
                );
                return None;
            }

            // Busy-wait (in bare-metal, no sleep primitive here)
            core::hint::spin_loop();
        }
    }

    /// Send a short (12-bit) verb and return the response.
    ///
    /// # Safety
    /// BAR0 must remain valid.
    pub unsafe fn get_parameter(&mut self, codec: u8, nid: u8, param_id: u8) -> Option<u32> {
        let verb = encode_verb_short(codec, nid, VERB_GET_PARAMETER, param_id);
        unsafe { self.send_verb(verb) }
    }

    /// Send a GET verb (12-bit verb, 8-bit payload) and return the response.
    ///
    /// # Safety
    /// BAR0 must remain valid.
    pub unsafe fn get_verb(
        &mut self,
        codec: u8,
        nid: u8,
        verb_id: u16,
        payload: u8,
    ) -> Option<u32> {
        let verb = encode_verb_short(codec, nid, verb_id, payload);
        unsafe { self.send_verb(verb) }
    }

    /// Send a SET verb (12-bit verb, 8-bit payload).
    ///
    /// # Safety
    /// BAR0 must remain valid.
    pub unsafe fn set_verb(
        &mut self,
        codec: u8,
        nid: u8,
        verb_id: u16,
        payload: u8,
    ) -> Option<u32> {
        let verb = encode_verb_short(codec, nid, verb_id, payload);
        unsafe { self.send_verb(verb) }
    }

    /// Send a long (4-bit) verb with 16-bit payload and return the response.
    ///
    /// # Safety
    /// BAR0 must remain valid.
    pub unsafe fn send_verb_long(
        &mut self,
        codec: u8,
        nid: u8,
        verb_id: u8,
        payload: u16,
    ) -> Option<u32> {
        let verb = encode_verb_long(codec, nid, verb_id, payload);
        unsafe { self.send_verb(verb) }
    }
}
