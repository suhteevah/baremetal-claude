//! WASM type system: value types, function types, table/memory/global types.

use alloc::vec::Vec;
use core::fmt;

/// WebAssembly value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
}

impl ValType {
    pub fn from_byte(b: u8) -> Result<Self, &'static str> {
        match b {
            0x7F => Ok(ValType::I32),
            0x7E => Ok(ValType::I64),
            0x7D => Ok(ValType::F32),
            0x7C => Ok(ValType::F64),
            0x7B => Ok(ValType::V128),
            0x70 => Ok(ValType::FuncRef),
            0x6F => Ok(ValType::ExternRef),
            _ => Err("unknown value type"),
        }
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValType::I32 => write!(f, "i32"),
            ValType::I64 => write!(f, "i64"),
            ValType::F32 => write!(f, "f32"),
            ValType::F64 => write!(f, "f64"),
            ValType::V128 => write!(f, "v128"),
            ValType::FuncRef => write!(f, "funcref"),
            ValType::ExternRef => write!(f, "externref"),
        }
    }
}

/// Function signature: params -> results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

/// Limits: min and optional max.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub min: u32,
    pub max: Option<u32>,
}

/// Table type: element type + limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableType {
    pub element: ValType,
    pub limits: Limits,
}

/// Memory type: limits in 64KiB pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryType {
    pub limits: Limits,
}

/// Global mutability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    Const,
    Var,
}

/// Global type: value type + mutability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalType {
    pub val_type: ValType,
    pub mutability: Mutability,
}

/// Block type for structured control flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockType {
    /// No value (void).
    Empty,
    /// Single result type.
    Value(ValType),
    /// Type index into the type section.
    TypeIndex(u32),
}

/// Runtime value.
#[derive(Debug, Clone, Copy)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    FuncRef(Option<u32>),
    ExternRef(Option<u32>),
}

impl Value {
    pub fn val_type(&self) -> ValType {
        match self {
            Value::I32(_) => ValType::I32,
            Value::I64(_) => ValType::I64,
            Value::F32(_) => ValType::F32,
            Value::F64(_) => ValType::F64,
            Value::FuncRef(_) => ValType::FuncRef,
            Value::ExternRef(_) => ValType::ExternRef,
        }
    }

    pub fn as_i32(&self) -> Result<i32, &'static str> {
        match self {
            Value::I32(v) => Ok(*v),
            _ => Err("expected i32"),
        }
    }

    pub fn as_i64(&self) -> Result<i64, &'static str> {
        match self {
            Value::I64(v) => Ok(*v),
            _ => Err("expected i64"),
        }
    }

    pub fn as_f32(&self) -> Result<f32, &'static str> {
        match self {
            Value::F32(v) => Ok(*v),
            _ => Err("expected f32"),
        }
    }

    pub fn as_f64(&self) -> Result<f64, &'static str> {
        match self {
            Value::F64(v) => Ok(*v),
            _ => Err("expected f64"),
        }
    }

    pub fn default_for(ty: ValType) -> Value {
        match ty {
            ValType::I32 => Value::I32(0),
            ValType::I64 => Value::I64(0),
            ValType::F32 => Value::F32(0.0),
            ValType::F64 => Value::F64(0.0),
            ValType::V128 => Value::I64(0), // stub
            ValType::FuncRef => Value::FuncRef(None),
            ValType::ExternRef => Value::ExternRef(None),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::I32(v) => write!(f, "{v}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::F32(v) => write!(f, "{v}"),
            Value::F64(v) => write!(f, "{v}"),
            Value::FuncRef(Some(i)) => write!(f, "funcref({i})"),
            Value::FuncRef(None) => write!(f, "funcref(null)"),
            Value::ExternRef(Some(i)) => write!(f, "externref({i})"),
            Value::ExternRef(None) => write!(f, "externref(null)"),
        }
    }
}
