//! Windows Registry emulation — in-memory key-value tree.
//!
//! Provides the backing store for advapi32 registry APIs. Keys are organized
//! as a tree (like the real Windows registry), with each key holding named
//! values of types REG_SZ, REG_DWORD, REG_BINARY, etc.
//!
//! The entire registry lives in heap memory. In the future, it can be persisted
//! to VFS as a serialized blob.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// =============================================================================
// Registry value types
// =============================================================================

/// A registry value.
#[derive(Debug, Clone)]
pub enum RegValue {
    /// REG_NONE — no value type.
    None,
    /// REG_SZ — null-terminated UTF-16LE string.
    String(String),
    /// REG_EXPAND_SZ — expandable string with environment variable references.
    ExpandString(String),
    /// REG_BINARY — raw binary data.
    Binary(Vec<u8>),
    /// REG_DWORD — 32-bit integer (little-endian).
    Dword(u32),
    /// REG_DWORD_BIG_ENDIAN — 32-bit integer (big-endian).
    DwordBe(u32),
    /// REG_MULTI_SZ — list of null-terminated strings.
    MultiString(Vec<String>),
    /// REG_QWORD — 64-bit integer.
    Qword(u64),
}

/// Registry type constants (must match advapi32 constants).
const REG_NONE: u32 = 0;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const REG_DWORD_BIG_ENDIAN: u32 = 5;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

impl RegValue {
    /// Convert this value to its raw byte representation and type code.
    pub fn to_bytes(&self) -> (u32, Vec<u8>) {
        match self {
            RegValue::None => (REG_NONE, Vec::new()),
            RegValue::String(s) => {
                let utf16 = crate::unicode::utf8_to_utf16(s);
                let bytes: Vec<u8> = utf16.iter()
                    .flat_map(|&w| w.to_le_bytes())
                    .collect();
                (REG_SZ, bytes)
            }
            RegValue::ExpandString(s) => {
                let utf16 = crate::unicode::utf8_to_utf16(s);
                let bytes: Vec<u8> = utf16.iter()
                    .flat_map(|&w| w.to_le_bytes())
                    .collect();
                (REG_EXPAND_SZ, bytes)
            }
            RegValue::Binary(data) => (REG_BINARY, data.clone()),
            RegValue::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
            RegValue::DwordBe(v) => (REG_DWORD_BIG_ENDIAN, v.to_be_bytes().to_vec()),
            RegValue::MultiString(strings) => {
                let mut bytes = Vec::new();
                for s in strings {
                    let utf16 = crate::unicode::utf8_to_utf16(s);
                    for w in &utf16 {
                        bytes.extend_from_slice(&w.to_le_bytes());
                    }
                }
                // Double null terminator
                bytes.extend_from_slice(&[0, 0]);
                (REG_MULTI_SZ, bytes)
            }
            RegValue::Qword(v) => (REG_QWORD, v.to_le_bytes().to_vec()),
        }
    }

    /// Parse a value from raw bytes and type code.
    pub fn from_bytes(reg_type: u32, data: &[u8]) -> RegValue {
        match reg_type {
            REG_NONE => RegValue::None,
            REG_SZ | REG_EXPAND_SZ => {
                // Decode UTF-16LE bytes to string
                let u16_data: Vec<u16> = data.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                // Strip trailing null
                let text: Vec<u16> = u16_data.into_iter()
                    .take_while(|&c| c != 0)
                    .collect();
                let s = crate::unicode::utf16_slice_to_string(&text);
                if reg_type == REG_SZ {
                    RegValue::String(s)
                } else {
                    RegValue::ExpandString(s)
                }
            }
            REG_BINARY => RegValue::Binary(data.to_vec()),
            REG_DWORD => {
                let v = if data.len() >= 4 {
                    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                } else {
                    0
                };
                RegValue::Dword(v)
            }
            REG_DWORD_BIG_ENDIAN => {
                let v = if data.len() >= 4 {
                    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
                } else {
                    0
                };
                RegValue::DwordBe(v)
            }
            REG_MULTI_SZ => {
                let u16_data: Vec<u16> = data.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let mut strings = Vec::new();
                let mut current = Vec::new();
                for &c in &u16_data {
                    if c == 0 {
                        if current.is_empty() {
                            break; // Double null = end
                        }
                        strings.push(crate::unicode::utf16_slice_to_string(&current));
                        current.clear();
                    } else {
                        current.push(c);
                    }
                }
                RegValue::MultiString(strings)
            }
            REG_QWORD => {
                let v = if data.len() >= 8 {
                    u64::from_le_bytes([
                        data[0], data[1], data[2], data[3],
                        data[4], data[5], data[6], data[7],
                    ])
                } else {
                    0
                };
                RegValue::Qword(v)
            }
            _ => RegValue::Binary(data.to_vec()),
        }
    }
}

// =============================================================================
// Registry hive (in-memory tree)
// =============================================================================

/// A registry key node.
#[derive(Debug, Clone)]
struct RegKey {
    /// Named values stored under this key.
    values: BTreeMap<String, RegValue>,
    /// Subkey names (the actual subkeys are stored by path in the flat map).
    subkeys: Vec<String>,
}

impl RegKey {
    fn new() -> Self {
        Self {
            values: BTreeMap::new(),
            subkeys: Vec::new(),
        }
    }
}

/// Registry hive — flat map of path -> key.
pub struct RegistryHive {
    keys: BTreeMap<String, RegKey>,
}

