//! Shell32.dll API implementation — Shell basics.
//!
//! Maps SHGetFolderPathW, ShellExecuteW, SHGetFileInfoW to
//! ClaudioOS VFS paths under /var/claudio/users/{user}/.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::teb_peb;

// =============================================================================
// CSIDL constants (folder IDs)
// =============================================================================

pub const CSIDL_DESKTOP: i32 = 0x0000;
pub const CSIDL_INTERNET: i32 = 0x0001;
pub const CSIDL_PROGRAMS: i32 = 0x0002;
pub const CSIDL_CONTROLS: i32 = 0x0003;
pub const CSIDL_PRINTERS: i32 = 0x0004;
pub const CSIDL_PERSONAL: i32 = 0x0005;   // My Documents
pub const CSIDL_FAVORITES: i32 = 0x0006;
pub const CSIDL_STARTUP: i32 = 0x0007;
pub const CSIDL_RECENT: i32 = 0x0008;
pub const CSIDL_SENDTO: i32 = 0x0009;
pub const CSIDL_BITBUCKET: i32 = 0x000A;  // Recycle Bin
pub const CSIDL_STARTMENU: i32 = 0x000B;
pub const CSIDL_MYDOCUMENTS: i32 = 0x000C;
pub const CSIDL_MYMUSIC: i32 = 0x000D;
pub const CSIDL_MYVIDEO: i32 = 0x000E;
pub const CSIDL_DESKTOPDIRECTORY: i32 = 0x0010;
pub const CSIDL_DRIVES: i32 = 0x0011;     // My Computer
pub const CSIDL_NETWORK: i32 = 0x0012;
pub const CSIDL_NETHOOD: i32 = 0x0013;
pub const CSIDL_FONTS: i32 = 0x0014;
pub const CSIDL_TEMPLATES: i32 = 0x0015;
pub const CSIDL_COMMON_STARTMENU: i32 = 0x0016;
pub const CSIDL_COMMON_PROGRAMS: i32 = 0x0017;
pub const CSIDL_COMMON_STARTUP: i32 = 0x0018;
pub const CSIDL_COMMON_DESKTOPDIRECTORY: i32 = 0x0019;
pub const CSIDL_APPDATA: i32 = 0x001A;
pub const CSIDL_PRINTHOOD: i32 = 0x001B;
pub const CSIDL_LOCAL_APPDATA: i32 = 0x001C;
pub const CSIDL_COMMON_FAVORITES: i32 = 0x001F;
pub const CSIDL_INTERNET_CACHE: i32 = 0x0020;
pub const CSIDL_COOKIES: i32 = 0x0021;
pub const CSIDL_HISTORY: i32 = 0x0022;
pub const CSIDL_COMMON_APPDATA: i32 = 0x0023;
pub const CSIDL_WINDOWS: i32 = 0x0024;
pub const CSIDL_SYSTEM: i32 = 0x0025;
pub const CSIDL_PROGRAM_FILES: i32 = 0x0026;
pub const CSIDL_MYPICTURES: i32 = 0x0027;
pub const CSIDL_PROFILE: i32 = 0x0028;
pub const CSIDL_SYSTEMX86: i32 = 0x0029;
pub const CSIDL_PROGRAM_FILESX86: i32 = 0x002A;
pub const CSIDL_PROGRAM_FILES_COMMON: i32 = 0x002B;
pub const CSIDL_COMMON_TEMPLATES: i32 = 0x002D;
pub const CSIDL_COMMON_DOCUMENTS: i32 = 0x002E;
pub const CSIDL_COMMON_ADMINTOOLS: i32 = 0x002F;
pub const CSIDL_ADMINTOOLS: i32 = 0x0030;
pub const CSIDL_COMMON_MUSIC: i32 = 0x0035;
pub const CSIDL_COMMON_PICTURES: i32 = 0x0036;
pub const CSIDL_COMMON_VIDEO: i32 = 0x0037;
pub const CSIDL_RESOURCES: i32 = 0x0038;
pub const CSIDL_CDBURN_AREA: i32 = 0x003B;

/// CSIDL flag: create the folder if it doesn't exist.
pub const CSIDL_FLAG_CREATE: i32 = 0x8000;

// =============================================================================
// SHFILEINFO
// =============================================================================

/// SHFILEINFOW structure.
#[repr(C)]
pub struct ShFileInfoW {
    pub h_icon: u64,
    pub i_icon: i32,
    pub dw_attributes: DWord,
    pub sz_display_name: [u16; 260],
    pub sz_type_name: [u16; 80],
}

