//! Type system for ClaudioOS rustc-lite.
//!
//! Defines the internal type representation used after parsing.
//! Supports primitives, structs, enums, references, tuples, arrays,
//! function pointers, generics, and trait objects.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Unique type ID for interned types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Internal type representation (resolved from AST `Ty`).
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    // Primitives
    Bool,
    Char,
    I8, I16, I32, I64, I128, Isize,
    U8, U16, U32, U64, U128, Usize,
    F32, F64,
    Str,    // str (unsized)
    Unit,   // ()
    Never,  // !

    // Compound
    Tuple(Vec<Type>),
    Array(Box<Type>, usize),        // [T; N]
    Slice(Box<Type>),               // [T]
    Reference { mutable: bool, lifetime: Option<String>, inner: Box<Type> },
    RawPtr { mutable: bool, inner: Box<Type> },
    FnPtr { params: Vec<Type>, ret: Box<Type> },

    // Named types
    Struct(StructType),
    Enum(EnumType),

    // Generics
    TypeParam(String),              // T, U, etc. (unresolved)
    Generic { base: Box<Type>, args: Vec<Type> }, // Vec<T>, HashMap<K,V>

    // Trait objects
    DynTrait(Vec<String>),          // dyn Foo + Bar
    ImplTrait(Vec<String>),         // impl Foo + Bar

    // Inference variable (unknown, to be resolved)
    Infer(u32),

    // Error sentinel (allows type checking to continue after errors)
    Error,
}

impl Type {
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 | Type::Isize
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 | Type::Usize
        )
    }

    pub fn is_signed(&self) -> bool {
        matches!(
            self,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 | Type::Isize
        )
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }

    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_float()
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Type::Bool)
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, Type::Unit)
    }

    pub fn is_never(&self) -> bool {
        matches!(self, Type::Never)
    }

    pub fn is_reference(&self) -> bool {
        matches!(self, Type::Reference { .. })
    }

    pub fn is_copy(&self) -> bool {
        match self {
            Type::Bool | Type::Char
            | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 | Type::Isize
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 | Type::Usize
            | Type::F32 | Type::F64
            | Type::Unit | Type::Never => true,
            Type::Reference { mutable: false, .. } => true,
            Type::RawPtr { .. } => true,
            Type::Tuple(ts) => ts.iter().all(|t| t.is_copy()),
            Type::Array(t, _) => t.is_copy(),
            _ => false,
        }
    }

    /// Size in bytes (for Cranelift layout). Returns None for unsized types.
    pub fn size_bytes(&self) -> Option<usize> {
        Some(match self {
            Type::Unit | Type::Never => 0,
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 | Type::Char => 4,
            Type::I64 | Type::U64 | Type::F64 | Type::Isize | Type::Usize => 8,
            Type::I128 | Type::U128 => 16,
            Type::Reference { .. } | Type::RawPtr { .. } | Type::FnPtr { .. } => 8, // pointer size
            Type::Array(elem, n) => elem.size_bytes()? * n,
            Type::Tuple(ts) => {
                let mut size = 0;
                for t in ts {
                    let s = t.size_bytes()?;
                    let align = t.align_bytes()?;
                    size = (size + align - 1) & !(align - 1); // align up
                    size += s;
                }
                size
            }
            Type::Str | Type::Slice(_) => return None, // unsized
            _ => 8, // default pointer size for complex types
        })
    }

    /// Alignment in bytes.
    pub fn align_bytes(&self) -> Option<usize> {
        Some(match self {
            Type::Unit | Type::Never => 1,
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 | Type::Char => 4,
            Type::I64 | Type::U64 | Type::F64 | Type::Isize | Type::Usize
            | Type::I128 | Type::U128 => 8,
            Type::Reference { .. } | Type::RawPtr { .. } | Type::FnPtr { .. } => 8,
            Type::Array(elem, _) => elem.align_bytes()?,
            Type::Tuple(ts) => ts.iter().filter_map(|t| t.align_bytes()).max().unwrap_or(1),
            _ => 8,
        })
    }
}

// ─── Struct type info ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct StructType {
    pub name: String,
    pub fields: Vec<StructField>,
    pub generic_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
    pub offset: usize, // byte offset in struct layout
}

// ─── Enum type info ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct EnumType {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub generic_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub discriminant: i64,
    pub kind: EnumVariantKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantKind {
    Unit,
    Tuple(Vec<Type>),
    Struct(Vec<StructField>),
}

// ─── Trait info ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TraitInfo {
    pub name: String,
    pub methods: Vec<TraitMethod>,
    pub generic_params: Vec<String>,
    pub supertraits: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TraitMethod {
    pub name: String,
    pub params: Vec<Type>,  // includes self type
    pub ret: Type,
    pub has_default: bool,
}

// ─── Impl info ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImplInfo {
    pub self_ty: Type,
    pub trait_name: Option<String>,
    pub methods: Vec<MethodInfo>,
}

#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    pub is_static: bool, // no self parameter
}
