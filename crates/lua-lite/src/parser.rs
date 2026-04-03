//! Recursive descent parser for Lua 5.4.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::boxed::Box;

use crate::lexer::{Token, SpannedToken};
use crate::ast::*;

/// Parser state.
pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).map(|t| &t.token).unwrap_or(&Token::Eof)
    }

    fn line(&self) -> usize {
        self.tokens.get(self.pos).map(|t| t.line).unwrap_or(0)
    }

    fn advance(&mut self) -> &Token {
        let t = self.tokens.get(self.pos).map(|t| &t.token).unwrap_or(&Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let tok = self.peek().clone();
        if &tok == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("line {}: expected {:?}, got {:?}", self.line(), expected, tok))
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        let tok = self.peek().clone();
        match tok {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(format!("line {}: expected identifier, got {:?}", self.line(), tok)),
        }
    }

    fn check(&self, expected: &Token) -> bool {
        self.peek() == expected
    }

    fn match_token(&mut self, expected: &Token) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Parse a complete chunk (file/string).
    pub fn parse_chunk(&mut self) -> Result<Chunk, String> {
        let block = self.parse_block()?;
        if !self.peek().is_eof() {
            return Err(format!("line {}: unexpected token {:?}", self.line(), self.peek()));
        }
        Ok(block)
    }

    /// Parse a block of statements.
    fn parse_block(&mut self) -> Result<Block, String> {
        let mut stats = Vec::new();
        let mut ret = None;

        loop {
            // Skip semicolons
            while self.match_token(&Token::Semi) {}

            match self.peek() {
                Token::End | Token::Else | Token::ElseIf | Token::Until | Token::Eof => break,
                Token::Return => {
                    self.advance();
                    let mut values = Vec::new();
                    if !matches!(self.peek(), Token::End | Token::Else | Token::ElseIf | Token::Until | Token::Eof | Token::Semi) {
                        values = self.parse_explist()?;
                    }
                    self.match_token(&Token::Semi);
                    ret = Some(values);
                    break;
                }
                _ => {
                    stats.push(self.parse_stat()?);
                }
            }
        }

        Ok(Block { stats, ret })
    }

    /// Parse a single statement.
    fn parse_stat(&mut self) -> Result<Stat, String> {
        match self.peek().clone() {
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::Do => self.parse_do(),
            Token::For => self.parse_for(),
            Token::Repeat => self.parse_repeat(),
            Token::Function => self.parse_function_stat(),
            Token::Local => self.parse_local(),
            Token::Goto => {
                self.advance();
                let name = self.expect_ident()?;
                Ok(Stat::Goto(name))
            }
            Token::DblColon => {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&Token::DblColon)?;
                Ok(Stat::Label(name))
            }
            Token::Break => {
                self.advance();
                Ok(Stat::Break)
            }
            _ => self.parse_expr_stat(),
        }
    }

    fn parse_if(&mut self) -> Result<Stat, String> {
        self.expect(&Token::If)?;
        let cond = self.parse_exp()?;
        self.expect(&Token::Then)?;
        let body = self.parse_block()?;
        let mut conditions = vec![(cond, body)];
        let mut else_block = None;

        loop {
            if self.match_token(&Token::ElseIf) {
                let cond = self.parse_exp()?;
                self.expect(&Token::Then)?;
                let body = self.parse_block()?;
                conditions.push((cond, body));
            } else if self.match_token(&Token::Else) {
                else_block = Some(self.parse_block()?);
                break;
            } else {
                break;
            }
        }

        self.expect(&Token::End)?;
        Ok(Stat::If { conditions, else_block })
    }

    fn parse_while(&mut self) -> Result<Stat, String> {
        self.expect(&Token::While)?;
        let condition = self.parse_exp()?;
        self.expect(&Token::Do)?;
        let body = self.parse_block()?;
        self.expect(&Token::End)?;
        Ok(Stat::While { condition, body })
    }

    fn parse_do(&mut self) -> Result<Stat, String> {
        self.expect(&Token::Do)?;
        let block = self.parse_block()?;
        self.expect(&Token::End)?;
        Ok(Stat::Do(block))
    }

    fn parse_for(&mut self) -> Result<Stat, String> {
        self.expect(&Token::For)?;
        let name = self.expect_ident()?;

        if self.match_token(&Token::Eq) {
            // Numeric for
            let start = self.parse_exp()?;
            self.expect(&Token::Comma)?;
            let stop = self.parse_exp()?;
            let step = if self.match_token(&Token::Comma) {
                Some(self.parse_exp()?)
            } else {
                None
            };
            self.expect(&Token::Do)?;
            let body = self.parse_block()?;
            self.expect(&Token::End)?;
            Ok(Stat::ForNumeric { name, start, stop, step, body })
        } else {
            // Generic for
            let mut names = vec![name];
            while self.match_token(&Token::Comma) {
                names.push(self.expect_ident()?);
            }
            self.expect(&Token::In)?;
            let iterators = self.parse_explist()?;
            self.expect(&Token::Do)?;
            let body = self.parse_block()?;
            self.expect(&Token::End)?;
            Ok(Stat::ForGeneric { names, iterators, body })
        }
    }

    fn parse_repeat(&mut self) -> Result<Stat, String> {
        self.expect(&Token::Repeat)?;
        let body = self.parse_block()?;
        self.expect(&Token::Until)?;
        let condition = self.parse_exp()?;
        Ok(Stat::Repeat { body, condition })
    }

    fn parse_function_stat(&mut self) -> Result<Stat, String> {
        self.expect(&Token::Function)?;
        let name = self.parse_func_name()?;
        let (params, has_vararg) = self.parse_func_params()?;
        let body = self.parse_block()?;
        self.expect(&Token::End)?;
        Ok(Stat::FunctionDef { name, params, has_vararg, body })
    }

    fn parse_func_name(&mut self) -> Result<Exp, String> {
        let name = self.expect_ident()?;
        let mut exp = Exp::Ident(name);
        while self.match_token(&Token::Dot) {
            let field = self.expect_ident()?;
            exp = Exp::Field {
                table: Box::new(exp),
                field,
            };
        }
        if self.match_token(&Token::Colon) {
            let method = self.expect_ident()?;
            exp = Exp::Field {
                table: Box::new(exp),
                field: method,
            };
        }
        Ok(exp)
    }

    fn parse_func_params(&mut self) -> Result<(Vec<String>, bool), String> {
        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        let mut has_vararg = false;

        if !self.check(&Token::RParen) {
            if self.check(&Token::DotDotDot) {
                self.advance();
                has_vararg = true;
            } else {
                params.push(self.expect_ident()?);
                while self.match_token(&Token::Comma) {
                    if self.check(&Token::DotDotDot) {
                        self.advance();
                        has_vararg = true;
                        break;
                    }
                    params.push(self.expect_ident()?);
                }
            }
        }

        self.expect(&Token::RParen)?;
        Ok((params, has_vararg))
    }

    fn parse_local(&mut self) -> Result<Stat, String> {
        self.expect(&Token::Local)?;

        if self.match_token(&Token::Function) {
            let name = self.expect_ident()?;
            let (params, has_vararg) = self.parse_func_params()?;
            let body = self.parse_block()?;
            self.expect(&Token::End)?;
            return Ok(Stat::LocalFunction { name, params, has_vararg, body });
        }

        let mut names = vec![self.expect_ident()?];
        while self.match_token(&Token::Comma) {
            names.push(self.expect_ident()?);
        }

        let values = if self.match_token(&Token::Eq) {
            self.parse_explist()?
        } else {
            Vec::new()
        };

        Ok(Stat::Local { names, values })
    }

    fn parse_expr_stat(&mut self) -> Result<Stat, String> {
        let exp = self.parse_suffixed_exp()?;

        // Check for assignment
        if self.check(&Token::Eq) || self.check(&Token::Comma) {
            let mut targets = vec![exp];
            while self.match_token(&Token::Comma) {
                targets.push(self.parse_suffixed_exp()?);
            }
            self.expect(&Token::Eq)?;
            let values = self.parse_explist()?;
            return Ok(Stat::Assign { targets, values });
        }

        // Otherwise it's an expression statement (function call)
        Ok(Stat::ExprStat(exp))
    }

    // Expression parsing with precedence climbing

    fn parse_explist(&mut self) -> Result<Vec<Exp>, String> {
        let mut exps = vec![self.parse_exp()?];
        while self.match_token(&Token::Comma) {
            exps.push(self.parse_exp()?);
        }
        Ok(exps)
    }

    pub fn parse_exp(&mut self) -> Result<Exp, String> {
        self.parse_or_exp()
    }

    fn parse_or_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_and_exp()?;
        while self.match_token(&Token::Or) {
            let right = self.parse_and_exp()?;
            left = Exp::BinOp { op: BinaryOp::Or, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_and_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_comparison()?;
        while self.match_token(&Token::And) {
            let right = self.parse_comparison()?;
            left = Exp::BinOp { op: BinaryOp::And, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_bor_exp()?;
        loop {
            let op = match self.peek() {
                Token::Less => BinaryOp::Less,
                Token::Great => BinaryOp::Greater,
                Token::LessEq => BinaryOp::LessEq,
                Token::GreatEq => BinaryOp::GreaterEq,
                Token::EqEq => BinaryOp::Eq,
                Token::TildeEq => BinaryOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_bor_exp()?;
            left = Exp::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bor_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_bxor_exp()?;
        while self.match_token(&Token::Pipe) {
            let right = self.parse_bxor_exp()?;
            left = Exp::BinOp { op: BinaryOp::BOr, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bxor_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_band_exp()?;
        while self.match_token(&Token::Tilde) {
            let right = self.parse_band_exp()?;
            left = Exp::BinOp { op: BinaryOp::BXor, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_band_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_shift_exp()?;
        while self.match_token(&Token::Ampersand) {
            let right = self.parse_shift_exp()?;
            left = Exp::BinOp { op: BinaryOp::BAnd, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_shift_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_concat_exp()?;
        loop {
            let op = match self.peek() {
                Token::LessLess => BinaryOp::Shl,
                Token::GreatGreat => BinaryOp::Shr,
                _ => break,
            };
            self.advance();
            let right = self.parse_concat_exp()?;
            left = Exp::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_concat_exp(&mut self) -> Result<Exp, String> {
        let left = self.parse_add_exp()?;
        if self.match_token(&Token::DotDot) {
            // Right-associative
            let right = self.parse_concat_exp()?;
            return Ok(Exp::BinOp { op: BinaryOp::Concat, left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn parse_add_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_mul_exp()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_exp()?;
            left = Exp::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_mul_exp(&mut self) -> Result<Exp, String> {
        let mut left = self.parse_unary_exp()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                Token::SlashSlash => BinaryOp::IDiv,
                Token::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_exp()?;
            left = Exp::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary_exp(&mut self) -> Result<Exp, String> {
        match self.peek().clone() {
            Token::Not => {
                self.advance();
                let operand = self.parse_unary_exp()?;
                Ok(Exp::UnOp { op: UnaryOp::Not, operand: Box::new(operand) })
            }
            Token::Hash => {
                self.advance();
                let operand = self.parse_unary_exp()?;
                Ok(Exp::UnOp { op: UnaryOp::Len, operand: Box::new(operand) })
            }
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary_exp()?;
                Ok(Exp::UnOp { op: UnaryOp::Neg, operand: Box::new(operand) })
            }
            Token::Tilde => {
                self.advance();
                let operand = self.parse_unary_exp()?;
                Ok(Exp::UnOp { op: UnaryOp::BNot, operand: Box::new(operand) })
            }
            _ => self.parse_power_exp(),
        }
    }

    fn parse_power_exp(&mut self) -> Result<Exp, String> {
        let base = self.parse_suffixed_exp()?;
        if self.match_token(&Token::Caret) {
            // Right-associative
            let exp = self.parse_unary_exp()?;
            return Ok(Exp::BinOp { op: BinaryOp::Pow, left: Box::new(base), right: Box::new(exp) });
        }
        Ok(base)
    }

    fn parse_suffixed_exp(&mut self) -> Result<Exp, String> {
        let mut exp = self.parse_primary_exp()?;

        loop {
            match self.peek().clone() {
                Token::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    exp = Exp::Field { table: Box::new(exp), field };
                }
                Token::LBracket => {
                    self.advance();
                    let key = self.parse_exp()?;
                    self.expect(&Token::RBracket)?;
                    exp = Exp::Index { table: Box::new(exp), key: Box::new(key) };
                }
                Token::Colon => {
                    self.advance();
                    let method = self.expect_ident()?;
                    let args = self.parse_call_args()?;
                    exp = Exp::MethodCall { object: Box::new(exp), method, args };
                }
                Token::LParen | Token::LBrace | Token::Str(_) => {
                    let args = self.parse_call_args()?;
                    exp = Exp::Call { func: Box::new(exp), args };
                }
                _ => break,
            }
        }

        Ok(exp)
    }

    fn parse_call_args(&mut self) -> Result<Vec<Exp>, String> {
        match self.peek().clone() {
            Token::LParen => {
                self.advance();
                let args = if self.check(&Token::RParen) {
                    Vec::new()
                } else {
                    self.parse_explist()?
                };
                self.expect(&Token::RParen)?;
                Ok(args)
            }
            Token::LBrace => {
                let table = self.parse_table_constructor()?;
                Ok(vec![table])
            }
            Token::Str(s) => {
                self.advance();
                Ok(vec![Exp::Str(s)])
            }
            _ => Err(format!("line {}: expected function arguments", self.line())),
        }
    }

    fn parse_primary_exp(&mut self) -> Result<Exp, String> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.advance();
                Ok(Exp::Ident(name))
            }
            Token::LParen => {
                self.advance();
                let exp = self.parse_exp()?;
                self.expect(&Token::RParen)?;
                Ok(exp)
            }
            Token::Nil => { self.advance(); Ok(Exp::Nil) }
            Token::True => { self.advance(); Ok(Exp::True) }
            Token::False => { self.advance(); Ok(Exp::False) }
            Token::Integer(n) => { self.advance(); Ok(Exp::Integer(n)) }
            Token::Number(n) => { self.advance(); Ok(Exp::Number(n)) }
            Token::Str(s) => { self.advance(); Ok(Exp::Str(s)) }
            Token::DotDotDot => { self.advance(); Ok(Exp::VarArg) }
            Token::Function => {
                self.advance();
                let (params, has_vararg) = self.parse_func_params()?;
                let body = self.parse_block()?;
                self.expect(&Token::End)?;
                Ok(Exp::Function { params, has_vararg, body })
            }
            Token::LBrace => {
                self.parse_table_constructor()
            }
            tok => Err(format!("line {}: unexpected token {:?}", self.line(), tok)),
        }
    }

    fn parse_table_constructor(&mut self) -> Result<Exp, String> {
        self.expect(&Token::LBrace)?;
        let mut fields = Vec::new();

        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            match self.peek().clone() {
                Token::LBracket => {
                    self.advance();
                    let key = self.parse_exp()?;
                    self.expect(&Token::RBracket)?;
                    self.expect(&Token::Eq)?;
                    let value = self.parse_exp()?;
                    fields.push(TableField::IndexField { key, value });
                }
                Token::Ident(name) => {
                    // Lookahead: name = exp  vs  just exp
                    let saved = self.pos;
                    self.advance();
                    if self.match_token(&Token::Eq) {
                        let value = self.parse_exp()?;
                        fields.push(TableField::NameField { name, value });
                    } else {
                        self.pos = saved;
                        let exp = self.parse_exp()?;
                        fields.push(TableField::Positional(exp));
                    }
                }
                _ => {
                    let exp = self.parse_exp()?;
                    fields.push(TableField::Positional(exp));
                }
            }

            // Field separator: , or ;
            if !self.match_token(&Token::Comma) {
                self.match_token(&Token::Semi);
            }
        }

        self.expect(&Token::RBrace)?;
        Ok(Exp::TableConstructor(fields))
    }
}

/// Parse tokens into a Chunk (AST).
pub fn parse(tokens: Vec<SpannedToken>) -> Result<Chunk, String> {
    let mut parser = Parser::new(tokens);
    parser.parse_chunk()
}