/// SHGFI flags for SHGetFileInfoW.
pub const SHGFI_ICON: DWord = 0x000000100;
pub const SHGFI_DISPLAYNAME: DWord = 0x000000200;
pub const SHGFI_TYPENAME: DWord = 0x000000400;
pub const SHGFI_ATTRIBUTES: DWord = 0x000000800;
pub const SHGFI_LARGEICON: DWord = 0x000000000;
pub const SHGFI_SMALLICON: DWord = 0x000000001;
pub const SHGFI_USEFILEATTRIBUTES: DWord = 0x000000010;

// =============================================================================
// HRESULT constants
// =============================================================================

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_INVALIDARG: i32 = 0x80070057_u32 as i32;
pub const E_FAIL: i32 = 0x80004005_u32 as i32;

/// MAX_PATH
const MAX_PATH: usize = 260;

// =============================================================================
// Internal state
// =============================================================================

/// Base user path in the VFS.
const USER_BASE: &str = "C:\\Users\\ClaudioOS";

/// Current username.
static USERNAME: Mutex<Option<String>> = Mutex::new(None);

fn get_username() -> String {
    let name = USERNAME.lock();
    name.clone().unwrap_or_else(|| String::from("ClaudioOS"))
}

/// Map a CSIDL value to a VFS path.
fn csidl_to_path(csidl: i32) -> Option<String> {
    let user = get_username();
    let base = alloc::format!("C:\\Users\\{}", user);

    let path = match csidl & 0x7FFF {
        // Clear CREATE flag
        x if x == CSIDL_DESKTOP || x == CSIDL_DESKTOPDIRECTORY => {
            alloc::format!("{}\\Desktop", base)
        }
        x if x == CSIDL_PERSONAL || x == CSIDL_MYDOCUMENTS => {
            alloc::format!("{}\\Documents", base)
        }
        x if x == CSIDL_MYPICTURES => {
            alloc::format!("{}\\Pictures", base)
        }
        x if x == CSIDL_MYMUSIC || x == CSIDL_COMMON_MUSIC => {
            alloc::format!("{}\\Music", base)
        }
        x if x == CSIDL_MYVIDEO || x == CSIDL_COMMON_VIDEO => {
            alloc::format!("{}\\Videos", base)
        }
        x if x == CSIDL_FAVORITES || x == CSIDL_COMMON_FAVORITES => {
            alloc::format!("{}\\Favorites", base)
        }
        x if x == CSIDL_APPDATA => {
            alloc::format!("{}\\AppData\\Roaming", base)
        }
        x if x == CSIDL_LOCAL_APPDATA => {
            alloc::format!("{}\\AppData\\Local", base)
        }
        x if x == CSIDL_COMMON_APPDATA => {
            String::from("C:\\ProgramData")
        }
        x if x == CSIDL_STARTMENU || x == CSIDL_COMMON_STARTMENU => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu", base)
        }
        x if x == CSIDL_PROGRAMS || x == CSIDL_COMMON_PROGRAMS => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs", base)
        }
        x if x == CSIDL_STARTUP || x == CSIDL_COMMON_STARTUP => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup", base)
        }
        x if x == CSIDL_RECENT => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Recent", base)
        }
        x if x == CSIDL_TEMPLATES || x == CSIDL_COMMON_TEMPLATES => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Templates", base)
        }
        x if x == CSIDL_INTERNET_CACHE => {
            alloc::format!("{}\\AppData\\Local\\Microsoft\\Windows\\INetCache", base)
        }
        x if x == CSIDL_COOKIES => {
            alloc::format!("{}\\AppData\\Local\\Microsoft\\Windows\\INetCookies", base)
        }
        x if x == CSIDL_HISTORY => {
            alloc::format!("{}\\AppData\\Local\\Microsoft\\Windows\\History", base)
        }
        x if x == CSIDL_PROFILE => {
            base.clone()
        }
        x if x == CSIDL_FONTS => {
            String::from("C:\\Windows\\Fonts")
        }
        x if x == CSIDL_WINDOWS => {
            String::from("C:\\Windows")
        }
        x if x == CSIDL_SYSTEM => {
            String::from("C:\\Windows\\System32")
        }
        x if x == CSIDL_SYSTEMX86 => {
            String::from("C:\\Windows\\SysWOW64")
        }
        x if x == CSIDL_PROGRAM_FILES => {
            String::from("C:\\Program Files")
        }
        x if x == CSIDL_PROGRAM_FILESX86 => {
            String::from("C:\\Program Files (x86)")
        }
        x if x == CSIDL_PROGRAM_FILES_COMMON => {
            String::from("C:\\Program Files\\Common Files")
        }
        x if x == CSIDL_COMMON_DOCUMENTS => {
            String::from("C:\\Users\\Public\\Documents")
        }
        x if x == CSIDL_COMMON_DESKTOPDIRECTORY => {
            String::from("C:\\Users\\Public\\Desktop")
        }
        x if x == CSIDL_ADMINTOOLS || x == CSIDL_COMMON_ADMINTOOLS => {
            alloc::format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Administrative Tools", base)
        }
        x if x == CSIDL_CDBURN_AREA => {
            alloc::format!("{}\\AppData\\Local\\Microsoft\\Windows\\Burn\\Burn", base)
        }
        _ => return None,
    };

    Some(path)
}

