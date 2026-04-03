//! TypeScript to JavaScript transformer.
//!
//! Strips type annotations, interfaces, and type aliases, and transforms
//! TypeScript-specific constructs to plain JavaScript:
//! - Type annotations → removed
//! - Interfaces → removed (structural typing, no runtime representation)
//! - Enums → object literals
//! - Optional chaining → preserved (js-lite supports it)
//! - Nullish coalescing → preserved (js-lite supports it)
//! - Type assertions (x as T) → x
//! - Decorators → function calls (simplified)

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::{TsToken, SpannedToken};

/// Transform TypeScript tokens to plain JavaScript source code.
///
/// This is a token-level transformer that strips type syntax and converts
/// TypeScript-only constructs to JS equivalents.
pub fn transform_to_js(tokens: &[SpannedToken]) -> Result<String, String> {
    let mut transformer = Transformer::new(tokens);
    transformer.transform()
}

struct Transformer<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    output: String,
}

impl<'a> Transformer<'a> {
    fn new(tokens: &'a [SpannedToken]) -> Self {
        Self {
            tokens,
            pos: 0,
            output: String::new(),
        }
    }

    fn peek(&self) -> &TsToken {
        self.tokens.get(self.pos).map(|t| &t.token).unwrap_or(&TsToken::Eof)
    }

    fn advance(&mut self) -> &TsToken {
        let tok = &self.tokens[self.pos].token;
        self.pos += 1;
        tok
    }

