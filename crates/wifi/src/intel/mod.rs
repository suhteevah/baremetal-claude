//! Intel iwlwifi-equivalent driver stack for Wi-Fi 5 / 6 / 6E adapters.
//!
//! Supports AX201, AX200, AX210, and AC9260 families. See the crate root
//! docs for the layered architecture overview. All modules here are
//! vendor-specific; cross-vendor logic lives in the crate root
//! (`ieee80211.rs`, `wpa.rs`, `scan.rs`) and in `backend.rs`.

pub mod pci;
pub mod firmware;
pub mod commands;
pub mod tx_rx;
pub mod driver;

pub use driver::{IntelController, WifiState, IpConfig, ConnectionInfo};
pub use pci::{WifiDevice, identify, scan_pci_bus, INTEL_VENDOR_ID};