// =============================================================================
// Public API
// =============================================================================

/// SHGetFolderPathW — get the path of a special folder.
pub fn sh_get_folder_path_w(
    hwnd: HWnd,
    csidl: i32,
    token: Handle,
    flags: DWord,
    path: LpWStr,
) -> i32 {
    if path.is_null() {
        return E_INVALIDARG;
    }

    let folder_path = match csidl_to_path(csidl) {
        Some(p) => p,
        None => {
            log::warn!("[shell32] SHGetFolderPathW: unknown CSIDL 0x{:X}", csidl);
            return E_INVALIDARG;
        }
    };

    log::debug!("[shell32] SHGetFolderPathW: CSIDL=0x{:X} -> '{}'", csidl & 0x7FFF, folder_path);

    let utf16 = utf8_to_utf16(&folder_path);
    let copy_len = utf16.len().min(MAX_PATH);

    unsafe {
        core::ptr::copy_nonoverlapping(utf16.as_ptr(), path, copy_len);
        // Ensure null termination
        if copy_len < MAX_PATH {
            *path.add(copy_len) = 0;
        } else {
            *path.add(MAX_PATH - 1) = 0;
        }
    }

    S_OK
}

/// SHGetSpecialFolderPathW — convenience wrapper around SHGetFolderPathW.
pub fn sh_get_special_folder_path_w(
    hwnd: HWnd,
    path: LpWStr,
    csidl: i32,
    create: Bool,
) -> Bool {
    let csidl_with_create = if create != 0 { csidl | CSIDL_FLAG_CREATE } else { csidl };
    let hr = sh_get_folder_path_w(hwnd, csidl_with_create, 0, 0, path);
    if hr == S_OK { TRUE } else { FALSE }
}

/// ShellExecuteW — launch a program or open a document.
pub fn shell_execute_w(
    hwnd: HWnd,
    operation: LpcWStr,
    file: LpcWStr,
    parameters: LpcWStr,
    directory: LpcWStr,
    show_cmd: i32,
) -> u64 {
    let op = if !operation.is_null() {
        unsafe { utf16_to_utf8(operation) }
    } else {
        String::from("open")
    };

    let file_str = if !file.is_null() {
        unsafe { utf16_to_utf8(file) }
    } else {
        String::new()
    };

    let params = if !parameters.is_null() {
        unsafe { utf16_to_utf8(parameters) }
    } else {
        String::new()
    };

    let dir = if !directory.is_null() {
        unsafe { utf16_to_utf8(directory) }
    } else {
        String::new()
    };

    log::info!(
        "[shell32] ShellExecuteW: op='{}' file='{}' params='{}' dir='{}' show={}",
        op, file_str, params, dir, show_cmd
    );

    // In ClaudioOS, "open" maps to our process launcher.
    // For URLs, we'd use the wraith browser.
    // Return value > 32 means success on Windows.
    42 // SE_ERR_NOASSOC is 31, so 42 > 32 = success
}

/// SHGetFileInfoW — get file information (icon, type name, etc.).
pub fn sh_get_file_info_w(
    path: LpcWStr,
    file_attributes: DWord,
    file_info: *mut ShFileInfoW,
    file_info_size: u32,
    flags: DWord,
) -> u64 {
    if file_info.is_null() || file_info_size < core::mem::size_of::<ShFileInfoW>() as u32 {
        return 0;
    }

    let path_str = if !path.is_null() {
        unsafe { utf16_to_utf8(path) }
    } else {
        String::new()
    };

    log::trace!("[shell32] SHGetFileInfoW: '{}' flags=0x{:X}", path_str, flags);

    unsafe {
        (*file_info).h_icon = 0;
        (*file_info).i_icon = 0;
        (*file_info).dw_attributes = 0;
        (*file_info).sz_display_name = [0u16; 260];
        (*file_info).sz_type_name = [0u16; 80];
    }

    // Fill display name from the filename
    if flags & SHGFI_DISPLAYNAME != 0 {
        let display = path_str.rsplit('\\').next().unwrap_or(&path_str);
        let utf16 = utf8_to_utf16(display);
        let copy_len = utf16.len().min(259);
        unsafe {
            for (i, &ch) in utf16.iter().take(copy_len).enumerate() {
                (*file_info).sz_display_name[i] = ch;
            }
        }
    }

    // Fill type name
    if flags & SHGFI_TYPENAME != 0 {
        let ext = path_str.rsplit('.').next().unwrap_or("");
        let type_name = match ext.to_ascii_lowercase().as_str() {
            "exe" => "Application",
            "dll" => "Application Extension",
            "txt" => "Text Document",
            "bmp" | "png" | "jpg" | "jpeg" => "Image",
            "wav" | "mp3" => "Audio",
            "avi" | "mp4" => "Video",
            _ => "File",
        };
        let utf16 = utf8_to_utf16(type_name);
        let copy_len = utf16.len().min(79);
        unsafe {
            for (i, &ch) in utf16.iter().take(copy_len).enumerate() {
                (*file_info).sz_type_name[i] = ch;
            }
        }
    }

    // Return nonzero for success
    1
}

