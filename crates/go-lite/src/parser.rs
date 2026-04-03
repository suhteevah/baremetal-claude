//! Recursive descent Go parser.
//!
//! Parses Go source into AST nodes: package declaration, imports, top-level
//! declarations (func, var, const, type, struct, interface), statements
//! (if, for, switch, select, go, defer, return, assign, short declare :=),
//! and expressions with Go precedence.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use crate::ast::*;
use crate::lexer::{Token, TokenKind, Span};
use crate::types::*;

/// Parser state.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &TokenKind {
        self.tokens.get(self.pos).map(|t| &t.kind).unwrap_or(&TokenKind::Eof)
    }

    fn span(&self) -> Span {
        self.tokens.get(self.pos).map(|t| t.span).unwrap_or_default()
    }

    fn advance(&mut self) -> &TokenKind {
        let tok = &self.tokens[self.pos].kind;
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!(
                "{}:{}: expected {:?}, got {:?}",
                self.span().line, self.span().col, expected, self.peek()
            ))
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_semicolons(&mut self) {
        while self.eat(&TokenKind::Semicolon) {}
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            TokenKind::Ident(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(format!(
                "{}:{}: expected identifier, got {:?}",
                self.span().line, self.span().col, self.peek()
            )),
        }
    }

    /// Parse a complete Go source file.
    pub fn parse_file(&mut self) -> Result<Package, String> {
        let span = self.span();

        // package declaration
        self.expect(&TokenKind::Package)?;
        let name = self.expect_ident()?;
        self.eat_semicolons();

        // imports
        let mut imports = Vec::new();
        while *self.peek() == TokenKind::Import {
            self.advance();
            if self.eat(&TokenKind::LParen) {
                // grouped import
                while *self.peek() != TokenKind::RParen && *self.peek() != TokenKind::Eof {
                    imports.push(self.parse_import_spec()?);
                    self.eat_semicolons();
                }
                self.expect(&TokenKind::RParen)?;
            } else {
                imports.push(self.parse_import_spec()?);
            }
            self.eat_semicolons();
        }

        // top-level declarations
        let mut decls = Vec::new();
        while *self.peek() != TokenKind::Eof {
            decls.push(self.parse_top_level_decl()?);
            self.eat_semicolons();
        }

        Ok(Package { name, imports, decls, span })
    }

    fn parse_import_spec(&mut self) -> Result<ImportDecl, String> {
        let span = self.span();
        let mut alias = None;

        // Check for alias
        if let TokenKind::Ident(_) = self.peek() {
            let ident = self.expect_ident()?;
            alias = Some(ident);
        } else if *self.peek() == TokenKind::Dot {
            self.advance();
            alias = Some(String::from("."));
        }

        // Import path (string literal)
        let path = match self.peek().clone() {
            TokenKind::StringLit(ref s) => {
                let s = s.clone();
                self.advance();
                s
            }
            _ => return Err(format!("{}:{}: expected import path string", span.line, span.col)),
        };

        Ok(ImportDecl { alias, path, span })
    }

    fn parse_top_level_decl(&mut self) -> Result<TopLevelDecl, String> {
        match self.peek() {
            TokenKind::Func => self.parse_func_decl().map(TopLevelDecl::Func),
            TokenKind::Var => self.parse_var_decl().map(TopLevelDecl::Var),
            TokenKind::Const => self.parse_const_decl().map(TopLevelDecl::Const),
            TokenKind::Type => self.parse_type_decl().map(TopLevelDecl::Type),
            _ => Err(format!(
                "{}:{}: expected top-level declaration (func/var/const/type), got {:?}",
                self.span().line, self.span().col, self.peek()
            )),
        }
    }

    fn parse_func_decl(&mut self) -> Result<FuncDecl, String> {
        let span = self.span();
        self.expect(&TokenKind::Func)?;

        // Check for method receiver
        let receiver = if *self.peek() == TokenKind::LParen {
            let saved = self.pos;
            self.advance();
            // Try to parse as receiver
            if let Ok(recv) = self.try_parse_receiver() {
                Some(recv)
            } else {
                self.pos = saved;
                None
            }
        } else {
            None
        };

        let name = self.expect_ident()?;

        // Parameters
        self.expect(&TokenKind::LParen)?;
        let (params, is_variadic) = self.parse_param_list()?;
        self.expect(&TokenKind::RParen)?;

        // Return types
        let returns = self.parse_return_types()?;

        // Body
        let body = self.parse_block()?;

        Ok(FuncDecl {
            name,
            receiver,
            params,
            returns,
            body,
            is_variadic,
            span,
        })
    }

    fn try_parse_receiver(&mut self) -> Result<Receiver, String> {
        let name = self.expect_ident()?;
        let is_pointer = self.eat(&TokenKind::Star);
        let ty_name = self.expect_ident()?;
        self.expect(&TokenKind::RParen)?;

        Ok(Receiver {
            name,
            ty: GoType::Named(ty_name),
            is_pointer,
        })
    }

    fn parse_param_list(&mut self) -> Result<(Vec<Param>, bool), String> {
        let mut params = Vec::new();
        let mut is_variadic = false;

        if *self.peek() == TokenKind::RParen {
            return Ok((params, false));
        }

        loop {
            if self.eat(&TokenKind::Ellipsis) {
                is_variadic = true;
            }

            let name = self.expect_ident()?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        Ok((params, is_variadic))
    }

    fn parse_return_types(&mut self) -> Result<Vec<GoType>, String> {
        if *self.peek() == TokenKind::LBrace {
            return Ok(Vec::new());
        }

        if *self.peek() == TokenKind::LParen {
            self.advance();
            let mut types = Vec::new();
            while *self.peek() != TokenKind::RParen {
                // Skip optional parameter names in named returns
                if let TokenKind::Ident(_) = self.peek() {
                    let saved = self.pos;
                    let _name = self.expect_ident()?;
                    if self.is_type_start() {
                        types.push(self.parse_type()?);
                    } else {
                        self.pos = saved;
                        types.push(self.parse_type()?);
                    }
                } else {
                    types.push(self.parse_type()?);
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
            Ok(types)
        } else if self.is_type_start() {
            Ok(alloc::vec![self.parse_type()?])
        } else {
            Ok(Vec::new())
        }
    }

    fn is_type_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Ident(_)
                | TokenKind::Star
                | TokenKind::LBracket
                | TokenKind::Map
                | TokenKind::Chan
                | TokenKind::Func
                | TokenKind::Struct
                | TokenKind::Interface
                | TokenKind::Arrow
        )
    }

    fn parse_type(&mut self) -> Result<GoType, String> {
        match self.peek().clone() {
            TokenKind::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                let ty = match name.as_str() {
                    "bool" => GoType::Bool,
                    "int" => GoType::Int,
                    "int8" => GoType::Int8,
                    "int16" => GoType::Int16,
                    "int32" => GoType::Int32,
                    "int64" => GoType::Int64,
                    "uint" => GoType::Uint,
                    "uint8" => GoType::Uint8,
                    "uint16" => GoType::Uint16,
                    "uint32" => GoType::Uint32,
                    "uint64" => GoType::Uint64,
                    "uintptr" => GoType::Uintptr,
                    "float32" => GoType::Float32,
                    "float64" => GoType::Float64,
                    "complex64" => GoType::Complex64,
                    "complex128" => GoType::Complex128,
                    "string" => GoType::String,
                    "byte" => GoType::Byte,
                    "rune" => GoType::Rune,
                    "error" => GoType::Named(String::from("error")),
                    _ => {
                        // Check for qualified name: pkg.Type
                        if self.eat(&TokenKind::Dot) {
                            let field = self.expect_ident()?;
                            GoType::Qualified(name, field)
                        } else {
                            GoType::Named(name)
                        }
                    }
                };
                Ok(ty)
            }
            TokenKind::Star => {
                self.advance();
                let inner = self.parse_type()?;
                Ok(GoType::Pointer(Box::new(inner)))
            }
            TokenKind::LBracket => {
                self.advance();
                if self.eat(&TokenKind::RBracket) {
                    // Slice: []T
                    let elem = self.parse_type()?;
                    Ok(GoType::Slice(Box::new(elem)))
                } else {
                    // Array: [N]T
                    let size = match self.peek().clone() {
                        TokenKind::IntLit(n) => { self.advance(); n as usize }
                        _ => return Err(format!("{}:{}: expected array size", self.span().line, self.span().col)),
                    };
                    self.expect(&TokenKind::RBracket)?;
                    let elem = self.parse_type()?;
                    Ok(GoType::Array(Box::new(elem), size))
                }
            }
            TokenKind::Map => {
                self.advance();
                self.expect(&TokenKind::LBracket)?;
                let key = self.parse_type()?;
                self.expect(&TokenKind::RBracket)?;
                let value = self.parse_type()?;
                Ok(GoType::Map(Box::new(key), Box::new(value)))
            }
            TokenKind::Chan => {
                self.advance();
                let dir = if self.eat(&TokenKind::Arrow) {
                    ChanDir::Send
                } else {
                    ChanDir::Both
                };
                let elem = self.parse_type()?;
                Ok(GoType::Chan(Box::new(elem), dir))
            }
            TokenKind::Arrow => {
                self.advance(); // <-
                self.expect(&TokenKind::Chan)?;
                let elem = self.parse_type()?;
                Ok(GoType::Chan(Box::new(elem), ChanDir::Recv))
            }
            TokenKind::Func => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let mut params = Vec::new();
                let mut variadic = false;
                while *self.peek() != TokenKind::RParen {
                    if self.eat(&TokenKind::Ellipsis) {
                        variadic = true;
                    }
                    params.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
                let returns = self.parse_return_types()?;
                Ok(GoType::Func { params, returns, variadic })
            }
            TokenKind::Struct => {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                let mut fields = Vec::new();
                while *self.peek() != TokenKind::RBrace {
                    let name = self.expect_ident()?;
                    let ty = self.parse_type()?;
                    fields.push(StructFieldType { name, ty });
                    self.eat_semicolons();
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(GoType::Struct(fields))
            }
            TokenKind::Interface => {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                let mut methods = Vec::new();
                while *self.peek() != TokenKind::RBrace {
                    let name = self.expect_ident()?;
                    self.expect(&TokenKind::LParen)?;
                    let mut params = Vec::new();
                    while *self.peek() != TokenKind::RParen {
                        params.push(self.parse_type()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    let returns = self.parse_return_types()?;
                    methods.push(InterfaceMethodType { name, params, returns });
                    self.eat_semicolons();
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(GoType::Interface(methods))
            }
            _ => Err(format!(
                "{}:{}: expected type, got {:?}",
                self.span().line, self.span().col, self.peek()
            )),
        }
    }

    fn parse_var_decl(&mut self) -> Result<VarDecl, String> {
        let span = self.span();
        self.expect(&TokenKind::Var)?;

        let mut names = Vec::new();
        names.push(self.expect_ident()?);
        while self.eat(&TokenKind::Comma) {
            names.push(self.expect_ident()?);
        }

        let ty = if self.is_type_start() {
            Some(self.parse_type()?)
        } else {
            None
        };

        let values = if self.eat(&TokenKind::Assign) {
            self.parse_expr_list()?
        } else {
            Vec::new()
        };

        Ok(VarDecl { names, ty, values, span })
    }

    fn parse_const_decl(&mut self) -> Result<ConstDecl, String> {
        let span = self.span();
        self.expect(&TokenKind::Const)?;

        let mut names = Vec::new();
        names.push(self.expect_ident()?);
        while self.eat(&TokenKind::Comma) {
            names.push(self.expect_ident()?);
        }

        let ty = if self.is_type_start() {
            Some(self.parse_type()?)
        } else {
            None
        };

        let values = if self.eat(&TokenKind::Assign) {
            self.parse_expr_list()?
        } else {
            Vec::new()
        };

        Ok(ConstDecl { names, ty, values, span })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, String> {
        let span = self.span();
        self.expect(&TokenKind::Type)?;
        let name = self.expect_ident()?;

        let ty = match self.peek() {
            TokenKind::Struct => {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                let mut fields = Vec::new();
                while *self.peek() != TokenKind::RBrace {
                    let fname = self.expect_ident()?;
                    let fty = self.parse_type()?;
                    let tag = if let TokenKind::StringLit(s) = self.peek() {
                        let s = s.clone();
                        self.advance();
                        Some(s)
                    } else if let TokenKind::RawStringLit(s) = self.peek() {
                        let s = s.clone();
                        self.advance();
                        Some(s)
                    } else {
                        None
                    };
                    fields.push(StructField { name: fname, ty: fty, tag });
                    self.eat_semicolons();
                }
                self.expect(&TokenKind::RBrace)?;
                TypeDef::Struct(StructType { fields })
            }
            TokenKind::Interface => {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                let mut methods = Vec::new();
                let mut embedded = Vec::new();
                while *self.peek() != TokenKind::RBrace {
                    let mname = self.expect_ident()?;
                    if *self.peek() == TokenKind::LParen {
                        self.advance();
                        let mut params = Vec::new();
                        while *self.peek() != TokenKind::RParen {
                            params.push(self.parse_type()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RParen)?;
                        let returns = self.parse_return_types()?;
                        methods.push(InterfaceMethod { name: mname, params, returns });
                    } else {
                        // Embedded interface
                        embedded.push(mname);
                    }
                    self.eat_semicolons();
                }
                self.expect(&TokenKind::RBrace)?;
                TypeDef::Interface(InterfaceType { methods, embedded })
            }
            _ => {
                let ty = self.parse_type()?;
                TypeDef::Alias(ty)
            }
        };

        Ok(TypeDecl { name, ty, span })
    }

    fn parse_block(&mut self) -> Result<Block, String> {
        self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
            self.eat_semicolons();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block { stmts })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        match self.peek() {
            TokenKind::Var => self.parse_var_decl().map(Stmt::VarDecl),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Switch => self.parse_switch_stmt(),
            TokenKind::Select => self.parse_select_stmt(),
            TokenKind::Go => {
                self.advance();
                let expr = self.parse_expr()?;
                Ok(Stmt::Go(expr))
            }
            TokenKind::Defer => {
                self.advance();
                let expr = self.parse_expr()?;
                Ok(Stmt::Defer(expr))
            }
            TokenKind::Break => {
                self.advance();
                let label = if let TokenKind::Ident(s) = self.peek() {
                    let s = s.clone();
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                Ok(Stmt::Break(label))
            }
            TokenKind::Continue => {
                self.advance();
                let label = if let TokenKind::Ident(s) = self.peek() {
                    let s = s.clone();
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                Ok(Stmt::Continue(label))
            }
            TokenKind::Goto => {
                self.advance();
                let label = self.expect_ident()?;
                Ok(Stmt::Goto(label))
            }
            TokenKind::Fallthrough => {
                self.advance();
                Ok(Stmt::Fallthrough)
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Stmt::Block(block))
            }
            _ => self.parse_simple_stmt(),
        }
    }

    fn parse_simple_stmt(&mut self) -> Result<Stmt, String> {
        let exprs = self.parse_expr_list()?;

        // Check for short variable declaration: x, y := ...
        if *self.peek() == TokenKind::ColonAssign {
            self.advance();
            let values = self.parse_expr_list()?;
            let names: Vec<String> = exprs.into_iter().map(|e| {
                if let Expr::Ident(name) = e {
                    Ok(name)
                } else {
                    Err(String::from("expected identifier in short declaration"))
                }
            }).collect::<Result<_, _>>()?;
            return Ok(Stmt::ShortDecl { names, values });
        }

        // Check for assignment
        if let Some(op) = self.try_parse_assign_op() {
            let rhs = self.parse_expr_list()?;
            return Ok(Stmt::Assign { op, lhs: exprs, rhs });
        }

        // Check for send: ch <- val
        if *self.peek() == TokenKind::Arrow && exprs.len() == 1 {
            self.advance();
            let value = self.parse_expr()?;
            return Ok(Stmt::Send { channel: exprs.into_iter().next().unwrap(), value });
        }

        // Check for inc/dec
        if exprs.len() == 1 {
            if self.eat(&TokenKind::PlusPlus) {
                return Ok(Stmt::Inc(exprs.into_iter().next().unwrap()));
            }
            if self.eat(&TokenKind::MinusMinus) {
                return Ok(Stmt::Dec(exprs.into_iter().next().unwrap()));
            }
        }

        // Check for label
        if exprs.len() == 1 {
            if let Expr::Ident(ref name) = exprs[0] {
                if self.eat(&TokenKind::Colon) {
                    let label = name.clone();
                    let stmt = self.parse_stmt()?;
                    return Ok(Stmt::Label(label, Box::new(stmt)));
                }
            }
        }

        // Expression statement
        if exprs.len() == 1 {
            Ok(Stmt::Expr(exprs.into_iter().next().unwrap()))
        } else {
            Err(format!("{}:{}: unexpected expression list", self.span().line, self.span().col))
        }
    }

    fn try_parse_assign_op(&mut self) -> Option<AssignOp> {
        let op = match self.peek() {
            TokenKind::Assign => AssignOp::Assign,
            TokenKind::PlusAssign => AssignOp::AddAssign,
            TokenKind::MinusAssign => AssignOp::SubAssign,
            TokenKind::StarAssign => AssignOp::MulAssign,
            TokenKind::SlashAssign => AssignOp::DivAssign,
            TokenKind::PercentAssign => AssignOp::ModAssign,
            TokenKind::AmpAssign => AssignOp::AndAssign,
            TokenKind::PipeAssign => AssignOp::OrAssign,
            TokenKind::CaretAssign => AssignOp::XorAssign,
            TokenKind::ShlAssign => AssignOp::ShlAssign,
            TokenKind::ShrAssign => AssignOp::ShrAssign,
            TokenKind::AmpCaretAssign => AssignOp::AndNotAssign,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&TokenKind::Return)?;
        if *self.peek() == TokenKind::Semicolon || *self.peek() == TokenKind::RBrace || *self.peek() == TokenKind::Eof {
            Ok(Stmt::Return(Vec::new()))
        } else {
            let exprs = self.parse_expr_list()?;
            Ok(Stmt::Return(exprs))
        }
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&TokenKind::If)?;

        // Optional init statement
        let (init, cond) = self.parse_if_header()?;

        let body = self.parse_block()?;

        let else_body = if self.eat(&TokenKind::Else) {
            if *self.peek() == TokenKind::If {
                Some(ElseClause::If(Box::new(self.parse_if_stmt()?)))
            } else {
                Some(ElseClause::Block(self.parse_block()?))
            }
        } else {
            None
        };

        Ok(Stmt::If { init, cond, body, else_body })
    }

    fn parse_if_header(&mut self) -> Result<(Option<Box<Stmt>>, Expr), String> {
        // Try parsing: simple_stmt ; expr
        let saved = self.pos;
        let first_exprs = self.parse_expr_list()?;

        if *self.peek() == TokenKind::ColonAssign {
            // short decl as init: x := val; cond
            self.advance();
            let values = self.parse_expr_list()?;
            let names: Vec<String> = first_exprs.into_iter().map(|e| {
                if let Expr::Ident(name) = e {
                    Ok(name)
                } else {
                    Err(String::from("expected identifier"))
                }
            }).collect::<Result<_, _>>()?;
            self.expect(&TokenKind::Semicolon)?;
            let cond = self.parse_expr()?;
            return Ok((Some(Box::new(Stmt::ShortDecl { names, values })), cond));
        }

        if self.eat(&TokenKind::Semicolon) {
            // init; cond
            let init = if first_exprs.len() == 1 {
                Stmt::Expr(first_exprs.into_iter().next().unwrap())
            } else {
                self.pos = saved;
                return Err(format!("{}:{}: invalid if init statement", self.span().line, self.span().col));
            };
            let cond = self.parse_expr()?;
            return Ok((Some(Box::new(init)), cond));
        }

        // Just a condition
        if first_exprs.len() == 1 {
            Ok((None, first_exprs.into_iter().next().unwrap()))
        } else {
            Err(format!("{}:{}: expected single condition expression", self.span().line, self.span().col))
        }
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&TokenKind::For)?;

        // for { body } (infinite loop)
        if *self.peek() == TokenKind::LBrace {
            let body = self.parse_block()?;
            return Ok(Stmt::For { init: None, cond: None, post: None, body });
        }

        // Try to detect for-range
        let saved = self.pos;

        // for range expr { ... }
        if *self.peek() == TokenKind::Range {
            self.advance();
            let iter = self.parse_expr()?;
            let body = self.parse_block()?;
            return Ok(Stmt::ForRange {
                key: None,
                value: None,
                iter,
                body,
                is_assign: false,
            });
        }

        // Try: for key, value := range expr { ... }
        if let Ok(first_exprs) = {
            self.pos = saved;
            self.parse_expr_list()
        } {
            if *self.peek() == TokenKind::ColonAssign || *self.peek() == TokenKind::Assign {
                let is_assign = *self.peek() == TokenKind::Assign;
                self.advance();
                if self.eat(&TokenKind::Range) {
                    let iter = self.parse_expr()?;
                    let body = self.parse_block()?;
                    let key = first_exprs.first().and_then(|e| {
                        if let Expr::Ident(s) = e { Some(s.clone()) } else { None }
                    });
                    let value = first_exprs.get(1).and_then(|e| {
                        if let Expr::Ident(s) = e { Some(s.clone()) } else { None }
                    });
                    return Ok(Stmt::ForRange { key, value, iter, body, is_assign });
                }
                // Not a range, it's init := value; cond; post
                let values = self.parse_expr_list()?;
                let names: Result<Vec<String>, _> = first_exprs.into_iter().map(|e| {
                    if let Expr::Ident(name) = e {
                        Ok(name)
                    } else {
                        Err(String::from("expected ident"))
                    }
                }).collect();
                let init = if is_assign {
                    Stmt::Assign {
                        op: AssignOp::Assign,
                        lhs: names?.into_iter().map(Expr::Ident).collect(),
                        rhs: values,
                    }
                } else {
                    Stmt::ShortDecl { names: names?, values }
                };
                self.expect(&TokenKind::Semicolon)?;
                let cond = if *self.peek() != TokenKind::Semicolon {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semicolon)?;
                let post = if *self.peek() != TokenKind::LBrace {
                    Some(Box::new(self.parse_simple_stmt()?))
                } else {
                    None
                };
                let body = self.parse_block()?;
                return Ok(Stmt::For { init: Some(Box::new(init)), cond, post, body });
            }

            // for cond { body } (while-style)
            if first_exprs.len() == 1 && *self.peek() == TokenKind::LBrace {
                let body = self.parse_block()?;
                return Ok(Stmt::For {
                    init: None,
                    cond: Some(first_exprs.into_iter().next().unwrap()),
                    post: None,
                    body,
                });
            }

            // for init; cond; post { body } where init is expression
            if first_exprs.len() == 1 && *self.peek() == TokenKind::Semicolon {
                self.advance();
                let init = Stmt::Expr(first_exprs.into_iter().next().unwrap());
                let cond = if *self.peek() != TokenKind::Semicolon {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semicolon)?;
                let post = if *self.peek() != TokenKind::LBrace {
                    Some(Box::new(self.parse_simple_stmt()?))
                } else {
                    None
                };
                let body = self.parse_block()?;
                return Ok(Stmt::For { init: Some(Box::new(init)), cond, post, body });
            }
        }

        Err(format!("{}:{}: invalid for statement", self.span().line, self.span().col))
    }

    fn parse_switch_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&TokenKind::Switch)?;

        let init = None;
        let tag = if *self.peek() != TokenKind::LBrace {
            Some(self.parse_expr()?)
        } else {
            None
        };

        self.expect(&TokenKind::LBrace)?;
        let mut cases = Vec::new();
        while *self.peek() != TokenKind::RBrace {
            if self.eat(&TokenKind::Case) {
                let exprs = self.parse_expr_list()?;
                self.expect(&TokenKind::Colon)?;
                let mut body = Vec::new();
                while !matches!(self.peek(), TokenKind::Case | TokenKind::Default | TokenKind::RBrace) {
                    body.push(self.parse_stmt()?);
                    self.eat_semicolons();
                }
                cases.push(SwitchCase { exprs, is_default: false, body });
            } else if self.eat(&TokenKind::Default) {
                self.expect(&TokenKind::Colon)?;
                let mut body = Vec::new();
                while !matches!(self.peek(), TokenKind::Case | TokenKind::Default | TokenKind::RBrace) {
                    body.push(self.parse_stmt()?);
                    self.eat_semicolons();
                }
                cases.push(SwitchCase { exprs: Vec::new(), is_default: true, body });
            } else {
                return Err(format!("{}:{}: expected case or default", self.span().line, self.span().col));
            }
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::Switch { init, tag, cases })
    }

    fn parse_select_stmt(&mut self) -> Result<Stmt, String> {
        self.expect(&TokenKind::Select)?;
        self.expect(&TokenKind::LBrace)?;

        let mut cases = Vec::new();
        while *self.peek() != TokenKind::RBrace {
            if self.eat(&TokenKind::Case) {
                let comm = self.parse_stmt()?;
                self.expect(&TokenKind::Colon)?;
                let mut body = Vec::new();
                while !matches!(self.peek(), TokenKind::Case | TokenKind::Default | TokenKind::RBrace) {
                    body.push(self.parse_stmt()?);
                    self.eat_semicolons();
                }
                cases.push(SelectCase { comm: Some(comm), is_default: false, body });
            } else if self.eat(&TokenKind::Default) {
                self.expect(&TokenKind::Colon)?;
                let mut body = Vec::new();
                while !matches!(self.peek(), TokenKind::Case | TokenKind::Default | TokenKind::RBrace) {
                    body.push(self.parse_stmt()?);
                    self.eat_semicolons();
                }
                cases.push(SelectCase { comm: None, is_default: true, body });
            }
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::Select { cases })
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut exprs = Vec::new();
        exprs.push(self.parse_expr()?);
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr()?);
        }
        Ok(exprs)
    }

    // === Expression parsing with Go precedence ===

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_and_expr()?;
        while self.eat(&TokenKind::PipePipe) {
            let rhs = self.parse_and_expr()?;
            lhs = Expr::Binary { op: BinOp::LogOr, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_comparison_expr()?;
        while self.eat(&TokenKind::AmpAmp) {
            let rhs = self.parse_comparison_expr()?;
            lhs = Expr::Binary { op: BinOp::LogAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_comparison_expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_add_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::Ne => BinOp::Ne,
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::Ge => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_add_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_add_expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                TokenKind::Pipe => BinOp::BitOr,
                TokenKind::Caret => BinOp::BitXor,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_mul_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_mul_expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                TokenKind::Amp => BinOp::BitAnd,
                TokenKind::AmpCaret => BinOp::AndNot,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary_expr()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, String> {
        match self.peek() {
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::Unary { op: UnaryOp::Neg, operand: Box::new(operand) })
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::Unary { op: UnaryOp::LogNot, operand: Box::new(operand) })
            }
            TokenKind::Caret => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::Unary { op: UnaryOp::BitNot, operand: Box::new(operand) })
            }
            TokenKind::Amp => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::AddrOf(Box::new(operand)))
            }
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::Deref(Box::new(operand)))
            }
            TokenKind::Arrow => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::Receive(Box::new(operand)))
            }
            _ => self.parse_primary_expr(),
        }
    }

    fn parse_primary_expr(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_operand()?;

        // Postfix operations: call, index, selector, slice, type assertion
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    while *self.peek() != TokenKind::RParen {
                        args.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    // Allow trailing ellipsis for variadic calls
                    self.eat(&TokenKind::Ellipsis);
                    self.expect(&TokenKind::RParen)?;
                    expr = Expr::Call { func: Box::new(expr), args };
                }
                TokenKind::LBracket => {
                    self.advance();
                    // Slice or index
                    if self.eat(&TokenKind::Colon) {
                        // [:high] or [:high:max]
                        let high = if *self.peek() != TokenKind::RBracket && *self.peek() != TokenKind::Colon {
                            Some(Box::new(self.parse_expr()?))
                        } else {
                            None
                        };
                        let max = if self.eat(&TokenKind::Colon) {
                            Some(Box::new(self.parse_expr()?))
                        } else {
                            None
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice { expr: Box::new(expr), low: None, high, max };
                    } else {
                        let index = self.parse_expr()?;
                        if self.eat(&TokenKind::Colon) {
                            // [low:high] or [low:high:max]
                            let high = if *self.peek() != TokenKind::RBracket && *self.peek() != TokenKind::Colon {
                                Some(Box::new(self.parse_expr()?))
                            } else {
                                None
                            };
                            let max = if self.eat(&TokenKind::Colon) {
                                Some(Box::new(self.parse_expr()?))
                            } else {
                                None
                            };
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Slice {
                                expr: Box::new(expr),
                                low: Some(Box::new(index)),
                                high,
                                max,
                            };
                        } else {
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Index { expr: Box::new(expr), index: Box::new(index) };
                        }
                    }
                }
                TokenKind::Dot => {
                    self.advance();
                    if self.eat(&TokenKind::LParen) {
                        // Type assertion: x.(Type)
                        let ty = self.parse_type()?;
                        self.expect(&TokenKind::RParen)?;
                        expr = Expr::TypeAssert { expr: Box::new(expr), ty };
                    } else {
                        let field = self.expect_ident()?;
                        expr = Expr::Selector { expr: Box::new(expr), field };
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_operand(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            TokenKind::IntLit(n) => { self.advance(); Ok(Expr::IntLit(n)) }
            TokenKind::FloatLit(f) => { self.advance(); Ok(Expr::FloatLit(f)) }
            TokenKind::StringLit(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s))
            }
            TokenKind::RawStringLit(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s))
            }
            TokenKind::RuneLit(ch) => { self.advance(); Ok(Expr::RuneLit(ch)) }
            TokenKind::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                match name.as_str() {
                    "true" => Ok(Expr::BoolLit(true)),
                    "false" => Ok(Expr::BoolLit(false)),
                    "nil" => Ok(Expr::Nil),
                    "make" => {
                        self.expect(&TokenKind::LParen)?;
                        let ty = self.parse_type()?;
                        let mut args = Vec::new();
                        while self.eat(&TokenKind::Comma) {
                            args.push(self.parse_expr()?);
                        }
                        self.expect(&TokenKind::RParen)?;
                        Ok(Expr::Make { ty, args })
                    }
                    "new" => {
                        self.expect(&TokenKind::LParen)?;
                        let ty = self.parse_type()?;
                        self.expect(&TokenKind::RParen)?;
                        Ok(Expr::New(ty))
                    }
                    _ => {
                        // Check if this is a composite literal: Type{...}
                        if *self.peek() == TokenKind::LBrace {
                            let ty = GoType::Named(name);
                            self.advance();
                            let mut elts = Vec::new();
                            while *self.peek() != TokenKind::RBrace {
                                let first = self.parse_expr()?;
                                if self.eat(&TokenKind::Colon) {
                                    let value = self.parse_expr()?;
                                    elts.push(KeyValue { key: Some(first), value });
                                } else {
                                    elts.push(KeyValue { key: None, value: first });
                                }
                                if !self.eat(&TokenKind::Comma) {
                                    break;
                                }
                            }
                            self.expect(&TokenKind::RBrace)?;
                            Ok(Expr::CompositeLit { ty, elts })
                        } else {
                            Ok(Expr::Ident(name))
                        }
                    }
                }
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::Func => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let (params, _) = self.parse_param_list()?;
                self.expect(&TokenKind::RParen)?;
                let returns = self.parse_return_types()?;
                let body = self.parse_block()?;
                Ok(Expr::FuncLit { params, returns, body })
            }
            _ => Err(format!(
                "{}:{}: expected expression, got {:?}",
                self.span().line, self.span().col, self.peek()
            )),
        }
    }
}

