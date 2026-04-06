//! Type checker for ClaudioOS rustc-lite.
//!
//! Walks the AST, resolves types, checks assignments, infers locals,
//! resolves method calls, validates trait impls, and produces a typed
//! representation suitable for Cranelift IR lowering.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::*;
use crate::types::*;

// ─── Type environment ────────────────────────────────────────────────────

/// Tracks all type information during checking.
pub struct TypeEnv {
    /// Variable scopes (stack of maps).
    scopes: Vec<BTreeMap<String, Type>>,
    /// Named type definitions (struct, enum, type alias).
    type_defs: BTreeMap<String, Type>,
    /// Trait definitions.
    traits: BTreeMap<String, TraitInfo>,
    /// Impl blocks (self_type_name -> Vec<ImplInfo>).
    impls: BTreeMap<String, Vec<ImplInfo>>,
    /// Inference variable counter.
    infer_counter: u32,
    /// Resolved inference variables.
    infer_map: BTreeMap<u32, Type>,
    /// Accumulated errors (we continue checking after errors).
    pub errors: Vec<String>,
    /// Expected return type of the current function.
    current_return_type: Option<Type>,
}

impl TypeEnv {
    pub fn new() -> Self {
        let mut env = Self {
            scopes: vec![BTreeMap::new()],
            type_defs: BTreeMap::new(),
            traits: BTreeMap::new(),
            impls: BTreeMap::new(),
            infer_counter: 0,
            infer_map: BTreeMap::new(),
            errors: Vec::new(),
            current_return_type: None,
        };
        env.register_builtins();
        env
    }