/// Set the current username for path resolution.
pub fn set_username(name: &str) {
    *USERNAME.lock() = Some(String::from(name));
    log::debug!("[shell32] Username set to '{}'", name);
}

/// SHGetKnownFolderPath — newer API, same concept as SHGetFolderPathW.
pub fn sh_get_known_folder_path(
    rfid: *const Guid,
    flags: DWord,
    token: Handle,
    path: *mut LpWStr,
) -> i32 {
    if rfid.is_null() || path.is_null() {
        return E_INVALIDARG;
    }

    let id = unsafe { &*rfid };

    // Map common known folder GUIDs to CSIDL values
    let csidl = match (id.data1, id.data2, id.data3) {
        (0xFDD39AD0, 0x238F, 0x46AF) => CSIDL_PERSONAL,    // Documents
        (0xB4BFCC3A, 0xDB2C, 0x424C) => CSIDL_DESKTOP,     // Desktop
        (0x4BD8D571, 0x6D19, 0x48D3) => CSIDL_MYMUSIC,     // Music
        (0x33E28130, 0x4E1E, 0x4676) => CSIDL_MYPICTURES,  // Pictures
        (0x18989B1D, 0x99B5, 0x455B) => CSIDL_MYVIDEO,     // Videos
        (0x3EB685DB, 0x65F9, 0x4CF6) => CSIDL_APPDATA,     // RoamingAppData
        (0xF1B32785, 0x6FBA, 0x4FCF) => CSIDL_LOCAL_APPDATA, // LocalAppData
        (0x62AB5D82, 0xFDC1, 0x4DC3) => CSIDL_COMMON_APPDATA, // ProgramData
        (0x905E63B6, 0xC1BF, 0x494E) => CSIDL_PROGRAM_FILES, // ProgramFiles
        (0x7C5A40EF, 0xA0FB, 0x4BFC) => CSIDL_PROGRAM_FILESX86, // ProgramFilesX86
        (0x5E6C858F, 0x0E22, 0x4760) => CSIDL_PROFILE,     // Profile
        _ => {
            log::warn!(
                "[shell32] SHGetKnownFolderPath: unknown GUID {{{:08X}-{:04X}-{:04X}}}",
                id.data1, id.data2, id.data3
            );
            return E_INVALIDARG;
        }
    };

    let folder_path = match csidl_to_path(csidl) {
        Some(p) => p,
        None => return E_FAIL,
    };

    log::debug!("[shell32] SHGetKnownFolderPath -> '{}'", folder_path);

    // Allocate a UTF-16 string on the heap (caller frees with CoTaskMemFree)
    let utf16 = utf8_to_utf16(&folder_path);
    let buf = alloc::vec::Vec::from(utf16);
    let ptr = buf.as_ptr() as LpWStr;
    core::mem::forget(buf); // Leak intentionally — caller must free

    unsafe { *path = ptr; }
    S_OK
}

/// PathCombineW — combine two path components.
pub fn path_combine_w(
    dest: LpWStr,
    dir: LpcWStr,
    file: LpcWStr,
) -> LpWStr {
    if dest.is_null() { return core::ptr::null_mut(); }

    let dir_str = if !dir.is_null() { unsafe { utf16_to_utf8(dir) } } else { String::new() };
    let file_str = if !file.is_null() { unsafe { utf16_to_utf8(file) } } else { String::new() };

    let combined = if dir_str.is_empty() {
        file_str
    } else if file_str.is_empty() {
        dir_str
    } else {
        let sep = if dir_str.ends_with('\\') || dir_str.ends_with('/') { "" } else { "\\" };
        alloc::format!("{}{}{}", dir_str, sep, file_str)
    };

    let utf16 = utf8_to_utf16(&combined);
    let copy_len = utf16.len().min(MAX_PATH);
    unsafe {
        core::ptr::copy_nonoverlapping(utf16.as_ptr(), dest, copy_len);
        if copy_len < MAX_PATH { *dest.add(copy_len) = 0; }
    }

    dest
}
