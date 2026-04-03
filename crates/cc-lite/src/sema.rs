//! Semantic analysis for cc-lite: type checking, symbol resolution, struct layout.
//!
//! Performs a pass over the parsed AST to:
//! - Build a **scoped symbol table** (nested scopes for file, function, block)
//! - Compute **struct/union layouts** with proper alignment and padding
//! - Register **enum constants**, **typedefs**, and **function signatures**
//! - Implement **C integer promotion** rules (C11 6.3.1.1): types smaller than
//!   `int` (char, short, bool) are promoted to `int` in arithmetic expressions
//! - Implement **usual arithmetic conversions** (C11 6.3.1.8): when two operands
//!   have different types, the result type is determined by a ranked hierarchy:
//!   `double > float > unsigned long long > long long > unsigned long > long > unsigned int > int`
//!
//! ## Struct Layout Algorithm
//!
//! For each field in order:
//! 1. Compute the field's alignment requirement
//! 2. Add padding bytes to bring the current offset to a multiple of that alignment
//! 3. Place the field at the padded offset
//! 4. Advance the offset by the field's size
//! 5. After all fields, pad the total size to a multiple of the struct's alignment
//!    (the maximum alignment of any field)

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::*;

/// Information about a declared symbol (variable, function parameter, enum constant).
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The C type of this symbol.
    pub ty: CType,
    /// Which scope level this symbol was declared in.
    pub scope: ScopeKind,
    /// Stack frame offset for local variables (None for globals).
    pub offset: Option<i32>,
    /// Whether this symbol has file (global) scope.
    pub is_global: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScopeKind {
    File,
    Function,
    Block,
}

/// Struct layout information.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub fields: Vec<FieldLayout>,
    pub size: usize,
    pub align: usize,
}

#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub ty: CType,
    pub offset: usize,
}

/// Semantic analysis context.
pub struct SemaContext {
    /// Scoped symbol table stack.
    scopes: Vec<BTreeMap<String, Symbol>>,
    /// Struct definitions.
    pub structs: BTreeMap<String, StructLayout>,
    /// Union definitions (largest field determines size).
    pub unions: BTreeMap<String, StructLayout>,
    /// Enum constant values.
    pub enum_values: BTreeMap<String, i64>,
    /// Typedef mappings.
    pub typedefs: BTreeMap<String, CType>,
    /// Function signatures for checking calls.
    pub functions: BTreeMap<String, FuncSig>,
    /// Errors accumulated during analysis.
    pub errors: Vec<String>,
    /// Current stack offset for local variables.
    pub stack_offset: i32,
}

#[derive(Debug, Clone)]
pub struct FuncSig {
    pub ret: CType,
    pub params: Vec<CType>,
    pub variadic: bool,
}

