//! Advapi32.dll API implementation — registry and security.
//!
//! Registry operations map to our in-memory key-value tree (see `registry` module).
//! Cryptographic functions map to ClaudioOS CSPRNG.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::handles::{self, HandleObject};
use crate::registry::{self, RegistryHive, RegValue};
use crate::teb_peb;

// =============================================================================
// Registry constants
// =============================================================================

/// Predefined registry key handles.
pub const HKEY_CLASSES_ROOT: HKey = 0x80000000;
pub const HKEY_CURRENT_USER: HKey = 0x80000001;
pub const HKEY_LOCAL_MACHINE: HKey = 0x80000002;
pub const HKEY_USERS: HKey = 0x80000003;
pub const HKEY_CURRENT_CONFIG: HKey = 0x80000005;

/// Registry access rights.
pub const KEY_READ: DWord = 0x20019;
pub const KEY_WRITE: DWord = 0x20006;
pub const KEY_ALL_ACCESS: DWord = 0xF003F;
pub const KEY_QUERY_VALUE: DWord = 0x0001;
pub const KEY_SET_VALUE: DWord = 0x0002;
pub const KEY_CREATE_SUB_KEY: DWord = 0x0004;
pub const KEY_ENUMERATE_SUB_KEYS: DWord = 0x0008;

/// Registry value types.
pub const REG_NONE: DWord = 0;
pub const REG_SZ: DWord = 1;
pub const REG_EXPAND_SZ: DWord = 2;
pub const REG_BINARY: DWord = 3;
pub const REG_DWORD: DWord = 4;
pub const REG_DWORD_BIG_ENDIAN: DWord = 5;
pub const REG_MULTI_SZ: DWord = 7;
pub const REG_QWORD: DWord = 11;

/// Registry error codes.
pub const ERROR_SUCCESS: DWord = 0;
pub const ERROR_FILE_NOT_FOUND: DWord = 2;
pub const ERROR_MORE_DATA: DWord = 234;
pub const ERROR_NO_MORE_ITEMS: DWord = 259;

/// Disposition values for RegCreateKeyExW.
pub const REG_CREATED_NEW_KEY: DWord = 1;
pub const REG_OPENED_EXISTING_KEY: DWord = 2;

// =============================================================================
// Registry API
// =============================================================================

/// Map a predefined key to a hive path prefix.
fn hive_prefix(hkey: HKey) -> &'static str {
    match hkey {
        HKEY_CLASSES_ROOT => "HKCR",
        HKEY_CURRENT_USER => "HKCU",
        HKEY_LOCAL_MACHINE => "HKLM",
        HKEY_USERS => "HKU",
        HKEY_CURRENT_CONFIG => "HKCC",
        _ => "HKLM", // fallback
    }
}

/// Resolve a registry handle to its stored path, or use the predefined key prefix.
fn resolve_key_path(hkey: HKey) -> String {
    // Check if it's a predefined key
    if hkey >= 0x80000000 && hkey <= 0x80000005 {
        return String::from(hive_prefix(hkey));
    }

    // Look up in handle table
    if let Some(HandleObject::RegistryKey { path }) = handles::get_handle(hkey) {
        path
    } else {
        String::from("HKLM")
    }
}

/// RegOpenKeyExW — open a registry key.
pub fn reg_open_key_ex_w(
    hkey: HKey,
    sub_key: LpcWStr,
    options: DWord,
    sam_desired: DWord,
    result_key: *mut HKey,
) -> DWord {
    let sub = if !sub_key.is_null() {
        unsafe { utf16_to_utf8(sub_key) }
    } else {
        String::new()
    };

    let parent_path = resolve_key_path(hkey);
    let full_path = if sub.is_empty() {
        parent_path.clone()
    } else {
        alloc::format!("{}\\{}", parent_path, sub)
    };

    log::debug!("[advapi32] RegOpenKeyExW: '{}'", full_path);

    // Check if the key exists
    if !registry::key_exists(&full_path) {
        teb_peb::set_last_error(ERROR_FILE_NOT_FOUND);
        return ERROR_FILE_NOT_FOUND;
    }

    let handle = handles::alloc_handle(HandleObject::RegistryKey { path: full_path });
    if !result_key.is_null() {
        unsafe { *result_key = handle; }
    }

    ERROR_SUCCESS
}

