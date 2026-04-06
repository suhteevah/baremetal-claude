//! Full Rust lexer/tokenizer for ClaudioOS.
//!
//! Tokenizes Rust source into a flat token stream. Handles all Rust keywords,
//! operators, punctuation, literals (integer, float, string, char, byte string,
//! raw string), identifiers, lifetimes, attributes, and comments.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ─── Token types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ── Keywords ──
    As,
    Async,
    Await,
    Break,
    Const,
    Continue,
    Crate,
    Dyn,
    Else,
    Enum,
    Extern,
    False,
    Fn,
    For,
    If,
    Impl,
    In,
    Let,
    Loop,
    Match,
    Mod,
    Move,
    Mut,
    Pub,
    Ref,
    Return,
    SelfLower, // self
    SelfUpper, // Self
    Static,
    Struct,
    Super,
    Trait,
    True,
    Type,
    Unsafe,
    Use,
    Where,
    While,
    Yield,

    // ── Literals ──
    IntLit(i128),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),
    ByteLit(u8),
    ByteStringLit(Vec<u8>),
    BoolLit(bool),

    // ── Identifier + lifetime ──
    Ident(String),
    Lifetime(String), // 'a, 'static, etc.

    // ── Operators ──
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Bang,
    Amp,
    Pipe,
    AmpAmp,
    PipePipe,
    Shl,
    Shr,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    CaretEq,
    AmpEq,
    PipeEq,
    ShlEq,
    ShrEq,
    Eq,
    EqEq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    DotDot,
    DotDotEq,
    DotDotDot,
    FatArrow,   // =>
    ThinArrow,  // ->
    Underscore, // _

    // ── Delimiters ──
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,

    // ── Punctuation ──
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    At,
    Pound,
    Tilde,
    Question,

    // ── Special ──
    Eof,
}

impl Token {
    pub fn is_eof(&self) -> bool {
        matches!(self, Token::Eof)
    }
}

// ─── Span info (line, col) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub struct Spanned {
    pub token: Token,
    pub span: Span,
}

