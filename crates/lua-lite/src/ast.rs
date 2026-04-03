//! AST node definitions for Lua 5.4.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// A chunk is a sequence of statements (= a block).
pub type Chunk = Block;

/// A block is a list of statements with an optional return.
#[derive(Debug, Clone)]
pub struct Block {
    pub stats: Vec<Stat>,
    pub ret: Option<Vec<Exp>>,
}

/// Statements.
#[derive(Debug, Clone)]
pub enum Stat {
    /// Assignment: targets = values
    Assign {
        targets: Vec<Exp>,
        values: Vec<Exp>,
    },
    /// do block end
    Do(Block),
    /// while exp do block end
    While {
        condition: Exp,
        body: Block,
    },
    /// repeat block until exp
    Repeat {
        body: Block,
        condition: Exp,
    },
    /// if ... then ... elseif ... else ... end
    If {
        conditions: Vec<(Exp, Block)>,
        else_block: Option<Block>,
    },
    /// for name = start, stop [, step] do block end
    ForNumeric {
        name: String,
        start: Exp,
        stop: Exp,
        step: Option<Exp>,
        body: Block,
    },
    /// for namelist in explist do block end
    ForGeneric {
        names: Vec<String>,
        iterators: Vec<Exp>,
        body: Block,
    },
    /// function funcname funcbody
    FunctionDef {
        name: Exp,
        params: Vec<String>,
        has_vararg: bool,
        body: Block,
    },
    /// local function name funcbody
    LocalFunction {
        name: String,
        params: Vec<String>,
        has_vararg: bool,
        body: Block,
    },
    /// local namelist [= explist]
    Local {
        names: Vec<String>,
        values: Vec<Exp>,
    },
    /// return explist
    Return(Vec<Exp>),
    /// break
    Break,
    /// goto name
    Goto(String),
    /// ::name::
    Label(String),
    /// Expression statement (function call)
    ExprStat(Exp),
}

/// Expressions.
#[derive(Debug, Clone)]
pub enum Exp {
    Nil,
    True,
    False,
    Integer(i64),
    Number(f64),
    Str(String),
    VarArg,

    /// Variable reference
    Ident(String),

    /// Unary operation
    UnOp {
        op: UnaryOp,
        operand: Box<Exp>,
    },

    /// Binary operation
    BinOp {
        op: BinaryOp,
        left: Box<Exp>,
        right: Box<Exp>,
    },

    /// Function definition (anonymous)
    Function {
        params: Vec<String>,
        has_vararg: bool,
        body: Block,
    },

    /// Function call: func(args)
    Call {
        func: Box<Exp>,
        args: Vec<Exp>,
    },

    /// Method call: obj:method(args)
    MethodCall {
        object: Box<Exp>,
        method: String,
        args: Vec<Exp>,
    },

    /// Table index: table[key]
    Index {
        table: Box<Exp>,
        key: Box<Exp>,
    },

    /// Field access: table.field
    Field {
        table: Box<Exp>,
        field: String,
    },

    /// Table constructor: { fields }
    TableConstructor(Vec<TableField>),
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,    // -
    Not,    // not
    Len,    // #
    BNot,   // ~
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,        // +
    Sub,        // -
    Mul,        // *
    Div,        // /
    IDiv,       // //
    Mod,        // %
    Pow,        // ^
    Concat,     // ..
    Eq,         // ==
    NotEq,      // ~=
    Less,       // <
    LessEq,     // <=
    Greater,    // >
    GreaterEq,  // >=
    And,        // and
    Or,         // or
    BAnd,       // &
    BOr,        // |
    BXor,       // ~
    Shl,        // <<
    Shr,        // >>
}

/// Table constructor field.
#[derive(Debug, Clone)]
pub enum TableField {
    /// [exp] = exp
    IndexField { key: Exp, value: Exp },
    /// name = exp
    NameField { name: String, value: Exp },
    /// exp (positional)
    Positional(Exp),
}
