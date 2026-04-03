//! P/Invoke interop: managed-to-native call marshaling.
//!
//! Implements DllImport functionality for .NET, marshaling managed types
//! to native representations (LPWSTR <-> System.String, struct marshaling)
//! and routing calls to ClaudioOS native functions.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::types::ClrValue;

/// A registered native function that can be called via P/Invoke.
#[derive(Clone)]
pub struct NativeFunction {
    /// DLL name (e.g., "kernel32.dll", "user32.dll").
    pub dll_name: String,
    /// Function name (e.g., "MessageBoxW", "GetTickCount").
    pub function_name: String,
    /// The native function pointer (as u64 for storage).
    pub function_ptr: u64,
    /// Calling convention.
    pub calling_convention: CallingConvention,
    /// Character set for string marshaling.
    pub char_set: CharSet,
}

// Manual Debug impl since fn pointers don't impl Debug
impl core::fmt::Debug for NativeFunction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NativeFunction")
            .field("dll_name", &self.dll_name)
            .field("function_name", &self.function_name)
            .field("function_ptr", &self.function_ptr)
            .field("calling_convention", &self.calling_convention)
            .field("char_set", &self.char_set)
            .finish()
    }
}

/// Calling convention for P/Invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallingConvention {
    /// Windows x64 calling convention (RCX, RDX, R8, R9, stack).
    StdCall,
    /// C calling convention.
    Cdecl,
    /// Same as StdCall on x64.
    WinApi,
    /// Thiscall (used for COM).
    ThisCall,
    /// Fast call.
    FastCall,
}

/// Character set for string marshaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharSet {
    /// ANSI strings (char*).
    Ansi,
    /// Unicode strings (wchar_t* / LPWSTR).
    Unicode,
    /// Auto-detect (Unicode on NT, ANSI on 9x).
    Auto,
}

/// String marshaling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringMarshalMode {
    /// Marshal as LPWSTR (UTF-16LE, null-terminated).
    LpWStr,
    /// Marshal as LPSTR (UTF-8/ANSI, null-terminated).
    LpStr,
    /// Marshal as BSTR (COM string).
    BStr,
}

/// P/Invoke marshaling error.
#[derive(Debug, Clone)]
pub enum InteropError {
    /// DLL not found.
    DllNotFound(String),
    /// Function not found in DLL.
    FunctionNotFound { dll: String, function: String },
    /// Marshaling failed.
    MarshalingError(String),
    /// Calling convention mismatch.
    CallingConventionMismatch,
}

/// Global P/Invoke function registry.
static PINVOKE_REGISTRY: Mutex<Option<PInvokeRegistry>> = Mutex::new(None);

/// Registry of native functions available for P/Invoke.
pub struct PInvokeRegistry {
    /// Functions indexed by "dll_name!function_name".
    functions: BTreeMap<String, NativeFunction>,
}

impl PInvokeRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            functions: BTreeMap::new(),
        }
    }

    /// Register a native function.
    pub fn register(&mut self, func: NativeFunction) {
        let key = alloc::format!("{}!{}", func.dll_name.to_lowercase(), func.function_name);
        log::trace!("[dotnet-interop] Registered P/Invoke: {}", key);
        self.functions.insert(key, func);
    }

    /// Look up a native function.
    pub fn resolve(
        &self,
        dll_name: &str,
        function_name: &str,
    ) -> Result<&NativeFunction, InteropError> {
        let key = alloc::format!("{}!{}", dll_name.to_lowercase(), function_name);
        self.functions.get(&key).ok_or_else(|| InteropError::FunctionNotFound {
            dll: String::from(dll_name),
            function: String::from(function_name),
        })
    }

    /// Get the number of registered functions.
    pub fn function_count(&self) -> usize {
        self.functions.len()
    }
}

/// Marshal a .NET System.String to a null-terminated UTF-16LE buffer (LPWSTR).
///
/// Returns the buffer as a Vec<u8>.
pub fn marshal_string_to_lpwstr(s: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity((s.len() + 1) * 2);
    for c in s.encode_utf16() {
        buf.push(c as u8);
        buf.push((c >> 8) as u8);
    }
    // Null terminator
    buf.push(0);
    buf.push(0);
    buf
}

/// Unmarshal a null-terminated UTF-16LE buffer (LPWSTR) to a Rust String.
pub fn unmarshal_lpwstr(data: &[u8]) -> String {
    let mut chars = Vec::new();
    for chunk in data.chunks(2) {
        if chunk.len() < 2 {
            break;
        }
        let code_unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if code_unit == 0 {
            break;
        }
        if let Some(c) = char::from_u32(code_unit as u32) {
            chars.push(c);
        }
    }
    chars.into_iter().collect()
}

/// Marshal a .NET System.String to a null-terminated UTF-8 buffer (LPSTR).
pub fn marshal_string_to_lpstr(s: &str) -> Vec<u8> {
    let mut buf = Vec::from(s.as_bytes());
    buf.push(0); // null terminator
    buf
}

