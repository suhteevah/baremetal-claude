//! WinRT type projections.
//!
//! Maps WinRT types to Rust-friendly wrappers: Windows.Foundation
//! (IStringable, IClosable, Uri, DateTime, TimeSpan), Windows.Storage
//! (StorageFile, StorageFolder), Windows.UI (Colors), Windows.Networking
//! (HostName).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::activation::{self, WinRtValue, HString};

// ─── Windows.Foundation ─────────────────────────────────────────────────────

/// Windows.Foundation.Uri projection.
#[derive(Debug, Clone)]
pub struct Uri {
    /// Object handle in the WinRT runtime.
    pub handle: u64,
    /// The original URI string.
    pub raw_uri: String,
    /// Parsed scheme (e.g., "https").
    pub scheme: String,
    /// Parsed host.
    pub host: String,
    /// Parsed port (-1 if not specified).
    pub port: i32,
    /// Parsed path.
    pub path: String,
    /// Parsed query string (without leading '?').
    pub query: String,
}

impl Uri {
    /// Create a new Uri from a string.
    pub fn create(uri_string: &str) -> Result<Self, i32> {
        let handle = activation::ro_activate_instance("Windows.Foundation.Uri")
            .unwrap_or(0);

        // Simple URI parsing
        let (scheme, rest) = if let Some(idx) = uri_string.find("://") {
            (
                String::from(&uri_string[..idx]),
                &uri_string[idx + 3..],
            )
        } else {
            (String::from(""), uri_string)
        };

        let (authority, path_and_query) = if let Some(idx) = rest.find('/') {
            (&rest[..idx], &rest[idx..])
        } else {
            (rest, "/")
        };

        let (host, port) = if let Some(idx) = authority.find(':') {
            let p: i32 = authority[idx + 1..].parse().unwrap_or(-1);
            (String::from(&authority[..idx]), p)
        } else {
            (String::from(authority), -1)
        };

        let (path, query) = if let Some(idx) = path_and_query.find('?') {
            (
                String::from(&path_and_query[..idx]),
                String::from(&path_and_query[idx + 1..]),
            )
        } else {
            (String::from(path_and_query), String::new())
        };

        Ok(Self {
            handle,
            raw_uri: String::from(uri_string),
            scheme,
            host,
            port,
            path,
            query,
        })
    }

    /// Get the absolute URI string.
    pub fn absolute_uri(&self) -> &str {
        &self.raw_uri
    }

    /// IStringable.ToString()
    pub fn to_string(&self) -> String {
        self.raw_uri.clone()
    }
}

/// Windows.Foundation.DateTime projection.
///
/// Represents a point in time as 100-nanosecond intervals since
/// January 1, 1601 (Windows FILETIME epoch).
#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    /// Universal time in 100-nanosecond intervals.
    pub universal_time: i64,
}

impl DateTime {
    /// Create a DateTime from a universal time value.
    pub fn from_universal_time(ticks: i64) -> Self {
        Self {
            universal_time: ticks,
        }
    }

    /// Create a DateTime representing "now" (stub: returns epoch).
    pub fn now() -> Self {
        Self { universal_time: 0 }
    }

    /// Convert to Unix timestamp (seconds since 1970-01-01).
    pub fn to_unix_timestamp(&self) -> i64 {
        // Windows FILETIME epoch is 1601-01-01.
        // Unix epoch is 1970-01-01.
        // Difference is 11644473600 seconds.
        const EPOCH_DIFF: i64 = 11_644_473_600;
        (self.universal_time / 10_000_000) - EPOCH_DIFF
    }
}

/// Windows.Foundation.TimeSpan projection.
///
/// Represents a time interval as 100-nanosecond ticks.
#[derive(Debug, Clone, Copy)]
pub struct TimeSpan {
    /// Duration in 100-nanosecond intervals.
    pub duration: i64,
}

impl TimeSpan {
    /// Create a TimeSpan from a duration in 100-nanosecond ticks.
    pub fn from_ticks(ticks: i64) -> Self {
        Self { duration: ticks }
    }

    /// Create a TimeSpan from seconds.
    pub fn from_seconds(seconds: f64) -> Self {
        Self {
            duration: (seconds * 10_000_000.0) as i64,
        }
    }

    /// Create a TimeSpan from milliseconds.
    pub fn from_milliseconds(ms: f64) -> Self {
        Self {
            duration: (ms * 10_000.0) as i64,
        }
    }

