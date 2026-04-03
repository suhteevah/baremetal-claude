//! C++ AST extensions over C AST.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::Span;

/// C++ translation unit.
#[derive(Debug, Clone)]
pub struct CppTranslationUnit {
    pub decls: Vec<CppDecl>,
}

/// Top-level C++ declaration.
#[derive(Debug, Clone)]
pub enum CppDecl {
    /// Class or struct definition
    ClassDef(ClassDef),
    /// Function definition (free or friend)
    FuncDef(CppFuncDef),
    /// Namespace
    Namespace(NamespaceDef),
    /// Template declaration
    Template(TemplateDef),
    /// Using declaration/directive
    Using(UsingDecl),
    /// Variable declaration
    VarDecl(CppVarDecl),
    /// Type alias: using Name = Type
    TypeAlias { name: String, ty: CppType },
    /// Forward declaration
    ForwardDecl { name: String },
}

/// Class/struct definition with C++ features.
#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: String,
    pub is_struct: bool, // struct vs class (default access)
    pub bases: Vec<BaseClass>,
    pub members: Vec<ClassMember>,
    pub is_final: bool,
    pub span: Span,
}

/// Base class specification.
#[derive(Debug, Clone)]
pub struct BaseClass {
    pub name: String,
    pub access: AccessSpec,
    pub is_virtual: bool,
}

/// Access specifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessSpec {
    Public,
    Protected,
    Private,
}

/// Class member.
#[derive(Debug, Clone)]
pub enum ClassMember {
    /// Access specifier change
    Access(AccessSpec),
    /// Method definition
    Method(MethodDef),
    /// Constructor
    Constructor(Constructor),
    /// Destructor
    Destructor(Destructor),
    /// Field
    Field(CppVarDecl),
    /// Nested class
    NestedClass(ClassDef),
    /// Friend declaration
    Friend(String),
    /// Operator overload
    OperatorOverload(OperatorDef),
}

/// Method definition.
#[derive(Debug, Clone)]
pub struct MethodDef {
    pub name: String,
    pub return_type: CppType,
    pub params: Vec<CppParam>,
    pub body: Option<CppBlock>,
    pub is_virtual: bool,
    pub is_override: bool,
    pub is_final: bool,
    pub is_static: bool,
    pub is_const: bool,
    pub is_pure_virtual: bool, // = 0
    pub is_noexcept: bool,
    pub access: AccessSpec,
    pub span: Span,
}

/// Constructor.
#[derive(Debug, Clone)]
pub struct Constructor {
    pub class_name: String,
    pub params: Vec<CppParam>,
    pub init_list: Vec<MemberInit>,
    pub body: CppBlock,
    pub is_explicit: bool,
    pub span: Span,
}

/// Destructor.
#[derive(Debug, Clone)]
pub struct Destructor {
    pub class_name: String,
    pub body: CppBlock,
    pub is_virtual: bool,
    pub span: Span,
}

/// Member initializer in constructor.
#[derive(Debug, Clone)]
pub struct MemberInit {
    pub name: String,
    pub args: Vec<CppExpr>,
}

/// Namespace definition.
#[derive(Debug, Clone)]
pub struct NamespaceDef {
    pub name: String,
    pub decls: Vec<CppDecl>,
    pub span: Span,
}

/// Template declaration.
#[derive(Debug, Clone)]
pub struct TemplateDef {
    pub params: Vec<TemplateParam>,
    pub decl: Box<CppDecl>,
    pub span: Span,
}

/// Template parameter.
#[derive(Debug, Clone)]
pub struct TemplateParam {
    pub name: String,
    pub is_typename: bool, // typename vs class
    pub default: Option<CppType>,
}

/// Using declaration.
#[derive(Debug, Clone)]
pub struct UsingDecl {
    pub path: String,
    pub is_namespace: bool, // using namespace X
    pub span: Span,
}

/// Operator overload definition.
#[derive(Debug, Clone)]
pub struct OperatorDef {
    pub op: String, // "+", "<<", "==", etc.
    pub return_type: CppType,
    pub params: Vec<CppParam>,
    pub body: CppBlock,
    pub is_friend: bool,
    pub span: Span,
}

/// C++ function definition.
#[derive(Debug, Clone)]
pub struct CppFuncDef {
    pub name: String,
    pub qualified_name: Option<String>, // Namespace::Class::method
    pub return_type: CppType,
    pub params: Vec<CppParam>,
    pub body: CppBlock,
    pub is_constexpr: bool,
    pub is_noexcept: bool,
    pub template_params: Vec<TemplateParam>,
    pub span: Span,
}

/// C++ parameter.
#[derive(Debug, Clone)]
pub struct CppParam {
    pub name: Option<String>,
    pub ty: CppType,
    pub default_value: Option<CppExpr>,
    pub is_const: bool,
}

