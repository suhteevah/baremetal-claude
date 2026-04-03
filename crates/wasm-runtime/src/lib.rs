//! claudio-wasm-runtime: A no_std WebAssembly interpreter for ClaudioOS.
//!
//! Implements WASM MVP (version 1) with WASI preview1 stubs for basic I/O.
//! Designed for bare-metal execution with no OS dependencies.
//!
//! # Usage
//! ```ignore
//! let bytes = include_bytes!("module.wasm");
//! let mut instance = claudio_wasm_runtime::load_wasm(bytes).unwrap();
//! let result = claudio_wasm_runtime::call_export(&mut instance, "add", &[
//!     Value::I32(2), Value::I32(3),
//! ]).unwrap();
//! ```

#![no_std]

extern crate alloc;

pub mod binary;
pub mod types;
pub mod module;
pub mod memory;
pub mod table;
pub mod interpreter;
pub mod instance;
pub mod wasi;
pub mod driver;

pub use types::Value;
pub use instance::WasmInstance;
pub use driver::{load_wasm, call_export, call_export_with_stdout, run_wasi};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Build a minimal WASM module that exports an `add(i32, i32) -> i32` function.
    fn make_add_module() -> alloc::vec::Vec<u8> {
        let mut wasm = alloc::vec::Vec::new();

        // Magic + version
        wasm.extend_from_slice(&[0x00, 0x61, 0x73, 0x6D]); // \0asm
        wasm.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version 1

        // Type section: 1 type -> (i32, i32) -> i32
        wasm.extend_from_slice(&[0x01]); // section id
        wasm.extend_from_slice(&[0x07]); // section size
        wasm.extend_from_slice(&[0x01]); // 1 type
        wasm.extend_from_slice(&[0x60]); // func type
        wasm.extend_from_slice(&[0x02, 0x7F, 0x7F]); // 2 params: i32, i32
        wasm.extend_from_slice(&[0x01, 0x7F]); // 1 result: i32

        // Function section: 1 function -> type 0
        wasm.extend_from_slice(&[0x03]); // section id
        wasm.extend_from_slice(&[0x02]); // section size
        wasm.extend_from_slice(&[0x01]); // 1 function
        wasm.extend_from_slice(&[0x00]); // type index 0

        // Export section: export "add" as function 0
        wasm.extend_from_slice(&[0x07]); // section id
        wasm.extend_from_slice(&[0x07]); // section size
        wasm.extend_from_slice(&[0x01]); // 1 export
        wasm.extend_from_slice(&[0x03]); // name length
        wasm.extend_from_slice(b"add");  // name
        wasm.extend_from_slice(&[0x00]); // export kind: func
        wasm.extend_from_slice(&[0x00]); // func index 0

        // Code section: 1 body
        wasm.extend_from_slice(&[0x0A]); // section id
        wasm.extend_from_slice(&[0x09]); // section size
        wasm.extend_from_slice(&[0x01]); // 1 body
        wasm.extend_from_slice(&[0x07]); // body size
        wasm.extend_from_slice(&[0x00]); // 0 local declarations
        // local.get 0
        wasm.extend_from_slice(&[0x20, 0x00]);
        // local.get 1
        wasm.extend_from_slice(&[0x20, 0x01]);
        // i32.add
        wasm.extend_from_slice(&[0x6A]);
        // end
        wasm.extend_from_slice(&[0x0B]);

        wasm
    }

    #[test]
    fn test_parse_and_run_add() {
        let wasm = make_add_module();
        let mut instance = load_wasm(&wasm).unwrap();
        let result = call_export(&mut instance, "add", &[Value::I32(7), Value::I32(35)]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_i32().unwrap(), 42);
    }

    #[test]
    fn test_i32_arithmetic() {
        let wasm = make_add_module();
        let mut instance = load_wasm(&wasm).unwrap();
        let result = call_export(&mut instance, "add", &[Value::I32(-10), Value::I32(10)]).unwrap();
        assert_eq!(result[0].as_i32().unwrap(), 0);
    }

    #[test]
    fn test_invalid_magic() {
        let bad = alloc::vec![0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00];
        assert!(load_wasm(&bad).is_err());
    }

    #[test]
    fn test_memory_bounds() {
        let mut mem = memory::LinearMemory::new(1, None).unwrap();
        assert!(mem.read_u8(0).is_ok());
        assert!(mem.read_u8(65535).is_ok());
        assert!(mem.read_u8(65536).is_err());
    }

    #[test]
    fn test_memory_grow() {
        let mut mem = memory::LinearMemory::new(1, Some(3)).unwrap();
        assert_eq!(mem.size_pages(), 1);
        assert_eq!(mem.grow(1), 1); // returns old size
        assert_eq!(mem.size_pages(), 2);
        assert_eq!(mem.grow(1), 2);
        assert_eq!(mem.size_pages(), 3);
        assert_eq!(mem.grow(1), -1); // exceeds max
    }

    #[test]
    fn test_table_basic() {
        let limits = types::Limits { min: 4, max: Some(10) };
        let mut table = table::Table::new(&limits);
        assert_eq!(table.size(), 4);
        assert_eq!(table.get(0).unwrap(), None);
        table.set(0, Some(42)).unwrap();
        assert_eq!(table.get(0).unwrap(), Some(42));
        assert!(table.get(4).is_err());
    }

    #[test]
    fn test_value_types() {
        let v = Value::I32(42);
        assert_eq!(v.as_i32().unwrap(), 42);
        assert!(v.as_i64().is_err());

        let v = Value::F64(3.14);
        assert!((v.as_f64().unwrap() - 3.14).abs() < f64::EPSILON);
    }

    #[test]
    fn test_leb128_decode() {
        let data = [0xE5, 0x8E, 0x26]; // 624485 unsigned
        let mut reader = binary::BinaryReader::new(&data);
        assert_eq!(reader.read_u32_leb128().unwrap(), 624485);
    }
}
