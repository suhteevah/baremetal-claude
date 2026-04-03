//! C++ parser extending C with classes, templates, namespaces, lambdas.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::*;
use crate::lexer::{CppToken, CppTokenKind, Span};

/// C++ parser state.
pub struct CppParser<'a> {
    tokens: &'a [CppToken],
    pos: usize,
}

impl<'a> CppParser<'a> {
    pub fn new(tokens: &'a [CppToken]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &CppTokenKind {
        self.tokens.get(self.pos).map(|t| &t.kind).unwrap_or(&CppTokenKind::Eof)
    }

    fn span(&self) -> Span {
        self.tokens.get(self.pos).map(|t| t.span).unwrap_or_default()
    }

    fn advance(&mut self) -> &CppTokenKind {
        let tok = &self.tokens[self.pos].kind;
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &CppTokenKind) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(alloc::format!("{}:{}: expected {:?}, got {:?}",
                self.span().line, self.span().col, expected, self.peek()))
        }
    }

    fn eat(&mut self, kind: &CppTokenKind) -> bool {
        if self.peek() == kind { self.advance(); true } else { false }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            CppTokenKind::Ident(ref s) => { let s = s.clone(); self.advance(); Ok(s) }
            _ => Err(alloc::format!("{}:{}: expected identifier, got {:?}",
                self.span().line, self.span().col, self.peek())),
        }
    }

    /// Parse a C++ translation unit.
    pub fn parse(&mut self) -> Result<CppTranslationUnit, String> {
        let mut decls = Vec::new();
        while *self.peek() != CppTokenKind::Eof {
            decls.push(self.parse_decl()?);
        }
        Ok(CppTranslationUnit { decls })
    }

    fn parse_decl(&mut self) -> Result<CppDecl, String> {
        match self.peek() {
            CppTokenKind::Class => self.parse_class_def(false),
            CppTokenKind::Namespace => self.parse_namespace(),
            CppTokenKind::Template => self.parse_template(),
            CppTokenKind::Using => self.parse_using(),
            _ => {
                // Try to parse as function or variable declaration
                self.parse_func_or_var_decl()
            }
        }
    }

    fn parse_class_def(&mut self, is_struct: bool) -> Result<CppDecl, String> {
        let span = self.span();
        self.advance(); // class or struct

        let name = self.expect_ident()?;
        let is_final = self.eat(&CppTokenKind::Final);

        // Base classes
        let mut bases = Vec::new();
        if self.eat(&CppTokenKind::Colon) {
            loop {
                let access = match self.peek() {
                    CppTokenKind::Public => { self.advance(); AccessSpec::Public }
                    CppTokenKind::Protected => { self.advance(); AccessSpec::Protected }
                    CppTokenKind::Private => { self.advance(); AccessSpec::Private }
                    _ => if is_struct { AccessSpec::Public } else { AccessSpec::Private },
                };
                let is_virtual = self.eat(&CppTokenKind::Virtual);
                let base_name = self.expect_ident()?;
                bases.push(BaseClass { name: base_name, access, is_virtual });
                if !self.eat(&CppTokenKind::Comma) { break; }
            }
        }

        // Members
        self.expect(&CppTokenKind::LBrace)?;
        let mut members = Vec::new();
        while *self.peek() != CppTokenKind::RBrace && *self.peek() != CppTokenKind::Eof {
            members.push(self.parse_class_member()?);
        }
        self.expect(&CppTokenKind::RBrace)?;
        self.eat(&CppTokenKind::Semicolon);

        Ok(CppDecl::ClassDef(ClassDef {
            name, is_struct, bases, members, is_final, span,
        }))
    }

    fn parse_class_member(&mut self) -> Result<ClassMember, String> {
        match self.peek() {
            CppTokenKind::Public => {
                self.advance();
                self.expect(&CppTokenKind::Colon)?;
                Ok(ClassMember::Access(AccessSpec::Public))
            }
            CppTokenKind::Private => {
                self.advance();
                self.expect(&CppTokenKind::Colon)?;
                Ok(ClassMember::Access(AccessSpec::Private))
            }
            CppTokenKind::Protected => {
                self.advance();
                self.expect(&CppTokenKind::Colon)?;
                Ok(ClassMember::Access(AccessSpec::Protected))
            }
            CppTokenKind::Virtual => {
                self.advance();
                // Virtual method or destructor
                if self.peek() == &CppTokenKind::Tilde {
                    // Virtual destructor
                    self.advance();
                    let name = self.expect_ident()?;
                    self.expect(&CppTokenKind::LParen)?;
                    self.expect(&CppTokenKind::RParen)?;
                    let body = self.parse_block()?;
                    Ok(ClassMember::Destructor(Destructor {
                        class_name: name, body, is_virtual: true, span: self.span(),
                    }))
                } else {
                    // Virtual method — parse as method with virtual flag
                    let ret_type = self.parse_type()?;
                    let name = self.expect_ident()?;
                    self.expect(&CppTokenKind::LParen)?;
                    let params = self.parse_param_list()?;
                    self.expect(&CppTokenKind::RParen)?;
                    let is_const = self.eat(&CppTokenKind::Ident(String::from("const")));
                    let is_override = self.eat(&CppTokenKind::Override);
                    let is_pure_virtual = if self.eat(&CppTokenKind::Assign) {
                        self.expect(&CppTokenKind::IntLit(0))?;
                        true
                    } else {
                        false
                    };
                    let body = if is_pure_virtual {
                        self.eat(&CppTokenKind::Semicolon);
                        None
                    } else if *self.peek() == CppTokenKind::Semicolon {
                        self.advance();
                        None
                    } else {
                        Some(self.parse_block()?)
                    };
                    Ok(ClassMember::Method(MethodDef {
                        name, return_type: ret_type, params, body,
                        is_virtual: true, is_override, is_final: false,
                        is_static: false, is_const, is_pure_virtual,
                        is_noexcept: false, access: AccessSpec::Public,
                        span: self.span(),
                    }))
                }
            }
            CppTokenKind::Tilde => {
                // Destructor
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&CppTokenKind::LParen)?;
                self.expect(&CppTokenKind::RParen)?;
                let body = self.parse_block()?;
                Ok(ClassMember::Destructor(Destructor {
                    class_name: name, body, is_virtual: false, span: self.span(),
                }))
            }
            CppTokenKind::Friend => {
                self.advance();
                let name = self.expect_ident()?;
                self.eat(&CppTokenKind::Semicolon);
                Ok(ClassMember::Friend(name))
            }
            _ => {
                // Method, constructor, or field — simplified parsing
                let ty = self.parse_type()?;
                let name = self.expect_ident()?;

                if *self.peek() == CppTokenKind::LParen {
                    // Method or constructor
                    self.advance();
                    let params = self.parse_param_list()?;
                    self.expect(&CppTokenKind::RParen)?;
                    let is_const = self.eat(&CppTokenKind::Ident(String::from("const")));
                    let body = if *self.peek() == CppTokenKind::Semicolon {
                        self.advance();
                        None
                    } else {
                        Some(self.parse_block()?)
                    };
                    Ok(ClassMember::Method(MethodDef {
                        name, return_type: ty, params, body,
                        is_virtual: false, is_override: false, is_final: false,
                        is_static: false, is_const, is_pure_virtual: false,
                        is_noexcept: false, access: AccessSpec::Public,
                        span: self.span(),
                    }))
                } else {
                    // Field
                    let init = if self.eat(&CppTokenKind::Assign) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.eat(&CppTokenKind::Semicolon);
                    Ok(ClassMember::Field(CppVarDecl {
                        name, ty, init, is_static: false, is_const: false,
                        is_constexpr: false, is_mutable: false, access: AccessSpec::Public,
                        span: self.span(),
                    }))
                }
            }
        }
    }

    fn parse_namespace(&mut self) -> Result<CppDecl, String> {
        let span = self.span();
        self.expect(&CppTokenKind::Namespace)?;
        let name = self.expect_ident()?;
        self.expect(&CppTokenKind::LBrace)?;
        let mut decls = Vec::new();
        while *self.peek() != CppTokenKind::RBrace && *self.peek() != CppTokenKind::Eof {
            decls.push(self.parse_decl()?);
        }
        self.expect(&CppTokenKind::RBrace)?;
        Ok(CppDecl::Namespace(NamespaceDef { name, decls, span }))
    }

    fn parse_template(&mut self) -> Result<CppDecl, String> {
        let span = self.span();
        self.expect(&CppTokenKind::Template)?;
        self.expect(&CppTokenKind::Lt)?;
        let mut params = Vec::new();
        while *self.peek() != CppTokenKind::Gt {
            let is_typename = self.eat(&CppTokenKind::Typename) || self.eat(&CppTokenKind::Class);
            let name = self.expect_ident()?;
            let default = if self.eat(&CppTokenKind::Assign) {
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(TemplateParam { name, is_typename, default });
            if !self.eat(&CppTokenKind::Comma) { break; }
        }
        self.expect(&CppTokenKind::Gt)?;
        let decl = self.parse_decl()?;
        Ok(CppDecl::Template(TemplateDef { params, decl: Box::new(decl), span }))
    }

    fn parse_using(&mut self) -> Result<CppDecl, String> {
        let span = self.span();
        self.expect(&CppTokenKind::Using)?;
        if self.eat(&CppTokenKind::Namespace) {
            let path = self.expect_ident()?;
            self.eat(&CppTokenKind::Semicolon);
            Ok(CppDecl::Using(UsingDecl { path, is_namespace: true, span }))
        } else {
            let name = self.expect_ident()?;
            if self.eat(&CppTokenKind::Assign) {
                let ty = self.parse_type()?;
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppDecl::TypeAlias { name, ty })
            } else {
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppDecl::Using(UsingDecl { path: name, is_namespace: false, span }))
            }
        }
    }

    fn parse_func_or_var_decl(&mut self) -> Result<CppDecl, String> {
        let span = self.span();
        let ty = self.parse_type()?;
        let name = self.expect_ident()?;

        if *self.peek() == CppTokenKind::LParen {
            // Function
            self.advance();
            let params = self.parse_param_list()?;
            self.expect(&CppTokenKind::RParen)?;
            let is_noexcept = self.eat(&CppTokenKind::Noexcept);
            let body = self.parse_block()?;
            Ok(CppDecl::FuncDef(CppFuncDef {
                name, qualified_name: None, return_type: ty, params, body,
                is_constexpr: false, is_noexcept, template_params: Vec::new(), span,
            }))
        } else {
            // Variable
            let init = if self.eat(&CppTokenKind::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.eat(&CppTokenKind::Semicolon);
            Ok(CppDecl::VarDecl(CppVarDecl {
                name, ty, init, is_static: false, is_const: false,
                is_constexpr: false, is_mutable: false, access: AccessSpec::Public, span,
            }))
        }
    }

    fn parse_type(&mut self) -> Result<CppType, String> {
        let mut is_const = self.eat(&CppTokenKind::Ident(String::from("const")));

        let base = match self.peek().clone() {
            CppTokenKind::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                match name.as_str() {
                    "void" => CppType::Void,
                    "int" => CppType::Int,
                    "long" => CppType::Long,
                    "char" => CppType::Char,
                    "float" => CppType::Float,
                    "double" => CppType::Double,
                    "const" => { is_const = true; return self.parse_type(); }
                    _ => {
                        if *self.peek() == CppTokenKind::ScopeRes {
                            self.advance();
                            let member = self.expect_ident()?;
                            CppType::Qualified(name, member)
                        } else if *self.peek() == CppTokenKind::Lt {
                            // Template type
                            self.advance();
                            let mut args = Vec::new();
                            while *self.peek() != CppTokenKind::Gt {
                                args.push(self.parse_type()?);
                                if !self.eat(&CppTokenKind::Comma) { break; }
                            }
                            self.expect(&CppTokenKind::Gt)?;
                            CppType::Template { name, args }
                        } else {
                            CppType::Named(name)
                        }
                    }
                }
            }
            CppTokenKind::Bool => { self.advance(); CppType::Bool }
            CppTokenKind::Auto => { self.advance(); CppType::Auto }
            _ => return Err(alloc::format!("{}:{}: expected type, got {:?}",
                self.span().line, self.span().col, self.peek())),
        };

        // Pointer/reference suffixes
        let ty = if self.eat(&CppTokenKind::Star) {
            CppType::Pointer(Box::new(base))
        } else if self.eat(&CppTokenKind::Amp) {
            if self.eat(&CppTokenKind::Amp) {
                CppType::RvalueRef(Box::new(base))
            } else {
                CppType::Reference(Box::new(base))
            }
        } else {
            base
        };

        if is_const {
            Ok(CppType::Const(Box::new(ty)))
        } else {
            Ok(ty)
        }
    }

    fn parse_param_list(&mut self) -> Result<Vec<CppParam>, String> {
        let mut params = Vec::new();
        if *self.peek() == CppTokenKind::RParen {
            return Ok(params);
        }
        loop {
            let is_const = self.eat(&CppTokenKind::Ident(String::from("const")));
            let ty = self.parse_type()?;
            let name = if let CppTokenKind::Ident(_) = self.peek() {
                Some(self.expect_ident()?)
            } else {
                None
            };
            let default_value = if self.eat(&CppTokenKind::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            params.push(CppParam { name, ty, default_value, is_const });
            if !self.eat(&CppTokenKind::Comma) { break; }
        }
        Ok(params)
    }

    fn parse_block(&mut self) -> Result<CppBlock, String> {
        self.expect(&CppTokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != CppTokenKind::RBrace && *self.peek() != CppTokenKind::Eof {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&CppTokenKind::RBrace)?;
        Ok(CppBlock { stmts })
    }

    fn parse_stmt(&mut self) -> Result<CppStmt, String> {
        match self.peek() {
            CppTokenKind::Ident(ref s) if s == "return" => {
                self.advance();
                if *self.peek() == CppTokenKind::Semicolon {
                    self.advance();
                    Ok(CppStmt::Return(None))
                } else {
                    let expr = self.parse_expr()?;
                    self.eat(&CppTokenKind::Semicolon);
                    Ok(CppStmt::Return(Some(expr)))
                }
            }
            CppTokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(CppStmt::Block(block))
            }
            CppTokenKind::Ident(ref s) if s == "if" => {
                self.advance();
                self.expect(&CppTokenKind::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&CppTokenKind::RParen)?;
                let then_body = Box::new(self.parse_stmt()?);
                let else_body = if self.eat(&CppTokenKind::Ident(String::from("else"))) {
                    Some(Box::new(self.parse_stmt()?))
                } else {
                    None
                };
                Ok(CppStmt::If { cond, then_body, else_body })
            }
            CppTokenKind::Ident(ref s) if s == "while" => {
                self.advance();
                self.expect(&CppTokenKind::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&CppTokenKind::RParen)?;
                let body = Box::new(self.parse_stmt()?);
                Ok(CppStmt::While { cond, body })
            }
            CppTokenKind::Ident(ref s) if s == "for" => {
                self.advance();
                self.expect(&CppTokenKind::LParen)?;
                // Simplified: just parse 3-clause for
                let init = if *self.peek() != CppTokenKind::Semicolon {
                    Some(Box::new(self.parse_stmt()?))
                } else {
                    self.advance();
                    None
                };
                let cond = if *self.peek() != CppTokenKind::Semicolon {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.eat(&CppTokenKind::Semicolon);
                let incr = if *self.peek() != CppTokenKind::RParen {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&CppTokenKind::RParen)?;
                let body = Box::new(self.parse_stmt()?);
                Ok(CppStmt::For { init, cond, incr, body })
            }
            CppTokenKind::Try => {
                self.advance();
                let try_body = self.parse_block()?;
                let mut catches = Vec::new();
                while self.eat(&CppTokenKind::Catch) {
                    self.expect(&CppTokenKind::LParen)?;
                    let param_type = self.parse_type()?;
                    let param_name = if let CppTokenKind::Ident(_) = self.peek() {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    self.expect(&CppTokenKind::RParen)?;
                    let body = self.parse_block()?;
                    catches.push(CatchClause { param_name, param_type, body });
                }
                Ok(CppStmt::TryCatch { try_body, catches })
            }
            CppTokenKind::Throw => {
                self.advance();
                let expr = self.parse_expr()?;
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppStmt::Throw(expr))
            }
            CppTokenKind::Delete => {
                self.advance();
                if self.eat(&CppTokenKind::LBracket) {
                    self.expect(&CppTokenKind::RBracket)?;
                    let expr = self.parse_expr()?;
                    self.eat(&CppTokenKind::Semicolon);
                    Ok(CppStmt::DeleteArray(expr))
                } else {
                    let expr = self.parse_expr()?;
                    self.eat(&CppTokenKind::Semicolon);
                    Ok(CppStmt::Delete(expr))
                }
            }
            CppTokenKind::Ident(ref s) if s == "break" => {
                self.advance();
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppStmt::Break)
            }
            CppTokenKind::Ident(ref s) if s == "continue" => {
                self.advance();
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppStmt::Continue)
            }
            _ => {
                let expr = self.parse_expr()?;
                self.eat(&CppTokenKind::Semicolon);
                Ok(CppStmt::Expr(expr))
            }
        }
    }

    fn parse_expr(&mut self) -> Result<CppExpr, String> {
        self.parse_assignment_expr()
    }

    fn parse_assignment_expr(&mut self) -> Result<CppExpr, String> {
        let lhs = self.parse_ternary_expr()?;
        if let Some(op) = self.try_assign_op() {
            let rhs = self.parse_assignment_expr()?;
            Ok(CppExpr::Assign { op, lhs: Box::new(lhs), rhs: Box::new(rhs) })
        } else {
            Ok(lhs)
        }
    }

    fn try_assign_op(&mut self) -> Option<CppAssignOp> {
        let op = match self.peek() {
            CppTokenKind::Assign => CppAssignOp::Assign,
            CppTokenKind::PlusAssign => CppAssignOp::AddAssign,
            CppTokenKind::MinusAssign => CppAssignOp::SubAssign,
            CppTokenKind::StarAssign => CppAssignOp::MulAssign,
            CppTokenKind::SlashAssign => CppAssignOp::DivAssign,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn parse_ternary_expr(&mut self) -> Result<CppExpr, String> {
        let cond = self.parse_or_expr()?;
        if self.eat(&CppTokenKind::Question) {
            let then_expr = self.parse_expr()?;
            self.expect(&CppTokenKind::Colon)?;
            let else_expr = self.parse_expr()?;
            Ok(CppExpr::Ternary {
                cond: Box::new(cond),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            })
        } else {
            Ok(cond)
        }
    }

    fn parse_or_expr(&mut self) -> Result<CppExpr, String> {
        let mut lhs = self.parse_and_expr()?;
        while self.eat(&CppTokenKind::PipePipe) {
            let rhs = self.parse_and_expr()?;
            lhs = CppExpr::Binary { op: CppBinOp::LogOr, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_and_expr(&mut self) -> Result<CppExpr, String> {
        let mut lhs = self.parse_comparison_expr()?;
        while self.eat(&CppTokenKind::AmpAmp) {
            let rhs = self.parse_comparison_expr()?;
            lhs = CppExpr::Binary { op: CppBinOp::LogAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_comparison_expr(&mut self) -> Result<CppExpr, String> {
        let mut lhs = self.parse_add_expr()?;
        loop {
            let op = match self.peek() {
                CppTokenKind::EqEq => CppBinOp::Eq,
                CppTokenKind::Ne => CppBinOp::Ne,
                CppTokenKind::Lt => CppBinOp::Lt,
                CppTokenKind::Le => CppBinOp::Le,
                CppTokenKind::Gt => CppBinOp::Gt,
                CppTokenKind::Ge => CppBinOp::Ge,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_add_expr()?;
            lhs = CppExpr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_add_expr(&mut self) -> Result<CppExpr, String> {
        let mut lhs = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                CppTokenKind::Plus => CppBinOp::Add,
                CppTokenKind::Minus => CppBinOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_mul_expr()?;
            lhs = CppExpr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_mul_expr(&mut self) -> Result<CppExpr, String> {
        let mut lhs = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                CppTokenKind::Star => CppBinOp::Mul,
                CppTokenKind::Slash => CppBinOp::Div,
                CppTokenKind::Percent => CppBinOp::Mod,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary_expr()?;
            lhs = CppExpr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_unary_expr(&mut self) -> Result<CppExpr, String> {
        match self.peek() {
            CppTokenKind::Minus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::Unary { op: CppUnaryOp::Neg, operand: Box::new(operand) })
            }
            CppTokenKind::Bang => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::Unary { op: CppUnaryOp::LogNot, operand: Box::new(operand) })
            }
            CppTokenKind::Amp => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::AddrOf(Box::new(operand)))
            }
            CppTokenKind::Star => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::Deref(Box::new(operand)))
            }
            CppTokenKind::New => {
                self.advance();
                let ty = self.parse_type()?;
                if self.eat(&CppTokenKind::LParen) {
                    let mut args = Vec::new();
                    while *self.peek() != CppTokenKind::RParen {
                        args.push(self.parse_expr()?);
                        if !self.eat(&CppTokenKind::Comma) { break; }
                    }
                    self.expect(&CppTokenKind::RParen)?;
                    Ok(CppExpr::New { ty, args })
                } else {
                    Ok(CppExpr::New { ty, args: Vec::new() })
                }
            }
            CppTokenKind::PlusPlus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::Unary { op: CppUnaryOp::PreIncr, operand: Box::new(operand) })
            }
            CppTokenKind::MinusMinus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(CppExpr::Unary { op: CppUnaryOp::PreDecr, operand: Box::new(operand) })
            }
            _ => self.parse_postfix_expr(),
        }
    }

    fn parse_postfix_expr(&mut self) -> Result<CppExpr, String> {
        let mut expr = self.parse_primary_expr()?;
        loop {
            match self.peek() {
                CppTokenKind::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    while *self.peek() != CppTokenKind::RParen {
                        args.push(self.parse_expr()?);
                        if !self.eat(&CppTokenKind::Comma) { break; }
                    }
                    self.expect(&CppTokenKind::RParen)?;
                    expr = CppExpr::Call { func: Box::new(expr), args };
                }
                CppTokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&CppTokenKind::RBracket)?;
                    expr = CppExpr::Index { array: Box::new(expr), index: Box::new(index) };
                }
                CppTokenKind::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    expr = CppExpr::Member { object: Box::new(expr), field };
                }
                CppTokenKind::Arrow => {
                    self.advance();
                    let field = self.expect_ident()?;
                    expr = CppExpr::ArrowMember { object: Box::new(expr), field };
                }
                CppTokenKind::ScopeRes => {
                    if let CppExpr::Ident(ref scope) = expr {
                        let scope = scope.clone();
                        self.advance();
                        let name = self.expect_ident()?;
                        expr = CppExpr::ScopeRes { scope, name };
                    } else {
                        break;
                    }
                }
                CppTokenKind::PlusPlus => {
                    self.advance();
                    expr = CppExpr::PostIncr(Box::new(expr));
                }
                CppTokenKind::MinusMinus => {
                    self.advance();
                    expr = CppExpr::PostDecr(Box::new(expr));
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_primary_expr(&mut self) -> Result<CppExpr, String> {
        match self.peek().clone() {
            CppTokenKind::IntLit(n) => { self.advance(); Ok(CppExpr::IntLit(n)) }
            CppTokenKind::FloatLit(f) => { self.advance(); Ok(CppExpr::FloatLit(f)) }
            CppTokenKind::CharLit(c) => { self.advance(); Ok(CppExpr::CharLit(c)) }
            CppTokenKind::StringLit(ref s) => { let s = s.clone(); self.advance(); Ok(CppExpr::StringLit(s)) }
            CppTokenKind::True => { self.advance(); Ok(CppExpr::BoolLit(true)) }
            CppTokenKind::False => { self.advance(); Ok(CppExpr::BoolLit(false)) }
            CppTokenKind::Nullptr => { self.advance(); Ok(CppExpr::Nullptr) }
            CppTokenKind::This => { self.advance(); Ok(CppExpr::This) }
            CppTokenKind::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                Ok(CppExpr::Ident(name))
            }
            CppTokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&CppTokenKind::RParen)?;
                Ok(expr)
            }
            CppTokenKind::LBracket => {
                // Lambda expression [capture](params){body}
                self.advance();
                let mut captures = Vec::new();
                while *self.peek() != CppTokenKind::RBracket {
                    match self.peek() {
                        CppTokenKind::Amp => {
                            self.advance();
                            if *self.peek() == CppTokenKind::RBracket || *self.peek() == CppTokenKind::Comma {
                                captures.push(LambdaCapture::DefaultByRef);
                            } else {
                                let name = self.expect_ident()?;
                                captures.push(LambdaCapture::ByRef(name));
                            }
                        }
                        CppTokenKind::Assign => {
                            self.advance();
                            captures.push(LambdaCapture::DefaultByValue);
                        }
                        CppTokenKind::This => {
                            self.advance();
                            captures.push(LambdaCapture::ThisCapture);
                        }
                        CppTokenKind::Ident(_) => {
                            let name = self.expect_ident()?;
                            captures.push(LambdaCapture::ByValue(name));
                        }
                        _ => break,
                    }
                    if !self.eat(&CppTokenKind::Comma) { break; }
                }
                self.expect(&CppTokenKind::RBracket)?;
                self.expect(&CppTokenKind::LParen)?;
                let params = self.parse_param_list()?;
                self.expect(&CppTokenKind::RParen)?;
                let body = self.parse_block()?;
                Ok(CppExpr::Lambda { captures, params, body })
            }
            _ => Err(alloc::format!("{}:{}: expected expression, got {:?}",
                self.span().line, self.span().col, self.peek())),
        }
    }
}

/// Parse C++ source tokens.
pub fn parse_cpp_tokens(tokens: &[CppToken]) -> Result<CppTranslationUnit, String> {
    let mut parser = CppParser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize_cpp;

    #[test]
    fn test_parse_class() {
        let tokens = tokenize_cpp("class Foo { public: int x; };").unwrap();
        let tu = parse_cpp_tokens(&tokens).unwrap();
        assert_eq!(tu.decls.len(), 1);
    }

    #[test]
    fn test_parse_namespace() {
        let tokens = tokenize_cpp("namespace foo { int bar() { return 42; } }").unwrap();
        let tu = parse_cpp_tokens(&tokens).unwrap();
        assert_eq!(tu.decls.len(), 1);
    }

    #[test]
    fn test_parse_template() {
        let tokens = tokenize_cpp("template<typename T> T identity(T x) { return x; }").unwrap();
        let tu = parse_cpp_tokens(&tokens).unwrap();
        assert_eq!(tu.decls.len(), 1);
    }
}
