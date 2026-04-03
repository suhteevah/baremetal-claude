//! Base Class Library (BCL) stubs for .NET.
//!
//! Implements essential BCL types: Console, String, Int32, Convert, Math,
//! List<T>, Dictionary<K,V>, File I/O. These are the minimum needed to run
//! basic C# console applications.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::types::ClrValue;

/// Output buffer for Console.WriteLine (since we have no real stdout in bare metal,
/// we collect output and can retrieve it).
static CONSOLE_OUTPUT: spin::Mutex<Option<Vec<String>>> = spin::Mutex::new(None);

/// Input buffer for Console.ReadLine.
static CONSOLE_INPUT: spin::Mutex<Option<Vec<String>>> = spin::Mutex::new(None);

/// Initialize BCL subsystem.
pub fn init() {
    *CONSOLE_OUTPUT.lock() = Some(Vec::new());
    *CONSOLE_INPUT.lock() = Some(Vec::new());
    log::info!("[dotnet-bcl] Base Class Library initialized");
}

/// Push a line to the console input buffer (for Console.ReadLine).
pub fn push_console_input(line: String) {
    if let Some(ref mut buf) = *CONSOLE_INPUT.lock() {
        buf.push(line);
    }
}

/// Get all console output lines.
pub fn get_console_output() -> Vec<String> {
    CONSOLE_OUTPUT.lock().as_ref().map(|v| v.clone()).unwrap_or_default()
}

/// Clear console output buffer.
pub fn clear_console_output() {
    if let Some(ref mut buf) = *CONSOLE_OUTPUT.lock() {
        buf.clear();
    }
}

// ─── Console ────────────────────────────────────────────────────────────────

/// Console.WriteLine(string)
pub fn console_write_line(args: &[ClrValue]) -> ClrValue {
    let text = if args.is_empty() {
        String::new()
    } else {
        clr_value_to_string(&args[0])
    };
    log::info!("[dotnet-console] {}", text);
    if let Some(ref mut buf) = *CONSOLE_OUTPUT.lock() {
        buf.push(text);
    }
    ClrValue::Void
}

/// Console.Write(string)
pub fn console_write(args: &[ClrValue]) -> ClrValue {
    let text = if args.is_empty() {
        String::new()
    } else {
        clr_value_to_string(&args[0])
    };
    log::info!("[dotnet-console] {}", text);
    if let Some(ref mut buf) = *CONSOLE_OUTPUT.lock() {
        buf.push(text);
    }
    ClrValue::Void
}

