//! Go tokenizer with automatic semicolon insertion.
//!
//! Implements Go's lexical rules including raw strings (`...`), rune literals,
//! and the semicolon insertion rules from the Go spec.

use alloc::string::String;
use alloc::vec::Vec;

/// Source location for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A Go token with its source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // === Keywords ===
    Break,
    Case,
    Chan,
    Const,
    Continue,
    Default,
    Defer,
    Else,
    Fallthrough,
    For,
    Func,
    Go,
    Goto,
    If,
    Import,
    Interface,
    Map,
    Package,
    Range,
    Return,
    Select,
    Struct,
    Switch,
    Type,
    Var,

    // === Identifiers & Literals ===
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    RawStringLit(String),
    RuneLit(char),

    // === Operators ===
    Plus,          // +
    Minus,         // -
    Star,          // *
    Slash,         // /
    Percent,       // %
    Amp,           // &
    Pipe,          // |
    Caret,         // ^
    Shl,           // <<
    Shr,           // >>
    AmpCaret,      // &^
    PlusAssign,    // +=
    MinusAssign,   // -=
    StarAssign,    // *=
    SlashAssign,   // /=
    PercentAssign, // %=
    AmpAssign,     // &=
    PipeAssign,    // |=
    CaretAssign,   // ^=
    ShlAssign,     // <<=
    ShrAssign,     // >>=
    AmpCaretAssign,// &^=
    AmpAmp,        // &&
    PipePipe,      // ||
    Arrow,         // <-
    PlusPlus,      // ++
    MinusMinus,    // --
    EqEq,          // ==
    Lt,            // <
    Gt,            // >
    Assign,        // =
    Bang,          // !
    Ne,            // !=
    Le,            // <=
    Ge,            // >=
    ColonAssign,   // :=
    Ellipsis,      // ...
    Dot,           // .

    // === Delimiters ===
    LParen,        // (
    RParen,        // )
    LBrace,        // {
    RBrace,        // }
    LBracket,      // [
    RBracket,      // ]
    Semicolon,     // ;
    Colon,         // :
    Comma,         // ,

    // === Special ===
    Eof,
}

/// Tokenize Go source code, including automatic semicolon insertion.
pub fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();

    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }

    // Automatic semicolon insertion
    let tokens = insert_semicolons(tokens);

    Ok(tokens)
}

struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn peek_ahead(&self, n: usize) -> Option<u8> {
        self.source.get(self.pos + n).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace (but not newlines — they matter for semicolon insertion)
            while let Some(ch) = self.peek() {
                if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                    self.advance();
                } else {
                    break;
                }
            }

            // Skip line comments
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'/') {
                while let Some(ch) = self.advance() {
                    if ch == b'\n' {
                        break;
                    }
                }
                continue;
            }

            // Skip block comments
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'*') {
                self.advance();
                self.advance();
                loop {
                    match self.advance() {
                        Some(b'*') if self.peek() == Some(b'/') => {
                            self.advance();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
                continue;
            }

            break;
        }
    }

    fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace_and_comments();

        let span = self.span();

        let ch = match self.peek() {
            Some(ch) => ch,
            None => return Ok(Token { kind: TokenKind::Eof, span }),
        };

        // Number literals
        if ch.is_ascii_digit() {
            return self.lex_number(span);
        }

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return self.lex_ident(span);
        }

        // String literals
        if ch == b'"' {
            return self.lex_string(span);
        }

        // Raw string literals
        if ch == b'`' {
            return self.lex_raw_string(span);
        }

        // Rune literals
        if ch == b'\'' {
            return self.lex_rune(span);
        }

        // Operators and delimiters
        self.lex_operator(span)
    }

    fn lex_number(&mut self, span: Span) -> Result<Token, String> {
        let start = self.pos;
        let mut is_float = false;

        // Handle 0x, 0o, 0b prefixes
        if self.peek() == Some(b'0') {
            match self.peek_ahead(1) {
                Some(b'x') | Some(b'X') => {
                    self.advance();
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_hexdigit() || ch == b'_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let s: String = self.source[start..self.pos]
                        .iter()
                        .filter(|&&c| c != b'_')
                        .map(|&c| c as char)
                        .collect();
                    let val = i64::from_str_radix(&s[2..], 16)
                        .map_err(|e| alloc::format!("{}:{}: bad hex literal: {}", span.line, span.col, e))?;
                    return Ok(Token { kind: TokenKind::IntLit(val), span });
                }
                Some(b'o') | Some(b'O') => {
                    self.advance();
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if (b'0'..=b'7').contains(&ch) || ch == b'_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let s: String = self.source[start..self.pos]
                        .iter()
                        .filter(|&&c| c != b'_')
                        .map(|&c| c as char)
                        .collect();
                    let val = i64::from_str_radix(&s[2..], 8)
                        .map_err(|e| alloc::format!("{}:{}: bad octal literal: {}", span.line, span.col, e))?;
                    return Ok(Token { kind: TokenKind::IntLit(val), span });
                }
                Some(b'b') | Some(b'B') => {
                    self.advance();
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if ch == b'0' || ch == b'1' || ch == b'_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let s: String = self.source[start..self.pos]
                        .iter()
                        .filter(|&&c| c != b'_')
                        .map(|&c| c as char)
                        .collect();
                    let val = i64::from_str_radix(&s[2..], 2)
                        .map_err(|e| alloc::format!("{}:{}: bad binary literal: {}", span.line, span.col, e))?;
                    return Ok(Token { kind: TokenKind::IntLit(val), span });
                }
                _ => {}
            }
        }

        // Decimal integer or float
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        if self.peek() == Some(b'.') && self.peek_ahead(1).map_or(false, |c| c.is_ascii_digit()) {
            is_float = true;
            self.advance(); // '.'
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == b'_' {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Exponent
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            is_float = true;
            self.advance();
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
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

        let s: String = self.source[start..self.pos]
            .iter()
            .filter(|&&c| c != b'_')
            .map(|&c| c as char)
            .collect();

        if is_float {
            let val: f64 = s.parse()
                .map_err(|e| alloc::format!("{}:{}: bad float: {}", span.line, span.col, e))?;
            Ok(Token { kind: TokenKind::FloatLit(val), span })
        } else {
            let val: i64 = s.parse()
                .map_err(|e| alloc::format!("{}:{}: bad integer: {}", span.line, span.col, e))?;
            Ok(Token { kind: TokenKind::IntLit(val), span })
        }
    }

    fn lex_ident(&mut self, span: Span) -> Result<Token, String> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        let word: String = self.source[start..self.pos].iter().map(|&c| c as char).collect();

        let kind = match word.as_str() {
            "break" => TokenKind::Break,
            "case" => TokenKind::Case,
            "chan" => TokenKind::Chan,
            "const" => TokenKind::Const,
            "continue" => TokenKind::Continue,
            "default" => TokenKind::Default,
            "defer" => TokenKind::Defer,
            "else" => TokenKind::Else,
            "fallthrough" => TokenKind::Fallthrough,
            "for" => TokenKind::For,
            "func" => TokenKind::Func,
            "go" => TokenKind::Go,
            "goto" => TokenKind::Goto,
            "if" => TokenKind::If,
            "import" => TokenKind::Import,
            "interface" => TokenKind::Interface,
            "map" => TokenKind::Map,
            "package" => TokenKind::Package,
            "range" => TokenKind::Range,
            "return" => TokenKind::Return,
            "select" => TokenKind::Select,
            "struct" => TokenKind::Struct,
            "switch" => TokenKind::Switch,
            "type" => TokenKind::Type,
            "var" => TokenKind::Var,
            _ => TokenKind::Ident(word),
        };

        Ok(Token { kind, span })
    }

    fn lex_string(&mut self, span: Span) -> Result<Token, String> {
        self.advance(); // opening "
        let mut s = String::new();

        loop {
            match self.advance() {
                Some(b'"') => break,
                Some(b'\\') => {
                    let esc = self.advance().ok_or_else(|| {
                        alloc::format!("{}:{}: unterminated string escape", span.line, span.col)
                    })?;
                    match esc {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'\\' => s.push('\\'),
                        b'"' => s.push('"'),
                        b'0' => s.push('\0'),
                        b'x' => {
                            let h = self.advance().unwrap_or(b'0');
                            let l = self.advance().unwrap_or(b'0');
                            let val = hex_digit(h) * 16 + hex_digit(l);
                            s.push(val as char);
                        }
                        _ => {
                            s.push('\\');
                            s.push(esc as char);
                        }
                    }
                }
                Some(ch) => s.push(ch as char),
                None => return Err(alloc::format!("{}:{}: unterminated string", span.line, span.col)),
            }
        }

        Ok(Token { kind: TokenKind::StringLit(s), span })
    }

    fn lex_raw_string(&mut self, span: Span) -> Result<Token, String> {
        self.advance(); // opening `
        let mut s = String::new();
        loop {
            match self.advance() {
                Some(b'`') => break,
                Some(ch) => s.push(ch as char),
                None => return Err(alloc::format!("{}:{}: unterminated raw string", span.line, span.col)),
            }
        }
        Ok(Token { kind: TokenKind::RawStringLit(s), span })
    }

    fn lex_rune(&mut self, span: Span) -> Result<Token, String> {
        self.advance(); // opening '
        let ch = match self.advance() {
            Some(b'\\') => {
                let esc = self.advance().ok_or_else(|| {
                    alloc::format!("{}:{}: unterminated rune escape", span.line, span.col)
                })?;
                match esc {
                    b'n' => '\n',
                    b't' => '\t',
                    b'r' => '\r',
                    b'\\' => '\\',
                    b'\'' => '\'',
                    b'0' => '\0',
                    _ => esc as char,
                }
            }
            Some(ch) => ch as char,
            None => return Err(alloc::format!("{}:{}: unterminated rune", span.line, span.col)),
        };
        if self.advance() != Some(b'\'') {
            return Err(alloc::format!("{}:{}: unterminated rune literal", span.line, span.col));
        }
        Ok(Token { kind: TokenKind::RuneLit(ch), span })
    }

    fn lex_operator(&mut self, span: Span) -> Result<Token, String> {
        let ch = self.advance().unwrap();
        let kind = match ch {
            b'+' => match self.peek() {
                Some(b'+') => { self.advance(); TokenKind::PlusPlus }
                Some(b'=') => { self.advance(); TokenKind::PlusAssign }
                _ => TokenKind::Plus,
            },
            b'-' => match self.peek() {
                Some(b'-') => { self.advance(); TokenKind::MinusMinus }
                Some(b'=') => { self.advance(); TokenKind::MinusAssign }
                _ => TokenKind::Minus,
            },
            b'*' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::StarAssign }
                _ => TokenKind::Star,
            },
            b'/' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::SlashAssign }
                _ => TokenKind::Slash,
            },
            b'%' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::PercentAssign }
                _ => TokenKind::Percent,
            },
            b'&' => match self.peek() {
                Some(b'&') => { self.advance(); TokenKind::AmpAmp }
                Some(b'^') => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::AmpCaretAssign
                    } else {
                        TokenKind::AmpCaret
                    }
                }
                Some(b'=') => { self.advance(); TokenKind::AmpAssign }
                _ => TokenKind::Amp,
            },
            b'|' => match self.peek() {
                Some(b'|') => { self.advance(); TokenKind::PipePipe }
                Some(b'=') => { self.advance(); TokenKind::PipeAssign }
                _ => TokenKind::Pipe,
            },
            b'^' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::CaretAssign }
                _ => TokenKind::Caret,
            },
            b'<' => match self.peek() {
                Some(b'<') => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::ShlAssign
                    } else {
                        TokenKind::Shl
                    }
                }
                Some(b'=') => { self.advance(); TokenKind::Le }
                Some(b'-') => { self.advance(); TokenKind::Arrow }
                _ => TokenKind::Lt,
            },
            b'>' => match self.peek() {
                Some(b'>') => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::ShrAssign
                    } else {
                        TokenKind::Shr
                    }
                }
                Some(b'=') => { self.advance(); TokenKind::Ge }
                _ => TokenKind::Gt,
            },
            b'=' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::EqEq }
                _ => TokenKind::Assign,
            },
            b'!' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::Ne }
                _ => TokenKind::Bang,
            },
            b':' => match self.peek() {
                Some(b'=') => { self.advance(); TokenKind::ColonAssign }
                _ => TokenKind::Colon,
            },
            b'.' => {
                if self.peek() == Some(b'.') && self.peek_ahead(1) == Some(b'.') {
                    self.advance();
                    self.advance();
                    TokenKind::Ellipsis
                } else {
                    TokenKind::Dot
                }
            }
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            _ => return Err(alloc::format!("{}:{}: unexpected character '{}'", span.line, span.col, ch as char)),
        };

        Ok(Token { kind, span })
    }
}

