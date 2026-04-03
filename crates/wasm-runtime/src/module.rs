//! Parsed WASM module representation.

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::*;

/// A parsed WebAssembly module.
#[derive(Debug, Clone)]
pub struct WasmModule {
    /// Type section: function signatures.
    pub types: Vec<FuncType>,
    /// Import section.
    pub imports: Vec<Import>,
    /// Function section: type indices for defined functions.
    pub functions: Vec<u32>,
    /// Table section.
    pub tables: Vec<TableType>,
    /// Memory section.
    pub memories: Vec<MemoryType>,
    /// Global section.
    pub globals: Vec<Global>,
    /// Export section.
    pub exports: Vec<Export>,
    /// Start function index.
    pub start: Option<u32>,
    /// Element section (for table initialization).
    pub elements: Vec<Element>,
    /// Code section: function bodies.
    pub code: Vec<CodeBody>,
    /// Data section: memory initialization.
    pub data: Vec<DataSegment>,
}

impl WasmModule {
    pub fn new() -> Self {
        Self {
            types: Vec::new(),
            imports: Vec::new(),
            functions: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            exports: Vec::new(),
            start: None,
            elements: Vec::new(),
            code: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Count of imported functions.
    pub fn import_func_count(&self) -> usize {
        self.imports.iter().filter(|i| matches!(i.desc, ImportDesc::Func(_))).count()
    }

    /// Get the type index for a function by its absolute function index.
    pub fn func_type_idx(&self, func_idx: u32) -> Option<u32> {
        let import_funcs = self.import_func_count();
        let idx = func_idx as usize;
        if idx < import_funcs {
            // Imported function
            match &self.imports.iter()
                .filter(|i| matches!(i.desc, ImportDesc::Func(_)))
                .nth(idx)?
                .desc
            {
                ImportDesc::Func(type_idx) => Some(*type_idx),
                _ => None,
            }
        } else {
            self.functions.get(idx - import_funcs).copied()
        }
    }

    /// Get the FuncType for a function index.
    pub fn func_type(&self, func_idx: u32) -> Option<&FuncType> {
        let type_idx = self.func_type_idx(func_idx)?;
        self.types.get(type_idx as usize)
    }

    /// Find an export by name.
    pub fn find_export(&self, name: &str) -> Option<&Export> {
        self.exports.iter().find(|e| e.name == name)
    }
}

/// Import descriptor.
#[derive(Debug, Clone)]
pub enum ImportDesc {
    Func(u32),
    Table(TableType),
    Memory(MemoryType),
    Global(GlobalType),
}

/// An import entry.
#[derive(Debug, Clone)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub desc: ImportDesc,
}

/// Export descriptor.
#[derive(Debug, Clone)]
pub enum ExportDesc {
    Func(u32),
    Table(u32),
    Memory(u32),
    Global(u32),
}

/// An export entry.
#[derive(Debug, Clone)]
pub struct Export {
    pub name: String,
    pub desc: ExportDesc,
}

/// A global variable definition.
#[derive(Debug, Clone)]
pub struct Global {
    pub global_type: GlobalType,
    pub init: ConstExpr,
}

/// Constant expression (for globals, data offsets, element offsets).
#[derive(Debug, Clone)]
pub enum ConstExpr {
    Value(Value),
    GlobalGet(u32),
}

/// Element segment for table initialization.
#[derive(Debug, Clone)]
pub struct Element {
    pub table_idx: u32,
    pub offset: ConstExpr,
    pub init: Vec<u32>,
}

/// Code body: locals + bytecode.
#[derive(Debug, Clone)]
pub struct CodeBody {
    pub locals: Vec<ValType>,
    pub code: Vec<u8>,
}

/// Data segment for memory initialization.
#[derive(Debug, Clone)]
pub struct DataSegment {
    pub memory_idx: u32,
    pub offset: ConstExpr,
    pub data: Vec<u8>,
}