/// Console.ReadLine() -> string
pub fn console_read_line(_args: &[ClrValue]) -> ClrValue {
    let line = {
        let mut input = CONSOLE_INPUT.lock();
        if let Some(ref mut buf) = *input {
            if !buf.is_empty() {
                buf.remove(0)
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    };
    ClrValue::String(line)
}

// ─── String methods ─────────────────────────────────────────────────────────

/// String.Concat(string, string) -> string
pub fn string_concat(args: &[ClrValue]) -> ClrValue {
    let mut result = String::new();
    for arg in args {
        result.push_str(&clr_value_to_string(arg));
    }
    ClrValue::String(result)
}

/// String.Format(string, params object[]) -> string
///
/// Supports {0}, {1}, etc. placeholders.
pub fn string_format(args: &[ClrValue]) -> ClrValue {
    if args.is_empty() {
        return ClrValue::String(String::new());
    }
    let fmt = clr_value_to_string(&args[0]);
    let mut result = fmt.clone();

    for (i, arg) in args[1..].iter().enumerate() {
        let placeholder = format!("{{{}}}", i);
        let replacement = clr_value_to_string(arg);
        result = result.replace(&placeholder, &replacement);
    }

    ClrValue::String(result)
}

/// String.Substring(int startIndex) -> string
/// String.Substring(int startIndex, int length) -> string
pub fn string_substring(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let start = args.first().and_then(|v| v.to_i32()).unwrap_or(0) as usize;

    if args.len() >= 2 {
        let length = args[1].to_i32().unwrap_or(0) as usize;
        let end = (start + length).min(s.len());
        ClrValue::String(s.get(start..end).unwrap_or("").into())
    } else {
        ClrValue::String(s.get(start..).unwrap_or("").into())
    }
}

/// String.Split(char separator) -> string[]
pub fn string_split(this: &ClrValue, args: &[ClrValue]) -> Vec<ClrValue> {
    let s = clr_value_to_string(this);
    let sep = match args.first() {
        Some(ClrValue::Char(c)) => *c,
        Some(ClrValue::String(s)) if !s.is_empty() => s.chars().next().unwrap_or(','),
        _ => ',',
    };

    s.split(sep)
        .map(|part| ClrValue::String(String::from(part)))
        .collect()
}

/// String.Trim() -> string
pub fn string_trim(this: &ClrValue, _args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    ClrValue::String(s.trim().into())
}

/// String.Length -> int
pub fn string_length(this: &ClrValue) -> ClrValue {
    let s = clr_value_to_string(this);
    ClrValue::I4(s.len() as i32)
}

/// String.Contains(string value) -> bool
pub fn string_contains(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let search = args.first().map(clr_value_to_string).unwrap_or_default();
    ClrValue::Bool(s.contains(&search))
}

/// String.Replace(string old, string new) -> string
pub fn string_replace(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let old = args.first().map(clr_value_to_string).unwrap_or_default();
    let new = args.get(1).map(clr_value_to_string).unwrap_or_default();
    ClrValue::String(s.replace(&old, &new))
}

/// String.ToUpper() -> string
pub fn string_to_upper(this: &ClrValue, _args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    ClrValue::String(s.to_uppercase())
}

/// String.ToLower() -> string
pub fn string_to_lower(this: &ClrValue, _args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    ClrValue::String(s.to_lowercase())
}

/// String.StartsWith(string) -> bool
pub fn string_starts_with(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let prefix = args.first().map(clr_value_to_string).unwrap_or_default();
    ClrValue::Bool(s.starts_with(&prefix))
}

/// String.EndsWith(string) -> bool
pub fn string_ends_with(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let suffix = args.first().map(clr_value_to_string).unwrap_or_default();
    ClrValue::Bool(s.ends_with(&suffix))
}

/// String.IndexOf(string) -> int
pub fn string_index_of(this: &ClrValue, args: &[ClrValue]) -> ClrValue {
    let s = clr_value_to_string(this);
    let search = args.first().map(clr_value_to_string).unwrap_or_default();
    let idx = s.find(&search).map(|i| i as i32).unwrap_or(-1);
    ClrValue::I4(idx)
}

// ─── Int32 / Convert ────────────────────────────────────────────────────────

/// Int32.Parse(string) -> int
pub fn int32_parse(args: &[ClrValue]) -> ClrValue {
    let s = args.first().map(clr_value_to_string).unwrap_or_default();
    let val: i32 = s.trim().parse().unwrap_or(0);
    ClrValue::I4(val)
}

/// Int32.TryParse(string, out int) -> bool
pub fn int32_try_parse(args: &[ClrValue]) -> ClrValue {
    let s = args.first().map(clr_value_to_string).unwrap_or_default();
    ClrValue::Bool(s.trim().parse::<i32>().is_ok())
}

/// Int32.ToString() -> string
pub fn int32_to_string(this: &ClrValue, _args: &[ClrValue]) -> ClrValue {
    ClrValue::String(clr_value_to_string(this))
}

/// Convert.ToInt32(object) -> int
pub fn convert_to_int32(args: &[ClrValue]) -> ClrValue {
    let val = args.first().unwrap_or(&ClrValue::I4(0));
    ClrValue::I4(val.to_i32().unwrap_or(0))
}

/// Convert.ToString(object) -> string
pub fn convert_to_string(args: &[ClrValue]) -> ClrValue {
    let val = args.first().unwrap_or(&ClrValue::Null);
    ClrValue::String(clr_value_to_string(val))
}

/// Convert.ToDouble(object) -> double
pub fn convert_to_double(args: &[ClrValue]) -> ClrValue {
    let val = args.first().unwrap_or(&ClrValue::R8(0.0));
    ClrValue::R8(val.to_f64().unwrap_or(0.0))
}

/// Convert.ToBoolean(object) -> bool
pub fn convert_to_boolean(args: &[ClrValue]) -> ClrValue {
    let val = args.first().unwrap_or(&ClrValue::Bool(false));
    ClrValue::Bool(val.to_bool())
}

// ─── Math ───────────────────────────────────────────────────────────────────

/// Math.Abs(int) -> int / Math.Abs(double) -> double
pub fn math_abs(args: &[ClrValue]) -> ClrValue {
    match args.first() {
        Some(ClrValue::I4(v)) => ClrValue::I4(v.wrapping_abs()),
        Some(ClrValue::I8(v)) => ClrValue::I8(v.wrapping_abs()),
        Some(ClrValue::R4(v)) => ClrValue::R4(if *v < 0.0 { -v } else { *v }),
        Some(ClrValue::R8(v)) => ClrValue::R8(if *v < 0.0 { -v } else { *v }),
        _ => ClrValue::I4(0),
    }
}

/// Math.Max(int, int) -> int / Math.Max(double, double) -> double
pub fn math_max(args: &[ClrValue]) -> ClrValue {
    if args.len() < 2 {
        return args.first().cloned().unwrap_or(ClrValue::I4(0));
    }
    match (&args[0], &args[1]) {
        (ClrValue::I4(a), ClrValue::I4(b)) => ClrValue::I4(if *a > *b { *a } else { *b }),
        (ClrValue::I8(a), ClrValue::I8(b)) => ClrValue::I8(if *a > *b { *a } else { *b }),
        (ClrValue::R8(a), ClrValue::R8(b)) => ClrValue::R8(if *a > *b { *a } else { *b }),
        _ => {
            let a = args[0].to_f64().unwrap_or(0.0);
            let b = args[1].to_f64().unwrap_or(0.0);
            ClrValue::R8(if a > b { a } else { b })
        }
    }
}

/// Math.Min(int, int) -> int / Math.Min(double, double) -> double
pub fn math_min(args: &[ClrValue]) -> ClrValue {
    if args.len() < 2 {
        return args.first().cloned().unwrap_or(ClrValue::I4(0));
    }
    match (&args[0], &args[1]) {
        (ClrValue::I4(a), ClrValue::I4(b)) => ClrValue::I4(if *a < *b { *a } else { *b }),
        (ClrValue::I8(a), ClrValue::I8(b)) => ClrValue::I8(if *a < *b { *a } else { *b }),
        (ClrValue::R8(a), ClrValue::R8(b)) => ClrValue::R8(if *a < *b { *a } else { *b }),
        _ => {
            let a = args[0].to_f64().unwrap_or(0.0);
            let b = args[1].to_f64().unwrap_or(0.0);
            ClrValue::R8(if a < b { a } else { b })
        }
    }
}

/// Math.Floor(double) -> double
pub fn math_floor(args: &[ClrValue]) -> ClrValue {
    let v = args.first().and_then(|v| v.to_f64()).unwrap_or(0.0);
    // Manual floor: truncate toward negative infinity
    let trunc = v as i64 as f64;
    let result = if v < trunc { trunc - 1.0 } else { trunc };
    ClrValue::R8(result)
}

/// Math.Ceiling(double) -> double
pub fn math_ceiling(args: &[ClrValue]) -> ClrValue {
    let v = args.first().and_then(|v| v.to_f64()).unwrap_or(0.0);
    let trunc = v as i64 as f64;
    let result = if v > trunc { trunc + 1.0 } else { trunc };
    ClrValue::R8(result)
}

/// Math.Round(double) -> double
pub fn math_round(args: &[ClrValue]) -> ClrValue {
    let v = args.first().and_then(|v| v.to_f64()).unwrap_or(0.0);
    // Banker's rounding (round half to even)
    let floor = {
        let trunc = v as i64 as f64;
        if v < trunc { trunc - 1.0 } else { trunc }
    };
    let frac = v - floor;
    let result = if frac > 0.5 {
        floor + 1.0
    } else if frac < 0.5 {
        floor
    } else {
        // Exactly 0.5 — round to even
        if (floor as i64) % 2 == 0 { floor } else { floor + 1.0 }
    };
    ClrValue::R8(result)
}

/// Math.Sqrt(double) -> double
pub fn math_sqrt(args: &[ClrValue]) -> ClrValue {
    let v = args.first().and_then(|v| v.to_f64()).unwrap_or(0.0);
    // Newton's method for sqrt
    if v < 0.0 {
        return ClrValue::R8(f64::NAN);
    }
    if v == 0.0 {
        return ClrValue::R8(0.0);
    }
    let mut guess = v / 2.0;
    for _ in 0..64 {
        let new_guess = (guess + v / guess) / 2.0;
        if (new_guess - guess).abs() < 1e-15 {
            break;
        }
        guess = new_guess;
    }
    ClrValue::R8(guess)
}

/// Math.Pow(double, double) -> double
pub fn math_pow(args: &[ClrValue]) -> ClrValue {
    let base = args.first().and_then(|v| v.to_f64()).unwrap_or(0.0);
    let exp = args.get(1).and_then(|v| v.to_f64()).unwrap_or(0.0);

    // Handle integer exponents
    if exp == (exp as i64) as f64 {
        let n = exp as i64;
        let mut result = 1.0;
        let mut b = base;
        let mut e = if n < 0 { -n } else { n } as u64;
        while e > 0 {
            if e & 1 == 1 {
                result *= b;
            }
            b *= b;
            e >>= 1;
        }
        if n < 0 {
            result = 1.0 / result;
        }
        return ClrValue::R8(result);
    }

    // Fallback for non-integer exponents (approximate)
    ClrValue::R8(0.0) // TODO: implement exp(y * ln(x))
}

// ─── List<T> (simplified) ───────────────────────────────────────────────────

/// A simple List<T> backed by a Vec of ClrValue.
#[derive(Debug, Clone)]
pub struct ManagedList {
    pub elements: Vec<ClrValue>,
    pub element_type: String,
}

impl ManagedList {
    pub fn new(element_type: String) -> Self {
        Self {
            elements: Vec::new(),
            element_type,
        }
    }

    pub fn add(&mut self, value: ClrValue) {
        self.elements.push(value);
    }

    pub fn get(&self, index: usize) -> Option<&ClrValue> {
        self.elements.get(index)
    }

    pub fn set(&mut self, index: usize, value: ClrValue) -> bool {
        if index < self.elements.len() {
            self.elements[index] = value;
            true
        } else {
            false
        }
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        if index < self.elements.len() {
            self.elements.remove(index);
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.elements.len()
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    pub fn contains(&self, value: &ClrValue) -> bool {
        self.elements.iter().any(|v| clr_values_equal(v, value))
    }
}

// ─── Dictionary<K,V> (simplified) ──────────────────────────────────────────

/// A simple Dictionary<string, ClrValue>.
#[derive(Debug, Clone)]
pub struct ManagedDictionary {
    pub entries: BTreeMap<String, ClrValue>,
    pub key_type: String,
    pub value_type: String,
}

impl ManagedDictionary {
    pub fn new(key_type: String, value_type: String) -> Self {
        Self {
            entries: BTreeMap::new(),
            key_type,
            value_type,
        }
    }

    pub fn add(&mut self, key: String, value: ClrValue) {
        self.entries.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&ClrValue> {
        self.entries.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

// ─── File I/O stubs ─────────────────────────────────────────────────────────

/// Simulated file system for File.ReadAllText / File.WriteAllText.
static FILE_SYSTEM: spin::Mutex<Option<BTreeMap<String, String>>> = spin::Mutex::new(None);

/// Initialize the simulated file system.
pub fn init_filesystem() {
    *FILE_SYSTEM.lock() = Some(BTreeMap::new());
}

/// File.ReadAllText(string path) -> string
pub fn file_read_all_text(args: &[ClrValue]) -> ClrValue {
    let path = args.first().map(clr_value_to_string).unwrap_or_default();
    let content = FILE_SYSTEM
        .lock()
        .as_ref()
        .and_then(|fs| fs.get(&path).cloned())
        .unwrap_or_default();
    ClrValue::String(content)
}

/// File.WriteAllText(string path, string contents) -> void
pub fn file_write_all_text(args: &[ClrValue]) -> ClrValue {
    let path = args.first().map(clr_value_to_string).unwrap_or_default();
    let contents = args.get(1).map(clr_value_to_string).unwrap_or_default();
    if let Some(ref mut fs) = *FILE_SYSTEM.lock() {
        fs.insert(path, contents);
    }
    ClrValue::Void
}

/// File.Exists(string path) -> bool
pub fn file_exists(args: &[ClrValue]) -> ClrValue {
    let path = args.first().map(clr_value_to_string).unwrap_or_default();
    let exists = FILE_SYSTEM
        .lock()
        .as_ref()
        .map(|fs| fs.contains_key(&path))
        .unwrap_or(false);
    ClrValue::Bool(exists)
}

/// File.Delete(string path) -> void
pub fn file_delete(args: &[ClrValue]) -> ClrValue {
    let path = args.first().map(clr_value_to_string).unwrap_or_default();
    if let Some(ref mut fs) = *FILE_SYSTEM.lock() {
        fs.remove(&path);
    }
    ClrValue::Void
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Convert a ClrValue to its string representation.
pub fn clr_value_to_string(val: &ClrValue) -> String {
    match val {
        ClrValue::Null => String::from(""),
        ClrValue::Bool(b) => if *b { String::from("True") } else { String::from("False") },
        ClrValue::I1(v) => format!("{}", v),
        ClrValue::U1(v) => format!("{}", v),
        ClrValue::I2(v) => format!("{}", v),
        ClrValue::U2(v) => format!("{}", v),
        ClrValue::I4(v) => format!("{}", v),
        ClrValue::U4(v) => format!("{}", v),
        ClrValue::I8(v) => format!("{}", v),
        ClrValue::U8(v) => format!("{}", v),
        ClrValue::R4(v) => format!("{}", v),
        ClrValue::R8(v) => format!("{}", v),
        ClrValue::Char(c) => format!("{}", c),
        ClrValue::IntPtr(v) => format!("{}", v),
        ClrValue::UIntPtr(v) => format!("{}", v),
        ClrValue::String(s) => s.clone(),
        ClrValue::ObjectRef(h) => format!("Object@{}", h),
        ClrValue::Boxed(inner) => clr_value_to_string(inner),
        ClrValue::ArrayRef(h) => format!("Array@{}", h),
        ClrValue::Void => String::new(),
    }
}

/// Compare two ClrValues for equality.
fn clr_values_equal(a: &ClrValue, b: &ClrValue) -> bool {
    match (a, b) {
        (ClrValue::Null, ClrValue::Null) => true,
        (ClrValue::Bool(x), ClrValue::Bool(y)) => x == y,
        (ClrValue::I4(x), ClrValue::I4(y)) => x == y,
        (ClrValue::I8(x), ClrValue::I8(y)) => x == y,
        (ClrValue::String(x), ClrValue::String(y)) => x == y,
        (ClrValue::ObjectRef(x), ClrValue::ObjectRef(y)) => x == y,
        _ => false,
    }
}

/// Resolve a BCL method call by type and method name.
///
/// Returns Some(result) if the method is a known BCL method, None otherwise.
pub fn try_call_bcl_method(
    type_name: &str,
    method_name: &str,
    this: Option<&ClrValue>,
    args: &[ClrValue],
) -> Option<ClrValue> {
    match (type_name, method_name) {
        // Console
        ("System.Console", "WriteLine") => Some(console_write_line(args)),
        ("System.Console", "Write") => Some(console_write(args)),
        ("System.Console", "ReadLine") => Some(console_read_line(args)),

        // String static methods
        ("System.String", "Concat") => Some(string_concat(args)),
        ("System.String", "Format") => Some(string_format(args)),

        // String instance methods
        ("System.String", "Substring") => this.map(|t| string_substring(t, args)),
        ("System.String", "Trim") => this.map(|t| string_trim(t, args)),
        ("System.String", "Contains") => this.map(|t| string_contains(t, args)),
        ("System.String", "Replace") => this.map(|t| string_replace(t, args)),
        ("System.String", "ToUpper") => this.map(|t| string_to_upper(t, args)),
        ("System.String", "ToLower") => this.map(|t| string_to_lower(t, args)),
        ("System.String", "StartsWith") => this.map(|t| string_starts_with(t, args)),
        ("System.String", "EndsWith") => this.map(|t| string_ends_with(t, args)),
        ("System.String", "IndexOf") => this.map(|t| string_index_of(t, args)),
        ("System.String", "get_Length") => this.map(|t| string_length(t)),
        ("System.String", "ToString") => this.map(|t| ClrValue::String(clr_value_to_string(t))),

        // Int32
        ("System.Int32", "Parse") => Some(int32_parse(args)),
        ("System.Int32", "TryParse") => Some(int32_try_parse(args)),
        ("System.Int32", "ToString") => this.map(|t| int32_to_string(t, args)),

        // Convert
        ("System.Convert", "ToInt32") => Some(convert_to_int32(args)),
        ("System.Convert", "ToString") => Some(convert_to_string(args)),
        ("System.Convert", "ToDouble") => Some(convert_to_double(args)),
        ("System.Convert", "ToBoolean") => Some(convert_to_boolean(args)),

        // Math
        ("System.Math", "Abs") => Some(math_abs(args)),
        ("System.Math", "Max") => Some(math_max(args)),
        ("System.Math", "Min") => Some(math_min(args)),
        ("System.Math", "Floor") => Some(math_floor(args)),
        ("System.Math", "Ceiling") => Some(math_ceiling(args)),
        ("System.Math", "Round") => Some(math_round(args)),
        ("System.Math", "Sqrt") => Some(math_sqrt(args)),
        ("System.Math", "Pow") => Some(math_pow(args)),

        // File
        ("System.IO.File", "ReadAllText") => Some(file_read_all_text(args)),
        ("System.IO.File", "WriteAllText") => Some(file_write_all_text(args)),
        ("System.IO.File", "Exists") => Some(file_exists(args)),
        ("System.IO.File", "Delete") => Some(file_delete(args)),

        // Object
        ("System.Object", "ToString") => {
            this.map(|t| ClrValue::String(clr_value_to_string(t)))
        }
        ("System.Object", "GetType") => Some(ClrValue::String(String::from(type_name))),
        ("System.Object", "Equals") => {
            if let (Some(t), Some(arg)) = (this, args.first()) {
                Some(ClrValue::Bool(clr_values_equal(t, arg)))
            } else {
                Some(ClrValue::Bool(false))
            }
        }
        ("System.Object", "GetHashCode") => {
            Some(ClrValue::I4(0)) // stub
        }

        _ => None,
    }
}