impl RegistryHive {
    fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }
}

/// Global registry.
static REGISTRY: Mutex<Option<RegistryHive>> = Mutex::new(None);

/// Initialize the registry with default keys.
pub fn init() {
    let mut reg = RegistryHive::new();

    // Create predefined hive roots
    for root in &["HKCR", "HKCU", "HKLM", "HKU", "HKCC"] {
        reg.keys.insert(String::from(*root), RegKey::new());
    }

    // Default HKLM\SOFTWARE entries
    let sw_path = String::from("HKLM\\SOFTWARE");
    reg.keys.insert(sw_path, RegKey::new());

    let ms_path = String::from("HKLM\\SOFTWARE\\Microsoft");
    reg.keys.insert(ms_path, RegKey::new());

    // Windows NT CurrentVersion — programs often query this
    let cv_path = String::from("HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let mut cv_key = RegKey::new();
    cv_key.values.insert(
        String::from("ProductName"),
        RegValue::String(String::from("ClaudioOS")),
    );
    cv_key.values.insert(
        String::from("CurrentBuildNumber"),
        RegValue::String(String::from("19045")),
    );
    cv_key.values.insert(
        String::from("CurrentVersion"),
        RegValue::String(String::from("10.0")),
    );
    reg.keys.insert(cv_path, cv_key);

    // HKCU defaults
    let hkcu_env = String::from("HKCU\\Environment");
    reg.keys.insert(hkcu_env, RegKey::new());

    *REGISTRY.lock() = Some(reg);
    log::info!("[registry] Initialized with default keys");
}

// =============================================================================
// Public API for advapi32
// =============================================================================

/// Normalize a key path (case-insensitive matching would go here).
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\")
}

/// Check if a key exists.
pub fn key_exists(path: &str) -> bool {
    let norm = normalize_path(path);
    let reg = REGISTRY.lock();
    reg.as_ref().map(|r| r.keys.contains_key(&norm)).unwrap_or(false)
}

/// Create a key (and all parent keys as needed).
pub fn create_key(path: &str) {
    let norm = normalize_path(path);
    let mut reg = REGISTRY.lock();
    if reg.is_none() {
        init();
        // Re-lock after init
    }
    let registry = reg.as_mut().unwrap();

    // Create parent keys if they don't exist
    let parts: Vec<&str> = norm.split('\\').collect();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            current.push('\\');
        }
        current.push_str(part);

        if !registry.keys.contains_key(&current) {
            registry.keys.insert(current.clone(), RegKey::new());

            // Add as subkey of parent
            if i > 0 {
                let parent: String = parts[..i].join("\\");
                if let Some(parent_key) = registry.keys.get_mut(&parent) {
                    if !parent_key.subkeys.contains(&String::from(*part)) {
                        parent_key.subkeys.push(String::from(*part));
                    }
                }
            }
        }
    }
}

/// Delete a key.
pub fn delete_key(path: &str) -> bool {
    let norm = normalize_path(path);
    let mut reg = REGISTRY.lock();
    if let Some(ref mut registry) = *reg {
        // Remove the key and all subkeys
        let keys_to_remove: Vec<String> = registry.keys.keys()
            .filter(|k| k.starts_with(&norm))
            .cloned()
            .collect();

        if keys_to_remove.is_empty() {
            return false;
        }

        for key in keys_to_remove {
            registry.keys.remove(&key);
        }

        // Remove from parent's subkey list
        if let Some(pos) = norm.rfind('\\') {
            let parent = &norm[..pos];
            let child = &norm[pos + 1..];
            if let Some(parent_key) = registry.keys.get_mut(parent) {
                parent_key.subkeys.retain(|s| s != child);
            }
        }

        true
    } else {
        false
    }
}

/// Get a value from a key.
pub fn get_value(key_path: &str, value_name: &str) -> Option<RegValue> {
    let norm = normalize_path(key_path);
    let reg = REGISTRY.lock();
    reg.as_ref()
        .and_then(|r| r.keys.get(&norm))
        .and_then(|k| k.values.get(value_name))
        .cloned()
}

/// Set a value on a key.
pub fn set_value(key_path: &str, value_name: &str, value: RegValue) {
    let norm = normalize_path(key_path);
    let mut reg = REGISTRY.lock();
    if let Some(ref mut registry) = *reg {
        // Create key if it doesn't exist
        if !registry.keys.contains_key(&norm) {
            registry.keys.insert(norm.clone(), RegKey::new());
        }
        if let Some(key) = registry.keys.get_mut(&norm) {
            key.values.insert(String::from(value_name), value);
        }
    }
}

/// List subkeys of a key.
pub fn list_subkeys(key_path: &str) -> Vec<String> {
    let norm = normalize_path(key_path);
    let reg = REGISTRY.lock();
    reg.as_ref()
        .and_then(|r| r.keys.get(&norm))
        .map(|k| k.subkeys.clone())
        .unwrap_or_default()
}

/// List values of a key as (name, value) pairs.
pub fn list_values(key_path: &str) -> Vec<(String, RegValue)> {
    let norm = normalize_path(key_path);
    let reg = REGISTRY.lock();
    reg.as_ref()
        .and_then(|r| r.keys.get(&norm))
        .map(|k| k.values.iter().map(|(n, v)| (n.clone(), v.clone())).collect())
        .unwrap_or_default()
}
