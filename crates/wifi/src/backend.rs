//! Vendor-independent Wi-Fi backend trait.
//!
//! A `VendorBackend` hides the transport differences between Intel iwlwifi,
//! Realtek rtw89, and whatever else ClaudioOS adds later. The cross-vendor
//! layers (`scan`, `wpa`, 802.11 management in `ieee80211`) can drive any
//! backend through this trait.
//!
//! The shape tracks what [`crate::intel::IntelController`] does today:
//!
//! - `init` builds the controller from PCI BAR0 / IRQ info
//! - `load_firmware` uploads microcode into device SRAM and waits for alive
//! - `start_scan` / `poll_scan_results` drive an active or passive scan
//! - `associate` walks auth + assoc request + add-station
//! - `tx_frame` enqueues a raw 802.11 frame on the data queue
//! - `poll_rx_frame` drains one RX descriptor if one is ready
//! - `mac_addr` / `state` expose identity and lifecycle state
//!
//! Backends are free to stub anything they do not yet implement — the
//! Realtek backend in this crate is entirely TODO, but the trait signature
//! is already carved so it can be filled in without breaking upstream
//! callers.

use alloc::string::String;
use alloc::vec::Vec;

use crate::intel::driver::WifiState;
use crate::scan::{ScanConfig, ScannedNetwork};

/// Minimal description of a PCI device handed to [`VendorBackend::init`].
///
/// Intentionally vendor-neutral — the same struct is used for Intel and
/// Realtek probes. See [`crate::probe`].
#[derive(Debug, Clone, Copy)]
pub struct PciProbe {
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0: u64,
    pub irq_line: u8,
    pub phys_mem_offset: u64,
}

/// Vendor-agnostic Wi-Fi driver operations.
///
/// Implementations live in `crate::intel` and `crate::realtek`. Cross-vendor
/// code (scan orchestration, WPA handshake glue that needs to send/receive
/// frames) takes `&mut dyn VendorBackend` rather than touching a specific
/// controller type.
pub trait VendorBackend {
    /// Human-readable device name for log output.
    fn name(&self) -> &'static str;

    /// MAC address, valid after `load_firmware` has completed.
    fn mac_addr(&self) -> [u8; 6];

    /// Current connection state.
    fn state(&self) -> WifiState;

    /// Upload vendor firmware blob and wait for the alive notification.
    fn load_firmware(&mut self, fw_data: &[u8]) -> Result<(), &'static str>;

    /// Kick off a scan with the given config. Implementations may block or
    /// return immediately; callers should follow up with `poll_scan_results`.
    fn start_scan(&mut self, config: ScanConfig) -> Result<(), &'static str>;

    /// Drain any accumulated scan results. Returns the finalized list once
    /// the scan-complete notification has been observed.
    fn poll_scan_results(&mut self) -> Result<Vec<ScannedNetwork>, &'static str>;

    /// Full connect sequence: auth → assoc → WPA2 4-way handshake → install keys.
    fn associate(&mut self, ssid: &str, password: &str) -> Result<(), &'static str>;

    /// Disassociate and clear keys.
    fn disassociate(&mut self) -> Result<(), &'static str>;

    /// Enqueue a raw 802.11 (or EAPOL) frame on the data queue.
    fn tx_frame(&mut self, frame: &[u8]) -> Result<(), &'static str>;

    /// Pop one RX descriptor if available. Returns the raw frame bytes.
    fn poll_rx_frame(&mut self) -> Option<Vec<u8>>;

    /// Optional: SSID of the currently connected network, if any.
    fn connected_ssid(&self) -> Option<String> {
        None
    }
}
