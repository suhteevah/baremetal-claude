//! Parser for python-lite.
//!
//! Converts a token stream into an AST (Vec<Stmt>). Uses recursive descent
//! with Pratt-style precedence for expressions.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::tokenizer::Token;

// ---------------------------------------------------------------------------
// AST types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Expression statement (including bare function calls).
    Expr(Expr),
    /// Variable assignment: `name = expr` or tuple unpacking `a, b = expr`
    Assign {
        target: AssignTarget,
        value: Expr,
    },
    /// Augmented assignment: `name += expr`, etc.
    AugAssign {
        target: AssignTarget,
        op: BinOp,
        value: Expr,
    },
    /// if / elif / else
    If {
        condition: Expr,
        body: Vec<Stmt>,
        elif_clauses: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    /// while loop
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    /// for loop: `for var in iterable: body` (var can be tuple pattern)
    For {
        var: String,
        var_tuple: Option<Vec<String>>,
        iterable: Expr,
        body: Vec<Stmt>,
    },
    /// Function definition
    FuncDef {
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    /// Class definition
    ClassDef {
        name: String,
        bases: Vec<Expr>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    /// return statement
    Return(Option<Expr>),
    /// break
    Break,
    /// continue
    Continue,
    /// pass
    Pass,
    /// del statement
    Del(Expr),
    /// raise statement
    Raise(Option<Expr>),
    /// try/except/else/finally
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        else_body: Option<Vec<Stmt>>,
        finally_body: Option<Vec<Stmt>>,
    },
    /// with statement
    With {
        context: Expr,
        var: Option<String>,
        body: Vec<Stmt>,
    },
    /// import module
    Import {
        module: String,
        alias: Option<String>,
    },
    /// from module import names
    FromImport {
        module: String,
        names: Vec<(String, Option<String>)>, // (name, alias)
    },
    /// global x, y
    Global(Vec<String>),
    /// nonlocal x, y
    Nonlocal(Vec<String>),
    /// yield expression as statement
    YieldStmt(Option<Expr>),
    /// yield from
    YieldFromStmt(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExceptHandler {
    pub exc_type: Option<String>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    pub is_args: bool,    // *args
    pub is_kwargs: bool,  // **kwargs
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    Name(String),
    Index { obj: Expr, index: Expr },
    Attr { obj: Expr, attr: String },
    Tuple(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FStrPart>), // f-string
    Bool(bool),
    None,
    Name(String),
    List(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    Set(Vec<Expr>),
    Tuple(Vec<Expr>),
    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Compare {
        left: Box<Expr>,
        ops: Vec<CmpOp>,
        comparators: Vec<Expr>,
    },
    BoolOp {
        left: Box<Expr>,
        op: BoolOpKind,
        right: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
        kwargs: Vec<(String, Expr)>,
        star_args: Option<Box<Expr>>,
        dstar_args: Option<Box<Expr>>,
    },
    Index {
        obj: Box<Expr>,
        index: Box<Expr>,
    },
    Slice {
        obj: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    Attribute {
        obj: Box<Expr>,
        attr: String,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },
    IfExpr {
        body: Box<Expr>,
        test: Box<Expr>,
        orelse: Box<Expr>,
    },
    ListComp {
        elt: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    SetComp {
        elt: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    DictComp {
        key: Box<Expr>,
        value: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    GeneratorExp {
        elt: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    Yield(Option<Box<Expr>>),
    YieldFrom(Box<Expr>),
    Walrus {
        target: String,
        value: Box<Expr>,
    },
    Starred(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStrPart {
    Literal(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comprehension {
    pub target: String,
    pub target_tuple: Option<Vec<String>>,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    BitOr,
    BitAnd,
    BitXor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    In,
    NotIn,
    Is,
    IsNot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoolOpKind {
    And,
    Or,
}

// ---------------------------------------------------------------------------
// Parser state
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>, String> {
    let mut p = Parser { tokens, pos: 0 };
    p.parse_block_top()
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn peek_ahead(&self, n: usize) -> &Token {
        self.tokens.get(self.pos + n).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let got = self.advance();
        if core::mem::discriminant(&got) == core::mem::discriminant(expected) {
            Ok(())
        } else {
            Err(alloc::format!("expected {:?}, got {:?}", expected, got))
        }
    }

    fn at(&self, tok: &Token) -> bool {
        core::mem::discriminant(self.peek()) == core::mem::discriminant(tok)
    }

    fn skip_newlines(&mut self) {
        while self.at(&Token::Newline) {
            self.advance();
        }
    }

    // -----------------------------------------------------------------------
    // Top-level block (no leading INDENT)
    // -----------------------------------------------------------------------

    fn parse_block_top(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(&Token::Eof) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    // -----------------------------------------------------------------------
    // Indented block (after INDENT, until DEDENT)
    // -----------------------------------------------------------------------

    fn parse_indented_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.skip_newlines();
        self.expect(&Token::Indent)?;
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(&Token::Dedent) && !self.at(&Token::Eof) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        if self.at(&Token::Dedent) {
            self.advance();
        }
        Ok(stmts)
    }

    // -----------------------------------------------------------------------
    // Statement
    // -----------------------------------------------------------------------

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        // Handle decorators
        if self.at(&Token::At) {
            return self.parse_decorated();
        }

        match self.peek().clone() {
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::For => self.parse_for(),
            Token::Def => self.parse_def(Vec::new()),
            Token::Class => self.parse_class(Vec::new()),
            Token::Return => self.parse_return(),
            Token::Break => { self.advance(); Ok(Stmt::Break) }
            Token::Continue => { self.advance(); Ok(Stmt::Continue) }
            Token::Pass => { self.advance(); Ok(Stmt::Pass) }
            Token::Del => self.parse_del(),
            Token::Raise => self.parse_raise(),
            Token::Try => self.parse_try(),
            Token::With => self.parse_with(),
            Token::Import => self.parse_import(),
            Token::From => self.parse_from_import(),
            Token::Global => self.parse_global(),
            Token::Nonlocal => self.parse_nonlocal(),
            Token::Yield => self.parse_yield_stmt(),
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_decorated(&mut self) -> Result<Stmt, String> {
        let mut decorators = Vec::new();
        while self.at(&Token::At) {
            self.advance();
            let dec = self.parse_expression()?;
            decorators.push(dec);
            // skip newline after decorator
            self.skip_newlines();
        }
        match self.peek().clone() {
            Token::Def => self.parse_def(decorators),
            Token::Class => self.parse_class(decorators),
            _ => Err(String::from("decorator must be followed by def or class")),
        }
    }

    fn parse_expr_or_assign(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expression()?;

        // Check for tuple-like assignment: a, b = expr
        // If we see a comma, collect more names for tuple unpacking
        if self.at(&Token::Comma) {
            let mut names = Vec::new();
            if let Expr::Name(n) = &expr {
                names.push(n.clone());
            } else {
                return Err(String::from("invalid tuple unpacking target"));
            }
            while self.at(&Token::Comma) {
                self.advance();
                if self.at(&Token::Assign) {
                    break;
                }
                match self.advance() {
                    Token::Ident(n) => names.push(n),
                    other => return Err(alloc::format!("expected name in tuple, got {:?}", other)),
                }
            }
            if self.at(&Token::Assign) {
                self.advance();
                let value = self.parse_expression()?;
                return Ok(Stmt::Assign {
                    target: AssignTarget::Tuple(names),
                    value,
                });
            }
            // Otherwise it's a tuple expression statement
            let mut elements = vec![expr];
            for n in names[1..].iter() {
                elements.push(Expr::Name(n.clone()));
            }
            return Ok(Stmt::Expr(Expr::Tuple(elements)));
        }

        // Check for assignment or augmented assignment.
        match self.peek() {
            Token::Assign => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::Assign { target, value })
            }
            Token::PlusEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Add, value })
            }
            Token::MinusEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Sub, value })
            }
            Token::StarEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Mul, value })
            }
            Token::SlashEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Div, value })
            }
            Token::PercentEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Mod, value })
            }
            Token::DoubleStarEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::Pow, value })
            }
            Token::DoubleSlashEq => {
                self.advance();
                let value = self.parse_expression()?;
                let target = expr_to_assign_target(expr)?;
                Ok(Stmt::AugAssign { target, op: BinOp::FloorDiv, value })
            }
            _ => Ok(Stmt::Expr(expr)),
        }
    }

    // -----------------------------------------------------------------------
    // if / elif / else
    // -----------------------------------------------------------------------

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::If)?;
        let condition = self.parse_expression()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;

        let mut elif_clauses = Vec::new();
        let mut else_body = Option::None;

        loop {
            self.skip_newlines();
            if self.at(&Token::Elif) {
                self.advance();
                let cond = self.parse_expression()?;
                self.expect(&Token::Colon)?;
                let block = self.parse_indented_block()?;
                elif_clauses.push((cond, block));
            } else if self.at(&Token::Else) {
                self.advance();
                self.expect(&Token::Colon)?;
                else_body = Some(self.parse_indented_block()?);
                break;
            } else {
                break;
            }
        }

        Ok(Stmt::If {
            condition,
            body,
            elif_clauses,
            else_body,
        })
    }

    // -----------------------------------------------------------------------
    // while
    // -----------------------------------------------------------------------

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::While)?;
        let condition = self.parse_expression()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;
        Ok(Stmt::While { condition, body })
    }

    // -----------------------------------------------------------------------
    // for (supports tuple unpacking)
    // -----------------------------------------------------------------------

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::For)?;
        let first_var = match self.advance() {
            Token::Ident(name) => name,
            other => return Err(alloc::format!("expected identifier after 'for', got {:?}", other)),
        };

        // Check for tuple unpacking: for a, b in ...
        let mut var_tuple = None;
        if self.at(&Token::Comma) {
            let mut vars = vec![first_var.clone()];
            while self.at(&Token::Comma) {
                self.advance();
                match self.advance() {
                    Token::Ident(n) => vars.push(n),
                    other => return Err(alloc::format!("expected identifier, got {:?}", other)),
                }
            }
            var_tuple = Some(vars);
        }

        self.expect(&Token::In)?;
        let iterable = self.parse_expression()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;
        Ok(Stmt::For { var: first_var, var_tuple, iterable, body })
    }

    // -----------------------------------------------------------------------
    // def (with decorators, *args, **kwargs, default params)
    // -----------------------------------------------------------------------

    fn parse_def(&mut self, decorators: Vec<Expr>) -> Result<Stmt, String> {
        self.expect(&Token::Def)?;
        let name = match self.advance() {
            Token::Ident(n) => n,
            other => return Err(alloc::format!("expected function name, got {:?}", other)),
        };
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        // Optional return annotation
        if self.at(&Token::Arrow) {
            self.advance();
            let _ = self.parse_expression()?; // discard annotation
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;
        Ok(Stmt::FuncDef { name, params, body, decorators })
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, String> {
        let mut params = Vec::new();
        while !self.at(&Token::RParen) && !self.at(&Token::Eof) {
            let mut is_args = false;
            let mut is_kwargs = false;

            if self.at(&Token::DoubleStar) {
                self.advance();
                is_kwargs = true;
            } else if self.at(&Token::Star) {
                self.advance();
                is_args = true;
            }

            let name = match self.advance() {
                Token::Ident(p) => p,
                other => return Err(alloc::format!("expected parameter name, got {:?}", other)),
            };

            // Optional type annotation
            if self.at(&Token::Colon) {
                self.advance();
                let _ = self.parse_expression()?; // discard annotation
            }

            let default = if self.at(&Token::Assign) {
                self.advance();
                Some(self.parse_expression()?)
            } else {
                None
            };

            params.push(Param { name, default, is_args, is_kwargs });

            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        Ok(params)
    }

    // -----------------------------------------------------------------------
    // class
    // -----------------------------------------------------------------------

    fn parse_class(&mut self, decorators: Vec<Expr>) -> Result<Stmt, String> {
        self.expect(&Token::Class)?;
        let name = match self.advance() {
            Token::Ident(n) => n,
            other => return Err(alloc::format!("expected class name, got {:?}", other)),
        };
        let mut bases = Vec::new();
        if self.at(&Token::LParen) {
            self.advance();
            while !self.at(&Token::RParen) && !self.at(&Token::Eof) {
                bases.push(self.parse_expression()?);
                if self.at(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(&Token::RParen)?;
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;
        Ok(Stmt::ClassDef { name, bases, body, decorators })
    }

    // -----------------------------------------------------------------------
    // return
    // -----------------------------------------------------------------------

    fn parse_return(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Return)?;
        if self.at(&Token::Newline) || self.at(&Token::Eof) || self.at(&Token::Dedent) {
            Ok(Stmt::Return(Option::None))
        } else {
            let expr = self.parse_expression()?;
            Ok(Stmt::Return(Some(expr)))
        }
    }

    // -----------------------------------------------------------------------
    // del
    // -----------------------------------------------------------------------

    fn parse_del(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Del)?;
        let expr = self.parse_expression()?;
        Ok(Stmt::Del(expr))
    }

    // -----------------------------------------------------------------------
    // raise
    // -----------------------------------------------------------------------

    fn parse_raise(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Raise)?;
        if self.at(&Token::Newline) || self.at(&Token::Eof) || self.at(&Token::Dedent) {
            Ok(Stmt::Raise(None))
        } else {
            let expr = self.parse_expression()?;
            Ok(Stmt::Raise(Some(expr)))
        }
    }

    // -----------------------------------------------------------------------
    // try/except/else/finally
    // -----------------------------------------------------------------------

    fn parse_try(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Try)?;
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;

        let mut handlers = Vec::new();
        let mut else_body = None;
        let mut finally_body = None;

        loop {
            self.skip_newlines();
            if self.at(&Token::Except) {
                self.advance();
                let mut exc_type = None;
                let mut name = None;

                if !self.at(&Token::Colon) {
                    let etype = match self.advance() {
                        Token::Ident(n) => n,
                        other => return Err(alloc::format!("expected exception type, got {:?}", other)),
                    };
                    exc_type = Some(etype);
                    if self.at(&Token::As) {
                        self.advance();
                        match self.advance() {
                            Token::Ident(n) => name = Some(n),
                            other => return Err(alloc::format!("expected name after 'as', got {:?}", other)),
                        }
                    }
                }
                self.expect(&Token::Colon)?;
                let handler_body = self.parse_indented_block()?;
                handlers.push(ExceptHandler { exc_type, name, body: handler_body });
            } else if self.at(&Token::Else) {
                self.advance();
                self.expect(&Token::Colon)?;
                else_body = Some(self.parse_indented_block()?);
            } else if self.at(&Token::Finally) {
                self.advance();
                self.expect(&Token::Colon)?;
                finally_body = Some(self.parse_indented_block()?);
                break;
            } else {
                break;
            }
        }

        Ok(Stmt::Try { body, handlers, else_body, finally_body })
    }

    // -----------------------------------------------------------------------
    // with
    // -----------------------------------------------------------------------

    fn parse_with(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::With)?;
        let context = self.parse_expression()?;
        let var = if self.at(&Token::As) {
            self.advance();
            match self.advance() {
                Token::Ident(n) => Some(n),
                other => return Err(alloc::format!("expected name after 'as', got {:?}", other)),
            }
        } else {
            None
        };
        self.expect(&Token::Colon)?;
        let body = self.parse_indented_block()?;
        Ok(Stmt::With { context, var, body })
    }

    // -----------------------------------------------------------------------
    // import
    // -----------------------------------------------------------------------

    fn parse_import(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Import)?;
        let mut module = match self.advance() {
            Token::Ident(n) => n,
            other => return Err(alloc::format!("expected module name, got {:?}", other)),
        };
        // support dotted names: import os.path
        while self.at(&Token::Dot) {
            self.advance();
            match self.advance() {
                Token::Ident(n) => {
                    module.push('.');
                    module.push_str(&n);
                }
                other => return Err(alloc::format!("expected name after '.', got {:?}", other)),
            }
        }
        let alias = if self.at(&Token::As) {
            self.advance();
            match self.advance() {
                Token::Ident(n) => Some(n),
                other => return Err(alloc::format!("expected alias, got {:?}", other)),
            }
        } else {
            None
        };
        Ok(Stmt::Import { module, alias })
    }

    fn parse_from_import(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::From)?;
        let mut module = match self.advance() {
            Token::Ident(n) => n,
            other => return Err(alloc::format!("expected module name, got {:?}", other)),
        };
        while self.at(&Token::Dot) {
            self.advance();
            match self.advance() {
                Token::Ident(n) => {
                    module.push('.');
                    module.push_str(&n);
                }
                other => return Err(alloc::format!("expected name after '.', got {:?}", other)),
            }
        }
        self.expect(&Token::Import)?;
        let mut names = Vec::new();
        loop {
            let name = match self.advance() {
                Token::Ident(n) => n,
                Token::Star => String::from("*"),
                other => return Err(alloc::format!("expected import name, got {:?}", other)),
            };
            let alias = if self.at(&Token::As) {
                self.advance();
                match self.advance() {
                    Token::Ident(n) => Some(n),
                    other => return Err(alloc::format!("expected alias, got {:?}", other)),
                }
            } else {
                None
            };
            names.push((name, alias));
            if self.at(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(Stmt::FromImport { module, names })
    }

    // -----------------------------------------------------------------------
    // global / nonlocal
    // -----------------------------------------------------------------------

    fn parse_global(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Global)?;
        let mut names = Vec::new();
        loop {
            match self.advance() {
                Token::Ident(n) => names.push(n),
                other => return Err(alloc::format!("expected name, got {:?}", other)),
            }
            if self.at(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(Stmt::Global(names))
    }

    fn parse_nonlocal(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Nonlocal)?;
        let mut names = Vec::new();
        loop {
            match self.advance() {
                Token::Ident(n) => names.push(n),
                other => return Err(alloc::format!("expected name, got {:?}", other)),
            }
            if self.at(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(Stmt::Nonlocal(names))
    }

    // -----------------------------------------------------------------------
    // yield statement
    // -----------------------------------------------------------------------

    fn parse_yield_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::Yield)?;
        if self.at(&Token::From) {
            self.advance();
            let expr = self.parse_expression()?;
            Ok(Stmt::YieldFromStmt(expr))
        } else if self.at(&Token::Newline) || self.at(&Token::Eof) || self.at(&Token::Dedent) {
            Ok(Stmt::YieldStmt(None))
        } else {
            let expr = self.parse_expression()?;
            Ok(Stmt::YieldStmt(Some(expr)))
        }
    }

    // -----------------------------------------------------------------------
    // Expression parsing (precedence climbing)
    // -----------------------------------------------------------------------

    fn parse_expression(&mut self) -> Result<Expr, String> {
        // Check for lambda
        if self.at(&Token::Lambda) {
            return self.parse_lambda();
        }

        let expr = self.parse_walrus()?;

        // Check for ternary: expr if cond else expr
        if self.at(&Token::If) {
            self.advance();
            let test = self.parse_or()?;
            self.expect(&Token::Else)?;
            let orelse = self.parse_expression()?;
            return Ok(Expr::IfExpr {
                body: Box::new(expr),
                test: Box::new(test),
                orelse: Box::new(orelse),
            });
        }

        Ok(expr)
    }

    fn parse_lambda(&mut self) -> Result<Expr, String> {
        self.expect(&Token::Lambda)?;
        let mut params = Vec::new();
        while !self.at(&Token::Colon) && !self.at(&Token::Eof) {
            match self.advance() {
                Token::Ident(n) => params.push(n),
                other => return Err(alloc::format!("expected param name, got {:?}", other)),
            }
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_expression()?;
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
        })
    }

    fn parse_walrus(&mut self) -> Result<Expr, String> {
        let expr = self.parse_or()?;
        if self.at(&Token::Walrus) {
            self.advance();
            if let Expr::Name(name) = expr {
                let value = self.parse_or()?;
                return Ok(Expr::Walrus {
                    target: name,
                    value: Box::new(value),
                });
            } else {
                return Err(String::from("walrus operator target must be a name"));
            }
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.at(&Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BoolOp {
                left: Box::new(left),
                op: BoolOpKind::Or,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_not()?;
        while self.at(&Token::And) {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::BoolOp {
                left: Box::new(left),
                op: BoolOpKind::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.at(&Token::Not) {
            self.advance();
            let operand = self.parse_not()?;
            Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                operand: Box::new(operand),
            })
        } else {
            self.parse_comparison()
        }
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let left = self.parse_bitor()?;

        let mut ops = Vec::new();
        let mut comparators = Vec::new();

        loop {
            let op = match self.peek() {
                Token::Eq => CmpOp::Eq,
                Token::NotEq => CmpOp::NotEq,
                Token::Lt => CmpOp::Lt,
                Token::Gt => CmpOp::Gt,
                Token::LtEq => CmpOp::LtEq,
                Token::GtEq => CmpOp::GtEq,
                Token::In => CmpOp::In,
                Token::Not => {
                    // "not in"
                    if matches!(self.peek_ahead(1), Token::In) {
                        self.advance(); // skip 'not'
                        CmpOp::NotIn
                    } else {
                        break;
                    }
                }
                Token::Is => {
                    self.advance();
                    if self.at(&Token::Not) {
                        self.advance();
                        ops.push(CmpOp::IsNot);
                        comparators.push(self.parse_bitor()?);
                        continue;
                    } else {
                        ops.push(CmpOp::Is);
                        comparators.push(self.parse_bitor()?);
                        continue;
                    }
                }
                _ => break,
            };
            self.advance();
            let right = self.parse_bitor()?;
            ops.push(op);
            comparators.push(right);
        }

        if ops.is_empty() {
            Ok(left)
        } else {
            Ok(Expr::Compare {
                left: Box::new(left),
                ops,
                comparators,
            })
        }
    }

    fn parse_bitor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitxor()?;
        while self.at(&Token::Pipe) {
            self.advance();
            let right = self.parse_bitxor()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitOr,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitxor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitand()?;
        while self.at(&Token::Caret) {
            self.advance();
            let right = self.parse_bitand()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitXor,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitand(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add_sub()?;
        while self.at(&Token::Ampersand) {
            self.advance();
            let right = self.parse_add_sub()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitAnd,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_add_sub(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul_div()?;

        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_div()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_mul_div(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_power()?;

        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::DoubleSlash => BinOp::FloorDiv,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_power()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        let base = self.parse_unary()?;
        if self.at(&Token::DoubleStar) {
            self.advance();
            // Right-associative: recurse into parse_power.
            let exp = self.parse_power()?;
            Ok(Expr::BinOp {
                left: Box::new(base),
                op: BinOp::Pow,
                right: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.at(&Token::Minus) {
            self.advance();
            let operand = self.parse_unary()?;
            Ok(Expr::UnaryOp {
                op: UnaryOp::Neg,
                operand: Box::new(operand),
            })
        } else if self.at(&Token::Tilde) {
            self.advance();
            let operand = self.parse_unary()?;
            Ok(Expr::UnaryOp {
                op: UnaryOp::BitNot,
                operand: Box::new(operand),
            })
        } else if self.at(&Token::Star) {
            self.advance();
            let operand = self.parse_unary()?;
            Ok(Expr::Starred(Box::new(operand)))
        } else {
            self.parse_postfix()
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_atom()?;

        loop {
            match self.peek() {
                Token::LParen => {
                    self.advance();
                    let (args, kwargs, star_args, dstar_args) = self.parse_call_args()?;
                    self.expect(&Token::RParen)?;
                    expr = Expr::Call {
                        func: Box::new(expr),
                        args,
                        kwargs,
                        star_args,
                        dstar_args,
                    };
                }
                Token::LBracket => {
                    self.advance();
                    // Check for slice
                    expr = self.parse_index_or_slice(expr)?;
                    self.expect(&Token::RBracket)?;
                }
                Token::Dot => {
                    self.advance();
                    let attr = match self.advance() {
                        Token::Ident(name) => name,
                        other => return Err(alloc::format!("expected attribute name, got {:?}", other)),
                    };
                    expr = Expr::Attribute {
                        obj: Box::new(expr),
                        attr,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_index_or_slice(&mut self, obj: Expr) -> Result<Expr, String> {
        // Detect slice: obj[a:b:c]
        // Could start with : (no lower bound)
        if self.at(&Token::Colon) {
            // Slice with no lower
            self.advance();
            let upper = if !self.at(&Token::RBracket) && !self.at(&Token::Colon) {
                Some(Box::new(self.parse_expression()?))
            } else {
                None
            };
            let step = if self.at(&Token::Colon) {
                self.advance();
                if !self.at(&Token::RBracket) {
                    Some(Box::new(self.parse_expression()?))
                } else {
                    None
                }
            } else {
                None
            };
            return Ok(Expr::Slice {
                obj: Box::new(obj),
                lower: None,
                upper,
                step,
            });
        }

        let first = self.parse_expression()?;

        if self.at(&Token::Colon) {
            // It's a slice
            self.advance();
            let upper = if !self.at(&Token::RBracket) && !self.at(&Token::Colon) {
                Some(Box::new(self.parse_expression()?))
            } else {
                None
            };
            let step = if self.at(&Token::Colon) {
                self.advance();
                if !self.at(&Token::RBracket) {
                    Some(Box::new(self.parse_expression()?))
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Expr::Slice {
                obj: Box::new(obj),
                lower: Some(Box::new(first)),
                upper,
                step,
            })
        } else {
            Ok(Expr::Index {
                obj: Box::new(obj),
                index: Box::new(first),
            })
        }
    }

    fn parse_call_args(&mut self) -> Result<(Vec<Expr>, Vec<(String, Expr)>, Option<Box<Expr>>, Option<Box<Expr>>), String> {
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        let mut star_args = None;
        let mut dstar_args = None;

        while !self.at(&Token::RParen) && !self.at(&Token::Eof) {
            // Check for **kwargs
            if self.at(&Token::DoubleStar) {
                self.advance();
                let expr = self.parse_expression()?;
                dstar_args = Some(Box::new(expr));
                if self.at(&Token::Comma) { self.advance(); }
                continue;
            }
            // Check for *args
            if self.at(&Token::Star) {
                self.advance();
                let expr = self.parse_expression()?;
                star_args = Some(Box::new(expr));
                if self.at(&Token::Comma) { self.advance(); }
                continue;
            }

            let expr = self.parse_expression()?;

            // Check for keyword arg: name=value
            if self.at(&Token::Assign) {
                if let Expr::Name(name) = expr {
                    self.advance();
                    let val = self.parse_expression()?;
                    kwargs.push((name, val));
                } else {
                    return Err(String::from("keyword argument must be an identifier"));
                }
            } else {
                // Check for generator expression: func(x for x in ...)
                if self.at(&Token::For) {
                    let gen = self.parse_comp_tail(expr)?;
                    args.push(gen);
                } else {
                    args.push(expr);
                }
            }
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        Ok((args, kwargs, star_args, dstar_args))
    }

    fn parse_comp_tail(&mut self, elt: Expr) -> Result<Expr, String> {
        let generators = self.parse_comprehensions()?;
        Ok(Expr::GeneratorExp {
            elt: Box::new(elt),
            generators,
        })
    }

    fn parse_comprehensions(&mut self) -> Result<Vec<Comprehension>, String> {
        let mut generators = Vec::new();
        while self.at(&Token::For) {
            self.advance();
            let target = match self.advance() {
                Token::Ident(n) => n,
                other => return Err(alloc::format!("expected name in comprehension, got {:?}", other)),
            };
            let mut target_tuple = None;
            if self.at(&Token::Comma) {
                let mut vars = vec![target.clone()];
                while self.at(&Token::Comma) {
                    self.advance();
                    if self.at(&Token::In) { break; }
                    match self.advance() {
                        Token::Ident(n) => vars.push(n),
                        other => return Err(alloc::format!("expected name, got {:?}", other)),
                    }
                }
                target_tuple = Some(vars);
            }
            self.expect(&Token::In)?;
            let iter = self.parse_or()?;
            let mut ifs = Vec::new();
            while self.at(&Token::If) {
                self.advance();
                ifs.push(self.parse_or()?);
            }
            generators.push(Comprehension { target, target_tuple, iter, ifs });
        }
        Ok(generators)
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Int(n) => { self.advance(); Ok(Expr::Int(n)) }
            Token::Float(f) => { self.advance(); Ok(Expr::Float(f)) }
            Token::Str(s) => { self.advance(); Ok(Expr::Str(s)) }
            Token::FStr(s) => { self.advance(); Ok(parse_fstring(&s)?) }
            Token::True => { self.advance(); Ok(Expr::Bool(true)) }
            Token::False => { self.advance(); Ok(Expr::Bool(false)) }
            Token::None => { self.advance(); Ok(Expr::None) }
            Token::Ident(name) => { self.advance(); Ok(Expr::Name(name)) }
            Token::Yield => {
                self.advance();
                if self.at(&Token::From) {
                    self.advance();
                    let expr = self.parse_expression()?;
                    Ok(Expr::YieldFrom(Box::new(expr)))
                } else if self.at(&Token::RParen) || self.at(&Token::Newline) || self.at(&Token::Eof) {
                    Ok(Expr::Yield(None))
                } else {
                    let expr = self.parse_expression()?;
                    Ok(Expr::Yield(Some(Box::new(expr))))
                }
            }
            Token::LParen => {
                self.advance();
                // Empty tuple
                if self.at(&Token::RParen) {
                    self.advance();
                    return Ok(Expr::Tuple(Vec::new()));
                }
                let first = self.parse_expression()?;
                // Generator expression
                if self.at(&Token::For) {
                    let gen = self.parse_comp_tail(first)?;
                    self.expect(&Token::RParen)?;
                    return Ok(gen);
                }
                // Tuple or parenthesized expression
                if self.at(&Token::Comma) {
                    let mut elements = vec![first];
                    while self.at(&Token::Comma) {
                        self.advance();
                        if self.at(&Token::RParen) { break; }
                        elements.push(self.parse_expression()?);
                    }
                    self.expect(&Token::RParen)?;
                    return Ok(Expr::Tuple(elements));
                }
                self.expect(&Token::RParen)?;
                Ok(first)
            }
            Token::LBracket => {
                self.advance();
                if self.at(&Token::RBracket) {
                    self.advance();
                    return Ok(Expr::List(Vec::new()));
                }
                let first = self.parse_expression()?;
                // List comprehension
                if self.at(&Token::For) {
                    let generators = self.parse_comprehensions()?;
                    self.expect(&Token::RBracket)?;
                    return Ok(Expr::ListComp {
                        elt: Box::new(first),
                        generators,
                    });
                }
                // Regular list
                let mut elements = vec![first];
                while self.at(&Token::Comma) {
                    self.advance();
                    if self.at(&Token::RBracket) { break; }
                    elements.push(self.parse_expression()?);
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::List(elements))
            }
            Token::LBrace => {
                self.advance();
                // Empty dict
                if self.at(&Token::RBrace) {
                    self.advance();
                    return Ok(Expr::Dict(Vec::new()));
                }
                let first = self.parse_expression()?;
                if self.at(&Token::Colon) {
                    // Dict literal or dict comprehension
                    self.advance();
                    let first_val = self.parse_expression()?;
                    // Dict comprehension
                    if self.at(&Token::For) {
                        let generators = self.parse_comprehensions()?;
                        self.expect(&Token::RBrace)?;
                        return Ok(Expr::DictComp {
                            key: Box::new(first),
                            value: Box::new(first_val),
                            generators,
                        });
                    }
                    let mut pairs = vec![(first, first_val)];
                    while self.at(&Token::Comma) {
                        self.advance();
                        if self.at(&Token::RBrace) { break; }
                        let k = self.parse_expression()?;
                        self.expect(&Token::Colon)?;
                        let v = self.parse_expression()?;
                        pairs.push((k, v));
                    }
                    self.expect(&Token::RBrace)?;
                    Ok(Expr::Dict(pairs))
                } else if self.at(&Token::For) {
                    // Set comprehension
                    let generators = self.parse_comprehensions()?;
                    self.expect(&Token::RBrace)?;
                    Ok(Expr::SetComp {
                        elt: Box::new(first),
                        generators,
                    })
                } else {
                    // Set literal
                    let mut elements = vec![first];
                    while self.at(&Token::Comma) {
                        self.advance();
                        if self.at(&Token::RBrace) { break; }
                        elements.push(self.parse_expression()?);
                    }
                    self.expect(&Token::RBrace)?;
                    Ok(Expr::Set(elements))
                }
            }
            other => Err(alloc::format!("unexpected token: {:?}", other)),
        }
    }
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    match expr {
        Expr::Name(name) => Ok(AssignTarget::Name(name)),
        Expr::Index { obj, index } => Ok(AssignTarget::Index { obj: *obj, index: *index }),
        Expr::Attribute { obj, attr } => Ok(AssignTarget::Attr { obj: *obj, attr }),
        Expr::Tuple(elements) => {
            let mut names = Vec::new();
            for e in elements {
                if let Expr::Name(n) = e {
                    names.push(n);
                } else {
                    return Err(String::from("invalid tuple unpacking target"));
                }
            }
            Ok(AssignTarget::Tuple(names))
        }
        _ => Err(String::from("invalid assignment target")),
    }
}

/// Parse f-string template into parts
fn parse_fstring(template: &str) -> Result<Expr, String> {
    let mut parts = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    let mut lit = String::new();

    while i < chars.len() {
        if chars[i] == '{' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                lit.push('{');
                i += 2;
                continue;
            }
            if !lit.is_empty() {
                parts.push(FStrPart::Literal(core::mem::take(&mut lit)));
            }
            i += 1;
            let mut depth = 1;
            let mut expr_str = String::new();
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' { depth += 1; }
                if chars[i] == '}' { depth -= 1; }
                if depth > 0 {
                    expr_str.push(chars[i]);
                }
                i += 1;
            }
            // Parse the expression inside {}
            // Strip format spec after ':'
            let expr_part = if let Some(colon_pos) = expr_str.find(':') {
                &expr_str[..colon_pos]
            } else {
                &expr_str
            };
            let tokens = crate::tokenizer::tokenize(expr_part)?;
            let mut ast = crate::parser::parse(tokens)?;
            if ast.len() == 1 {
                if let Stmt::Expr(e) = ast.remove(0) {
                    parts.push(FStrPart::Expr(e));
                } else {
                    return Err(String::from("f-string expression must be an expression"));
                }
            } else {
                return Err(String::from("invalid f-string expression"));
            }
        } else if chars[i] == '}' && i + 1 < chars.len() && chars[i + 1] == '}' {
            lit.push('}');
            i += 2;
        } else {
            lit.push(chars[i]);
            i += 1;
        }
    }
    if !lit.is_empty() {
        parts.push(FStrPart::Literal(lit));
    }
    Ok(Expr::FStr(parts))
}