/// Parse Go source tokens into a package AST.
pub fn parse_tokens(tokens: &[Token]) -> Result<Package, String> {
    let mut parser = Parser::new(tokens);
    parser.parse_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    #[test]
    fn test_parse_hello_world() {
        let tokens = tokenize(r#"
            package main

            func main() {
                fmt.Println("hello")
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        assert_eq!(pkg.name, "main");
        assert_eq!(pkg.decls.len(), 1);
    }

    #[test]
    fn test_parse_var_decl() {
        let tokens = tokenize(r#"
            package main
            var x int = 42
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        assert_eq!(pkg.decls.len(), 1);
    }

    #[test]
    fn test_parse_for_loop() {
        let tokens = tokenize(r#"
            package main
            func main() {
                for i := 0; i < 10; i++ {
                    x := i * 2
                }
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        assert_eq!(pkg.decls.len(), 1);
    }

    #[test]
    fn test_parse_if_else() {
        let tokens = tokenize(r#"
            package main
            func main() {
                if x > 0 {
                    y := 1
                } else {
                    y := 2
                }
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        assert_eq!(pkg.decls.len(), 1);
    }

    #[test]
    fn test_parse_struct_type() {
        let tokens = tokenize(r#"
            package main
            type Point struct {
                X int
                Y int
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        assert_eq!(pkg.decls.len(), 1);
    }
}
