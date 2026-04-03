//! WASM binary format parser (WebAssembly spec, Ch. 5: Binary Format).
//!
//! Parses a `.wasm` file from raw bytes into a [`WasmModule`] structure.
//!
//! ## Binary Layout
//!
//! ```text
//! +-------------------+
//! | Magic: \0asm      |  4 bytes
//! | Version: 1        |  4 bytes (little-endian)
//! +-------------------+
//! | Section 1 (type)  |  id(1) + size(LEB128) + content
//! | Section 2 (import)|  id(2) + size(LEB128) + content
//! | ...               |
//! | Section 10 (code) |  id(10) + size(LEB128) + function bodies
//! | Section 11 (data) |  id(11) + size(LEB128) + data segments
//! +-------------------+
//! ```
//!
//! ## LEB128 Encoding
//!
//! WASM uses Little-Endian Base 128 (LEB128) for variable-length integers:
//! - Each byte contributes 7 data bits (bits 0-6)
//! - Bit 7 is a continuation flag: 1 = more bytes follow, 0 = last byte
//! - Unsigned LEB128 (`read_u32_leb128`): value bits are concatenated
//! - Signed LEB128 (`read_i32_leb128`, `read_i64_leb128`): sign-extends
//!   using the MSB of the last byte
//!
//! ## Section IDs
//!
//! | ID | Section | Content |
//! |----|---------|---------|
//! | 0  | Custom  | Name sections, debug info (skipped) |
//! | 1  | Type    | Function signatures (`(params) -> (results)`) |
//! | 2  | Import  | External functions, tables, memories, globals |
//! | 3  | Function| Type index for each defined function |
//! | 4  | Table   | Function reference tables (for `call_indirect`) |
//! | 5  | Memory  | Linear memory declarations (min/max pages) |
//! | 6  | Global  | Global variables with initial values |
//! | 7  | Export  | Exported functions/memories/globals by name |
//! | 8  | Start   | Optional start function index |
//! | 9  | Element | Table initialization segments |
//! | 10 | Code    | Function bodies (locals + bytecode) |
//! | 11 | Data    | Memory initialization segments |

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use crate::types::*;
use crate::module::*;

/// WASM magic number: \0asm
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
/// WASM version 1
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Section IDs
const SECTION_CUSTOM: u8 = 0;
const SECTION_TYPE: u8 = 1;
const SECTION_IMPORT: u8 = 2;
const SECTION_FUNCTION: u8 = 3;
const SECTION_TABLE: u8 = 4;
const SECTION_MEMORY: u8 = 5;
const SECTION_GLOBAL: u8 = 6;
const SECTION_EXPORT: u8 = 7;
const SECTION_START: u8 = 8;
const SECTION_ELEMENT: u8 = 9;
const SECTION_CODE: u8 = 10;
const SECTION_DATA: u8 = 11;