/// RegQueryValueExW — query a registry value.
pub fn reg_query_value_ex_w(
    hkey: HKey,
    value_name: LpcWStr,
    reserved: *mut DWord,
    value_type: *mut DWord,
    data: *mut u8,
    data_size: *mut DWord,
) -> DWord {
    let name = if !value_name.is_null() {
        unsafe { utf16_to_utf8(value_name) }
    } else {
        String::new()
    };

    let key_path = resolve_key_path(hkey);
    log::debug!("[advapi32] RegQueryValueExW: key='{}', value='{}'", key_path, name);

    match registry::get_value(&key_path, &name) {
        Some(value) => {
            let (reg_type, bytes) = value.to_bytes();

            if !value_type.is_null() {
                unsafe { *value_type = reg_type; }
            }

            let needed = bytes.len() as DWord;

            if data.is_null() || data_size.is_null() {
                // Just querying the size
                if !data_size.is_null() {
                    unsafe { *data_size = needed; }
                }
                return ERROR_SUCCESS;
            }

            let buf_size = unsafe { *data_size };
            if buf_size < needed {
                unsafe { *data_size = needed; }
                return ERROR_MORE_DATA;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
                *data_size = needed;
            }

            ERROR_SUCCESS
        }
        None => {
            teb_peb::set_last_error(ERROR_FILE_NOT_FOUND);
            ERROR_FILE_NOT_FOUND
        }
    }
}

/// RegSetValueExW — set a registry value.
pub fn reg_set_value_ex_w(
    hkey: HKey,
    value_name: LpcWStr,
    reserved: DWord,
    value_type: DWord,
    data: *const u8,
    data_size: DWord,
) -> DWord {
    let name = if !value_name.is_null() {
        unsafe { utf16_to_utf8(value_name) }
    } else {
        String::new()
    };

    let key_path = resolve_key_path(hkey);
    log::debug!("[advapi32] RegSetValueExW: key='{}', value='{}', type={}", key_path, name, value_type);

    if data.is_null() && data_size > 0 {
        return 87; // ERROR_INVALID_PARAMETER
    }

    let bytes = if !data.is_null() && data_size > 0 {
        unsafe { core::slice::from_raw_parts(data, data_size as usize) }.to_vec()
    } else {
        Vec::new()
    };

    let value = RegValue::from_bytes(value_type, &bytes);
    registry::set_value(&key_path, &name, value);

    ERROR_SUCCESS
}

/// RegCloseKey — close a registry key handle.
pub fn reg_close_key(hkey: HKey) -> DWord {
    log::trace!("[advapi32] RegCloseKey: hkey=0x{:X}", hkey);

    // Don't close predefined keys
    if hkey >= 0x80000000 && hkey <= 0x80000005 {
        return ERROR_SUCCESS;
    }

    handles::close_handle(hkey);
    ERROR_SUCCESS
}

/// RegCreateKeyExW — create or open a registry key.
pub fn reg_create_key_ex_w(
    hkey: HKey,
    sub_key: LpcWStr,
    reserved: DWord,
    class: LpcWStr,
    options: DWord,
    sam_desired: DWord,
    security_attributes: u64,
    result_key: *mut HKey,
    disposition: *mut DWord,
) -> DWord {
    let sub = if !sub_key.is_null() {
        unsafe { utf16_to_utf8(sub_key) }
    } else {
        String::new()
    };

    let parent_path = resolve_key_path(hkey);
    let full_path = if sub.is_empty() {
        parent_path.clone()
    } else {
        alloc::format!("{}\\{}", parent_path, sub)
    };

    log::debug!("[advapi32] RegCreateKeyExW: '{}'", full_path);

    let existed = registry::key_exists(&full_path);
    registry::create_key(&full_path);

    if !disposition.is_null() {
        unsafe {
            *disposition = if existed { REG_OPENED_EXISTING_KEY } else { REG_CREATED_NEW_KEY };
        }
    }

    let handle = handles::alloc_handle(HandleObject::RegistryKey { path: full_path });
    if !result_key.is_null() {
        unsafe { *result_key = handle; }
    }

    ERROR_SUCCESS
}

/// RegDeleteKeyW — delete a registry key.
pub fn reg_delete_key_w(hkey: HKey, sub_key: LpcWStr) -> DWord {
    let sub = if !sub_key.is_null() {
        unsafe { utf16_to_utf8(sub_key) }
    } else {
        String::new()
    };

    let parent_path = resolve_key_path(hkey);
    let full_path = if sub.is_empty() {
        parent_path
    } else {
        alloc::format!("{}\\{}", parent_path, sub)
    };

    log::debug!("[advapi32] RegDeleteKeyW: '{}'", full_path);

    if registry::delete_key(&full_path) {
        ERROR_SUCCESS
    } else {
        ERROR_FILE_NOT_FOUND
    }
}