    /// Get the total seconds.
    pub fn total_seconds(&self) -> f64 {
        self.duration as f64 / 10_000_000.0
    }

    /// Get the total milliseconds.
    pub fn total_milliseconds(&self) -> f64 {
        self.duration as f64 / 10_000.0
    }
}

// ─── Windows.Storage ────────────────────────────────────────────────────────

/// Windows.Storage.StorageFile projection.
#[derive(Debug, Clone)]
pub struct StorageFile {
    /// Object handle.
    pub handle: u64,
    /// File name.
    pub name: String,
    /// Full file path.
    pub path: String,
    /// File extension (e.g., ".txt").
    pub file_type: String,
    /// File content (simulated).
    pub content: Vec<u8>,
}

impl StorageFile {
    /// Create a StorageFile from a path (synchronous, for testing).
    pub fn from_path(path: &str) -> Self {
        let name = path
            .rsplit('/')
            .next()
            .or_else(|| path.rsplit('\\').next())
            .unwrap_or(path);
        let file_type = name
            .rsplit('.')
            .next()
            .map(|ext| alloc::format!(".{}", ext))
            .unwrap_or_default();

        Self {
            handle: 0,
            name: String::from(name),
            path: String::from(path),
            file_type,
            content: Vec::new(),
        }
    }
}

/// Windows.Storage.StorageFolder projection.
#[derive(Debug, Clone)]
pub struct StorageFolder {
    /// Object handle.
    pub handle: u64,
    /// Folder name.
    pub name: String,
    /// Full folder path.
    pub path: String,
}

impl StorageFolder {
    /// Create a StorageFolder from a path.
    pub fn from_path(path: &str) -> Self {
        let name = path
            .rsplit('/')
            .next()
            .or_else(|| path.rsplit('\\').next())
            .unwrap_or(path);

        Self {
            handle: 0,
            name: String::from(name),
            path: String::from(path),
        }
    }
}

// ─── Windows.UI ─────────────────────────────────────────────────────────────

/// A Windows.UI color (ARGB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn from_argb(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self { a, r, g, b }
    }

    /// Convert to a 32-bit ARGB value.
    pub fn to_argb(&self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }
}

/// Windows.UI.Colors — named color constants.
pub struct Colors;

impl Colors {
    pub const fn black() -> Color { Color::from_argb(255, 0, 0, 0) }
    pub const fn white() -> Color { Color::from_argb(255, 255, 255, 255) }
    pub const fn red() -> Color { Color::from_argb(255, 255, 0, 0) }
    pub const fn green() -> Color { Color::from_argb(255, 0, 128, 0) }
    pub const fn blue() -> Color { Color::from_argb(255, 0, 0, 255) }
    pub const fn yellow() -> Color { Color::from_argb(255, 255, 255, 0) }
    pub const fn cyan() -> Color { Color::from_argb(255, 0, 255, 255) }
    pub const fn magenta() -> Color { Color::from_argb(255, 255, 0, 255) }
    pub const fn gray() -> Color { Color::from_argb(255, 128, 128, 128) }
    pub const fn transparent() -> Color { Color::from_argb(0, 255, 255, 255) }
    pub const fn cornflower_blue() -> Color { Color::from_argb(255, 100, 149, 237) }
    pub const fn orange() -> Color { Color::from_argb(255, 255, 165, 0) }
    pub const fn purple() -> Color { Color::from_argb(255, 128, 0, 128) }
}

// ─── Windows.Networking ─────────────────────────────────────────────────────

/// Windows.Networking.HostName projection.
#[derive(Debug, Clone)]
pub struct HostName {
    /// Object handle.
    pub handle: u64,
    /// Display name.
    pub display_name: String,
    /// Raw name.
    pub raw_name: String,
    /// Host name type.
    pub host_type: HostNameType,
}

/// Windows.Networking.HostNameType enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostNameType {
    DomainName,
    Ipv4,
    Ipv6,
    Bluetooth,
}

impl HostName {
    /// Create a new HostName.
    pub fn create(name: &str) -> Self {
        let host_type = if name.contains(':') {
            HostNameType::Ipv6
        } else if name.chars().all(|c| c.is_ascii_digit() || c == '.') {
            HostNameType::Ipv4
        } else {
            HostNameType::DomainName
        };

        Self {
            handle: 0,
            display_name: String::from(name),
            raw_name: String::from(name),
            host_type,
        }
    }

    /// IStringable.ToString()
    pub fn to_string(&self) -> String {
        self.display_name.clone()
    }
}