// ─── Lexer ───────────────────────────────────────────────────────────────

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize(source: &str) -> Result<Vec<Spanned>, String> {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token()?;
            let is_eof = tok.token.is_eof();
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }

    fn peek2(&self) -> u8 {
        if self.pos + 1 < self.src.len() {
            self.src[self.pos + 1]
        } else {
            0
        }
    }

    fn advance(&mut self) -> u8 {
        let b = self.peek();
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        self.pos += 1;
        b
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
            offset: self.pos as u32,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.src.len() {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                b'/' if self.peek2() == b'/' => {
                    // Line comment
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.advance();
                    }
                }
                b'/' if self.peek2() == b'*' => {
                    // Block comment (nested)
                    self.advance();
                    self.advance();
                    let mut depth = 1u32;
                    while self.pos < self.src.len() && depth > 0 {
                        if self.peek() == b'/' && self.peek2() == b'*' {
                            self.advance();
                            self.advance();
                            depth += 1;
                        } else if self.peek() == b'*' && self.peek2() == b'/' {
                            self.advance();
                            self.advance();
                            depth -= 1;
                        } else {
                            self.advance();
                        }
                    }
                }
                _ => break,
            }
        }
    }

    pub fn next_token(&mut self) -> Result<Spanned, String> {
        self.skip_whitespace();
        let sp = self.span();

        if self.pos >= self.src.len() {
            return Ok(Spanned { token: Token::Eof, span: sp });
        }

        let b = self.peek();

        // ── String literals ──
        if b == b'"' {
            return self.lex_string(sp);
        }
        // Raw string r"..." or r#"..."#
        if b == b'r' && (self.peek2() == b'"' || self.peek2() == b'#') {
            return self.lex_raw_string(sp);
        }
        // Byte string b"..."
        if b == b'b' && self.peek2() == b'"' {
            return self.lex_byte_string(sp);
        }
        // Byte literal b'x'
        if b == b'b' && self.peek2() == b'\'' {
            return self.lex_byte_lit(sp);
        }
        // Char literal 'x' (but not lifetime 'ident)
        if b == b'\'' {
            return self.lex_char_or_lifetime(sp);
        }

        // ── Numbers ──
        if b.is_ascii_digit() {
            return self.lex_number(sp);
        }

        // ── Identifiers / keywords ──
        if b == b'_' || b.is_ascii_alphabetic() {
            return self.lex_ident(sp);
        }

        // ── Operators and punctuation ──
        self.lex_punct(sp)
    }

    fn lex_string(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // skip "
        let mut s = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(format!("unterminated string at {}:{}", sp.line, sp.col));
            }
            let b = self.advance();
            match b {
                b'"' => break,
                b'\\' => {
                    let esc = self.advance();
                    match esc {
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'\\' => s.push('\\'),
                        b'"' => s.push('"'),
                        b'\'' => s.push('\''),
                        b'0' => s.push('\0'),
                        b'x' => {
                            let hi = self.advance();
                            let lo = self.advance();
                            let val = hex_digit(hi)? * 16 + hex_digit(lo)?;
                            s.push(val as char);
                        }
                        b'u' => {
                            // \u{XXXX}
                            if self.advance() != b'{' {
                                return Err("expected '{' in unicode escape".into());
                            }
                            let mut val = 0u32;
                            loop {
                                let c = self.advance();
                                if c == b'}' {
                                    break;
                                }
                                val = val * 16 + hex_digit(c)? as u32;
                            }
                            s.push(char::from_u32(val).unwrap_or('\u{FFFD}'));
                        }
                        _ => {
                            return Err(format!("unknown escape \\{}", esc as char));
                        }
                    }
                }
                _ => s.push(b as char),
            }
        }
        Ok(Spanned { token: Token::StringLit(s), span: sp })
    }

    fn lex_raw_string(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // skip 'r'
        let mut hashes = 0u32;
        while self.peek() == b'#' {
            self.advance();
            hashes += 1;
        }
        if self.advance() != b'"' {
            return Err("expected '\"' after raw string prefix".into());
        }
        let mut s = String::new();
        'outer: loop {
            if self.pos >= self.src.len() {
                return Err("unterminated raw string".into());
            }
            let b = self.advance();
            if b == b'"' {
                let mut count = 0u32;
                while count < hashes && self.peek() == b'#' {
                    self.advance();
                    count += 1;
                }
                if count == hashes {
                    break 'outer;
                }
                s.push('"');
                for _ in 0..count {
                    s.push('#');
                }
            } else {
                s.push(b as char);
            }
        }
        Ok(Spanned { token: Token::StringLit(s), span: sp })
    }

    fn lex_byte_string(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // b
        self.advance(); // "
        let mut bytes = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                return Err("unterminated byte string".into());
            }
            let b = self.advance();
            match b {
                b'"' => break,
                b'\\' => {
                    let esc = self.advance();
                    match esc {
                        b'n' => bytes.push(b'\n'),
                        b'r' => bytes.push(b'\r'),
                        b't' => bytes.push(b'\t'),
                        b'\\' => bytes.push(b'\\'),
                        b'"' => bytes.push(b'"'),
                        b'0' => bytes.push(0),
                        b'x' => {
                            let hi = self.advance();
                            let lo = self.advance();
                            bytes.push(hex_digit(hi)? * 16 + hex_digit(lo)?);
                        }
                        _ => return Err(format!("unknown byte escape \\{}", esc as char)),
                    }
                }
                _ => bytes.push(b),
            }
        }
        Ok(Spanned { token: Token::ByteStringLit(bytes), span: sp })
    }

    fn lex_byte_lit(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // b
        self.advance(); // '
        let val = if self.peek() == b'\\' {
            self.advance();
            let esc = self.advance();
            match esc {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'\\' => b'\\',
                b'\'' => b'\'',
                b'0' => 0,
                b'x' => {
                    let hi = self.advance();
                    let lo = self.advance();
                    hex_digit(hi)? * 16 + hex_digit(lo)?
                }
                _ => return Err(format!("unknown byte escape \\{}", esc as char)),
            }
        } else {
            self.advance()
        };
        if self.advance() != b'\'' {
            return Err("unterminated byte literal".into());
        }
        Ok(Spanned { token: Token::ByteLit(val), span: sp })
    }

    fn lex_char_or_lifetime(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // skip '

        // Check for lifetime: 'ident or 'static
        if (self.peek().is_ascii_alphabetic() || self.peek() == b'_')
            && !(self.peek() == b'\\')
        {
            // Peek ahead to see if this is 'x' (char) or 'ident (lifetime)
            let start = self.pos;
            let mut end = start;
            while end < self.src.len()
                && (self.src[end].is_ascii_alphanumeric() || self.src[end] == b'_')
            {
                end += 1;
            }
            // If followed by ', it's a char literal like 'a'
            if end < self.src.len() && self.src[end] == b'\'' && end == start + 1 {
                // Single char literal
                let ch = self.advance() as char;
                self.advance(); // closing '
                return Ok(Spanned { token: Token::CharLit(ch), span: sp });
            }
            // Otherwise it's a lifetime
            let mut name = String::from("'");
            while self.pos < self.src.len()
                && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_')
            {
                name.push(self.advance() as char);
            }
            return Ok(Spanned { token: Token::Lifetime(name), span: sp });
        }

        // Escape or other char
        let ch = if self.peek() == b'\\' {
            self.advance();
            let esc = self.advance();
            match esc {
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'\\' => '\\',
                b'\'' => '\'',
                b'0' => '\0',
                b'x' => {
                    let hi = self.advance();
                    let lo = self.advance();
                    (hex_digit(hi)? * 16 + hex_digit(lo)?) as char
                }
                b'u' => {
                    if self.advance() != b'{' {
                        return Err("expected '{' in unicode escape".into());
                    }
                    let mut val = 0u32;
                    loop {
                        let c = self.advance();
                        if c == b'}' { break; }
                        val = val * 16 + hex_digit(c)? as u32;
                    }
                    char::from_u32(val).unwrap_or('\u{FFFD}')
                }
                _ => return Err(format!("unknown char escape \\{}", esc as char)),
            }
        } else {
            self.advance() as char
        };
        if self.advance() != b'\'' {
            return Err("unterminated char literal".into());
        }
        Ok(Spanned { token: Token::CharLit(ch), span: sp })
    }

    fn lex_number(&mut self, sp: Span) -> Result<Spanned, String> {
        let first = self.peek();

        // Hex, octal, binary
        if first == b'0' {
            match self.peek2() {
                b'x' | b'X' => return self.lex_hex(sp),
                b'o' | b'O' => return self.lex_octal(sp),
                b'b' | b'B' => return self.lex_binary(sp),
                _ => {}
            }
        }

        // Decimal integer or float
        let mut num_str = String::new();
        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            let b = self.advance();
            if b != b'_' {
                num_str.push(b as char);
            }
        }

        // Float: dot followed by digit (not ..)
        if self.peek() == b'.' && self.peek2() != b'.' && self.peek2().is_ascii_digit() {
            num_str.push('.');
            self.advance();
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                let b = self.advance();
                if b != b'_' {
                    num_str.push(b as char);
                }
            }
            // Exponent
            if self.peek() == b'e' || self.peek() == b'E' {
                num_str.push(self.advance() as char);
                if self.peek() == b'+' || self.peek() == b'-' {
                    num_str.push(self.advance() as char);
                }
                while self.peek().is_ascii_digit() || self.peek() == b'_' {
                    let b = self.advance();
                    if b != b'_' {
                        num_str.push(b as char);
                    }
                }
            }
            // Skip type suffix (f32/f64)
            self.skip_number_suffix();
            let val: f64 = num_str.parse().map_err(|e| format!("bad float: {}", e))?;
            return Ok(Spanned { token: Token::FloatLit(val), span: sp });
        }

        // Exponent without dot -> still float
        if self.peek() == b'e' || self.peek() == b'E' {
            num_str.push(self.advance() as char);
            if self.peek() == b'+' || self.peek() == b'-' {
                num_str.push(self.advance() as char);
            }
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                let b = self.advance();
                if b != b'_' {
                    num_str.push(b as char);
                }
            }
            self.skip_number_suffix();
            let val: f64 = num_str.parse().map_err(|e| format!("bad float: {}", e))?;
            return Ok(Spanned { token: Token::FloatLit(val), span: sp });
        }

        // Integer with optional suffix
        self.skip_number_suffix();
        let val: i128 = num_str.parse().map_err(|e| format!("bad int: {}", e))?;
        Ok(Spanned { token: Token::IntLit(val), span: sp })
    }

    fn lex_hex(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // 0
        self.advance(); // x
        let mut val: i128 = 0;
        let mut any = false;
        while self.peek().is_ascii_hexdigit() || self.peek() == b'_' {
            let b = self.advance();
            if b != b'_' {
                val = val * 16 + hex_digit(b)? as i128;
                any = true;
            }
        }
        if !any {
            return Err("expected hex digits after 0x".into());
        }
        self.skip_number_suffix();
        Ok(Spanned { token: Token::IntLit(val), span: sp })
    }

    fn lex_octal(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // 0
        self.advance(); // o
        let mut val: i128 = 0;
        let mut any = false;
        while (self.peek() >= b'0' && self.peek() <= b'7') || self.peek() == b'_' {
            let b = self.advance();
            if b != b'_' {
                val = val * 8 + (b - b'0') as i128;
                any = true;
            }
        }
        if !any {
            return Err("expected octal digits after 0o".into());
        }
        self.skip_number_suffix();
        Ok(Spanned { token: Token::IntLit(val), span: sp })
    }

    fn lex_binary(&mut self, sp: Span) -> Result<Spanned, String> {
        self.advance(); // 0
        self.advance(); // b
        let mut val: i128 = 0;
        let mut any = false;
        while self.peek() == b'0' || self.peek() == b'1' || self.peek() == b'_' {
            let b = self.advance();
            if b != b'_' {
                val = val * 2 + (b - b'0') as i128;
                any = true;
            }
        }
        if !any {
            return Err("expected binary digits after 0b".into());
        }
        self.skip_number_suffix();
        Ok(Spanned { token: Token::IntLit(val), span: sp })
    }

    fn skip_number_suffix(&mut self) {
        // i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
        let start = self.pos;
        if self.peek() == b'i' || self.peek() == b'u' || self.peek() == b'f' {
            self.advance();
            while self.peek().is_ascii_alphanumeric() {
                self.advance();
            }
            // Verify it's a valid suffix (otherwise rollback)
            let suffix = core::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
            match suffix {
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32"
                | "u64" | "u128" | "usize" | "f32" | "f64" => {}
                _ => {
                    self.pos = start;
                }
            }
        }
    }

    fn lex_ident(&mut self, sp: Span) -> Result<Spanned, String> {
        let mut ident = String::new();
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            ident.push(self.advance() as char);
        }
        let token = match ident.as_str() {
            "as" => Token::As,
            "async" => Token::Async,
            "await" => Token::Await,
            "break" => Token::Break,
            "const" => Token::Const,
            "continue" => Token::Continue,
            "crate" => Token::Crate,
            "dyn" => Token::Dyn,
            "else" => Token::Else,
            "enum" => Token::Enum,
            "extern" => Token::Extern,
            "false" => Token::False,
            "fn" => Token::Fn,
            "for" => Token::For,
            "if" => Token::If,
            "impl" => Token::Impl,
            "in" => Token::In,
            "let" => Token::Let,
            "loop" => Token::Loop,
            "match" => Token::Match,
            "mod" => Token::Mod,
            "move" => Token::Move,
            "mut" => Token::Mut,
            "pub" => Token::Pub,
            "ref" => Token::Ref,
            "return" => Token::Return,
            "self" => Token::SelfLower,
            "Self" => Token::SelfUpper,
            "static" => Token::Static,
            "struct" => Token::Struct,
            "super" => Token::Super,
            "trait" => Token::Trait,
            "true" => Token::True,
            "type" => Token::Type,
            "unsafe" => Token::Unsafe,
            "use" => Token::Use,
            "where" => Token::Where,
            "while" => Token::While,
            "yield" => Token::Yield,
            "_" => Token::Underscore,
            _ => Token::Ident(ident),
        };
        Ok(Spanned { token, span: sp })
    }

    fn lex_punct(&mut self, sp: Span) -> Result<Spanned, String> {
        let b = self.advance();
        let tok = match b {
            b'(' => Token::LParen,
            b')' => Token::RParen,
            b'[' => Token::LBracket,
            b']' => Token::RBracket,
            b'{' => Token::LBrace,
            b'}' => Token::RBrace,
            b',' => Token::Comma,
            b';' => Token::Semi,
            b'@' => Token::At,
            b'#' => Token::Pound,
            b'~' => Token::Tilde,
            b'?' => Token::Question,
            b'.' => {
                if self.peek() == b'.' {
                    self.advance();
                    if self.peek() == b'=' {
                        self.advance();
                        Token::DotDotEq
                    } else if self.peek() == b'.' {
                        self.advance();
                        Token::DotDotDot
                    } else {
                        Token::DotDot
                    }
                } else {
                    Token::Dot
                }
            }
            b':' => {
                if self.peek() == b':' {
                    self.advance();
                    Token::ColonColon
                } else {
                    Token::Colon
                }
            }
            b'=' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::EqEq
                } else if self.peek() == b'>' {
                    self.advance();
                    Token::FatArrow
                } else {
                    Token::Eq
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::Ne
                } else {
                    Token::Bang
                }
            }
            b'<' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::Le
                } else if self.peek() == b'<' {
                    self.advance();
                    if self.peek() == b'=' {
                        self.advance();
                        Token::ShlEq
                    } else {
                        Token::Shl
                    }
                } else {
                    Token::Lt
                }
            }
            b'>' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::Ge
                } else if self.peek() == b'>' {
                    self.advance();
                    if self.peek() == b'=' {
                        self.advance();
                        Token::ShrEq
                    } else {
                        Token::Shr
                    }
                } else {
                    Token::Gt
                }
            }
            b'+' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::PlusEq
                } else {
                    Token::Plus
                }
            }
            b'-' => {
                if self.peek() == b'>' {
                    self.advance();
                    Token::ThinArrow
                } else if self.peek() == b'=' {
                    self.advance();
                    Token::MinusEq
                } else {
                    Token::Minus
                }
            }
            b'*' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::StarEq
                } else {
                    Token::Star
                }
            }
            b'/' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::SlashEq
                } else {
                    Token::Slash
                }
            }
            b'%' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::PercentEq
                } else {
                    Token::Percent
                }
            }
            b'^' => {
                if self.peek() == b'=' {
                    self.advance();
                    Token::CaretEq
                } else {
                    Token::Caret
                }
            }
            b'&' => {
                if self.peek() == b'&' {
                    self.advance();
                    Token::AmpAmp
                } else if self.peek() == b'=' {
                    self.advance();
                    Token::AmpEq
                } else {
                    Token::Amp
                }
            }
            b'|' => {
                if self.peek() == b'|' {
                    self.advance();
                    Token::PipePipe
                } else if self.peek() == b'=' {
                    self.advance();
                    Token::PipeEq
                } else {
                    Token::Pipe
                }
            }
            _ => {
                return Err(format!(
                    "unexpected character '{}' at {}:{}",
                    b as char, sp.line, sp.col
                ));
            }
        };
        Ok(Spanned { token: tok, span: sp })
    }
}

