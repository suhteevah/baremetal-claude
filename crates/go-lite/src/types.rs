//! Go type system.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Go type representation.
#[derive(Debug, Clone, PartialEq)]
pub enum GoType {
    // === Primitive types ===
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Uintptr,
    Float32,
    Float64,
    Complex64,
    Complex128,
    String,
    Byte,   // alias for uint8
    Rune,   // alias for int32

    // === Composite types ===
    /// Slice: []T
    Slice(Box<GoType>),
    /// Array: [N]T
    Array(Box<GoType>, usize),
    /// Map: map[K]V
    Map(Box<GoType>, Box<GoType>),
    /// Channel: chan T, chan<- T, <-chan T
    Chan(Box<GoType>, ChanDir),
    /// Struct: named or anonymous
    Struct(Vec<StructFieldType>),
    /// Interface
    Interface(Vec<InterfaceMethodType>),
    /// Pointer: *T
    Pointer(Box<GoType>),
    /// Function type: func(params) returns
    Func {
        params: Vec<GoType>,
        returns: Vec<GoType>,
        variadic: bool,
    },

    /// Named type (user-defined)
    Named(String),
    /// Qualified name: pkg.Type
    Qualified(String, String),

    /// Void (for statements, not a real Go type)
    Void,
}

/// Channel direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChanDir {
    /// Bidirectional: chan T
    Both,
    /// Send-only: chan<- T
    Send,
    /// Receive-only: <-chan T
    Recv,
}

/// Struct field in type definition.
#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldType {
    pub name: String,
    pub ty: GoType,
}

/// Interface method in type definition.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceMethodType {
    pub name: String,
    pub params: Vec<GoType>,
    pub returns: Vec<GoType>,
}

impl GoType {
    /// Size in bytes on x86_64.
    pub fn size(&self) -> usize {
        match self {
            GoType::Bool => 1,
            GoType::Int8 | GoType::Uint8 | GoType::Byte => 1,
            GoType::Int16 | GoType::Uint16 => 2,
            GoType::Int32 | GoType::Uint32 | GoType::Rune | GoType::Float32 => 4,
            GoType::Int64 | GoType::Uint64 | GoType::Float64
            | GoType::Int | GoType::Uint | GoType::Uintptr => 8,
            GoType::Complex64 => 8,
            GoType::Complex128 => 16,
            GoType::String => 16,  // (ptr, len)
            GoType::Slice(_) => 24, // (ptr, len, cap)
            GoType::Array(inner, n) => inner.size() * n,
            GoType::Map(_, _) => 8, // pointer to runtime hash map
            GoType::Chan(_, _) => 8, // pointer to runtime channel
            GoType::Pointer(_) => 8,
            GoType::Func { .. } => 8, // function pointer
            GoType::Interface(_) => 16, // (type_id, data_ptr)
            GoType::Struct(fields) => {
                fields.iter().map(|f| f.ty.size()).sum()
            }
            GoType::Named(_) | GoType::Qualified(_, _) => 8, // resolved later
            GoType::Void => 0,
        }
    }

    /// Alignment in bytes.
    pub fn align(&self) -> usize {
        match self {
            GoType::Bool | GoType::Int8 | GoType::Uint8 | GoType::Byte => 1,
            GoType::Int16 | GoType::Uint16 => 2,
            GoType::Int32 | GoType::Uint32 | GoType::Rune | GoType::Float32 => 4,
            _ => 8,
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            GoType::Int | GoType::Int8 | GoType::Int16 | GoType::Int32 | GoType::Int64
            | GoType::Uint | GoType::Uint8 | GoType::Uint16 | GoType::Uint32 | GoType::Uint64
            | GoType::Uintptr | GoType::Byte | GoType::Rune
        )
    }

    pub fn is_float(&self) -> bool {
        matches!(self, GoType::Float32 | GoType::Float64)
    }

    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_float()
    }

    pub fn is_signed(&self) -> bool {
        matches!(
            self,
            GoType::Int | GoType::Int8 | GoType::Int16 | GoType::Int32 | GoType::Int64 | GoType::Rune
        )
    }
}