/// RegEnumKeyExW — enumerate subkeys.
pub fn reg_enum_key_ex_w(
    hkey: HKey,
    index: DWord,
    name: LpWStr,
    name_size: *mut DWord,
    reserved: *mut DWord,
    class: LpWStr,
    class_size: *mut DWord,
    last_write_time: *mut u64,
) -> DWord {
    let key_path = resolve_key_path(hkey);
    log::trace!("[advapi32] RegEnumKeyExW: key='{}', index={}", key_path, index);

    let subkeys = registry::list_subkeys(&key_path);

    if (index as usize) >= subkeys.len() {
        return ERROR_NO_MORE_ITEMS;
    }

    let subkey_name = &subkeys[index as usize];

    if !name.is_null() && !name_size.is_null() {
        let encoded = crate::unicode::utf8_to_utf16(subkey_name);
        let needed = encoded.len() as DWord;
        let buf_size = unsafe { *name_size };

        if buf_size < needed {
            unsafe { *name_size = needed; }
            return ERROR_MORE_DATA;
        }

        unsafe {
            let buf = core::slice::from_raw_parts_mut(name, buf_size as usize);
            buf[..encoded.len()].copy_from_slice(&encoded);
            *name_size = (encoded.len() - 1) as DWord; // exclude null
        }
    }

    if !last_write_time.is_null() {
        unsafe { *last_write_time = 0; }
    }

    ERROR_SUCCESS
}

/// RegEnumValueW — enumerate values under a key.
pub fn reg_enum_value_w(
    hkey: HKey,
    index: DWord,
    name: LpWStr,
    name_size: *mut DWord,
    reserved: *mut DWord,
    value_type: *mut DWord,
    data: *mut u8,
    data_size: *mut DWord,
) -> DWord {
    let key_path = resolve_key_path(hkey);
    log::trace!("[advapi32] RegEnumValueW: key='{}', index={}", key_path, index);

    let values = registry::list_values(&key_path);

    if (index as usize) >= values.len() {
        return ERROR_NO_MORE_ITEMS;
    }

    let (val_name, val) = &values[index as usize];

    // Write value name
    if !name.is_null() && !name_size.is_null() {
        let encoded = crate::unicode::utf8_to_utf16(val_name);
        let buf_size = unsafe { *name_size };
        let needed = encoded.len() as DWord;

        if buf_size < needed {
            unsafe { *name_size = needed; }
            return ERROR_MORE_DATA;
        }

        unsafe {
            let buf = core::slice::from_raw_parts_mut(name, buf_size as usize);
            buf[..encoded.len()].copy_from_slice(&encoded);
            *name_size = (encoded.len() - 1) as DWord;
        }
    }

    // Write value data
    let (reg_type, bytes) = val.to_bytes();

    if !value_type.is_null() {
        unsafe { *value_type = reg_type; }
    }

    if !data.is_null() && !data_size.is_null() {
        let buf_size = unsafe { *data_size };
        if buf_size < bytes.len() as DWord {
            unsafe { *data_size = bytes.len() as DWord; }
            return ERROR_MORE_DATA;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            *data_size = bytes.len() as DWord;
        }
    }

    ERROR_SUCCESS
}

// =============================================================================
// Cryptography
// =============================================================================

/// CryptAcquireContextW — acquire a handle to a crypto provider.
pub fn crypt_acquire_context_w(
    prov: *mut Handle,
    container: LpcWStr,
    provider: LpcWStr,
    prov_type: DWord,
    flags: DWord,
) -> Bool {
    log::debug!("[advapi32] CryptAcquireContextW: type={}, flags=0x{:X}", prov_type, flags);

    if !prov.is_null() {
        // Return a stub provider handle
        unsafe { *prov = 0xCCCC_0001; }
    }
    TRUE
}

/// CryptGenRandom — generate random bytes.
pub fn crypt_gen_random(prov: Handle, len: DWord, buffer: *mut u8) -> Bool {
    if buffer.is_null() || len == 0 {
        return FALSE;
    }

    log::trace!("[advapi32] CryptGenRandom: len={}", len);

    // Use a simple PRNG seeded from tick count for now.
    // In a real implementation, this would use ClaudioOS CSPRNG (RDRAND/RDSEED).
    let mut seed = crate::kernel32::get_tick_count64();
    unsafe {
        for i in 0..len as usize {
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *buffer.add(i) = (seed & 0xFF) as u8;
        }
    }

    TRUE
}
