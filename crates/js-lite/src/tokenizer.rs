//! JavaScript tokenizer for js-lite.
//!
//! Converts source text into a flat stream of tokens. Handles:
//! - Identifiers and keywords
//! - Number literals (integer, float, hex)
//! - String literals (single/double quotes, basic escapes)
//! - Template literals (backtick strings with `${expr}` interpolation)
//! - Regex literals (/pattern/flags)
//! - All JS operators including bitwise and logical
//! - Semicolons, commas, braces, brackets, parens
//! - Single-line (//) and multi-line (/* */) comments (skipped)

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    Str(String),
    TemplateLiteral(Vec<TemplatePart>),
    Regex(String, String), // (pattern, flags)
    True,
    False,
    Null,
    Undefined,

    // Identifier
    Ident(String),

    // Keywords
    Var,
    Let,
    Const,
    Function,
    Return,
    If,
    Else,
    For,
    While,
    Do,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    New,
    This,
    Typeof,
    Instanceof,
    In,
    Of,
    Try,
    Catch,
    Finally,
    Throw,
    Void,
    Delete,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    DoubleStar,   // **
    Assign,       // =
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign, // %=
    AmpAssign,    // &=
    PipeAssign,   // |=
    CaretAssign,  // ^=
    ShlAssign,    // <<=
    ShrAssign,    // >>=
    UshrAssign,   // >>>=
    Eq,           // ==
    StrictEq,     // ===
    NotEq,        // !=
    StrictNotEq,  // !==
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,          // &&
    Or,           // ||
    Not,          // !
    BitAnd,       // &
    BitOr,        // |
    BitXor,       // ^
    BitNot,       // ~
    Shl,          // <<
    Shr,          // >>
    Ushr,         // >>>
    Question,     // ?
    NullishCoalesce, // ??
    OptionalChain,   // ?.
    Arrow,        // =>
    Spread,       // ...
    PlusPlus,     // ++
    MinusMinus,   // --

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Dot,

    Eof,
}

/// A part of a template literal.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    /// Raw string portion.
    Str(String),
    /// Expression tokens inside `${...}`.
    Expr(Vec<Token>),
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut tokenizer = Tokenizer::new(source);
    tokenizer.tokenize_all()
}

struct Tokenizer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Tracks whether the last meaningful token could precede a regex literal.
    /// After operators, `(`, `[`, `{`, `,`, `;`, `return`, etc. a `/` starts a regex.
    /// After identifiers, numbers, `)`, `]`, `}`, etc. a `/` is division.
    last_token_allows_regex: bool,
}