    fn register_builtins(&mut self) {
        // Register built-in types
        self.type_defs.insert("String".into(), Type::Struct(StructType {
            name: "String".into(),
            fields: vec![
                StructField { name: "ptr".into(), ty: Type::RawPtr { mutable: true, inner: alloc::boxed::Box::new(Type::U8) }, offset: 0 },
                StructField { name: "len".into(), ty: Type::Usize, offset: 8 },
                StructField { name: "cap".into(), ty: Type::Usize, offset: 16 },
            ],
            generic_params: Vec::new(),
        }));

        self.type_defs.insert("Vec".into(), Type::Struct(StructType {
            name: "Vec".into(),
            fields: vec![
                StructField { name: "ptr".into(), ty: Type::RawPtr { mutable: true, inner: alloc::boxed::Box::new(Type::TypeParam("T".into())) }, offset: 0 },
                StructField { name: "len".into(), ty: Type::Usize, offset: 8 },
                StructField { name: "cap".into(), ty: Type::Usize, offset: 16 },
            ],
            generic_params: vec!["T".into()],
        }));

        self.type_defs.insert("Box".into(), Type::Struct(StructType {
            name: "Box".into(),
            fields: vec![
                StructField { name: "ptr".into(), ty: Type::RawPtr { mutable: true, inner: alloc::boxed::Box::new(Type::TypeParam("T".into())) }, offset: 0 },
            ],
            generic_params: vec!["T".into()],
        }));

        self.type_defs.insert("Option".into(), Type::Enum(EnumType {
            name: "Option".into(),
            variants: vec![
                EnumVariant { name: "None".into(), discriminant: 0, kind: EnumVariantKind::Unit },
                EnumVariant { name: "Some".into(), discriminant: 1, kind: EnumVariantKind::Tuple(vec![Type::TypeParam("T".into())]) },
            ],
            generic_params: vec!["T".into()],
        }));

        self.type_defs.insert("Result".into(), Type::Enum(EnumType {
            name: "Result".into(),
            variants: vec![
                EnumVariant { name: "Ok".into(), discriminant: 0, kind: EnumVariantKind::Tuple(vec![Type::TypeParam("T".into())]) },
                EnumVariant { name: "Err".into(), discriminant: 1, kind: EnumVariantKind::Tuple(vec![Type::TypeParam("E".into())]) },
            ],
            generic_params: vec!["T".into(), "E".into()],
        }));

        self.type_defs.insert("HashMap".into(), Type::Struct(StructType {
            name: "HashMap".into(),
            fields: Vec::new(),
            generic_params: vec!["K".into(), "V".into()],
        }));

        // Register core traits
        self.traits.insert("Clone".into(), TraitInfo {
            name: "Clone".into(),
            methods: vec![TraitMethod {
                name: "clone".into(),
                params: vec![Type::Reference { mutable: false, lifetime: None, inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())) }],
                ret: Type::TypeParam("Self".into()),
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });

        self.traits.insert("Display".into(), TraitInfo {
            name: "Display".into(),
            methods: vec![TraitMethod {
                name: "fmt".into(),
                params: vec![Type::Reference { mutable: false, lifetime: None, inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())) }],
                ret: Type::Unit,
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });

        self.traits.insert("Debug".into(), TraitInfo {
            name: "Debug".into(),
            methods: vec![TraitMethod {
                name: "fmt".into(),
                params: vec![Type::Reference { mutable: false, lifetime: None, inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())) }],
                ret: Type::Unit,
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });

        self.traits.insert("Iterator".into(), TraitInfo {
            name: "Iterator".into(),
            methods: vec![TraitMethod {
                name: "next".into(),
                params: vec![Type::Reference { mutable: true, lifetime: None, inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())) }],
                ret: Type::Enum(EnumType {
                    name: "Option".into(),
                    variants: Vec::new(),
                    generic_params: vec!["Item".into()],
                }),
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });

        self.traits.insert("Drop".into(), TraitInfo {
            name: "Drop".into(),
            methods: vec![TraitMethod {
                name: "drop".into(),
                params: vec![Type::Reference { mutable: true, lifetime: None, inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())) }],
                ret: Type::Unit,
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });

        self.traits.insert("Default".into(), TraitInfo {
            name: "Default".into(),
            methods: vec![TraitMethod {
                name: "default".into(),
                params: Vec::new(),
                ret: Type::TypeParam("Self".into()),
                has_default: false,
            }],
            generic_params: Vec::new(),
            supertraits: Vec::new(),
        });
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_var(&mut self, name: &str, ty: Type) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.into(), ty);
        }
    }

    fn lookup_var(&self, name: &str) -> Option<&Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    fn fresh_infer(&mut self) -> Type {
        let id = self.infer_counter;
        self.infer_counter += 1;
        Type::Infer(id)
    }

    fn resolve_infer(&self, ty: &Type) -> Type {
        match ty {
            Type::Infer(id) => {
                if let Some(resolved) = self.infer_map.get(id) {
                    self.resolve_infer(resolved)
                } else {
                    ty.clone()
                }
            }
            Type::Reference { mutable, lifetime, inner } => Type::Reference {
                mutable: *mutable,
                lifetime: lifetime.clone(),
                inner: alloc::boxed::Box::new(self.resolve_infer(inner)),
            },
            Type::Array(elem, n) => Type::Array(alloc::boxed::Box::new(self.resolve_infer(elem)), *n),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.resolve_infer(t)).collect()),
            Type::Generic { base, args } => Type::Generic {
                base: alloc::boxed::Box::new(self.resolve_infer(base)),
                args: args.iter().map(|t| self.resolve_infer(t)).collect(),
            },
            _ => ty.clone(),
        }
    }

    fn unify(&mut self, a: &Type, b: &Type) -> Result<Type, String> {
        let a = self.resolve_infer(a);
        let b = self.resolve_infer(b);

        if a == b {
            return Ok(a);
        }

        match (&a, &b) {
            (Type::Infer(id), _) => {
                self.infer_map.insert(*id, b.clone());
                Ok(b)
            }
            (_, Type::Infer(id)) => {
                self.infer_map.insert(*id, a.clone());
                Ok(a)
            }
            (Type::Never, _) => Ok(b), // ! coerces to anything
            (_, Type::Never) => Ok(a),
            (Type::Error, _) | (_, Type::Error) => Ok(Type::Error),
            (Type::Reference { mutable: m1, inner: i1, .. }, Type::Reference { mutable: m2, inner: i2, .. }) => {
                let inner = self.unify(i1, i2)?;
                Ok(Type::Reference {
                    mutable: *m1 && *m2,
                    lifetime: None,
                    inner: alloc::boxed::Box::new(inner),
                })
            }
            (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => {
                let mut ts = Vec::new();
                for (x, y) in a.iter().zip(b.iter()) {
                    ts.push(self.unify(x, y)?);
                }
                Ok(Type::Tuple(ts))
            }
            _ => {
                Err(format!("type mismatch: expected {:?}, found {:?}", a, b))
            }
        }
    }

    fn error(&mut self, msg: String) {
        self.errors.push(msg);
    }

    // ── Resolve AST Ty to internal Type ──────────────────────────────

    pub fn resolve_ast_type(&mut self, ty: &Ty) -> Type {
        match ty {
            Ty::Path(path) => self.resolve_type_path(path),
            Ty::Reference { lifetime, is_mut, inner } => {
                let inner_ty = self.resolve_ast_type(inner);
                Type::Reference {
                    mutable: *is_mut,
                    lifetime: lifetime.clone(),
                    inner: alloc::boxed::Box::new(inner_ty),
                }
            }
            Ty::Slice(inner) => Type::Slice(alloc::boxed::Box::new(self.resolve_ast_type(inner))),
            Ty::Array(inner, _count) => {
                let elem = self.resolve_ast_type(inner);
                Type::Array(alloc::boxed::Box::new(elem), 0) // TODO: evaluate const expr
            }
            Ty::Tuple(ts) => {
                if ts.is_empty() {
                    Type::Unit
                } else {
                    Type::Tuple(ts.iter().map(|t| self.resolve_ast_type(t)).collect())
                }
            }
            Ty::Fn { params, ret } => {
                let param_types: Vec<Type> = params.iter().map(|p| self.resolve_ast_type(p)).collect();
                let ret_type = ret.as_ref().map(|r| self.resolve_ast_type(r)).unwrap_or(Type::Unit);
                Type::FnPtr { params: param_types, ret: alloc::boxed::Box::new(ret_type) }
            }
            Ty::Never => Type::Never,
            Ty::Infer => self.fresh_infer(),
            Ty::SelfType => Type::TypeParam("Self".into()),
            Ty::RawPtr { is_mut, inner } => {
                Type::RawPtr { mutable: *is_mut, inner: alloc::boxed::Box::new(self.resolve_ast_type(inner)) }
            }
            Ty::ImplTrait(bounds) => {
                let names: Vec<String> = bounds.iter().map(|b| b.path.name().into()).collect();
                Type::ImplTrait(names)
            }
            Ty::DynTrait(bounds) => {
                let names: Vec<String> = bounds.iter().map(|b| b.path.name().into()).collect();
                Type::DynTrait(names)
            }
        }
    }

    fn resolve_type_path(&mut self, path: &Path) -> Type {
        let name = path.name();
        match name {
            "bool" => Type::Bool,
            "char" => Type::Char,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" => Type::I64,
            "i128" => Type::I128,
            "isize" => Type::Isize,
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" => Type::U64,
            "u128" => Type::U128,
            "usize" => Type::Usize,
            "f32" => Type::F32,
            "f64" => Type::F64,
            "str" => Type::Str,
            "Self" => Type::TypeParam("Self".into()),
            _ => {
                // Check type defs
                if let Some(ty) = self.type_defs.get(name).cloned() {
                    // Apply generic args if present
                    let last = path.segments.last().unwrap();
                    if !last.generics.is_empty() {
                        let args: Vec<Type> = last
                            .generics
                            .iter()
                            .filter_map(|a| match a {
                                GenericArg::Type(t) => Some(self.resolve_ast_type(t)),
                                _ => None,
                            })
                            .collect();
                        Type::Generic {
                            base: alloc::boxed::Box::new(ty),
                            args,
                        }
                    } else {
                        ty
                    }
                } else {
                    // Could be a generic type parameter
                    Type::TypeParam(name.into())
                }
            }
        }
    }

    // ── Check source file ────────────────────────────────────────────

    pub fn check_file(&mut self, file: &SourceFile) {
        // First pass: register all type definitions
        for item in &file.items {
            self.register_item(item);
        }
        // Second pass: check function bodies and expressions
        for item in &file.items {
            self.check_item(item);
        }
    }

    fn register_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Struct(s) => {
                let fields: Vec<StructField> = match &s.kind {
                    StructKind::Named(fs) => fs
                        .iter()
                        .enumerate()
                        .map(|(i, f)| StructField {
                            name: f.name.clone(),
                            ty: self.resolve_ast_type(&f.ty),
                            offset: i * 8, // simplified layout
                        })
                        .collect(),
                    StructKind::Tuple(fs) => fs
                        .iter()
                        .enumerate()
                        .map(|(i, f)| StructField {
                            name: format!("{}", i),
                            ty: self.resolve_ast_type(&f.ty),
                            offset: i * 8,
                        })
                        .collect(),
                    StructKind::Unit => Vec::new(),
                };
                let generic_params: Vec<String> = s
                    .generics
                    .params
                    .iter()
                    .filter_map(|p| match p {
                        GenericParam::Type { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                self.type_defs.insert(
                    s.name.clone(),
                    Type::Struct(StructType {
                        name: s.name.clone(),
                        fields,
                        generic_params,
                    }),
                );
            }
            ItemKind::Enum(e) => {
                let variants: Vec<EnumVariant> = e
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let kind = match &v.kind {
                            VariantKind::Unit => EnumVariantKind::Unit,
                            VariantKind::Tuple(ts) => {
                                EnumVariantKind::Tuple(ts.iter().map(|t| self.resolve_ast_type(t)).collect())
                            }
                            VariantKind::Struct(fs) => {
                                EnumVariantKind::Struct(
                                    fs.iter()
                                        .enumerate()
                                        .map(|(j, f)| StructField {
                                            name: f.name.clone(),
                                            ty: self.resolve_ast_type(&f.ty),
                                            offset: j * 8,
                                        })
                                        .collect(),
                                )
                            }
                        };
                        EnumVariant {
                            name: v.name.clone(),
                            discriminant: i as i64,
                            kind,
                        }
                    })
                    .collect();
                let generic_params: Vec<String> = e
                    .generics
                    .params
                    .iter()
                    .filter_map(|p| match p {
                        GenericParam::Type { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                self.type_defs.insert(
                    e.name.clone(),
                    Type::Enum(EnumType {
                        name: e.name.clone(),
                        variants,
                        generic_params,
                    }),
                );
            }
            ItemKind::Trait(t) => {
                let methods: Vec<TraitMethod> = t
                    .items
                    .iter()
                    .filter_map(|item| {
                        if let ItemKind::Function(f) = &item.kind {
                            let params: Vec<Type> = f
                                .params
                                .iter()
                                .map(|p| match p {
                                    FnParam::SelfParam { is_ref, is_mut, .. } => {
                                        if *is_ref {
                                            Type::Reference {
                                                mutable: *is_mut,
                                                lifetime: None,
                                                inner: alloc::boxed::Box::new(Type::TypeParam("Self".into())),
                                            }
                                        } else {
                                            Type::TypeParam("Self".into())
                                        }
                                    }
                                    FnParam::Typed { ty, .. } => self.resolve_ast_type(ty),
                                })
                                .collect();
                            let ret = f
                                .ret_type
                                .as_ref()
                                .map(|t| self.resolve_ast_type(t))
                                .unwrap_or(Type::Unit);
                            Some(TraitMethod {
                                name: f.name.clone(),
                                params,
                                ret,
                                has_default: f.body.is_some(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                self.traits.insert(
                    t.name.clone(),
                    TraitInfo {
                        name: t.name.clone(),
                        methods,
                        generic_params: t
                            .generics
                            .params
                            .iter()
                            .filter_map(|p| match p {
                                GenericParam::Type { name, .. } => Some(name.clone()),
                                _ => None,
                            })
                            .collect(),
                        supertraits: t.supertraits.iter().map(|b| b.path.name().into()).collect(),
                    },
                );
            }
            ItemKind::TypeAlias(ta) => {
                if let Some(ty) = &ta.ty {
                    let resolved = self.resolve_ast_type(ty);
                    self.type_defs.insert(ta.name.clone(), resolved);
                }
            }
            ItemKind::Const(c) => {
                let ty = self.resolve_ast_type(&c.ty);
                self.define_var(&c.name, ty);
            }
            ItemKind::Static(s) => {
                let ty = self.resolve_ast_type(&s.ty);
                self.define_var(&s.name, ty);
            }
            _ => {}
        }
    }

    fn check_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Function(f) => self.check_fn(f),
            ItemKind::Impl(imp) => {
                for item in &imp.items {
                    self.check_item(item);
                }
            }
            ItemKind::Trait(t) => {
                for item in &t.items {
                    self.check_item(item);
                }
            }
            ItemKind::Mod(ModDef::Loaded { items, .. }) => {
                for item in items {
                    self.register_item(item);
                }
                for item in items {
                    self.check_item(item);
                }
            }
            _ => {}
        }
    }

    fn check_fn(&mut self, f: &FnDef) {
        self.push_scope();

        let ret_type = f
            .ret_type
            .as_ref()
            .map(|t| self.resolve_ast_type(t))
            .unwrap_or(Type::Unit);
        self.current_return_type = Some(ret_type.clone());

        // Define parameters
        for param in &f.params {
            match param {
                FnParam::SelfParam { .. } => {
                    self.define_var("self", Type::TypeParam("Self".into()));
                }
                FnParam::Typed { pat, ty } => {
                    let resolved = self.resolve_ast_type(ty);
                    self.bind_pattern(pat, &resolved);
                }
            }
        }

        // Register function name for recursion
        let fn_type = Type::FnPtr {
            params: f
                .params
                .iter()
                .map(|p| match p {
                    FnParam::SelfParam { .. } => Type::TypeParam("Self".into()),
                    FnParam::Typed { ty, .. } => self.resolve_ast_type(ty),
                })
                .collect(),
            ret: alloc::boxed::Box::new(ret_type.clone()),
        };
        self.define_var(&f.name, fn_type);

        // Check body
        if let Some(body) = &f.body {
            let body_ty = self.check_block(body);
            if let Err(e) = self.unify(&body_ty, &ret_type) {
                self.error(format!("in fn {}: {}", f.name, e));
            }
        }

        self.current_return_type = None;
        self.pop_scope();
    }

    fn bind_pattern(&mut self, pat: &Pattern, ty: &Type) {
        match pat {
            Pattern::Ident { name, .. } => {
                self.define_var(name, ty.clone());
            }
            Pattern::Tuple(pats) => {
                if let Type::Tuple(ts) = ty {
                    for (p, t) in pats.iter().zip(ts.iter()) {
                        self.bind_pattern(p, t);
                    }
                }
            }
            Pattern::Ref { pat, .. } => {
                if let Type::Reference { inner, .. } = ty {
                    self.bind_pattern(pat, inner);
                }
            }
            Pattern::Wildcard | Pattern::Rest => {}
            _ => {}
        }
    }

    // ── Check blocks and statements ──────────────────────────────────

    fn check_block(&mut self, block: &Block) -> Type {
        self.push_scope();
        let mut last_ty = Type::Unit;

        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { pat, ty, init } => {
                    let declared = ty.as_ref().map(|t| self.resolve_ast_type(t));
                    let init_ty = init.as_ref().map(|e| self.check_expr(e));

                    let final_ty = match (declared, init_ty) {
                        (Some(d), Some(i)) => {
                            if let Err(e) = self.unify(&d, &i) {
                                self.error(e);
                            }
                            d
                        }
                        (Some(d), None) => d,
                        (None, Some(i)) => i,
                        (None, None) => self.fresh_infer(),
                    };
                    self.bind_pattern(pat, &final_ty);
                    last_ty = Type::Unit;
                }
                Stmt::Expr(expr) => {
                    self.check_expr(expr);
                    last_ty = Type::Unit;
                }
                Stmt::ExprNoSemi(expr) => {
                    last_ty = self.check_expr(expr);
                }
                Stmt::Item(item) => {
                    self.register_item(item);
                    self.check_item(item);
                    last_ty = Type::Unit;
                }
                Stmt::Semi => {
                    last_ty = Type::Unit;
                }
            }
        }

        self.pop_scope();
        last_ty
    }

    // ── Check expressions ────────────────────────────────────────────

    pub fn check_expr(&mut self, expr: &Expr) -> Type {
        match &expr.kind {
            ExprKind::IntLit(_) => Type::I32, // default integer type
            ExprKind::FloatLit(_) => Type::F64, // default float type
            ExprKind::StringLit(_) => Type::Reference {
                mutable: false,
                lifetime: Some("'static".into()),
                inner: alloc::boxed::Box::new(Type::Str),
            },
            ExprKind::CharLit(_) => Type::Char,
            ExprKind::BoolLit(_) => Type::Bool,
            ExprKind::ByteLit(_) => Type::U8,
            ExprKind::ByteStringLit(bs) => Type::Reference {
                mutable: false,
                lifetime: Some("'static".into()),
                inner: alloc::boxed::Box::new(Type::Array(
                    alloc::boxed::Box::new(Type::U8),
                    bs.len(),
                )),
            },

            ExprKind::Path(path) => {
                let name = path.name();
                if let Some(ty) = self.lookup_var(name) {
                    ty.clone()
                } else if let Some(ty) = self.type_defs.get(name) {
                    ty.clone()
                } else {
                    // Could be an enum variant
                    if path.segments.len() >= 2 {
                        let type_name = &path.segments[path.segments.len() - 2].ident;
                        if let Some(Type::Enum(e)) = self.type_defs.get(type_name.as_str()) {
                            if e.variants.iter().any(|v| v.name == name) {
                                return Type::Enum(e.clone());
                            }
                        }
                    }
                    self.error(format!("undefined: {}", name));
                    Type::Error
                }
            }

            ExprKind::Block(block) => self.check_block(block),

            ExprKind::Tuple(exprs) => {
                if exprs.is_empty() {
                    Type::Unit
                } else {
                    Type::Tuple(exprs.iter().map(|e| self.check_expr(e)).collect())
                }
            }

            ExprKind::Array(exprs) => {
                if exprs.is_empty() {
                    let elem = self.fresh_infer();
                    Type::Array(alloc::boxed::Box::new(elem), 0)
                } else {
                    let first = self.check_expr(&exprs[0]);
                    for e in &exprs[1..] {
                        let t = self.check_expr(e);
                        if let Err(err) = self.unify(&first, &t) {
                            self.error(err);
                        }
                    }
                    Type::Array(alloc::boxed::Box::new(first), exprs.len())
                }
            }

            ExprKind::ArrayRepeat { value, count: _ } => {
                let elem = self.check_expr(value);
                Type::Array(alloc::boxed::Box::new(elem), 0) // count evaluated later
            }

            ExprKind::Binary { op, lhs, rhs } => {
                let lt = self.check_expr(lhs);
                let rt = self.check_expr(rhs);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                        if let Err(e) = self.unify(&lt, &rt) {
                            self.error(e);
                        }
                        lt
                    }
                    BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                        if let Err(e) = self.unify(&lt, &rt) {
                            self.error(e);
                        }
                        lt
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        if let Err(e) = self.unify(&lt, &rt) {
                            self.error(e);
                        }
                        Type::Bool
                    }
                    BinOp::And | BinOp::Or => {
                        if let Err(e) = self.unify(&lt, &Type::Bool) {
                            self.error(e);
                        }
                        if let Err(e) = self.unify(&rt, &Type::Bool) {
                            self.error(e);
                        }
                        Type::Bool
                    }
                }
            }

            ExprKind::Unary { op, expr } => {
                let t = self.check_expr(expr);
                match op {
                    UnaryOp::Neg => {
                        if !t.is_numeric() && !matches!(t, Type::Infer(_) | Type::Error) {
                            self.error(format!("cannot negate {:?}", t));
                        }
                        t
                    }
                    UnaryOp::Not => {
                        if !t.is_bool() && !t.is_integer() && !matches!(t, Type::Infer(_) | Type::Error) {
                            self.error(format!("cannot apply ! to {:?}", t));
                        }
                        t
                    }
                    UnaryOp::Deref => {
                        match t {
                            Type::Reference { inner, .. } | Type::RawPtr { inner, .. } => *inner,
                            _ => {
                                self.error(format!("cannot deref {:?}", t));
                                Type::Error
                            }
                        }
                    }
                }
            }

            ExprKind::Cast { expr, ty } => {
                self.check_expr(expr);
                self.resolve_ast_type(ty)
            }

            ExprKind::Assign { lhs, rhs } => {
                let lt = self.check_expr(lhs);
                let rt = self.check_expr(rhs);
                if let Err(e) = self.unify(&lt, &rt) {
                    self.error(e);
                }
                Type::Unit
            }

            ExprKind::AssignOp { lhs, rhs, .. } => {
                let lt = self.check_expr(lhs);
                let rt = self.check_expr(rhs);
                if let Err(e) = self.unify(&lt, &rt) {
                    self.error(e);
                }
                Type::Unit
            }

            ExprKind::Field { expr, name } => {
                let t = self.check_expr(expr);
                self.resolve_field(&t, name)
            }

            ExprKind::TupleIndex { expr, index } => {
                let t = self.check_expr(expr);
                match t {
                    Type::Tuple(ts) => {
                        if (*index as usize) < ts.len() {
                            ts[*index as usize].clone()
                        } else {
                            self.error(format!("tuple index {} out of range", index));
                            Type::Error
                        }
                    }
                    _ => {
                        self.error(format!("tuple index on non-tuple {:?}", t));
                        Type::Error
                    }
                }
            }

            ExprKind::Index { expr, index } => {
                let t = self.check_expr(expr);
                let _idx = self.check_expr(index);
                match &t {
                    Type::Array(elem, _) | Type::Slice(elem) => (**elem).clone(),
                    Type::Generic { base, args } => {
                        if let Type::Struct(s) = &**base {
                            if s.name == "Vec" && !args.is_empty() {
                                return args[0].clone();
                            }
                        }
                        self.error(format!("cannot index {:?}", t));
                        Type::Error
                    }
                    _ => {
                        self.error(format!("cannot index {:?}", t));
                        Type::Error
                    }
                }
            }

            ExprKind::Call { func, args } => {
                let fn_ty = self.check_expr(func);
                let arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();

                match fn_ty {
                    Type::FnPtr { params, ret } => {
                        for (p, a) in params.iter().zip(arg_types.iter()) {
                            if let Err(e) = self.unify(p, a) {
                                self.error(e);
                            }
                        }
                        *ret
                    }
                    _ => {
                        // Constructor-like calls (e.g., Some(x))
                        self.fresh_infer()
                    }
                }
            }

            ExprKind::MethodCall { receiver, method, args, .. } => {
                let recv_ty = self.check_expr(receiver);
                let _arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();
                self.resolve_method(&recv_ty, method)
            }

            ExprKind::If { cond, then_block, else_expr } => {
                let cond_ty = self.check_expr(cond);
                if !matches!(cond_ty, Type::Bool | Type::Infer(_) | Type::Error)
                    && !matches!(&cond.kind, ExprKind::Let { .. })
                {
                    self.error(format!("if condition must be bool, got {:?}", cond_ty));
                }
                let then_ty = self.check_block(then_block);
                if let Some(else_e) = else_expr {
                    let else_ty = self.check_expr(else_e);
                    match self.unify(&then_ty, &else_ty) {
                        Ok(t) => t,
                        Err(e) => {
                            self.error(e);
                            then_ty
                        }
                    }
                } else {
                    Type::Unit
                }
            }

            ExprKind::Match { expr, arms } => {
                let _scrutinee = self.check_expr(expr);
                let mut result_ty: Option<Type> = None;
                for arm in arms {
                    self.push_scope();
                    // Bind pattern variables (simplified)
                    let body_ty = self.check_expr(&arm.body);
                    if let Some(ref prev) = result_ty {
                        if let Err(e) = self.unify(prev, &body_ty) {
                            self.error(e);
                        }
                    } else {
                        result_ty = Some(body_ty);
                    }
                    self.pop_scope();
                }
                result_ty.unwrap_or(Type::Unit)
            }

            ExprKind::Loop { body, .. } => {
                self.check_block(body);
                // Loop type is determined by break expressions
                self.fresh_infer()
            }

            ExprKind::While { cond, body, .. } => {
                self.check_expr(cond);
                self.check_block(body);
                Type::Unit
            }

            ExprKind::For { pat, iter, body, .. } => {
                let iter_ty = self.check_expr(iter);
                // Simplified: assume iterator yields the inner type
                let elem_ty = self.infer_iter_element(&iter_ty);
                self.push_scope();
                self.bind_pattern(pat, &elem_ty);
                self.check_block(body);
                self.pop_scope();
                Type::Unit
            }

            ExprKind::Return(val) => {
                let val_ty = val.as_ref().map(|e| self.check_expr(e)).unwrap_or(Type::Unit);
                let ret = self.current_return_type.clone();
                if let Some(ret) = ret {
                    if let Err(e) = self.unify(&ret, &val_ty) {
                        self.error(format!("return type mismatch: {}", e));
                    }
                }
                Type::Never
            }

            ExprKind::Break { value, .. } => {
                if let Some(val) = value {
                    self.check_expr(val);
                }
                Type::Never
            }

            ExprKind::Continue { .. } => Type::Never,

            ExprKind::Closure { params, ret_type, body, .. } => {
                self.push_scope();
                let param_types: Vec<Type> = params
                    .iter()
                    .map(|p| {
                        let ty = p.ty.as_ref().map(|t| self.resolve_ast_type(t)).unwrap_or_else(|| self.fresh_infer());
                        self.bind_pattern(&p.pat, &ty);
                        ty
                    })
                    .collect();
                let body_ty = self.check_expr(body);
                let ret = ret_type
                    .as_ref()
                    .map(|t| self.resolve_ast_type(t))
                    .unwrap_or(body_ty);
                self.pop_scope();
                Type::FnPtr {
                    params: param_types,
                    ret: alloc::boxed::Box::new(ret),
                }
            }

            ExprKind::Ref { is_mut, expr } => {
                let inner = self.check_expr(expr);
                Type::Reference {
                    mutable: *is_mut,
                    lifetime: None,
                    inner: alloc::boxed::Box::new(inner),
                }
            }

            ExprKind::Deref(expr) => {
                let t = self.check_expr(expr);
                match t {
                    Type::Reference { inner, .. } | Type::RawPtr { inner, .. } => *inner,
                    _ => {
                        self.error(format!("cannot deref {:?}", t));
                        Type::Error
                    }
                }
            }

            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.check_expr(s); }
                if let Some(e) = end { self.check_expr(e); }
                // Range<T> - simplified
                Type::Struct(StructType {
                    name: "Range".into(),
                    fields: Vec::new(),
                    generic_params: Vec::new(),
                })
            }

            ExprKind::StructLit { path, fields, rest } => {
                let type_name = path.name();
                let struct_ty = self.type_defs.get(type_name).cloned();
                if let Some(Type::Struct(s)) = &struct_ty {
                    for field in fields {
                        let val_ty = self.check_expr(&field.value);
                        if let Some(sf) = s.fields.iter().find(|f| f.name == field.name) {
                            if let Err(e) = self.unify(&sf.ty, &val_ty) {
                                self.error(format!("field {}: {}", field.name, e));
                            }
                        }
                    }
                    if let Some(r) = rest {
                        self.check_expr(r);
                    }
                    struct_ty.unwrap()
                } else {
                    self.error(format!("unknown struct: {}", type_name));
                    Type::Error
                }
            }

            ExprKind::Try(expr) => {
                let t = self.check_expr(expr);
                // Simplified: Result<T, E>.? returns T
                self.fresh_infer()
            }

            ExprKind::Macro { path, .. } => {
                let name = path.name();
                match name {
                    "println" | "print" | "eprintln" | "eprint" | "write" | "writeln" => Type::Unit,
                    "format" => Type::Struct(StructType {
                        name: "String".into(),
                        fields: Vec::new(),
                        generic_params: Vec::new(),
                    }),
                    "vec" => {
                        let elem = self.fresh_infer();
                        Type::Generic {
                            base: alloc::boxed::Box::new(
                                self.type_defs.get("Vec").cloned().unwrap_or(Type::Error),
                            ),
                            args: vec![elem],
                        }
                    }
                    "panic" | "unreachable" | "unimplemented" | "todo" => Type::Never,
                    "assert" | "assert_eq" | "assert_ne" | "debug_assert" => Type::Unit,
                    _ => self.fresh_infer(),
                }
            }

            ExprKind::Await(expr) => {
                self.check_expr(expr);
                self.fresh_infer() // Future<Output = T> -> T
            }

            ExprKind::Unsafe(block) => self.check_block(block),

            ExprKind::Let { pat, expr } => {
                let t = self.check_expr(expr);
                self.bind_pattern(pat, &t);
                Type::Bool // let expressions evaluate to bool in if-let/while-let
            }
        }
    }

    // ── Field and method resolution ──────────────────────────────────

    fn resolve_field(&mut self, ty: &Type, name: &str) -> Type {
        match ty {
            Type::Struct(s) => {
                if let Some(f) = s.fields.iter().find(|f| f.name == name) {
                    f.ty.clone()
                } else {
                    self.error(format!("no field {} on struct {}", name, s.name));
                    Type::Error
                }
            }
            Type::Generic { base, args } => {
                if let Type::Struct(s) = &**base {
                    if let Some(f) = s.fields.iter().find(|f| f.name == name) {
                        // Substitute generic params
                        self.substitute(&f.ty, &s.generic_params, args)
                    } else {
                        self.error(format!("no field {} on {}", name, s.name));
                        Type::Error
                    }
                } else {
                    self.error(format!("field access on non-struct {:?}", ty));
                    Type::Error
                }
            }
            Type::Reference { inner, .. } => self.resolve_field(inner, name),
            _ => {
                self.error(format!("no field {} on {:?}", name, ty));
                Type::Error
            }
        }
    }

    fn resolve_method(&mut self, ty: &Type, name: &str) -> Type {
        // Check impls
        let type_name = self.type_name(ty);
        if let Some(impls) = self.impls.get(&type_name).cloned() {
            for imp in &impls {
                if let Some(m) = imp.methods.iter().find(|m| m.name == name) {
                    return m.ret.clone();
                }
            }
        }

        // Well-known methods
        match name {
            "len" => return Type::Usize,
            "is_empty" => return Type::Bool,
            "push" | "pop" | "clear" | "remove" | "insert" => return Type::Unit,
            "clone" => return ty.clone(),
            "to_string" => return Type::Struct(StructType {
                name: "String".into(),
                fields: Vec::new(),
                generic_params: Vec::new(),
            }),
            "iter" | "iter_mut" | "into_iter" | "map" | "filter"
            | "enumerate" | "zip" | "take" | "skip" | "chain"
            | "flat_map" | "filter_map" | "inspect" | "peekable" => {
                return self.fresh_infer(); // iterator adapter
            }
            "collect" | "fold" | "reduce" => return self.fresh_infer(),
            "unwrap" | "expect" | "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" => {
                return self.fresh_infer();
            }
            "ok" | "err" | "map_err" | "and_then" | "or_else" => {
                return self.fresh_infer();
            }
            "is_some" | "is_none" | "is_ok" | "is_err" | "contains" => {
                return Type::Bool;
            }
            "as_ref" | "as_mut" | "as_slice" | "as_str" | "as_bytes" => {
                return self.fresh_infer();
            }
            "get" | "get_mut" | "first" | "last" => return self.fresh_infer(),
            "sort" | "sort_by" | "reverse" | "dedup" | "retain" | "truncate" | "resize" | "extend" => {
                return Type::Unit;
            }
            "split" | "lines" | "chars" | "bytes" | "trim" | "starts_with" | "ends_with"
            | "replace" | "to_uppercase" | "to_lowercase" | "find" | "rfind" => {
                return self.fresh_infer();
            }
            "with_capacity" | "new" | "default" => return self.fresh_infer(),
            _ => {}
        }

        // Auto-deref through references
        if let Type::Reference { inner, .. } = ty {
            return self.resolve_method(inner, name);
        }

        self.fresh_infer() // fallback - allow unknown methods
    }

    fn substitute(&self, ty: &Type, params: &[String], args: &[Type]) -> Type {
        match ty {
            Type::TypeParam(name) => {
                if let Some(idx) = params.iter().position(|p| p == name) {
                    if idx < args.len() {
                        return args[idx].clone();
                    }
                }
                ty.clone()
            }
            Type::Reference { mutable, lifetime, inner } => Type::Reference {
                mutable: *mutable,
                lifetime: lifetime.clone(),
                inner: alloc::boxed::Box::new(self.substitute(inner, params, args)),
            },
            Type::Array(elem, n) => Type::Array(
                alloc::boxed::Box::new(self.substitute(elem, params, args)),
                *n,
            ),
            Type::Tuple(ts) => Type::Tuple(
                ts.iter().map(|t| self.substitute(t, params, args)).collect(),
            ),
            _ => ty.clone(),
        }
    }

    fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Struct(s) => s.name.clone(),
            Type::Enum(e) => e.name.clone(),
            Type::Generic { base, .. } => self.type_name(base),
            Type::Reference { inner, .. } => self.type_name(inner),
            _ => format!("{:?}", ty),
        }
    }

    fn infer_iter_element(&self, ty: &Type) -> Type {
        match ty {
            Type::Array(elem, _) | Type::Slice(elem) => (**elem).clone(),
            Type::Generic { base, args } => {
                if let Type::Struct(s) = &**base {
                    if s.name == "Vec" && !args.is_empty() {
                        return args[0].clone();
                    }
                }
                Type::Infer(self.infer_counter)
            }
            _ => Type::Infer(self.infer_counter),
        }
    }

    /// Get the list of type checking errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn check(src: &str) -> TypeEnv {
        let tokens = Lexer::tokenize(src).unwrap();
        let file = Parser::parse_file(tokens).unwrap();
        let mut env = TypeEnv::new();
        env.check_file(&file);
        env
    }

    #[test]
    fn simple_fn_types() {
        let env = check("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(env.errors.is_empty(), "errors: {:?}", env.errors);
    }

    #[test]
    fn type_mismatch() {
        let env = check("fn bad() -> bool { 42 }");
        assert!(!env.errors.is_empty());
    }

    #[test]
    fn let_binding_inference() {
        let env = check("fn foo() { let x = 42; let y = x + 1; }");
        assert!(env.errors.is_empty(), "errors: {:?}", env.errors);
    }

    #[test]
    fn struct_field_access() {
        let env = check(r#"
            struct Point { x: f64, y: f64 }
            fn dist(p: Point) -> f64 { p.x }
        "#);
        assert!(env.errors.is_empty(), "errors: {:?}", env.errors);
    }

    #[test]
    fn if_else_type() {
        let env = check("fn foo(x: bool) -> i32 { if x { 1 } else { 2 } }");
        assert!(env.errors.is_empty(), "errors: {:?}", env.errors);
    }
}
