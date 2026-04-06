//! Complete Rust AST for ClaudioOS rustc-lite.
//!
//! Covers: items (fn, struct, enum, impl, trait, use, mod, const, static,
//! type alias), expressions, statements, patterns, types, generics, where
//! clauses, visibility, attributes, and macro invocations.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// ─── Top-level ───────────────────────────────────────────────────────────

/// A complete source file / compilation unit.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub items: Vec<Item>,
}

// ─── Visibility ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Private,
    Pub,
    PubCrate,
    PubSuper,
}

// ─── Attributes ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Attribute {
    pub path: Path,
    pub args: Option<String>, // raw token stream inside parens
}

// ─── Items ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Item {
    pub vis: Visibility,
    pub attrs: Vec<Attribute>,
    pub kind: ItemKind,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    Function(FnDef),
    Struct(StructDef),
    Enum(EnumDef),
    Impl(ImplBlock),
    Trait(TraitDef),
    TypeAlias(TypeAlias),
    Const(ConstDef),
    Static(StaticDef),
    Use(UsePath),
    Mod(ModDef),
    ExternBlock(ExternBlock),
}

// ─── Function ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub generics: Generics,
    pub params: Vec<FnParam>,
    pub ret_type: Option<Ty>,
    pub where_clause: WhereClause,
    pub body: Option<Block>, // None for trait method declarations
    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_const: bool,
    pub abi: Option<String>, // extern "C"
}

#[derive(Debug, Clone)]
pub enum FnParam {
    SelfParam {
        is_ref: bool,
        is_mut: bool,
        lifetime: Option<String>,
    },
    Typed {
        pat: Pattern,
        ty: Ty,
    },
}

// ─── Struct ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub generics: Generics,
    pub where_clause: WhereClause,
    pub kind: StructKind,
}

#[derive(Debug, Clone)]
pub enum StructKind {
    Named(Vec<FieldDef>),      // struct Foo { x: i32 }
    Tuple(Vec<TupleFieldDef>), // struct Foo(i32, i32);
    Unit,                      // struct Foo;
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub vis: Visibility,
    pub name: String,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct TupleFieldDef {
    pub vis: Visibility,
    pub ty: Ty,
}

// ─── Enum ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub generics: Generics,
    pub where_clause: WhereClause,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub kind: VariantKind,
    pub discriminant: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum VariantKind {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<FieldDef>),
}

// ─── Impl ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub generics: Generics,
    pub trait_path: Option<Path>, // impl Trait for Type
    pub self_ty: Ty,
    pub where_clause: WhereClause,
    pub items: Vec<Item>,
    pub is_unsafe: bool,
    pub is_negative: bool, // impl !Trait for Type
}

// ─── Trait ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub name: String,
    pub generics: Generics,
    pub where_clause: WhereClause,
    pub supertraits: Vec<TraitBound>,
    pub items: Vec<Item>,
    pub is_unsafe: bool,
    pub is_auto: bool,
}

// ─── Type alias ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub name: String,
    pub generics: Generics,
    pub where_clause: WhereClause,
    pub ty: Option<Ty>, // None for associated types in traits
}

// ─── Const / Static ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct StaticDef {
    pub name: String,
    pub ty: Ty,
    pub value: Option<Expr>,
    pub is_mut: bool,
}

// ─── Use ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum UsePath {
    Simple(Path, Option<String>),       // use a::b::c as d;
    Glob(Path),                         // use a::b::*;
    Group(Path, Vec<UsePath>),          // use a::b::{c, d};
}

// ─── Mod ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ModDef {
    Loaded { name: String, items: Vec<Item> },
    Unloaded { name: String }, // mod foo; (external file)
}

// ─── Extern block ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExternBlock {
    pub abi: Option<String>,
    pub items: Vec<Item>,
}

// ─── Generics ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Generics {
    pub params: Vec<GenericParam>,
}