impl<'a> Tokenizer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
            last_token_allows_regex: true,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.src.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while self.pos < self.src.len() && matches!(self.src[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            }

            // Skip single-line comments
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'/' {
                self.pos += 2;
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }

            // Skip multi-line comments
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'*' {
                self.pos += 2;
                while self.pos + 1 < self.src.len() {
                    if self.src[self.pos] == b'*' && self.src[self.pos + 1] == b'/' {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                }
                continue;
            }

            break;
        }
    }

    fn read_string(&mut self, quote: u8) -> Result<String, String> {
        self.advance(); // consume opening quote
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err(String::from("unterminated string literal")),
                Some(b'\\') => {
                    match self.advance() {
                        Some(b'n') => s.push('\n'),
                        Some(b'r') => s.push('\r'),
                        Some(b't') => s.push('\t'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'\'') => s.push('\''),
                        Some(b'"') => s.push('"'),
                        Some(b'`') => s.push('`'),
                        Some(b'0') => s.push('\0'),
                        Some(b'/') => s.push('/'),
                        Some(b'x') => {
                            let h = self.read_hex_escape(2)?;
                            if let Some(c) = char::from_u32(h) {
                                s.push(c);
                            }
                        }
                        Some(b'u') => {
                            let h = if self.peek() == Some(b'{') {
                                self.advance(); // {
                                let val = self.read_hex_escape_variable()?;
                                if self.peek() == Some(b'}') { self.advance(); }
                                val
                            } else {
                                self.read_hex_escape(4)?
                            };
                            if let Some(c) = char::from_u32(h) {
                                s.push(c);
                            }
                        }
                        Some(ch) => {
                            // Unknown escape -- keep the character as-is
                            s.push(ch as char);
                        }
                        None => return Err(String::from("unterminated string escape")),
                    }
                }
                Some(ch) if ch == quote => return Ok(s),
                Some(ch) => {
                    // Handle multi-byte UTF-8
                    if ch < 0x80 {
                        s.push(ch as char);
                    } else {
                        // Reconstruct UTF-8 character
                        let start = self.pos - 1;
                        let remaining = &self.src[start..];
                        if let Some(c) = core::str::from_utf8(remaining).ok().and_then(|s| s.chars().next()) {
                            s.push(c);
                            self.pos = start + c.len_utf8();
                        } else {
                            s.push(ch as char);
                        }
                    }
                }
            }
        }
    }

    fn read_template_literal(&mut self) -> Result<Token, String> {
        self.advance(); // consume opening backtick
        let mut parts = Vec::new();
        let mut current_str = String::new();

        loop {
            match self.peek() {
                None => return Err(String::from("unterminated template literal")),
                Some(b'`') => {
                    self.advance();
                    if !current_str.is_empty() {
                        parts.push(TemplatePart::Str(current_str));
                    }
                    return Ok(Token::TemplateLiteral(parts));
                }
                Some(b'$') if self.peek_at(1) == Some(b'{') => {
                    self.advance(); // $
                    self.advance(); // {
                    if !current_str.is_empty() {
                        parts.push(TemplatePart::Str(core::mem::take(&mut current_str)));
                    }
                    // Tokenize the expression inside ${...}
                    let expr_tokens = self.tokenize_template_expr()?;
                    parts.push(TemplatePart::Expr(expr_tokens));
                }
                Some(b'\\') => {
                    self.advance();
                    match self.advance() {
                        Some(b'n') => current_str.push('\n'),
                        Some(b'r') => current_str.push('\r'),
                        Some(b't') => current_str.push('\t'),
                        Some(b'\\') => current_str.push('\\'),
                        Some(b'`') => current_str.push('`'),
                        Some(b'$') => current_str.push('$'),
                        Some(ch) => current_str.push(ch as char),
                        None => return Err(String::from("unterminated template escape")),
                    }
                }
                Some(ch) => {
                    self.advance();
                    if ch < 0x80 {
                        current_str.push(ch as char);
                    } else {
                        let start = self.pos - 1;
                        let remaining = &self.src[start..];
                        if let Some(c) = core::str::from_utf8(remaining).ok().and_then(|s| s.chars().next()) {
                            current_str.push(c);
                            self.pos = start + c.len_utf8();
                        } else {
                            current_str.push(ch as char);
                        }
                    }
                }
            }
        }
    }

    fn tokenize_template_expr(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        let mut brace_depth = 1u32;
        loop {
            self.skip_whitespace_and_comments();
            match self.peek() {
                None => return Err(String::from("unterminated template expression")),
                Some(b'}') => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        self.advance();
                        return Ok(tokens);
                    }
                    self.advance();
                    tokens.push(Token::RBrace);
                }
                Some(b'{') => {
                    self.advance();
                    brace_depth += 1;
                    tokens.push(Token::LBrace);
                }
                _ => {
                    let tok = self.next_token()?;
                    if tok == Token::Eof {
                        return Err(String::from("unterminated template expression"));
                    }
                    tokens.push(tok);
                }
            }
        }
    }

    fn read_hex_escape(&mut self, count: usize) -> Result<u32, String> {
        let mut val = 0u32;
        for _ in 0..count {
            match self.advance() {
                Some(ch) => {
                    let digit = match ch {
                        b'0'..=b'9' => (ch - b'0') as u32,
                        b'a'..=b'f' => (ch - b'a' + 10) as u32,
                        b'A'..=b'F' => (ch - b'A' + 10) as u32,
                        _ => return Err(String::from("invalid hex escape")),
                    };
                    val = val * 16 + digit;
                }
                None => return Err(String::from("unterminated hex escape")),
            }
        }
        Ok(val)
    }

    fn read_hex_escape_variable(&mut self) -> Result<u32, String> {
        let mut val = 0u32;
        let mut count = 0;
        while let Some(ch) = self.peek() {
            if ch == b'}' { break; }
            self.advance();
            let digit = match ch {
                b'0'..=b'9' => (ch - b'0') as u32,
                b'a'..=b'f' => (ch - b'a' + 10) as u32,
                b'A'..=b'F' => (ch - b'A' + 10) as u32,
                _ => return Err(String::from("invalid hex escape")),
            };
            val = val * 16 + digit;
            count += 1;
            if count > 6 { return Err(String::from("hex escape too long")); }
        }
        Ok(val)
    }

    fn read_number(&mut self) -> Result<Token, String> {
        let start = self.pos;

        // Check for 0x hex literal
        if self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'x') | Some(b'X')) {
            self.advance(); // 0
            self.advance(); // x
            let hex_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_hexdigit() {
                    self.advance();
                } else {
                    break;
                }
            }
            let hex_str = core::str::from_utf8(&self.src[hex_start..self.pos])
                .map_err(|_| String::from("invalid hex literal"))?;
            let val = u64::from_str_radix(hex_str, 16)
                .map_err(|_| String::from("invalid hex literal"))?;
            return Ok(Token::Number(val as f64));
        }

        // Check for 0b binary literal
        if self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'b') | Some(b'B')) {
            self.advance(); // 0
            self.advance(); // b
            let bin_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch == b'0' || ch == b'1' {
                    self.advance();
                } else {
                    break;
                }
            }
            let bin_str = core::str::from_utf8(&self.src[bin_start..self.pos])
                .map_err(|_| String::from("invalid binary literal"))?;
            let val = u64::from_str_radix(bin_str, 2)
                .map_err(|_| String::from("invalid binary literal"))?;
            return Ok(Token::Number(val as f64));
        }

        // Regular decimal number
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }

        let mut is_float = false;

        // Check for decimal point
        if self.peek() == Some(b'.') && self.peek_at(1).map_or(false, |c| c.is_ascii_digit()) {
            is_float = true;
            self.advance(); // .
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Check for exponent
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.advance();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.advance();
            }
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        let num_str = core::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| String::from("invalid number"))?;

        if is_float {
            let val = parse_float(num_str)?;
            Ok(Token::Number(val))
        } else {
            // Try parsing as integer first, then float for large numbers
            if let Ok(val) = parse_int(num_str) {
                Ok(Token::Number(val as f64))
            } else {
                let val = parse_float(num_str)?;
                Ok(Token::Number(val))
            }
        }
    }

    fn read_regex(&mut self) -> Result<Token, String> {
        self.advance(); // consume opening /
        let mut pattern = String::new();
        let mut in_class = false;

        loop {
            match self.advance() {
                None => return Err(String::from("unterminated regex literal")),
                Some(b'\\') => {
                    pattern.push('\\');
                    if let Some(ch) = self.advance() {
                        pattern.push(ch as char);
                    }
                }
                Some(b'[') => {
                    in_class = true;
                    pattern.push('[');
                }
                Some(b']') => {
                    in_class = false;
                    pattern.push(']');
                }
                Some(b'/') if !in_class => {
                    // End of pattern, read flags
                    let mut flags = String::new();
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_alphabetic() {
                            flags.push(ch as char);
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    return Ok(Token::Regex(pattern, flags));
                }
                Some(ch) => pattern.push(ch as char),
            }
        }
    }

    fn read_identifier(&mut self) -> Token {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'$' {
                self.advance();
            } else {
                break;
            }
        }
        let word = core::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
        match word {
            "var" => Token::Var,
            "let" => Token::Let,
            "const" => Token::Const,
            "function" => Token::Function,
            "return" => Token::Return,
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "while" => Token::While,
            "do" => Token::Do,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "switch" => Token::Switch,
            "case" => Token::Case,
            "default" => Token::Default,
            "new" => Token::New,
            "this" => Token::This,
            "typeof" => Token::Typeof,
            "instanceof" => Token::Instanceof,
            "in" => Token::In,
            "of" => Token::Of,
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            "undefined" => Token::Undefined,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "finally" => Token::Finally,
            "throw" => Token::Throw,
            "void" => Token::Void,
            "delete" => Token::Delete,
            _ => Token::Ident(String::from(word)),
        }
    }

    fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace_and_comments();

        let ch = match self.peek() {
            None => return Ok(Token::Eof),
            Some(ch) => ch,
        };

        let tok = match ch {
            b'"' | b'\'' => {
                let s = self.read_string(ch)?;
                Token::Str(s)
            }
            b'`' => self.read_template_literal()?,
            b'0'..=b'9' => self.read_number()?,
            b'.' => {
                if self.peek_at(1).map_or(false, |c| c.is_ascii_digit()) {
                    self.read_number()?
                } else if self.peek_at(1) == Some(b'.') && self.peek_at(2) == Some(b'.') {
                    self.advance(); self.advance(); self.advance();
                    Token::Spread
                } else {
                    self.advance();
                    Token::Dot
                }
            }
            b'/' => {
                if self.last_token_allows_regex {
                    self.read_regex()?
                } else {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        Token::SlashAssign
                    } else {
                        Token::Slash
                    }
                }
            }
            b'+' => {
                self.advance();
                if self.peek() == Some(b'+') { self.advance(); Token::PlusPlus }
                else if self.peek() == Some(b'=') { self.advance(); Token::PlusAssign }
                else { Token::Plus }
            }
            b'-' => {
                self.advance();
                if self.peek() == Some(b'-') { self.advance(); Token::MinusMinus }
                else if self.peek() == Some(b'=') { self.advance(); Token::MinusAssign }
                else { Token::Minus }
            }
            b'*' => {
                self.advance();
                if self.peek() == Some(b'*') { self.advance(); Token::DoubleStar }
                else if self.peek() == Some(b'=') { self.advance(); Token::StarAssign }
                else { Token::Star }
            }
            b'%' => {
                self.advance();
                if self.peek() == Some(b'=') { self.advance(); Token::PercentAssign }
                else { Token::Percent }
            }
            b'=' => {
                self.advance();
                if self.peek() == Some(b'=') {
                    self.advance();
                    if self.peek() == Some(b'=') { self.advance(); Token::StrictEq }
                    else { Token::Eq }
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Assign
                }
            }
            b'!' => {
                self.advance();
                if self.peek() == Some(b'=') {
                    self.advance();
                    if self.peek() == Some(b'=') { self.advance(); Token::StrictNotEq }
                    else { Token::NotEq }
                } else {
                    Token::Not
                }
            }
            b'<' => {
                self.advance();
                if self.peek() == Some(b'<') {
                    self.advance();
                    if self.peek() == Some(b'=') { self.advance(); Token::ShlAssign }
                    else { Token::Shl }
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    Token::LtEq
                } else {
                    Token::Lt
                }
            }
            b'>' => {
                self.advance();
                if self.peek() == Some(b'>') {
                    self.advance();
                    if self.peek() == Some(b'>') {
                        self.advance();
                        if self.peek() == Some(b'=') { self.advance(); Token::UshrAssign }
                        else { Token::Ushr }
                    } else if self.peek() == Some(b'=') {
                        self.advance();
                        Token::ShrAssign
                    } else {
                        Token::Shr
                    }
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    Token::GtEq
                } else {
                    Token::Gt
                }
            }
            b'&' => {
                self.advance();
                if self.peek() == Some(b'&') { self.advance(); Token::And }
                else if self.peek() == Some(b'=') { self.advance(); Token::AmpAssign }
                else { Token::BitAnd }
            }
            b'|' => {
                self.advance();
                if self.peek() == Some(b'|') { self.advance(); Token::Or }
                else if self.peek() == Some(b'=') { self.advance(); Token::PipeAssign }
                else { Token::BitOr }
            }
            b'^' => {
                self.advance();
                if self.peek() == Some(b'=') { self.advance(); Token::CaretAssign }
                else { Token::BitXor }
            }
            b'~' => { self.advance(); Token::BitNot }
            b'?' => {
                self.advance();
                if self.peek() == Some(b'?') { self.advance(); Token::NullishCoalesce }
                else if self.peek() == Some(b'.') { self.advance(); Token::OptionalChain }
                else { Token::Question }
            }
            b'(' => { self.advance(); Token::LParen }
            b')' => { self.advance(); Token::RParen }
            b'{' => { self.advance(); Token::LBrace }
            b'}' => { self.advance(); Token::RBrace }
            b'[' => { self.advance(); Token::LBracket }
            b']' => { self.advance(); Token::RBracket }
            b',' => { self.advance(); Token::Comma }
            b';' => { self.advance(); Token::Semicolon }
            b':' => { self.advance(); Token::Colon }
            _ if ch.is_ascii_alphabetic() || ch == b'_' || ch == b'$' => {
                self.read_identifier()
            }
            _ => {
                self.advance();
                return Err(alloc::format!("unexpected character: {:?}", ch as char));
            }
        };

        // Update regex-context flag
        self.last_token_allows_regex = matches!(
            tok,
            Token::Assign | Token::PlusAssign | Token::MinusAssign |
            Token::StarAssign | Token::SlashAssign | Token::PercentAssign |
            Token::AmpAssign | Token::PipeAssign | Token::CaretAssign |
            Token::ShlAssign | Token::ShrAssign | Token::UshrAssign |
            Token::Plus | Token::Minus | Token::Star | Token::Percent |
            Token::Eq | Token::StrictEq | Token::NotEq | Token::StrictNotEq |
            Token::Lt | Token::Gt | Token::LtEq | Token::GtEq |
            Token::And | Token::Or | Token::Not |
            Token::BitAnd | Token::BitOr | Token::BitXor | Token::BitNot |
            Token::Shl | Token::Shr | Token::Ushr |
            Token::Question | Token::NullishCoalesce |
            Token::Comma | Token::Semicolon | Token::Colon |
            Token::LParen | Token::LBracket | Token::LBrace |
            Token::Return | Token::Throw | Token::Typeof | Token::Void |
            Token::Delete | Token::New | Token::In | Token::Instanceof |
            Token::Case | Token::Arrow | Token::Spread | Token::DoubleStar
        );

        Ok(tok)
    }

    fn tokenize_all(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            if tok == Token::Eof {
                tokens.push(Token::Eof);
                break;
            }
            tokens.push(tok);
        }
        Ok(tokens)
    }
}

