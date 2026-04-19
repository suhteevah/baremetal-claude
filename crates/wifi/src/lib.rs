//! ClaudioOS Wi-Fi stack (multi-vendor).
//!
//! Vendor-independent code lives at the crate root:
//!
//! - [`ieee80211`] — 802.11 frame building / parsing (beacon, auth, assoc,
//!   QoS data, CCMP header, RSN IE)
//! - [`wpa`] — WPA2-PSK key derivation, 4-way handshake, AES-CCMP
//! - [`scan`] — scan configuration, result aggregation
//! - [`backend`] — the [`VendorBackend`] trait that every vendor
//!   implements and that the cross-vendor layers consume
//!
//! Vendor-specific drivers live in submodules:
//!
//! - [`intel`] — iwlwifi-equivalent for AC9260 / AX200 / AX201 / AX210
//! - [`realtek`] — rtw89-equivalent for RTL8852BE (stubs; TODO)
//!
//! The top-level [`WifiController`] is a thin dispatcher: call [`probe`] with
//! a PCI device descriptor and you get back the right vendor controller.
//!
//! # Usage
//!
//! ```ignore
//! use claudio_wifi::{probe, backend::PciProbe, WifiController};
//!
//! let pci = PciProbe {
//!     vendor_id: 0x8086,
//!     device_id: 0x43F0,
//!     bar0: 0xFE10_0000,
//!     irq_line: 11,
//!     phys_mem_offset: 0xFFFF_8000_0000_0000,
//! };
//!
//! let mut wifi = unsafe { probe(&pci)?.expect("no supported WiFi adapter") };
//! wifi.load_firmware(fw_data)?;
//! let networks = wifi.scan_networks()?;
//! wifi.connect("MyNetwork", "hunter2")?;
//! ```

#![no_std]

extern crate alloc;

pub mod backend;
pub mod ieee80211;
pub mod intel;
pub mod realtek;
pub mod scan;
pub mod wpa;

use alloc::string::String;
use alloc::vec::Vec;

use crate::backend::PciProbe;
use crate::intel::driver::{ConnectionInfo, IntelController, IpConfig, WifiState};
use crate::scan::{ScanConfig, ScannedNetwork};

pub use scan::ScannedNetwork as Network;

/// Intel PCI vendor ID (0x8086).
pub const INTEL_VENDOR: u16 = 0x8086;
/// Realtek PCI vendor ID (0x10EC).
pub const REALTEK_VENDOR: u16 = 0x10EC;

/// Vendor-dispatching Wi-Fi controller.
///
/// Constructed through [`probe`]. Forwards the public API (`load_firmware`,
/// `scan_networks`, `connect`, `disconnect`, `status`, `ip_config`,
/// `transmit`) to the appropriate vendor backend. Today the Intel arm is
/// fully wired; the Realtek arm is a stub placeholder.
pub enum WifiController {
    Intel(IntelController),
    Realtek(realtek::RealtekController),
}

/// Intel WiFi device variant, selected from PCI device ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiVariant {
    /// Intel Wireless-AC 9260 (CNVi, 2x2 802.11ac).
    AC9260,
    /// Intel Wi-Fi 6 AX200 (discrete PCIe, 2x2 802.11ax).
    AX200,
    /// Intel Wi-Fi 6 AX201 (CNVi, 2x2 802.11ax). Most common in 10th/11th gen laptops.
    AX201,
    /// Intel Wi-Fi 6E AX210 (discrete PCIe, 2x2 802.11ax with 6 GHz).
    AX210,
}

impl WifiVariant {
    /// Human-readable name for log messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::AC9260 => "Intel Wireless-AC 9260",
            Self::AX200 => "Intel Wi-Fi 6 AX200",
            Self::AX201 => "Intel Wi-Fi 6 AX201",
            Self::AX210 => "Intel Wi-Fi 6E AX210",
        }
    }

    /// The firmware image filename expected on the FAT32 persist partition.
    pub fn firmware_name(self) -> &'static str {
        match self {
            Self::AC9260 => "iwlwifi-9260-th-b0-jf-b0-46.ucode",
            Self::AX200 => "iwlwifi-cc-a0-46.ucode",
            Self::AX201 => "iwlwifi-QuZ-a0-hr-b0-77.ucode",
            Self::AX210 => "iwlwifi-ty-a0-gf-a0-77.ucode",
        }
    }
}

