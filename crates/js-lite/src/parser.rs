//! JavaScript parser for js-lite.
//!
//! Recursive descent parser that produces an AST from a token stream.
//! Supports: variable declarations, assignments, function declarations/expressions,
//! if/else, for, while, do-while, switch, try/catch/finally, object/array literals,
//! member access (dot and bracket), function calls, arrow functions, ternary,
//! binary/unary operators, typeof, new, void, delete, throw.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::tokenizer::{Token, TemplatePart};

// ---------------------------------------------------------------------------
// AST nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Ident(String),
    This,

    /// Template literal: list of string parts and expression parts
    TemplateLiteral(Vec<TemplateExprPart>),

    /// Array literal: [a, b, c]
    Array(Vec<Expr>),

    /// Object literal: { key: value, ... }
    Object(Vec<(PropKey, Expr)>),

    /// Binary operation: a + b
    Binary(Box<Expr>, BinOp, Box<Expr>),

    /// Unary operation: !a, -a, ~a, typeof a, void a, delete a
    Unary(UnaryOp, Box<Expr>),

    /// Postfix operation: a++, a--
    Postfix(Box<Expr>, PostfixOp),

    /// Assignment: a = b, a += b, etc.
    Assign(Box<Expr>, AssignOp, Box<Expr>),

    /// Member access: obj.prop
    Member(Box<Expr>, String),

    /// Computed member access: obj[expr]
    Index(Box<Expr>, Box<Expr>),

    /// Optional chaining: obj?.prop
    OptionalMember(Box<Expr>, String),

    /// Function call: f(a, b)
    Call(Box<Expr>, Vec<Expr>),

    /// new Foo(args)
    New(Box<Expr>, Vec<Expr>),

    /// Ternary: cond ? a : b
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),

    /// Arrow function: (params) => body
    Arrow(Vec<String>, Box<Stmt>),

    /// Function expression: function(params) { body }
    FunctionExpr(Option<String>, Vec<String>, Vec<Stmt>),

    /// Regex literal: /pattern/flags
    Regex(String, String),

    /// typeof expr
    Typeof(Box<Expr>),

    /// void expr
    Void(Box<Expr>),

    /// delete expr
    Delete(Box<Expr>),

    /// Comma expression: (a, b)
    Sequence(Vec<Expr>),

    /// Spread: ...expr
    Spread(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum TemplateExprPart {
    Str(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum PropKey {
    Ident(String),
    Str(String),
    Number(f64),
    Computed(Expr),
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod, Pow,
    Eq, StrictEq, NotEq, StrictNotEq,
    Lt, Gt, LtEq, GtEq,
    And, Or, NullishCoalesce,
    BitAnd, BitOr, BitXor, Shl, Shr, Ushr,
    Instanceof, In,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg, Pos, Not, BitNot,
    Typeof, Void, Delete,
}

#[derive(Debug, Clone, Copy)]
pub enum PostfixOp {
    Inc, Dec,
}

#[derive(Debug, Clone, Copy)]
pub enum AssignOp {
    Assign, AddAssign, SubAssign, MulAssign, DivAssign, ModAssign,
    BitAndAssign, BitOrAssign, BitXorAssign,
    ShlAssign, ShrAssign, UshrAssign,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// Expression statement
    Expr(Expr),

    /// Variable declaration: var/let/const name = expr
    VarDecl(VarKind, Vec<(String, Option<Expr>)>),

    /// Block: { stmts }
    Block(Vec<Stmt>),

    /// If statement: if (cond) then else
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),

    /// While loop
    While(Expr, Box<Stmt>),

    /// Do-while loop
    DoWhile(Box<Stmt>, Expr),

    /// For loop: for (init; cond; update) body
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),

    /// For-in loop: for (var x in obj) body
    ForIn(VarKind, String, Expr, Box<Stmt>),

    /// For-of loop: for (var x of iter) body
    ForOf(VarKind, String, Expr, Box<Stmt>),

    /// Function declaration
    FunctionDecl(String, Vec<String>, Vec<Stmt>),

    /// Return statement
    Return(Option<Expr>),

    /// Break
    Break,

    /// Continue
    Continue,

    /// Switch
    Switch(Expr, Vec<SwitchCase>),

    /// Try/catch/finally
    TryCatch(Vec<Stmt>, Option<(Option<String>, Vec<Stmt>)>, Option<Vec<Stmt>>),

    /// Throw
    Throw(Expr),

    /// Empty statement (lone semicolon)
    Empty,
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// None = default case
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Copy)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>, String> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let got = self.advance();
        if &got == expected {
            Ok(())
        } else {
            Err(alloc::format!("expected {:?}, got {:?}", expected, got))
        }
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_semicolon(&mut self) {
        // JavaScript has ASI (automatic semicolon insertion).
        // We just consume semicolons if present, but don't require them.
        self.eat(&Token::Semicolon);
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    // -------------------------------------------------------------------
    // Program / statements
    // -------------------------------------------------------------------

    fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !self.at_eof() {
            if self.eat(&Token::Semicolon) { continue; }
            stmts.push(self.parse_statement()?);
        }
        Ok(stmts)
    }

    fn parse_statement(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Token::LBrace => self.parse_block(),
            Token::Var => self.parse_var_decl(VarKind::Var),
            Token::Let => self.parse_var_decl(VarKind::Let),
            Token::Const => self.parse_var_decl(VarKind::Const),
            Token::Function => self.parse_function_decl(),
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::Do => self.parse_do_while(),
            Token::For => self.parse_for(),
            Token::Return => self.parse_return(),
            Token::Break => { self.advance(); self.eat_semicolon(); Ok(Stmt::Break) }
            Token::Continue => { self.advance(); self.eat_semicolon(); Ok(Stmt::Continue) }
            Token::Switch => self.parse_switch(),
            Token::Try => self.parse_try(),
            Token::Throw => self.parse_throw(),
            Token::Semicolon => { self.advance(); Ok(Stmt::Empty) }
            _ => self.parse_expression_statement(),
        }
    }

    fn parse_block(&mut self) -> Result<Stmt, String> {
        self.expect(&Token::LBrace)?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            if self.eat(&Token::Semicolon) { continue; }
            stmts.push(self.parse_statement()?);
        }
        self.expect(&Token::RBrace)?;
        Ok(Stmt::Block(stmts))
    }

    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(&Token::LBrace)?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            if self.eat(&Token::Semicolon) { continue; }
            stmts.push(self.parse_statement()?);
        }
        self.expect(&Token::RBrace)?;
        Ok(stmts)
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> Result<Stmt, String> {
        self.advance(); // consume var/let/const
        let mut decls = Vec::new();

        loop {
            let name = match self.advance() {
                Token::Ident(n) => n,
                other => return Err(alloc::format!("expected identifier in var decl, got {:?}", other)),
            };

            let init = if self.eat(&Token::Assign) {
                Some(self.parse_assignment_expr()?)
            } else {
                None
            };

            decls.push((name, init));

            if !self.eat(&Token::Comma) {
                break;
            }
        }

        self.eat_semicolon();
        Ok(Stmt::VarDecl(kind, decls))
    }

    fn parse_function_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'function'
        let name = match self.advance() {
            Token::Ident(n) => n,
            other => return Err(alloc::format!("expected function name, got {:?}", other)),
        };
        let params = self.parse_param_list()?;
        let body = self.parse_block_body()?;
        Ok(Stmt::FunctionDecl(name, params, body))
    }

    fn parse_param_list(&mut self) -> Result<Vec<String>, String> {
        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            match self.advance() {
                Token::Ident(n) => params.push(n),
                Token::Spread => {
                    // Rest parameter: ...name
                    if let Token::Ident(n) = self.advance() {
                        params.push(n);
                    }
                }
                other => return Err(alloc::format!("expected parameter name, got {:?}", other)),
            }
            // Handle default values (skip them for now)
            if self.eat(&Token::Assign) {
                let _ = self.parse_assignment_expr()?;
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Ok(params)
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'if'
        self.expect(&Token::LParen)?;
        let cond = self.parse_expression()?;
        self.expect(&Token::RParen)?;
        let then = self.parse_statement()?;
        let else_branch = if self.eat(&Token::Else) {
            Some(Box::new(self.parse_statement()?))
        } else {
            None
        };
        Ok(Stmt::If(cond, Box::new(then), else_branch))
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'while'
        self.expect(&Token::LParen)?;
        let cond = self.parse_expression()?;
        self.expect(&Token::RParen)?;
        let body = self.parse_statement()?;
        Ok(Stmt::While(cond, Box::new(body)))
    }

    fn parse_do_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'do'
        let body = self.parse_statement()?;
        self.expect(&Token::While)?;
        self.expect(&Token::LParen)?;
        let cond = self.parse_expression()?;
        self.expect(&Token::RParen)?;
        self.eat_semicolon();
        Ok(Stmt::DoWhile(Box::new(body), cond))
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'for'
        self.expect(&Token::LParen)?;

        // Check for for-in / for-of
        let kind = match self.peek() {
            Token::Var => { self.advance(); Some(VarKind::Var) }
            Token::Let => { self.advance(); Some(VarKind::Let) }
            Token::Const => { self.advance(); Some(VarKind::Const) }
            _ => None,
        };

        if let Some(vk) = kind {
            if let Token::Ident(name) = self.peek().clone() {
                let name = name.clone();
                // Peek ahead: is this `for (var x in ...)` or `for (var x of ...)`?
                if matches!(self.peek_at(1), Token::In | Token::Of) {
                    self.advance(); // consume name
                    let is_of = matches!(self.peek(), Token::Of);
                    self.advance(); // consume in/of
                    let iter_expr = self.parse_expression()?;
                    self.expect(&Token::RParen)?;
                    let body = self.parse_statement()?;
                    return if is_of {
                        Ok(Stmt::ForOf(vk, name, iter_expr, Box::new(body)))
                    } else {
                        Ok(Stmt::ForIn(vk, name, iter_expr, Box::new(body)))
                    };
                }
            }

            // Regular for loop with var decl
            let mut decls = Vec::new();
            loop {
                let dname = match self.advance() {
                    Token::Ident(n) => n,
                    other => return Err(alloc::format!("expected ident in for-init, got {:?}", other)),
                };
                let init = if self.eat(&Token::Assign) {
                    Some(self.parse_assignment_expr()?)
                } else {
                    None
                };
                decls.push((dname, init));
                if !self.eat(&Token::Comma) { break; }
            }
            let init_stmt = Stmt::VarDecl(vk, decls);
            self.expect(&Token::Semicolon)?;
            let cond = if !matches!(self.peek(), Token::Semicolon) {
                Some(self.parse_expression()?)
            } else { None };
            self.expect(&Token::Semicolon)?;
            let update = if !matches!(self.peek(), Token::RParen) {
                Some(self.parse_expression()?)
            } else { None };
            self.expect(&Token::RParen)?;
            let body = self.parse_statement()?;
            return Ok(Stmt::For(Some(Box::new(init_stmt)), cond, update, Box::new(body)));
        }

        // No var/let/const -- could be for(expr; ...) or for(ident in/of ...)
        if matches!(self.peek(), Token::Semicolon) {
            self.advance();
            let cond = if !matches!(self.peek(), Token::Semicolon) {
                Some(self.parse_expression()?)
            } else { None };
            self.expect(&Token::Semicolon)?;
            let update = if !matches!(self.peek(), Token::RParen) {
                Some(self.parse_expression()?)
            } else { None };
            self.expect(&Token::RParen)?;
            let body = self.parse_statement()?;
            return Ok(Stmt::For(None, cond, update, Box::new(body)));
        }

        let expr = self.parse_expression()?;

        // Check for in/of after expression
        if matches!(self.peek(), Token::In | Token::Of) {
            if let Expr::Ident(name) = expr {
                let is_of = matches!(self.peek(), Token::Of);
                self.advance();
                let iter_expr = self.parse_expression()?;
                self.expect(&Token::RParen)?;
                let body = self.parse_statement()?;
                return if is_of {
                    Ok(Stmt::ForOf(VarKind::Var, name, iter_expr, Box::new(body)))
                } else {
                    Ok(Stmt::ForIn(VarKind::Var, name, iter_expr, Box::new(body)))
                };
            }
        }

        let init_stmt = Stmt::Expr(expr);
        self.expect(&Token::Semicolon)?;
        let cond = if !matches!(self.peek(), Token::Semicolon) {
            Some(self.parse_expression()?)
        } else { None };
        self.expect(&Token::Semicolon)?;
        let update = if !matches!(self.peek(), Token::RParen) {
            Some(self.parse_expression()?)
        } else { None };
        self.expect(&Token::RParen)?;
        let body = self.parse_statement()?;
        Ok(Stmt::For(Some(Box::new(init_stmt)), cond, update, Box::new(body)))
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'return'
        if matches!(self.peek(), Token::Semicolon | Token::RBrace | Token::Eof) {
            self.eat_semicolon();
            return Ok(Stmt::Return(None));
        }
        let val = self.parse_expression()?;
        self.eat_semicolon();
        Ok(Stmt::Return(Some(val)))
    }

    fn parse_switch(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'switch'
        self.expect(&Token::LParen)?;
        let disc = self.parse_expression()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;

        let mut cases = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let test = if self.eat(&Token::Case) {
                let expr = self.parse_expression()?;
                self.expect(&Token::Colon)?;
                Some(expr)
            } else if self.eat(&Token::Default) {
                self.expect(&Token::Colon)?;
                None
            } else {
                return Err(alloc::format!("expected case or default, got {:?}", self.peek()));
            };

            let mut body = Vec::new();
            while !matches!(self.peek(), Token::Case | Token::Default | Token::RBrace | Token::Eof) {
                if self.eat(&Token::Semicolon) { continue; }
                body.push(self.parse_statement()?);
            }
            cases.push(SwitchCase { test, body });
        }

        self.expect(&Token::RBrace)?;
        Ok(Stmt::Switch(disc, cases))
    }

    fn parse_try(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'try'
        let try_body = self.parse_block_body()?;

        let catch = if self.eat(&Token::Catch) {
            let param = if self.eat(&Token::LParen) {
                let name = match self.advance() {
                    Token::Ident(n) => Some(n),
                    _ => None,
                };
                self.expect(&Token::RParen)?;
                name
            } else {
                None
            };
            let body = self.parse_block_body()?;
            Some((param, body))
        } else {
            None
        };

        let finally = if self.eat(&Token::Finally) {
            Some(self.parse_block_body()?)
        } else {
            None
        };

        Ok(Stmt::TryCatch(try_body, catch, finally))
    }

    fn parse_throw(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'throw'
        let expr = self.parse_expression()?;
        self.eat_semicolon();
        Ok(Stmt::Throw(expr))
    }

    fn parse_expression_statement(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expression()?;
        self.eat_semicolon();
        Ok(Stmt::Expr(expr))
    }

    // -------------------------------------------------------------------
    // Expressions (precedence climbing)
    // -------------------------------------------------------------------

    fn parse_expression(&mut self) -> Result<Expr, String> {
        let expr = self.parse_assignment_expr()?;

        // Comma operator (sequence)
        if matches!(self.peek(), Token::Comma) {
            // Only treat as sequence in certain contexts
            // For simplicity, don't auto-sequence -- callers handle comma lists
        }

        Ok(expr)
    }

    fn parse_assignment_expr(&mut self) -> Result<Expr, String> {
        // Check for arrow function: (params) => body  or  ident => body
        if self.is_arrow_function() {
            return self.parse_arrow_function();
        }

        let left = self.parse_ternary()?;

        let assign_op = match self.peek() {
            Token::Assign => Some(AssignOp::Assign),
            Token::PlusAssign => Some(AssignOp::AddAssign),
            Token::MinusAssign => Some(AssignOp::SubAssign),
            Token::StarAssign => Some(AssignOp::MulAssign),
            Token::SlashAssign => Some(AssignOp::DivAssign),
            Token::PercentAssign => Some(AssignOp::ModAssign),
            Token::AmpAssign => Some(AssignOp::BitAndAssign),
            Token::PipeAssign => Some(AssignOp::BitOrAssign),
            Token::CaretAssign => Some(AssignOp::BitXorAssign),
            Token::ShlAssign => Some(AssignOp::ShlAssign),
            Token::ShrAssign => Some(AssignOp::ShrAssign),
            Token::UshrAssign => Some(AssignOp::UshrAssign),
            _ => None,
        };

        if let Some(op) = assign_op {
            self.advance();
            let right = self.parse_assignment_expr()?;
            return Ok(Expr::Assign(Box::new(left), op, Box::new(right)));
        }

        Ok(left)
    }

    fn is_arrow_function(&self) -> bool {
        // Simple heuristic: ident => or () => or (ident) => or (ident, ident) =>
        let start = self.pos;

        if let Token::Ident(_) = &self.tokens[start] {
            if matches!(self.tokens.get(start + 1), Some(Token::Arrow)) {
                return true;
            }
        }

        if !matches!(self.tokens.get(start), Some(Token::LParen)) {
            return false;
        }

        // Walk forward looking for matching ) followed by =>
        let mut depth = 0;
        let mut i = start;
        loop {
            match self.tokens.get(i) {
                Some(Token::LParen) => depth += 1,
                Some(Token::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(i + 1), Some(Token::Arrow));
                    }
                }
                Some(Token::Eof) | None => return false,
                _ => {}
            }
            i += 1;
            if i > start + 100 { return false; } // safety limit
        }
    }

    fn parse_arrow_function(&mut self) -> Result<Expr, String> {
        let params = if let Token::Ident(_) = self.peek() {
            let Token::Ident(name) = self.advance() else { unreachable!() };
            vec![name]
        } else {
            self.parse_param_list()?
        };
        self.expect(&Token::Arrow)?;

        let body = if matches!(self.peek(), Token::LBrace) {
            let stmts = self.parse_block_body()?;
            Stmt::Block(stmts)
        } else {
            let expr = self.parse_assignment_expr()?;
            Stmt::Return(Some(expr))
        };

        Ok(Expr::Arrow(params, Box::new(body)))
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let expr = self.parse_nullish()?;
        if self.eat(&Token::Question) {
            let then = self.parse_assignment_expr()?;
            self.expect(&Token::Colon)?;
            let else_expr = self.parse_assignment_expr()?;
            Ok(Expr::Ternary(Box::new(expr), Box::new(then), Box::new(else_expr)))
        } else {
            Ok(expr)
        }
    }

    fn parse_nullish(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_logical_or()?;
        while self.eat(&Token::NullishCoalesce) {
            let right = self.parse_logical_or()?;
            left = Expr::Binary(Box::new(left), BinOp::NullishCoalesce, Box::new(right));
        }
        Ok(left)
    }

    fn parse_logical_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_logical_and()?;
        while self.eat(&Token::Or) {
            let right = self.parse_logical_and()?;
            left = Expr::Binary(Box::new(left), BinOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_or()?;
        while self.eat(&Token::And) {
            let right = self.parse_bitwise_or()?;
            left = Expr::Binary(Box::new(left), BinOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_xor()?;
        while self.eat(&Token::BitOr) {
            let right = self.parse_bitwise_xor()?;
            left = Expr::Binary(Box::new(left), BinOp::BitOr, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_and()?;
        while self.eat(&Token::BitXor) {
            let right = self.parse_bitwise_and()?;
            left = Expr::Binary(Box::new(left), BinOp::BitXor, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while self.eat(&Token::BitAnd) {
            let right = self.parse_equality()?;
            left = Expr::Binary(Box::new(left), BinOp::BitAnd, Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Token::Eq => BinOp::Eq,
                Token::StrictEq => BinOp::StrictEq,
                Token::NotEq => BinOp::NotEq,
                Token::StrictNotEq => BinOp::StrictNotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match self.peek() {
                Token::Lt => BinOp::Lt,
                Token::Gt => BinOp::Gt,
                Token::LtEq => BinOp::LtEq,
                Token::GtEq => BinOp::GtEq,
                Token::Instanceof => BinOp::Instanceof,
                Token::In => BinOp::In,
                _ => break,
            };
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::Shl => BinOp::Shl,
                Token::Shr => BinOp::Shr,
                Token::Ushr => BinOp::Ushr,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_exponentiation()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_exponentiation()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_exponentiation(&mut self) -> Result<Expr, String> {
        let left = self.parse_unary()?;
        if self.eat(&Token::DoubleStar) {
            let right = self.parse_exponentiation()?; // right-associative
            Ok(Expr::Binary(Box::new(left), BinOp::Pow, Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Not, Box::new(expr)))
            }
            Token::BitNot => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::BitNot, Box::new(expr)))
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Neg, Box::new(expr)))
            }
            Token::Plus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Pos, Box::new(expr)))
            }
            Token::Typeof => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Typeof(Box::new(expr)))
            }
            Token::Void => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Void(Box::new(expr)))
            }
            Token::Delete => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Delete(Box::new(expr)))
            }
            Token::PlusPlus => {
                self.advance();
                let expr = self.parse_unary()?;
                // Pre-increment: transform to x = x + 1
                Ok(Expr::Assign(
                    Box::new(expr.clone()),
                    AssignOp::AddAssign,
                    Box::new(Expr::Number(1.0)),
                ))
            }
            Token::MinusMinus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Assign(
                    Box::new(expr.clone()),
                    AssignOp::SubAssign,
                    Box::new(Expr::Number(1.0)),
                ))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_call_or_member()?;

        loop {
            match self.peek() {
                Token::PlusPlus => {
                    self.advance();
                    expr = Expr::Postfix(Box::new(expr), PostfixOp::Inc);
                }
                Token::MinusMinus => {
                    self.advance();
                    expr = Expr::Postfix(Box::new(expr), PostfixOp::Dec);
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call_or_member(&mut self) -> Result<Expr, String> {
        let mut expr = if self.eat(&Token::New) {
            let callee = self.parse_primary()?;
            let args = if matches!(self.peek(), Token::LParen) {
                self.parse_arg_list()?
            } else {
                Vec::new()
            };
            Expr::New(Box::new(callee), args)
        } else {
            self.parse_primary()?
        };

        loop {
            match self.peek() {
                Token::LParen => {
                    let args = self.parse_arg_list()?;
                    expr = Expr::Call(Box::new(expr), args);
                }
                Token::Dot => {
                    self.advance();
                    let name = match self.advance() {
                        Token::Ident(n) => n,
                        // Allow keywords as property names
                        Token::Default => String::from("default"),
                        Token::Delete => String::from("delete"),
                        Token::In => String::from("in"),
                        Token::Instanceof => String::from("instanceof"),
                        Token::New => String::from("new"),
                        Token::Return => String::from("return"),
                        Token::This => String::from("this"),
                        Token::Typeof => String::from("typeof"),
                        Token::Void => String::from("void"),
                        Token::Null => String::from("null"),
                        Token::Undefined => String::from("undefined"),
                        Token::True => String::from("true"),
                        Token::False => String::from("false"),
                        Token::Var => String::from("var"),
                        Token::Let => String::from("let"),
                        Token::Const => String::from("const"),
                        Token::Function => String::from("function"),
                        Token::If => String::from("if"),
                        Token::Else => String::from("else"),
                        Token::For => String::from("for"),
                        Token::While => String::from("while"),
                        Token::Do => String::from("do"),
                        Token::Break => String::from("break"),
                        Token::Continue => String::from("continue"),
                        Token::Switch => String::from("switch"),
                        Token::Case => String::from("case"),
                        Token::Try => String::from("try"),
                        Token::Catch => String::from("catch"),
                        Token::Finally => String::from("finally"),
                        Token::Throw => String::from("throw"),
                        other => return Err(alloc::format!("expected property name after '.', got {:?}", other)),
                    };
                    expr = Expr::Member(Box::new(expr), name);
                }
                Token::OptionalChain => {
                    self.advance();
                    let name = match self.advance() {
                        Token::Ident(n) => n,
                        other => return Err(alloc::format!("expected property name after '?.', got {:?}", other)),
                    };
                    expr = Expr::OptionalMember(Box::new(expr), name);
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expression()?;
                    self.expect(&Token::RBracket)?;
                    expr = Expr::Index(Box::new(expr), Box::new(index));
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, String> {
        self.expect(&Token::LParen)?;
        let mut args = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            if self.eat(&Token::Spread) {
                let expr = self.parse_assignment_expr()?;
                args.push(Expr::Spread(Box::new(expr)));
            } else {
                args.push(self.parse_assignment_expr()?);
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Number(n) => { self.advance(); Ok(Expr::Number(n)) }
            Token::Str(s) => { self.advance(); Ok(Expr::Str(s)) }
            Token::True => { self.advance(); Ok(Expr::Bool(true)) }
            Token::False => { self.advance(); Ok(Expr::Bool(false)) }
            Token::Null => { self.advance(); Ok(Expr::Null) }
            Token::Undefined => { self.advance(); Ok(Expr::Undefined) }
            Token::This => { self.advance(); Ok(Expr::This) }
            Token::Ident(_) => {
                let Token::Ident(name) = self.advance() else { unreachable!() };
                Ok(Expr::Ident(name))
            }
            Token::Regex(pattern, flags) => {
                self.advance();
                Ok(Expr::Regex(pattern, flags))
            }
            Token::TemplateLiteral(parts) => {
                self.advance();
                self.convert_template_parts(parts)
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => self.parse_array_literal(),
            Token::LBrace => self.parse_object_literal(),
            Token::Function => self.parse_function_expr(),
            other => Err(alloc::format!("unexpected token in expression: {:?}", other)),
        }
    }

    fn convert_template_parts(&mut self, parts: Vec<TemplatePart>) -> Result<Expr, String> {
        let mut expr_parts = Vec::new();
        for part in parts {
            match part {
                TemplatePart::Str(s) => expr_parts.push(TemplateExprPart::Str(s)),
                TemplatePart::Expr(tokens) => {
                    let mut sub_parser = Parser::new(tokens);
                    // Add EOF
                    sub_parser.tokens.push(Token::Eof);
                    let expr = sub_parser.parse_expression()?;
                    expr_parts.push(TemplateExprPart::Expr(expr));
                }
            }
        }
        Ok(Expr::TemplateLiteral(expr_parts))
    }

    fn parse_array_literal(&mut self) -> Result<Expr, String> {
        self.expect(&Token::LBracket)?;
        let mut elements = Vec::new();
        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            if self.eat(&Token::Comma) {
                elements.push(Expr::Undefined); // sparse array
                continue;
            }
            if self.eat(&Token::Spread) {
                let expr = self.parse_assignment_expr()?;
                elements.push(Expr::Spread(Box::new(expr)));
            } else {
                elements.push(self.parse_assignment_expr()?);
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RBracket)?;
        Ok(Expr::Array(elements))
    }

    fn parse_object_literal(&mut self) -> Result<Expr, String> {
        self.expect(&Token::LBrace)?;
        let mut props = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let key = match self.peek().clone() {
                Token::Ident(name) => {
                    self.advance();
                    // Shorthand: { foo } means { foo: foo }
                    if matches!(self.peek(), Token::Comma | Token::RBrace) {
                        props.push((PropKey::Ident(name.clone()), Expr::Ident(name)));
                        self.eat(&Token::Comma);
                        continue;
                    }
                    PropKey::Ident(name)
                }
                Token::Str(s) => { self.advance(); PropKey::Str(s) }
                Token::Number(n) => { self.advance(); PropKey::Number(n) }
                Token::LBracket => {
                    self.advance();
                    let expr = self.parse_expression()?;
                    self.expect(&Token::RBracket)?;
                    PropKey::Computed(expr)
                }
                // Keywords can be property names
                _ => {
                    let tok = self.advance();
                    let name = alloc::format!("{:?}", tok);
                    // Try to extract a reasonable name
                    PropKey::Ident(keyword_as_ident(&tok))
                }
            };

            // Check for method shorthand: { foo(x) { ... } }
            if matches!(self.peek(), Token::LParen) {
                let params = self.parse_param_list()?;
                let body = self.parse_block_body()?;
                let key_name = match &key {
                    PropKey::Ident(n) => n.clone(),
                    _ => String::from("anonymous"),
                };
                props.push((key, Expr::FunctionExpr(Some(key_name), params, body)));
                self.eat(&Token::Comma);
                continue;
            }

            self.expect(&Token::Colon)?;
            let value = self.parse_assignment_expr()?;
            props.push((key, value));
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(Expr::Object(props))
    }

    fn parse_function_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume 'function'
        let name = if let Token::Ident(_) = self.peek() {
            let Token::Ident(n) = self.advance() else { unreachable!() };
            Some(n)
        } else {
            None
        };
        let params = self.parse_param_list()?;
        let body = self.parse_block_body()?;
        Ok(Expr::FunctionExpr(name, params, body))
    }
}

fn keyword_as_ident(tok: &Token) -> String {
    match tok {
        Token::Var => String::from("var"),
        Token::Let => String::from("let"),
        Token::Const => String::from("const"),
        Token::Function => String::from("function"),
        Token::Return => String::from("return"),
        Token::If => String::from("if"),
        Token::Else => String::from("else"),
        Token::For => String::from("for"),
        Token::While => String::from("while"),
        Token::Do => String::from("do"),
        Token::Break => String::from("break"),
        Token::Continue => String::from("continue"),
        Token::Switch => String::from("switch"),
        Token::Case => String::from("case"),
        Token::Default => String::from("default"),
        Token::New => String::from("new"),
        Token::This => String::from("this"),
        Token::Typeof => String::from("typeof"),
        Token::Instanceof => String::from("instanceof"),
        Token::In => String::from("in"),
        Token::Of => String::from("of"),
        Token::Try => String::from("try"),
        Token::Catch => String::from("catch"),
        Token::Finally => String::from("finally"),
        Token::Throw => String::from("throw"),
        Token::Void => String::from("void"),
        Token::Delete => String::from("delete"),
        Token::True => String::from("true"),
        Token::False => String::from("false"),
        Token::Null => String::from("null"),
        Token::Undefined => String::from("undefined"),
        _ => String::from("unknown"),
    }
}