fn hex_digit(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("invalid hex digit: '{}'", b as char)),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Token> {
        Lexer::tokenize(src)
            .unwrap()
            .into_iter()
            .map(|s| s.token)
            .filter(|t| !t.is_eof())
            .collect()
    }

    #[test]
    fn keywords() {
        assert_eq!(toks("fn let mut if else"), vec![
            Token::Fn, Token::Let, Token::Mut, Token::If, Token::Else,
        ]);
    }

    #[test]
    fn identifiers() {
        assert_eq!(toks("foo _bar Baz42"), vec![
            Token::Ident("foo".into()),
            Token::Ident("_bar".into()),
            Token::Ident("Baz42".into()),
        ]);
    }

    #[test]
    fn integers() {
        assert_eq!(toks("42 0xff 0o77 0b1010 1_000"), vec![
            Token::IntLit(42),
            Token::IntLit(255),
            Token::IntLit(63),
            Token::IntLit(10),
            Token::IntLit(1000),
        ]);
    }

    #[test]
    fn floats() {
        assert_eq!(toks("3.14 1e10 2.5e-3"), vec![
            Token::FloatLit(3.14),
            Token::FloatLit(1e10),
            Token::FloatLit(2.5e-3),
        ]);
    }

    #[test]
    fn strings() {
        assert_eq!(toks(r#""hello" "world\n""#), vec![
            Token::StringLit("hello".into()),
            Token::StringLit("world\n".into()),
        ]);
    }

    #[test]
    fn char_and_lifetime() {
        assert_eq!(toks("'a' 'b"), vec![
            Token::CharLit('a'),
            Token::Lifetime("'b".into()),
        ]);
    }

    #[test]
    fn operators() {
        assert_eq!(toks("+ - * / == != <= >= && || -> =>"), vec![
            Token::Plus, Token::Minus, Token::Star, Token::Slash,
            Token::EqEq, Token::Ne, Token::Le, Token::Ge,
            Token::AmpAmp, Token::PipePipe, Token::ThinArrow, Token::FatArrow,
        ]);
    }

    #[test]
    fn delimiters() {
        assert_eq!(toks("()[]{}"), vec![
            Token::LParen, Token::RParen,
            Token::LBracket, Token::RBracket,
            Token::LBrace, Token::RBrace,
        ]);
    }

    #[test]
    fn comments_skipped() {
        assert_eq!(toks("a // comment\nb /* block */ c"), vec![
            Token::Ident("a".into()),
            Token::Ident("b".into()),
            Token::Ident("c".into()),
        ]);
    }

    #[test]
    fn full_function() {
        let src = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let tokens = toks(src);
        assert_eq!(tokens[0], Token::Fn);
        assert_eq!(tokens[1], Token::Ident("add".into()));
        assert_eq!(tokens[2], Token::LParen);
    }
}