/// Probe a PCI device and return the matching vendor controller.
///
/// Returns `Ok(None)` for unrecognized PCI IDs so the caller can continue
/// scanning the bus.
///
/// # Safety
///
/// - `bar0` must be mapped into the kernel's address space.
/// - `phys_mem_offset` must be the correct virt-to-phys offset.
pub unsafe fn probe(pci: &PciProbe) -> Result<Option<WifiController>, &'static str> {
    match pci.vendor_id {
        INTEL_VENDOR => {
            let Some(variant) = intel::pci::identify(pci.vendor_id, pci.device_id) else {
                return Ok(None);
            };
            let device = intel::pci::WifiDevice {
                bus: 0,
                device: 0,
                function: 0,
                device_id: pci.device_id,
                variant,
                bar0: pci.bar0,
                irq_line: pci.irq_line,
            };
            let ctrl = unsafe { IntelController::init(&device, pci.phys_mem_offset)? };
            Ok(Some(WifiController::Intel(ctrl)))
        }
        REALTEK_VENDOR => {
            log::info!(
                "wifi::probe: Realtek device 0x{:04X} detected — readonly MMIO probe",
                pci.device_id
            );
            let ctrl = unsafe {
                realtek::RealtekController::init_readonly(pci.bar0, pci.phys_mem_offset)?
            };
            Ok(Some(WifiController::Realtek(ctrl)))
        }
        _ => {
            log::trace!(
                "wifi::probe: vendor 0x{:04X} not a known WiFi vendor",
                pci.vendor_id
            );
            Ok(None)
        }
    }
}

impl WifiController {
    /// Upload vendor firmware and wait for the device alive notification.
    pub fn load_firmware(&mut self, fw_data: &[u8]) -> Result<(), &'static str> {
        match self {
            Self::Intel(c) => c.load_firmware(fw_data),
            Self::Realtek(_) => Err("realtek: load_firmware not implemented"),
        }
    }

    /// Scan for available Wi-Fi networks using the default config.
    pub fn scan_networks(&mut self) -> Result<Vec<ScannedNetwork>, &'static str> {
        match self {
            Self::Intel(c) => c.scan_networks(),
            Self::Realtek(_) => Err("realtek: scan_networks not implemented"),
        }
    }

    /// Scan with a custom config.
    pub fn scan_networks_with_config(
        &mut self,
        config: ScanConfig,
    ) -> Result<Vec<ScannedNetwork>, &'static str> {
        match self {
            Self::Intel(c) => c.scan_networks_with_config(config),
            Self::Realtek(_) => Err("realtek: scan_networks_with_config not implemented"),
        }
    }

    /// Connect to a WPA2-PSK network.
    pub fn connect(&mut self, ssid: &str, password: &str) -> Result<(), &'static str> {
        match self {
            Self::Intel(c) => c.connect(ssid, password),
            Self::Realtek(_) => Err("realtek: connect not implemented"),
        }
    }

    /// Disconnect from the current network.
    pub fn disconnect(&mut self) -> Result<(), &'static str> {
        match self {
            Self::Intel(c) => c.disconnect(),
            Self::Realtek(_) => Err("realtek: disconnect not implemented"),
        }
    }

    /// Current connection status.
    pub fn status(&self) -> Option<ConnectionInfo> {
        match self {
            Self::Intel(c) => Some(c.status()),
            Self::Realtek(_) => None,
        }
    }

    /// Request DHCP (or return cached lease).
    pub fn ip_config(&mut self) -> Result<IpConfig, &'static str> {
        match self {
            Self::Intel(c) => c.ip_config(),
            Self::Realtek(_) => Err("realtek: ip_config not implemented"),
        }
    }

    /// Transmit a data frame (Ethernet-shaped payload). Wraps in 802.11 + CCMP.
    pub fn transmit(&mut self, payload: &[u8]) -> Result<(), &'static str> {
        match self {
            Self::Intel(c) => c.transmit(payload),
            Self::Realtek(_) => Err("realtek: transmit not implemented"),
        }
    }

    /// MAC address (valid after `load_firmware`).
    pub fn mac_address(&self) -> [u8; 6] {
        match self {
            Self::Intel(c) => c.mac_address(),
            Self::Realtek(_) => [0; 6],
        }
    }

    /// Current high-level state.
    pub fn state(&self) -> WifiState {
        match self {
            Self::Intel(c) => c.status().state,
            Self::Realtek(_) => WifiState::Uninitialized,
        }
    }

    /// Connected SSID, if any.
    pub fn connected_ssid(&self) -> Option<String> {
        match self {
            Self::Intel(c) => {
                let s = c.status().ssid;
                if s.is_empty() { None } else { Some(s) }
            }
            Self::Realtek(_) => None,
        }
    }
}