#[derive(Debug, Clone)]
pub enum GenericParam {
    Type {
        name: String,
        bounds: Vec<TraitBound>,
        default: Option<Ty>,
    },
    Lifetime {
        name: String,
        bounds: Vec<String>, // lifetime bounds: 'a: 'b + 'c
    },
    Const {
        name: String,
        ty: Ty,
        default: Option<Expr>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct WhereClause {
    pub predicates: Vec<WherePredicate>,
}

#[derive(Debug, Clone)]
pub enum WherePredicate {
    TypeBound {
        ty: Ty,
        bounds: Vec<TraitBound>,
    },
    LifetimeBound {
        lifetime: String,
        bounds: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct TraitBound {
    pub path: Path,
    pub generics: Vec<GenericArg>,
    pub is_maybe: bool, // ?Sized
}

#[derive(Debug, Clone)]
pub enum GenericArg {
    Type(Ty),
    Lifetime(String),
    Const(Expr),
    Binding { name: String, ty: Ty }, // Item = Foo
}

// ─── Paths ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Path {
    pub segments: Vec<PathSegment>,
    pub is_global: bool, // ::std::...
}

impl Path {
    pub fn simple(name: &str) -> Self {
        Path {
            segments: alloc::vec![PathSegment {
                ident: String::from(name),
                generics: Vec::new(),
            }],
            is_global: false,
        }
    }

    pub fn name(&self) -> &str {
        self.segments.last().map(|s| s.ident.as_str()).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct PathSegment {
    pub ident: String,
    pub generics: Vec<GenericArg>, // turbofish ::<T>
}

// ─── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Ty {
    Path(Path),                                // i32, Vec<T>, std::io::Error
    Reference { lifetime: Option<String>, is_mut: bool, inner: Box<Ty> },
    Slice(Box<Ty>),                            // [T]
    Array(Box<Ty>, Box<Expr>),                 // [T; N]
    Tuple(Vec<Ty>),                            // (A, B, C)
    Fn { params: Vec<Ty>, ret: Option<Box<Ty>> }, // fn(i32) -> bool
    Never,                                     // !
    Infer,                                     // _
    RawPtr { is_mut: bool, inner: Box<Ty> },   // *const T / *mut T
    ImplTrait(Vec<TraitBound>),                // impl Trait
    DynTrait(Vec<TraitBound>),                 // dyn Trait
    SelfType,                                  // Self
}

// ─── Expressions ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
}

impl Expr {
    pub fn new(kind: ExprKind) -> Self {
        Expr { kind }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    // Literals
    IntLit(i128),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),
    BoolLit(bool),
    ByteLit(u8),
    ByteStringLit(Vec<u8>),

    // Paths and identifiers
    Path(Path),

    // Compound
    Block(Block),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    ArrayRepeat { value: Box<Expr>, count: Box<Expr> }, // [0; 10]

    // Operations
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnaryOp, expr: Box<Expr> },
    Cast { expr: Box<Expr>, ty: Ty },       // expr as Type
    Assign { lhs: Box<Expr>, rhs: Box<Expr> },
    AssignOp { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> }, // +=, -=, etc.

    // Access
    Field { expr: Box<Expr>, name: String },
    TupleIndex { expr: Box<Expr>, index: u32 },
    Index { expr: Box<Expr>, index: Box<Expr> },
    Call { func: Box<Expr>, args: Vec<Expr> },
    MethodCall { receiver: Box<Expr>, method: String, generics: Vec<GenericArg>, args: Vec<Expr> },

    // Control flow
    If { cond: Box<Expr>, then_block: Block, else_expr: Option<Box<Expr>> },
    Match { expr: Box<Expr>, arms: Vec<MatchArm> },
    Loop { body: Block, label: Option<String> },
    While { cond: Box<Expr>, body: Block, label: Option<String> },
    For { pat: Pattern, iter: Box<Expr>, body: Block, label: Option<String> },
    Break { label: Option<String>, value: Option<Box<Expr>> },
    Continue { label: Option<String> },
    Return(Option<Box<Expr>>),

    // Closures
    Closure {
        params: Vec<ClosureParam>,
        ret_type: Option<Ty>,
        body: Box<Expr>,
        is_move: bool,
        is_async: bool,
    },

    // References
    Ref { is_mut: bool, expr: Box<Expr> },   // &expr, &mut expr
    Deref(Box<Expr>),                         // *expr

    // Range
    Range { start: Option<Box<Expr>>, end: Option<Box<Expr>>, inclusive: bool },

    // Struct literal
    StructLit {
        path: Path,
        fields: Vec<StructLitField>,
        rest: Option<Box<Expr>>, // ..other
    },

    // Try operator
    Try(Box<Expr>), // expr?

    // Macro invocation (simplified)
    Macro { path: Path, args: String },

    // Await
    Await(Box<Expr>), // expr.await

    // Unsafe block
    Unsafe(Block),

    // Let expression (if let, while let)
    Let { pat: Pattern, expr: Box<Expr> },
}

#[derive(Debug, Clone)]
pub struct StructLitField {
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct ClosureParam {
    pub pat: Pattern,
    pub ty: Option<Ty>,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

// ─── Binary + Unary operators ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    BitAnd, BitOr, BitXor, Shl, Shr,
    And, Or,
    Eq, Ne, Lt, Gt, Le, Ge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,   // -
    Not,   // !
    Deref, // *
}

// ─── Patterns ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,                                    // _
    Ident { name: String, is_mut: bool, is_ref: bool, binding: Option<Box<Pattern>> },
    Lit(Box<Expr>),                              // 42, "hello", true
    Tuple(Vec<Pattern>),                         // (a, b, c)
    Struct { path: Path, fields: Vec<FieldPat>, rest: bool },
    TupleStruct { path: Path, fields: Vec<Pattern> },
    Path(Path),                                  // Enum::Variant
    Ref { is_mut: bool, pat: Box<Pattern> },     // &pat, &mut pat
    Slice(Vec<Pattern>),                         // [a, b, ..]
    Range { start: Option<Box<Expr>>, end: Option<Box<Expr>>, inclusive: bool },
    Or(Vec<Pattern>),                            // pat1 | pat2
    Rest,                                        // ..
}

#[derive(Debug, Clone)]
pub struct FieldPat {
    pub name: String,
    pub pat: Pattern,
    pub is_shorthand: bool, // Foo { x } vs Foo { x: pat }
}

// ─── Statements ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        pat: Pattern,
        ty: Option<Ty>,
        init: Option<Expr>,
    },
    Expr(Expr),       // expression with trailing semicolon
    ExprNoSemi(Expr), // expression without semi (tail expression)
    Item(Item),       // nested item (fn, struct, etc.)
    Semi,             // bare semicolon
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

impl Block {
    pub fn empty() -> Self {
        Block { stmts: Vec::new() }
    }
}