/// Unmarshal a null-terminated UTF-8 buffer (LPSTR) to a Rust String.
pub fn unmarshal_lpstr(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

/// Marshal a ClrValue to a native representation for P/Invoke.
///
/// Returns the value as a u64 (register-width) for passing in registers.
pub fn marshal_to_native(value: &ClrValue, char_set: CharSet) -> u64 {
    match value {
        ClrValue::Null => 0,
        ClrValue::Bool(b) => if *b { 1 } else { 0 },
        ClrValue::I1(v) => *v as u64,
        ClrValue::U1(v) => *v as u64,
        ClrValue::I2(v) => *v as u64,
        ClrValue::U2(v) => *v as u64,
        ClrValue::I4(v) => *v as u64,
        ClrValue::U4(v) => *v as u64,
        ClrValue::I8(v) => *v as u64,
        ClrValue::U8(v) => *v,
        ClrValue::R4(v) => {
            let bits = v.to_bits();
            bits as u64
        }
        ClrValue::R8(v) => {
            v.to_bits()
        }
        ClrValue::IntPtr(v) => *v as u64,
        ClrValue::UIntPtr(v) => *v as u64,
        ClrValue::String(s) => {
            // Marshal string to a heap buffer and return the pointer.
            // The caller is responsible for freeing this.
            let buf = match char_set {
                CharSet::Unicode | CharSet::Auto => marshal_string_to_lpwstr(s),
                CharSet::Ansi => marshal_string_to_lpstr(s),
            };
            let ptr = buf.as_ptr() as u64;
            core::mem::forget(buf); // Leak intentionally — P/Invoke caller frees
            ptr
        }
        ClrValue::ObjectRef(h) => *h,
        _ => 0,
    }
}

/// Unmarshal a native value back to a ClrValue.
pub fn unmarshal_from_native(native_val: u64, target_type: &str) -> ClrValue {
    match target_type {
        "System.Void" | "void" => ClrValue::Void,
        "System.Boolean" | "bool" => ClrValue::Bool(native_val != 0),
        "System.Int32" | "int" => ClrValue::I4(native_val as i32),
        "System.UInt32" | "uint" => ClrValue::U4(native_val as u32),
        "System.Int64" | "long" => ClrValue::I8(native_val as i64),
        "System.UInt64" | "ulong" => ClrValue::U8(native_val),
        "System.IntPtr" => ClrValue::IntPtr(native_val as isize),
        "System.UIntPtr" => ClrValue::UIntPtr(native_val as usize),
        "System.String" | "string" => {
            // Assume LPWSTR pointer
            if native_val == 0 {
                ClrValue::Null
            } else {
                // In a real implementation, we'd read the string from the pointer.
                ClrValue::String(String::from("<native string>"))
            }
        }
        _ => ClrValue::I4(native_val as i32),
    }
}

/// Invoke a P/Invoke function.
///
/// Marshals arguments, calls the native function, and unmarshals the result.
pub fn invoke_pinvoke(
    dll_name: &str,
    function_name: &str,
    args: &[ClrValue],
    return_type: &str,
) -> Result<ClrValue, InteropError> {
    log::debug!(
        "[dotnet-interop] P/Invoke: {}!{}({} args) -> {}",
        dll_name, function_name, args.len(), return_type
    );

    // Look up the function in our registry
    let _func = with_registry(|reg| reg.resolve(dll_name, function_name).cloned())?;

    // In a full implementation, we would:
    // 1. Marshal each argument to native representation
    // 2. Set up the x64 calling convention (RCX, RDX, R8, R9, stack)
    // 3. Call the function pointer
    // 4. Unmarshal the return value
    //
    // For now, return a default value.

    log::warn!(
        "[dotnet-interop] P/Invoke {}!{} called but not yet executing native code",
        dll_name, function_name
    );

    Ok(unmarshal_from_native(0, return_type))
}

/// Initialize the P/Invoke interop subsystem.
pub fn init() {
    let mut reg = PINVOKE_REGISTRY.lock();
    if reg.is_none() {
        *reg = Some(PInvokeRegistry::new());
        log::info!("[dotnet-interop] P/Invoke interop initialized");
    }
}

/// Access the P/Invoke registry.
pub fn with_registry<F, R>(f: F) -> R
where
    F: FnOnce(&PInvokeRegistry) -> R,
{
    let reg = PINVOKE_REGISTRY.lock();
    f(reg.as_ref().expect("P/Invoke registry not initialized"))
}

/// Access the P/Invoke registry mutably.
pub fn with_registry_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut PInvokeRegistry) -> R,
{
    let mut reg = PINVOKE_REGISTRY.lock();
    f(reg.as_mut().expect("P/Invoke registry not initialized"))
}

/// Register a native function for P/Invoke access.
pub fn register_native_function(
    dll_name: &str,
    function_name: &str,
    function_ptr: u64,
    convention: CallingConvention,
    char_set: CharSet,
) {
    with_registry_mut(|reg| {
        reg.register(NativeFunction {
            dll_name: String::from(dll_name),
            function_name: String::from(function_name),
            function_ptr,
            calling_convention: convention,
            char_set,
        });
    });
}