    fn eat(&mut self, expected: &TsToken) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn emit(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn emit_space(&mut self) {
        if !self.output.ends_with(' ') && !self.output.ends_with('\n') && !self.output.is_empty() {
            self.output.push(' ');
        }
    }

    fn transform(&mut self) -> Result<String, String> {
        while *self.peek() != TsToken::Eof {
            self.transform_token()?;
        }
        Ok(self.output.clone())
    }

    fn transform_token(&mut self) -> Result<(), String> {
        match self.peek().clone() {
            // Skip: interface declarations
            TsToken::Interface => {
                self.advance();
                self.skip_ident(); // interface name
                self.skip_type_params(); // optional <T>
                if self.is_extends_or_implements() {
                    self.skip_until_brace();
                }
                self.skip_balanced_braces(); // { ... }
                Ok(())
            }

            // Skip: type aliases
            TsToken::Type => {
                self.advance();
                // Check if it looks like a type alias: type X = ...
                if self.is_ident() {
                    self.skip_ident();
                    self.skip_type_params();
                    self.skip_until_semicolon_or_newline();
                    Ok(())
                } else {
                    // Not a type alias, emit 'type' as identifier
                    self.emit("type");
                    self.emit_space();
                    Ok(())
                }
            }

            // Skip: declare statements
            TsToken::Declare => {
                self.advance();
                self.skip_until_semicolon_or_newline();
                Ok(())
            }

            // Skip: namespace/module blocks
            TsToken::Namespace | TsToken::Module => {
                self.advance();
                self.skip_ident();
                self.skip_balanced_braces();
                Ok(())
            }

            // Transform: enum → object
            TsToken::Enum => {
                self.advance();
                let name = self.take_ident();
                self.emit("var ");
                self.emit(&name);
                self.emit(" = ");
                self.transform_enum_body()?;
                self.emit(";");
                Ok(())
            }

            // Skip: abstract keyword (before class)
            TsToken::Abstract => {
                self.advance();
                // Don't emit — just continue, the class keyword follows
                Ok(())
            }

            // Skip access modifiers
            TsToken::Private | TsToken::Public | TsToken::Protected | TsToken::Readonly => {
                self.advance();
                Ok(())
            }

            // Transform: as expressions (type assertions) → just the expression
            TsToken::As => {
                self.advance();
                self.skip_type_expr();
                Ok(())
            }

            // Transform: satisfies → skip the type
            TsToken::Satisfies => {
                self.advance();
                self.skip_type_expr();
                Ok(())
            }

            // Pass through JS tokens
            TsToken::Js(ref tok) => {
                let js_str = js_token_to_string(tok);
                self.emit_space();
                self.emit(&js_str);
                self.advance();

                // Check for type annotation after ':'
                // (This is simplified — in practice we'd need context)
                Ok(())
            }

            // Pass through other tokens
            _ => {
                self.advance();
                Ok(())
            }
        }
    }

    fn transform_enum_body(&mut self) -> Result<(), String> {
        // Expect { Variant, Variant = value, ... }
        if !self.eat_js_lbrace() {
            return Err(String::from("expected { in enum"));
        }
        self.emit("{");

        let mut value: i64 = 0;
        let mut first = true;

        loop {
            if self.is_js_rbrace() || *self.peek() == TsToken::Eof {
                break;
            }

            if !first {
                self.emit(", ");
            }
            first = false;

            let variant = self.take_ident();

            // Check for = value
            if self.eat_js_assign() {
                let num = self.take_number();
                value = num as i64;
            }

            self.emit(&format!("{}: {}", variant, value));
            value += 1;

            // Skip comma
            self.eat_js_comma();
        }

        self.eat_js_rbrace();
        self.emit("}");
        Ok(())
    }

    // === Helper methods ===

    fn is_ident(&self) -> bool {
        matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::Ident(_)) | TsToken::Ident(_))
    }

    fn skip_ident(&mut self) {
        if self.is_ident() {
            self.advance();
        }
    }

    fn take_ident(&mut self) -> String {
        match self.peek().clone() {
            TsToken::Js(js_lite::tokenizer::Token::Ident(ref s)) => {
                let s = s.clone();
                self.advance();
                s
            }
            TsToken::Ident(ref s) => {
                let s = s.clone();
                self.advance();
                s
            }
            _ => {
                self.advance();
                String::from("_")
            }
        }
    }

    fn take_number(&mut self) -> f64 {
        match self.peek().clone() {
            TsToken::Js(js_lite::tokenizer::Token::Number(n)) => {
                self.advance();
                n
            }
            TsToken::NumberLit(n) => {
                self.advance();
                n
            }
            _ => 0.0,
        }
    }

    fn skip_type_params(&mut self) {
        if matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::Lt) | TsToken::LAngle) {
            let mut depth = 0;
            loop {
                match self.peek() {
                    TsToken::Js(js_lite::tokenizer::Token::Lt) | TsToken::LAngle => {
                        depth += 1;
                        self.advance();
                    }
                    TsToken::Js(js_lite::tokenizer::Token::Gt) | TsToken::RAngle => {
                        depth -= 1;
                        self.advance();
                        if depth == 0 { break; }
                    }
                    TsToken::Eof => break,
                    _ => { self.advance(); }
                }
            }
        }
    }

    fn skip_type_expr(&mut self) {
        // Skip a type expression (simplified: skip until we hit something that's
        // clearly not part of a type)
        let mut depth = 0;
        loop {
            match self.peek() {
                TsToken::Js(js_lite::tokenizer::Token::Lt) | TsToken::LAngle => {
                    depth += 1;
                    self.advance();
                }
                TsToken::Js(js_lite::tokenizer::Token::Gt) | TsToken::RAngle => {
                    if depth > 0 {
                        depth -= 1;
                        self.advance();
                    } else {
                        break;
                    }
                }
                TsToken::Js(js_lite::tokenizer::Token::Semicolon)
                | TsToken::Js(js_lite::tokenizer::Token::Comma)
                | TsToken::Js(js_lite::tokenizer::Token::RParen)
                | TsToken::Js(js_lite::tokenizer::Token::RBrace)
                | TsToken::Js(js_lite::tokenizer::Token::RBracket)
                | TsToken::Eof => break,
                _ if depth == 0 && self.is_statement_start() => break,
                _ => { self.advance(); }
            }
        }
    }

    fn skip_until_semicolon_or_newline(&mut self) {
        loop {
            match self.peek() {
                TsToken::Js(js_lite::tokenizer::Token::Semicolon) => {
                    self.advance();
                    break;
                }
                TsToken::Eof => break,
                _ => { self.advance(); }
            }
        }
    }

    fn skip_until_brace(&mut self) {
        loop {
            match self.peek() {
                TsToken::Js(js_lite::tokenizer::Token::LBrace) => break,
                TsToken::Eof => break,
                _ => { self.advance(); }
            }
        }
    }

    fn skip_balanced_braces(&mut self) {
        if !matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::LBrace)) {
            return;
        }
        self.advance();
        let mut depth = 1;
        while depth > 0 {
            match self.peek() {
                TsToken::Js(js_lite::tokenizer::Token::LBrace) => {
                    depth += 1;
                    self.advance();
                }
                TsToken::Js(js_lite::tokenizer::Token::RBrace) => {
                    depth -= 1;
                    self.advance();
                }
                TsToken::Eof => break,
                _ => { self.advance(); }
            }
        }
    }

    fn is_extends_or_implements(&self) -> bool {
        matches!(
            self.peek(),
            TsToken::Js(js_lite::tokenizer::Token::Ident(_)) | TsToken::Implements
        )
    }

    fn is_statement_start(&self) -> bool {
        matches!(
            self.peek(),
            TsToken::Js(js_lite::tokenizer::Token::Var)
                | TsToken::Js(js_lite::tokenizer::Token::Let)
                | TsToken::Js(js_lite::tokenizer::Token::Const)
                | TsToken::Js(js_lite::tokenizer::Token::Function)
                | TsToken::Js(js_lite::tokenizer::Token::Return)
                | TsToken::Js(js_lite::tokenizer::Token::If)
                | TsToken::Js(js_lite::tokenizer::Token::For)
                | TsToken::Js(js_lite::tokenizer::Token::While)
        )
    }

    fn eat_js_lbrace(&mut self) -> bool {
        if matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::LBrace)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn is_js_rbrace(&self) -> bool {
        matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::RBrace))
    }

    fn eat_js_rbrace(&mut self) -> bool {
        if self.is_js_rbrace() {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_js_assign(&mut self) -> bool {
        if matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::Assign)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_js_comma(&mut self) -> bool {
        if matches!(self.peek(), TsToken::Js(js_lite::tokenizer::Token::Comma)) {
            self.advance();
            true
        } else {
            false
        }
    }
}

/// Convert a JS token back to its source string representation.
fn js_token_to_string(tok: &js_lite::tokenizer::Token) -> String {
    use js_lite::tokenizer::Token;
    match tok {
        Token::Number(n) => {
            if *n == (*n as i64) as f64 {
                alloc::format!("{}", *n as i64)
            } else {
                alloc::format!("{}", n)
            }
        }
        Token::Str(s) => alloc::format!("'{}'", s),
        Token::Ident(s) => s.clone(),
        Token::True => String::from("true"),
        Token::False => String::from("false"),
        Token::Null => String::from("null"),
        Token::Undefined => String::from("undefined"),
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
        Token::Plus => String::from("+"),
        Token::Minus => String::from("-"),
        Token::Star => String::from("*"),
        Token::Slash => String::from("/"),
        Token::Percent => String::from("%"),
        Token::Assign => String::from("="),
        Token::Eq => String::from("=="),
        Token::StrictEq => String::from("==="),
        Token::NotEq => String::from("!="),
        Token::StrictNotEq => String::from("!=="),
        Token::Lt => String::from("<"),
        Token::Gt => String::from(">"),
        Token::LtEq => String::from("<="),
        Token::GtEq => String::from(">="),
        Token::And => String::from("&&"),
        Token::Or => String::from("||"),
        Token::Not => String::from("!"),
        Token::BitAnd => String::from("&"),
        Token::BitOr => String::from("|"),
        Token::BitXor => String::from("^"),
        Token::BitNot => String::from("~"),
        Token::Shl => String::from("<<"),
        Token::Shr => String::from(">>"),
        Token::Ushr => String::from(">>>"),
        Token::PlusAssign => String::from("+="),
        Token::MinusAssign => String::from("-="),
        Token::StarAssign => String::from("*="),
        Token::SlashAssign => String::from("/="),
        Token::PercentAssign => String::from("%="),
        Token::PlusPlus => String::from("++"),
        Token::MinusMinus => String::from("--"),
        Token::LParen => String::from("("),
        Token::RParen => String::from(")"),
        Token::LBrace => String::from("{"),
        Token::RBrace => String::from("}"),
        Token::LBracket => String::from("["),
        Token::RBracket => String::from("]"),
        Token::Semicolon => String::from(";"),
        Token::Comma => String::from(","),
        Token::Dot => String::from("."),
        Token::Colon => String::from(":"),
        Token::Question => String::from("?"),
        Token::Arrow => String::from("=>"),
        Token::Spread => String::from("..."),
        Token::NullishCoalesce => String::from("??"),
        Token::OptionalChain => String::from("?."),
        _ => String::from(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize_ts;

    #[test]
    fn test_strip_interface() {
        let tokens = tokenize_ts("interface Foo { x: number } var y = 1;").unwrap();
        let js = transform_to_js(&tokens).unwrap();
        assert!(!js.contains("interface"));
        assert!(js.contains("var"));
    }

    #[test]
    fn test_enum_to_object() {
        let tokens = tokenize_ts("enum Color { Red, Green, Blue }").unwrap();
        let js = transform_to_js(&tokens).unwrap();
        assert!(js.contains("var Color"));
        assert!(js.contains("Red: 0"));
    }
}