// ---------------------------------------------------------------------------
// Minimal float/int parsing (no_std)
// ---------------------------------------------------------------------------

fn parse_float(s: &str) -> Result<f64, String> {
    // Simple manual float parser for no_std
    let s = s.trim();
    if s.is_empty() {
        return Err(String::from("empty number"));
    }

    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };

    // Split on 'e' or 'E' for exponent
    let (mantissa_str, exp_str) = if let Some(pos) = s.find(|c: char| c == 'e' || c == 'E') {
        (&s[..pos], Some(&s[pos + 1..]))
    } else {
        (s, None)
    };

    // Parse mantissa
    let (int_part, frac_part) = if let Some(dot_pos) = mantissa_str.find('.') {
        (&mantissa_str[..dot_pos], Some(&mantissa_str[dot_pos + 1..]))
    } else {
        (mantissa_str, None)
    };

    let mut val: f64 = 0.0;
    for b in int_part.bytes() {
        if b.is_ascii_digit() {
            val = val * 10.0 + (b - b'0') as f64;
        }
    }

    if let Some(frac) = frac_part {
        let mut frac_mul = 0.1;
        for b in frac.bytes() {
            if b.is_ascii_digit() {
                val += (b - b'0') as f64 * frac_mul;
                frac_mul *= 0.1;
            }
        }
    }

    if let Some(exp) = exp_str {
        let (exp_neg, exp_digits) = if let Some(rest) = exp.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = exp.strip_prefix('+') {
            (false, rest)
        } else {
            (false, exp)
        };
        let mut exp_val: i32 = 0;
        for b in exp_digits.bytes() {
            if b.is_ascii_digit() {
                exp_val = exp_val * 10 + (b - b'0') as i32;
            }
        }
        if exp_neg { exp_val = -exp_val; }
        val *= pow10(exp_val);
    }

    if negative { val = -val; }
    Ok(val)
}

fn parse_int(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };

    let mut val: i64 = 0;
    for b in s.bytes() {
        if b.is_ascii_digit() {
            val = val.checked_mul(10).and_then(|v| v.checked_add((b - b'0') as i64))
                .ok_or_else(|| String::from("integer overflow"))?;
        } else {
            return Err(String::from("invalid integer"));
        }
    }
    if negative { val = -val; }
    Ok(val)
}

fn pow10(exp: i32) -> f64 {
    if exp >= 0 {
        let mut result = 1.0;
        for _ in 0..exp { result *= 10.0; }
        result
    } else {
        let mut result = 1.0;
        for _ in 0..(-exp) { result /= 10.0; }
        result
    }
}