/// Binary reader with cursor position.
pub struct BinaryReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BinaryReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn read_byte(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err(String::from("unexpected end of data"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err(String::from("unexpected end of data"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// Decode an unsigned LEB128 value (up to 32 bits).
    pub fn read_u32_leb128(&mut self) -> Result<u32, String> {
        let mut result: u32 = 0;
        let mut shift = 0;
        loop {
            let byte = self.read_byte()?;
            result |= ((byte & 0x7F) as u32) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 35 {
                return Err(String::from("LEB128 overflow (u32)"));
            }
        }
    }

    /// Decode a signed LEB128 value (up to 32 bits).
    pub fn read_i32_leb128(&mut self) -> Result<i32, String> {
        let mut result: i32 = 0;
        let mut shift = 0;
        let mut byte;
        loop {
            byte = self.read_byte()?;
            result |= ((byte & 0x7F) as i32) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift >= 35 {
                return Err(String::from("LEB128 overflow (i32)"));
            }
        }
        // Sign extend
        if shift < 32 && (byte & 0x40) != 0 {
            result |= !0 << shift;
        }
        Ok(result)
    }

    /// Decode a signed LEB128 value (up to 64 bits).
    pub fn read_i64_leb128(&mut self) -> Result<i64, String> {
        let mut result: i64 = 0;
        let mut shift = 0;
        let mut byte;
        loop {
            byte = self.read_byte()?;
            result |= ((byte & 0x7F) as i64) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift >= 70 {
                return Err(String::from("LEB128 overflow (i64)"));
            }
        }
        if shift < 64 && (byte & 0x40) != 0 {
            result |= !0i64 << shift;
        }
        Ok(result)
    }

    /// Read a UTF-8 name (length-prefixed).
    pub fn read_name(&mut self) -> Result<String, String> {
        let len = self.read_u32_leb128()? as usize;
        let bytes = self.read_bytes(len)?;
        core::str::from_utf8(bytes)
            .map(|s| String::from(s))
            .map_err(|_| String::from("invalid UTF-8 in name"))
    }

    pub fn read_f32(&mut self) -> Result<f32, String> {
        let bytes = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_f64(&mut self) -> Result<f64, String> {
        let bytes = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_valtype(&mut self) -> Result<ValType, String> {
        let b = self.read_byte()?;
        ValType::from_byte(b).map_err(|e| format!("invalid valtype 0x{:02x}: {}", b, e))
    }

    fn read_limits(&mut self) -> Result<Limits, String> {
        let flag = self.read_byte()?;
        let min = self.read_u32_leb128()?;
        let max = if flag & 0x01 != 0 {
            Some(self.read_u32_leb128()?)
        } else {
            None
        };
        Ok(Limits { min, max })
    }

    #[allow(dead_code)]
    fn read_block_type(&mut self) -> Result<BlockType, String> {
        let byte = self.data[self.pos];
        if byte == 0x40 {
            self.pos += 1;
            return Ok(BlockType::Empty);
        }
        // Try as valtype
        if let Ok(vt) = ValType::from_byte(byte) {
            self.pos += 1;
            return Ok(BlockType::Value(vt));
        }
        // Otherwise it's a type index as s33
        let idx = self.read_i32_leb128()?;
        Ok(BlockType::TypeIndex(idx as u32))
    }

    fn read_func_type(&mut self) -> Result<FuncType, String> {
        let tag = self.read_byte()?;
        if tag != 0x60 {
            return Err(format!("expected functype tag 0x60, got 0x{:02x}", tag));
        }
        let param_count = self.read_u32_leb128()? as usize;
        let mut params = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            params.push(self.read_valtype()?);
        }
        let result_count = self.read_u32_leb128()? as usize;
        let mut results = Vec::with_capacity(result_count);
        for _ in 0..result_count {
            results.push(self.read_valtype()?);
        }
        Ok(FuncType { params, results })
    }

    fn read_global_type(&mut self) -> Result<GlobalType, String> {
        let val_type = self.read_valtype()?;
        let mut_byte = self.read_byte()?;
        let mutability = match mut_byte {
            0x00 => Mutability::Const,
            0x01 => Mutability::Var,
            _ => return Err(format!("invalid mutability byte 0x{:02x}", mut_byte)),
        };
        Ok(GlobalType { val_type, mutability })
    }

    /// Read a constant expression (terminated by 0x0B).
    fn read_const_expr(&mut self) -> Result<ConstExpr, String> {
        let opcode = self.read_byte()?;
        let val = match opcode {
            0x41 => {
                let v = self.read_i32_leb128()?;
                Value::I32(v)
            }
            0x42 => {
                let v = self.read_i64_leb128()?;
                Value::I64(v)
            }
            0x43 => {
                let v = self.read_f32()?;
                Value::F32(v)
            }
            0x44 => {
                let v = self.read_f64()?;
                Value::F64(v)
            }
            0x23 => {
                // global.get
                let idx = self.read_u32_leb128()?;
                return {
                    let end = self.read_byte()?;
                    if end != 0x0B {
                        return Err(String::from("expected end opcode in const expr"));
                    }
                    Ok(ConstExpr::GlobalGet(idx))
                };
            }
            _ => return Err(format!("unsupported const expr opcode 0x{:02x}", opcode)),
        };
        let end = self.read_byte()?;
        if end != 0x0B {
            return Err(String::from("expected end opcode in const expr"));
        }
        Ok(ConstExpr::Value(val))
    }
}

/// Parse a complete WASM binary module.
pub fn parse_wasm(data: &[u8]) -> Result<WasmModule, String> {
    let mut r = BinaryReader::new(data);

    // Magic
    let magic = r.read_bytes(4)?;
    if magic != WASM_MAGIC {
        return Err(String::from("invalid WASM magic number"));
    }
    // Version
    let version = r.read_bytes(4)?;
    if version != WASM_VERSION {
        return Err(String::from("unsupported WASM version"));
    }

    let mut module = WasmModule::new();

    while !r.is_empty() {
        let section_id = r.read_byte()?;
        let section_size = r.read_u32_leb128()? as usize;
        let section_end = r.position() + section_size;

        match section_id {
            SECTION_CUSTOM => {
                // Skip custom sections
                r.pos = section_end;
            }
            SECTION_TYPE => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    module.types.push(r.read_func_type()?);
                }
            }
            SECTION_IMPORT => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let module_name = r.read_name()?;
                    let field_name = r.read_name()?;
                    let kind = r.read_byte()?;
                    let desc = match kind {
                        0x00 => ImportDesc::Func(r.read_u32_leb128()?),
                        0x01 => {
                            let elem = r.read_valtype()?;
                            let limits = r.read_limits()?;
                            ImportDesc::Table(TableType { element: elem, limits })
                        }
                        0x02 => {
                            let limits = r.read_limits()?;
                            ImportDesc::Memory(MemoryType { limits })
                        }
                        0x03 => {
                            let gt = r.read_global_type()?;
                            ImportDesc::Global(gt)
                        }
                        _ => return Err(format!("unknown import kind 0x{:02x}", kind)),
                    };
                    module.imports.push(Import {
                        module: module_name,
                        name: field_name,
                        desc,
                    });
                }
            }
            SECTION_FUNCTION => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    module.functions.push(r.read_u32_leb128()?);
                }
            }
            SECTION_TABLE => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let elem = r.read_valtype()?;
                    let limits = r.read_limits()?;
                    module.tables.push(TableType { element: elem, limits });
                }
            }
            SECTION_MEMORY => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let limits = r.read_limits()?;
                    module.memories.push(MemoryType { limits });
                }
            }
            SECTION_GLOBAL => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let global_type = r.read_global_type()?;
                    let init = r.read_const_expr()?;
                    module.globals.push(Global { global_type, init });
                }
            }
            SECTION_EXPORT => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let name = r.read_name()?;
                    let kind = r.read_byte()?;
                    let index = r.read_u32_leb128()?;
                    let desc = match kind {
                        0x00 => ExportDesc::Func(index),
                        0x01 => ExportDesc::Table(index),
                        0x02 => ExportDesc::Memory(index),
                        0x03 => ExportDesc::Global(index),
                        _ => return Err(format!("unknown export kind 0x{:02x}", kind)),
                    };
                    module.exports.push(Export { name, desc });
                }
            }
            SECTION_START => {
                module.start = Some(r.read_u32_leb128()?);
            }
            SECTION_ELEMENT => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let flags = r.read_u32_leb128()?;
                    // Simplified: only handle mode 0 (active, table 0, offset expr)
                    if flags == 0 {
                        let offset = r.read_const_expr()?;
                        let num_elems = r.read_u32_leb128()? as usize;
                        let mut init = Vec::with_capacity(num_elems);
                        for _ in 0..num_elems {
                            init.push(r.read_u32_leb128()?);
                        }
                        module.elements.push(Element {
                            table_idx: 0,
                            offset,
                            init,
                        });
                    } else {
                        // Skip unsupported element segment forms
                        r.pos = section_end;
                        break;
                    }
                }
            }
            SECTION_CODE => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let body_size = r.read_u32_leb128()? as usize;
                    let body_end = r.position() + body_size;
                    let local_decl_count = r.read_u32_leb128()? as usize;
                    let mut locals = Vec::new();
                    for _ in 0..local_decl_count {
                        let n = r.read_u32_leb128()? as usize;
                        let vt = r.read_valtype()?;
                        for _ in 0..n {
                            locals.push(vt);
                        }
                    }
                    let code_start = r.position();
                    let code_len = body_end - code_start;
                    let code = r.read_bytes(code_len)?.to_vec();
                    module.code.push(CodeBody { locals, code });
                }
            }
            SECTION_DATA => {
                let count = r.read_u32_leb128()? as usize;
                for _ in 0..count {
                    let flags = r.read_u32_leb128()?;
                    if flags == 0 {
                        let offset = r.read_const_expr()?;
                        let len = r.read_u32_leb128()? as usize;
                        let data = r.read_bytes(len)?.to_vec();
                        module.data.push(DataSegment {
                            memory_idx: 0,
                            offset,
                            data,
                        });
                    } else if flags == 1 {
                        // Passive data segment
                        let len = r.read_u32_leb128()? as usize;
                        let _data = r.read_bytes(len)?;
                        // Skip passive segments for now
                    } else {
                        r.pos = section_end;
                        break;
                    }
                }
            }
            _ => {
                log::warn!("wasm: skipping unknown section id {}", section_id);
                r.pos = section_end;
            }
        }

        if r.position() != section_end {
            log::warn!(
                "wasm: section {} size mismatch: pos {} vs expected {}",
                section_id, r.position(), section_end
            );
            r.pos = section_end;
        }
    }

    Ok(module)
}

/// Re-export BinaryReader for use by the interpreter to decode instructions inline.
pub use self::BinaryReader as InstructionReader;
