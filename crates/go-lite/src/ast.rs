//! Go AST node types.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::Span;
use crate::types::GoType;

/// A complete Go source file.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub imports: Vec<ImportDecl>,
    pub decls: Vec<TopLevelDecl>,
    pub span: Span,
}

/// Import declaration.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub alias: Option<String>,
    pub path: String,
    pub span: Span,
}

/// Top-level declaration.
#[derive(Debug, Clone)]
pub enum TopLevelDecl {
    Func(FuncDecl),
    Var(VarDecl),
    Const(ConstDecl),
    Type(TypeDecl),
}

/// Function declaration.
#[derive(Debug, Clone)]
pub struct FuncDecl {
    pub name: String,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub returns: Vec<GoType>,
    pub body: Block,
    pub is_variadic: bool,
    pub span: Span,
}

/// Method receiver.
#[derive(Debug, Clone)]
pub struct Receiver {
    pub name: String,
    pub ty: GoType,
    pub is_pointer: bool,
}

/// Function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: GoType,
}

/// Variable declaration.
#[derive(Debug, Clone)]
pub struct VarDecl {
    pub names: Vec<String>,
    pub ty: Option<GoType>,
    pub values: Vec<Expr>,
    pub span: Span,
}

/// Constant declaration.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub names: Vec<String>,
    pub ty: Option<GoType>,
    pub values: Vec<Expr>,
    pub span: Span,
}

/// Type declaration.
#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub ty: TypeDef,
    pub span: Span,
}

/// Type definition body.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Alias(GoType),
    Struct(StructType),
    Interface(InterfaceType),
}

/// Struct type definition.
#[derive(Debug, Clone)]
pub struct StructType {
    pub fields: Vec<StructField>,
}

/// Struct field.
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: GoType,
    pub tag: Option<String>,
}

/// Interface type definition.
#[derive(Debug, Clone)]
pub struct InterfaceType {
    pub methods: Vec<InterfaceMethod>,
    pub embedded: Vec<String>,
}

/// Interface method signature.
#[derive(Debug, Clone)]
pub struct InterfaceMethod {
    pub name: String,
    pub params: Vec<GoType>,
    pub returns: Vec<GoType>,
}

/// A block of statements.
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

/// Statement node.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// Variable declaration: var x int = 5
    VarDecl(VarDecl),
    /// Short variable declaration: x := 5
    ShortDecl {
        names: Vec<String>,
        values: Vec<Expr>,
    },
    /// Assignment: x = 5, x += 1
    Assign {
        op: AssignOp,
        lhs: Vec<Expr>,
        rhs: Vec<Expr>,
    },
    /// Expression statement
    Expr(Expr),
    /// Return statement
    Return(Vec<Expr>),
    /// If statement
    If {
        init: Option<Box<Stmt>>,
        cond: Expr,
        body: Block,
        else_body: Option<ElseClause>,
    },
    /// For loop (all three forms)
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        post: Option<Box<Stmt>>,
        body: Block,
    },
    /// For-range loop
    ForRange {
        key: Option<String>,
        value: Option<String>,
        iter: Expr,
        body: Block,
        is_assign: bool,
    },
    /// Switch statement
    Switch {
        init: Option<Box<Stmt>>,
        tag: Option<Expr>,
        cases: Vec<SwitchCase>,
    },
    /// Select statement
    Select {
        cases: Vec<SelectCase>,
    },
    /// Go statement: go f()
    Go(Expr),
    /// Defer statement: defer f()
    Defer(Expr),
    /// Send statement: ch <- val
    Send {
        channel: Expr,
        value: Expr,
    },
    /// Block statement
    Block(Block),
    /// Break
    Break(Option<String>),
    /// Continue
    Continue(Option<String>),
    /// Goto
    Goto(String),
    /// Label
    Label(String, Box<Stmt>),
    /// Fallthrough
    Fallthrough,
    /// Increment: x++
    Inc(Expr),
    /// Decrement: x--
    Dec(Expr),
    /// Empty statement
    Empty,
}

/// Else clause.
#[derive(Debug, Clone)]
pub enum ElseClause {
    Block(Block),
    If(Box<Stmt>),
}

/// Switch case.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub exprs: Vec<Expr>,
    pub is_default: bool,
    pub body: Vec<Stmt>,
}

/// Select case.
#[derive(Debug, Clone)]
pub struct SelectCase {
    pub comm: Option<Stmt>,
    pub is_default: bool,
    pub body: Vec<Stmt>,
}

/// Assignment operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Assign,       // =
    AddAssign,    // +=
    SubAssign,    // -=
    MulAssign,    // *=
    DivAssign,    // /=
    ModAssign,    // %=
    AndAssign,    // &=
    OrAssign,     // |=
    XorAssign,    // ^=
    ShlAssign,    // <<=
    ShrAssign,    // >>=
    AndNotAssign, // &^=
}

/// Expression node.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer literal
    IntLit(i64),
    /// Float literal
    FloatLit(f64),
    /// String literal
    StringLit(String),
    /// Rune literal
    RuneLit(char),
    /// Bool literal
    BoolLit(bool),
    /// nil
    Nil,
    /// Identifier
    Ident(String),

    /// Binary operation
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Unary operation
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    /// Function call: f(args...)
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Index expression: a[i]
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    /// Slice expression: a[low:high] or a[low:high:max]
    Slice {
        expr: Box<Expr>,
        low: Option<Box<Expr>>,
        high: Option<Box<Expr>>,
        max: Option<Box<Expr>>,
    },
    /// Selector expression: x.y
    Selector {
        expr: Box<Expr>,
        field: String,
    },
    /// Type assertion: x.(Type)
    TypeAssert {
        expr: Box<Expr>,
        ty: GoType,
    },
    /// Channel receive: <-ch
    Receive(Box<Expr>),

    /// Composite literal: Type{...}
    CompositeLit {
        ty: GoType,
        elts: Vec<KeyValue>,
    },
    /// Function literal (closure)
    FuncLit {
        params: Vec<Param>,
        returns: Vec<GoType>,
        body: Block,
    },

    /// make(type, args...)
    Make {
        ty: GoType,
        args: Vec<Expr>,
    },
    /// new(type)
    New(GoType),

    /// Address-of: &x
    AddrOf(Box<Expr>),
    /// Dereference: *x
    Deref(Box<Expr>),

    /// Type conversion: int(x)
    Convert {
        ty: GoType,
        expr: Box<Expr>,
    },
}

/// Key-value pair in composite literals.
#[derive(Debug, Clone)]
pub struct KeyValue {
    pub key: Option<Expr>,
    pub value: Expr,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    BitAnd, BitOr, BitXor, AndNot,
    Shl, Shr,
    Eq, Ne, Lt, Le, Gt, Ge,
    LogAnd, LogOr,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,      // -
    BitNot,   // ^
    LogNot,   // !
}