fn hex_digit(ch: u8) -> u8 {
    match ch {
        b'0'..=b'9' => ch - b'0',
        b'a'..=b'f' => ch - b'a' + 10,
        b'A'..=b'F' => ch - b'A' + 10,
        _ => 0,
    }
}

/// Apply Go's automatic semicolon insertion rules.
///
/// A semicolon is inserted after a line's final token if that token is:
/// - an identifier, literal, or one of: break continue fallthrough return ++ -- ) ] }
fn insert_semicolons(tokens: Vec<Token>) -> Vec<Token> {
    let mut result = Vec::with_capacity(tokens.len());

    for i in 0..tokens.len() {
        let tok = &tokens[i];
        result.push(tok.clone());

        // Check if this token should trigger semicolon insertion
        if needs_semicolon_after(&tok.kind) {
            // Check if the next non-whitespace token is on a new line or is EOF
            if let Some(next) = tokens.get(i + 1) {
                if next.span.line > tok.span.line || next.kind == TokenKind::Eof {
                    result.push(Token {
                        kind: TokenKind::Semicolon,
                        span: tok.span,
                    });
                }
            }
        }
    }

    result
}

fn needs_semicolon_after(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident(_)
            | TokenKind::IntLit(_)
            | TokenKind::FloatLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::RawStringLit(_)
            | TokenKind::RuneLit(_)
            | TokenKind::Break
            | TokenKind::Continue
            | TokenKind::Fallthrough
            | TokenKind::Return
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let tokens = tokenize("package main").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Package));
        assert!(matches!(tokens[1].kind, TokenKind::Ident(ref s) if s == "main"));
    }

    #[test]
    fn test_semicolon_insertion() {
        let tokens = tokenize("x := 42\ny := 10\n").unwrap();
        // Should have semicolons inserted after 42 and 10
        let semis: Vec<_> = tokens.iter().filter(|t| t.kind == TokenKind::Semicolon).collect();
        assert!(semis.len() >= 2);
    }

    #[test]
    fn test_operators() {
        let tokens = tokenize(":= <- &^ ... ++ --").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::ColonAssign));
        assert!(matches!(tokens[1].kind, TokenKind::Arrow));
        assert!(matches!(tokens[2].kind, TokenKind::AmpCaret));
        assert!(matches!(tokens[3].kind, TokenKind::Ellipsis));
    }

    #[test]
    fn test_string_escape() {
        let tokens = tokenize(r#""hello\nworld""#).unwrap();
        if let TokenKind::StringLit(ref s) = tokens[0].kind {
            assert!(s.contains('\n'));
        } else {
            panic!("expected string literal");
        }
    }

    #[test]
    fn test_raw_string() {
        let tokens = tokenize("`raw\\nstring`").unwrap();
        if let TokenKind::RawStringLit(ref s) = tokens[0].kind {
            assert_eq!(s, "raw\\nstring");
        } else {
            panic!("expected raw string literal");
        }
    }

    #[test]
    fn test_hex_literal() {
        let tokens = tokenize("0xFF").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::IntLit(255)));
    }
}