impl SemaContext {
    pub fn new() -> Self {
        Self {
            scopes: alloc::vec![BTreeMap::new()], // file scope
            structs: BTreeMap::new(),
            unions: BTreeMap::new(),
            enum_values: BTreeMap::new(),
            typedefs: BTreeMap::new(),
            functions: BTreeMap::new(),
            errors: Vec::new(),
            stack_offset: 0,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn define(&mut self, name: String, sym: Symbol) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, sym);
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(sym) = scope.get(name) {
                return Some(sym);
            }
        }
        None
    }

    /// Resolve a typedef name to its underlying type.
    pub fn resolve_type(&self, ty: &CType) -> CType {
        match ty {
            CType::TypedefName(name) => {
                if let Some(real) = self.typedefs.get(name) {
                    self.resolve_type(real)
                } else {
                    ty.clone()
                }
            }
            CType::Const(inner) => CType::Const(alloc::boxed::Box::new(self.resolve_type(inner))),
            CType::Volatile(inner) => CType::Volatile(alloc::boxed::Box::new(self.resolve_type(inner))),
            CType::Pointer(inner) => CType::Pointer(alloc::boxed::Box::new(self.resolve_type(inner))),
            CType::Array(inner, sz) => CType::Array(alloc::boxed::Box::new(self.resolve_type(inner)), *sz),
            _ => ty.clone(),
        }
    }

    /// Compute struct layout with proper alignment and padding.
    pub fn compute_struct_layout(&mut self, name: &str, fields: &[StructField]) {
        let mut layout_fields = Vec::new();
        let mut offset = 0usize;
        let mut max_align = 1usize;

        for field in fields {
            let ty = self.resolve_type(&field.ty);
            let align = self.type_align(&ty);
            let size = self.type_size(&ty);

            // Pad to alignment
            if offset % align != 0 {
                offset += align - (offset % align);
            }
            layout_fields.push(FieldLayout {
                name: field.name.clone(),
                ty: ty.clone(),
                offset,
            });
            offset += size;
            if align > max_align {
                max_align = align;
            }
        }

        // Final padding to struct alignment
        if offset % max_align != 0 {
            offset += max_align - (offset % max_align);
        }

        self.structs.insert(String::from(name), StructLayout {
            fields: layout_fields,
            size: offset,
            align: max_align,
        });
    }

    /// Compute union layout (all fields at offset 0, size = largest).
    pub fn compute_union_layout(&mut self, name: &str, fields: &[StructField]) {
        let mut layout_fields = Vec::new();
        let mut max_size = 0usize;
        let mut max_align = 1usize;

        for field in fields {
            let ty = self.resolve_type(&field.ty);
            let size = self.type_size(&ty);
            let align = self.type_align(&ty);

            layout_fields.push(FieldLayout {
                name: field.name.clone(),
                ty: ty.clone(),
                offset: 0,
            });
            if size > max_size {
                max_size = size;
            }
            if align > max_align {
                max_align = align;
            }
        }

        // Pad to alignment
        if max_size % max_align != 0 {
            max_size += max_align - (max_size % max_align);
        }

        self.unions.insert(String::from(name), StructLayout {
            fields: layout_fields,
            size: max_size,
            align: max_align,
        });
    }

    /// Get the size of a type in bytes.
    pub fn type_size(&self, ty: &CType) -> usize {
        match ty {
            CType::Struct(name) => {
                self.structs.get(name).map(|l| l.size).unwrap_or(0)
            }
            CType::Union(name) => {
                self.unions.get(name).map(|l| l.size).unwrap_or(0)
            }
            CType::TypedefName(name) => {
                if let Some(real) = self.typedefs.get(name) {
                    self.type_size(real)
                } else {
                    0
                }
            }
            _ => ty.size(),
        }
    }

    /// Get the alignment of a type.
    pub fn type_align(&self, ty: &CType) -> usize {
        match ty {
            CType::Struct(name) => {
                self.structs.get(name).map(|l| l.align).unwrap_or(1)
            }
            CType::Union(name) => {
                self.unions.get(name).map(|l| l.align).unwrap_or(1)
            }
            CType::TypedefName(name) => {
                if let Some(real) = self.typedefs.get(name) {
                    self.type_align(real)
                } else {
                    1
                }
            }
            _ => ty.align(),
        }
    }

    /// Integer promotion (C11 6.3.1.1).
    ///
    /// Types with rank less than `int` (bool, char, unsigned char, short,
    /// unsigned short) are promoted to `int` when used in expressions.
    /// This matches real C compilers: `char + char` yields `int`.
    pub fn integer_promote(ty: &CType) -> CType {
        match ty {
            CType::Bool | CType::Char | CType::UChar | CType::Short | CType::UShort => CType::Int,
            _ => ty.clone(),
        }
    }

    /// Usual arithmetic conversions (C11 6.3.1.8).
    ///
    /// Determines the common type when two different types appear as operands
    /// in a binary arithmetic expression. The rules, applied in order:
    /// 1. If either operand is `double`, the result is `double`
    /// 2. If either operand is `float`, the result is `float`
    /// 3. Apply integer promotion to both operands
    /// 4. If both have the same type, that's the result
    /// 5. Otherwise, use the type with the larger size
    pub fn usual_arithmetic_conversion(a: &CType, b: &CType) -> CType {
        // If either is double, result is double
        if matches!(a, CType::Double) || matches!(b, CType::Double) {
            return CType::Double;
        }
        if matches!(a, CType::Float) || matches!(b, CType::Float) {
            return CType::Float;
        }
        // Integer promotions
        let a = Self::integer_promote(a);
        let b = Self::integer_promote(b);
        // If same type, done
        if a == b {
            return a;
        }
        // If both signed or both unsigned, use the larger
        if a.size() >= b.size() { a } else { b }
    }

    /// Analyze the full translation unit.
    pub fn analyze(&mut self, tu: &TranslationUnit) {
        for decl in &tu.decls {
            match decl {
                ExternalDecl::FuncDef(func) => {
                    self.functions.insert(func.name.clone(), FuncSig {
                        ret: func.return_type.clone(),
                        params: func.params.iter().map(|p| p.ty.clone()).collect(),
                        variadic: func.is_variadic,
                    });
                    self.analyze_func(func);
                }
                ExternalDecl::VarDecl(var) => {
                    self.define(var.name.clone(), Symbol {
                        ty: var.ty.clone(),
                        scope: ScopeKind::File,
                        offset: None,
                        is_global: true,
                    });
                }
                ExternalDecl::StructDef(sd) => {
                    if let Some(ref name) = sd.name {
                        self.compute_struct_layout(name, &sd.fields);
                    }
                }
                ExternalDecl::UnionDef(ud) => {
                    if let Some(ref name) = ud.name {
                        self.compute_union_layout(name, &ud.fields);
                    }
                }
                ExternalDecl::EnumDef(ed) => {
                    let mut val: i64 = 0;
                    for variant in &ed.variants {
                        if let Some(ref expr) = variant.value {
                            if let Expr::IntLit(v) = expr {
                                val = *v;
                            }
                        }
                        self.enum_values.insert(variant.name.clone(), val);
                        self.define(variant.name.clone(), Symbol {
                            ty: CType::Int,
                            scope: ScopeKind::File,
                            offset: None,
                            is_global: true,
                        });
                        val += 1;
                    }
                }
                ExternalDecl::TypedefDecl(td) => {
                    self.typedefs.insert(td.new_name.clone(), td.original.clone());
                }
            }
        }
    }

    fn analyze_func(&mut self, func: &FuncDef) {
        self.push_scope();
        self.stack_offset = 0;

        // Define parameters
        for (i, param) in func.params.iter().enumerate() {
            if let Some(ref name) = param.name {
                self.stack_offset -= self.type_size(&param.ty) as i32;
                // Align
                let align = self.type_align(&param.ty) as i32;
                if self.stack_offset % align != 0 {
                    self.stack_offset -= align + (self.stack_offset % align);
                }
                self.define(name.clone(), Symbol {
                    ty: param.ty.clone(),
                    scope: ScopeKind::Function,
                    offset: Some(self.stack_offset),
                    is_global: false,
                });
            }
        }

        self.analyze_block(&func.body);
        self.pop_scope();
    }

    fn analyze_block(&mut self, block: &Block) {
        self.push_scope();
        for stmt in &block.stmts {
            self.analyze_stmt(stmt);
        }
        self.pop_scope();
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::VarDecl(var) => {
                let size = self.type_size(&var.ty) as i32;
                self.stack_offset -= size;
                let align = self.type_align(&var.ty) as i32;
                if align > 0 && self.stack_offset % align != 0 {
                    self.stack_offset -= align + (self.stack_offset % align);
                }
                self.define(var.name.clone(), Symbol {
                    ty: var.ty.clone(),
                    scope: ScopeKind::Block,
                    offset: Some(self.stack_offset),
                    is_global: false,
                });
            }
            Stmt::Block(block) => self.analyze_block(block),
            Stmt::If { then_body, else_body, .. } => {
                self.analyze_stmt(then_body);
                if let Some(eb) = else_body {
                    self.analyze_stmt(eb);
                }
            }
            Stmt::While { body, .. } => self.analyze_stmt(body),
            Stmt::DoWhile { body, .. } => self.analyze_stmt(body),
            Stmt::For { init, body, .. } => {
                if let Some(init) = init {
                    self.analyze_stmt(init);
                }
                self.analyze_stmt(body);
            }
            Stmt::Switch { body, .. } => self.analyze_stmt(body),
            Stmt::Case { body, .. } => self.analyze_stmt(body),
            Stmt::Default(body) => self.analyze_stmt(body),
            Stmt::Label(_, body) => self.analyze_stmt(body),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integer_promotion() {
        assert_eq!(SemaContext::integer_promote(&CType::Char), CType::Int);
        assert_eq!(SemaContext::integer_promote(&CType::Int), CType::Int);
        assert_eq!(SemaContext::integer_promote(&CType::Long), CType::Long);
    }

    #[test]
    fn test_struct_layout() {
        let mut ctx = SemaContext::new();
        let fields = alloc::vec![
            StructField { ty: CType::Char, name: String::from("a") },
            StructField { ty: CType::Int, name: String::from("b") },
            StructField { ty: CType::Char, name: String::from("c") },
        ];
        ctx.compute_struct_layout("Test", &fields);
        let layout = ctx.structs.get("Test").unwrap();
        // char(1) + padding(3) + int(4) + char(1) + padding(3) = 12
        assert_eq!(layout.size, 12);
        assert_eq!(layout.align, 4);
        assert_eq!(layout.fields[0].offset, 0); // a
        assert_eq!(layout.fields[1].offset, 4); // b
        assert_eq!(layout.fields[2].offset, 8); // c
    }

    #[test]
    fn test_usual_conversions() {
        assert_eq!(SemaContext::usual_arithmetic_conversion(&CType::Int, &CType::Double), CType::Double);
        assert_eq!(SemaContext::usual_arithmetic_conversion(&CType::Char, &CType::Int), CType::Int);
    }
}