/// C++ variable declaration.
#[derive(Debug, Clone)]
pub struct CppVarDecl {
    pub name: String,
    pub ty: CppType,
    pub init: Option<CppExpr>,
    pub is_static: bool,
    pub is_const: bool,
    pub is_constexpr: bool,
    pub is_mutable: bool,
    pub access: AccessSpec,
    pub span: Span,
}

/// C++ type representation.
#[derive(Debug, Clone, PartialEq)]
pub enum CppType {
    Void,
    Bool,
    Char,
    Int,
    Long,
    LongLong,
    Float,
    Double,
    Auto,        // type deduction
    Nullptr,     // std::nullptr_t
    Pointer(Box<CppType>),
    Reference(Box<CppType>),
    RvalueRef(Box<CppType>),  // T&&
    Const(Box<CppType>),
    Named(String),
    Qualified(String, String), // Ns::Type
    Template { name: String, args: Vec<CppType> }, // T<U, V>
    Array(Box<CppType>, Option<usize>),
    FuncPtr { ret: Box<CppType>, params: Vec<CppType> },
}

/// Block of statements.
#[derive(Debug, Clone)]
pub struct CppBlock {
    pub stmts: Vec<CppStmt>,
}

/// C++ statement.
#[derive(Debug, Clone)]
pub enum CppStmt {
    Expr(CppExpr),
    Return(Option<CppExpr>),
    VarDecl(CppVarDecl),
    If { cond: CppExpr, then_body: Box<CppStmt>, else_body: Option<Box<CppStmt>> },
    While { cond: CppExpr, body: Box<CppStmt> },
    For { init: Option<Box<CppStmt>>, cond: Option<CppExpr>, incr: Option<CppExpr>, body: Box<CppStmt> },
    RangeFor { var_name: String, var_type: CppType, range: CppExpr, body: Box<CppStmt> },
    Switch { expr: CppExpr, cases: Vec<(Option<CppExpr>, Vec<CppStmt>)> },
    Block(CppBlock),
    Break,
    Continue,
    TryCatch { try_body: CppBlock, catches: Vec<CatchClause> },
    Throw(CppExpr),
    Delete(CppExpr),
    DeleteArray(CppExpr),
    Empty,
}

/// Catch clause.
#[derive(Debug, Clone)]
pub struct CatchClause {
    pub param_name: Option<String>,
    pub param_type: CppType,
    pub body: CppBlock,
}

/// C++ expression.
#[derive(Debug, Clone)]
pub enum CppExpr {
    IntLit(i64),
    FloatLit(f64),
    CharLit(u8),
    StringLit(Vec<u8>),
    BoolLit(bool),
    Nullptr,
    Ident(String),
    This,

    Binary { op: CppBinOp, lhs: Box<CppExpr>, rhs: Box<CppExpr> },
    Unary { op: CppUnaryOp, operand: Box<CppExpr> },
    Assign { op: CppAssignOp, lhs: Box<CppExpr>, rhs: Box<CppExpr> },

    Call { func: Box<CppExpr>, args: Vec<CppExpr> },
    MethodCall { object: Box<CppExpr>, method: String, args: Vec<CppExpr> },
    Index { array: Box<CppExpr>, index: Box<CppExpr> },
    Member { object: Box<CppExpr>, field: String },
    ArrowMember { object: Box<CppExpr>, field: String },
    ScopeRes { scope: String, name: String },

    New { ty: CppType, args: Vec<CppExpr> },
    NewArray { ty: CppType, size: Box<CppExpr> },
    Cast { ty: CppType, expr: Box<CppExpr> },

    Lambda { captures: Vec<LambdaCapture>, params: Vec<CppParam>, body: CppBlock },
    InitList(Vec<CppExpr>),
    Ternary { cond: Box<CppExpr>, then_expr: Box<CppExpr>, else_expr: Box<CppExpr> },

    PostIncr(Box<CppExpr>),
    PostDecr(Box<CppExpr>),
    Sizeof(Box<CppExpr>),
    SizeofType(CppType),
    AddrOf(Box<CppExpr>),
    Deref(Box<CppExpr>),
}

/// Lambda capture.
#[derive(Debug, Clone)]
pub enum LambdaCapture {
    ByValue(String),
    ByRef(String),
    ThisCapture,
    DefaultByValue,  // =
    DefaultByRef,    // &
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CppBinOp {
    Add, Sub, Mul, Div, Mod,
    BitAnd, BitOr, BitXor, Shl, Shr,
    Eq, Ne, Lt, Le, Gt, Ge,
    LogAnd, LogOr,
    Spaceship, // <=>
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CppUnaryOp {
    Neg, BitNot, LogNot, PreIncr, PreDecr,
}

/// Assignment operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CppAssignOp {
    Assign, AddAssign, SubAssign, MulAssign, DivAssign, ModAssign,
    AndAssign, OrAssign, XorAssign, ShlAssign, ShrAssign,
}
