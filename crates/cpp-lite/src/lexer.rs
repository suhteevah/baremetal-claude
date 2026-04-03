//! C++ lexer extensions over C tokens.
//!
//! Adds: class, public, private, protected, virtual, override, final,
//! template, typename, namespace, using, new, delete, throw, try, catch,
//! nullptr, auto, constexpr, static_assert, operator, bool, true, false,
//! this, ::, ->, <<, >>

use alloc::string::String;
use alloc::vec::Vec;

/// C++ token kind (extends C tokens).
#[derive(Debug, Clone, PartialEq)]
pub enum CppTokenKind {
    // C++ keywords
    Class,
    Public,
    Private,
    Protected,
    Virtual,
    Override,
    Final,
    Template,
    Typename,
    Namespace,
    Using,
    New,
    Delete,
    Throw,
    Try,
    Catch,
    Nullptr,
    Auto,
    Constexpr,
    StaticAssert,
    Operator,
    Bool,
    True,
    False,
    This,
    Friend,
    Mutable,
    Explicit,
    Noexcept,

    // C++ operators
    ScopeRes,       // ::
    ArrowStar,      // ->*
    DotStar,        // .*

    // Standard C keywords/operators (delegate to cc-lite's lexer in practice)
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    CharLit(u8),
    StringLit(Vec<u8>),

    // Reuse C operators
    Plus, Minus, Star, Slash, Percent,
    Amp, Pipe, Caret, Tilde, Bang,
    Assign, Lt, Gt, Question, Dot,
    Arrow,
    PlusPlus, MinusMinus,
    Shl, Shr,
    Le, Ge, EqEq, Ne,
    AmpAmp, PipePipe,
    PlusAssign, MinusAssign, StarAssign, SlashAssign, PercentAssign,
    AmpAssign, PipeAssign, CaretAssign, ShlAssign, ShrAssign,
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Semicolon, Colon, Comma, Ellipsis,

    Eof,
}

/// Source location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A positioned C++ token.
#[derive(Debug, Clone)]
pub struct CppToken {
    pub kind: CppTokenKind,
    pub span: Span,
}

/// Tokenize C++ source code.
pub fn tokenize_cpp(source: &str) -> Result<Vec<CppToken>, String> {
    let mut lexer = CppLexer::new(source);
    let mut tokens = Vec::new();

    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == CppTokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }

    Ok(tokens)
}

struct CppLexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> CppLexer<'a> {
    fn new(source: &'a str) -> Self {
        Self { source: source.as_bytes(), pos: 0, line: 1, col: 1 }
    }

    fn span(&self) -> Span { Span { line: self.line, col: self.col } }

    fn peek(&self) -> Option<u8> { self.source.get(self.pos).copied() }
    fn peek_ahead(&self, n: usize) -> Option<u8> { self.source.get(self.pos + n).copied() }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' { self.line += 1; self.col = 1; } else { self.col += 1; }
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        loop {
            while let Some(ch) = self.peek() {
                if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                    self.advance();
                } else {
                    break;
                }
            }
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'/') {
                while let Some(ch) = self.advance() { if ch == b'\n' { break; } }
                continue;
            }
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'*') {
                self.advance(); self.advance();
                loop {
                    match self.advance() {
                        Some(b'*') if self.peek() == Some(b'/') => { self.advance(); break; }
                        None => break,
                        _ => {}
                    }
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Result<CppToken, String> {
        self.skip_whitespace();
        let span = self.span();

        let ch = match self.peek() {
            Some(ch) => ch,
            None => return Ok(CppToken { kind: CppTokenKind::Eof, span }),
        };

        // Numbers
        if ch.is_ascii_digit() {
            return self.lex_number(span);
        }

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return self.lex_ident(span);
        }

        // Strings
        if ch == b'"' {
            return self.lex_string(span);
        }

        // Chars
        if ch == b'\'' {
            return self.lex_char(span);
        }

        // Operators
        self.lex_operator(span)
    }

    fn lex_number(&mut self, span: Span) -> Result<CppToken, String> {
        let start = self.pos;
        let mut is_float = false;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == b'_' { self.advance(); }
            else { break; }
        }
        if self.peek() == Some(b'.') && self.peek_ahead(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.advance();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() { self.advance(); } else { break; }
            }
        }

        let s: String = self.source[start..self.pos].iter()
            .filter(|&&c| c != b'_').map(|&c| c as char).collect();

        if is_float {
            let val: f64 = s.parse().map_err(|e| alloc::format!("bad float: {}", e))?;
            Ok(CppToken { kind: CppTokenKind::FloatLit(val), span })
        } else {
            let val: i64 = s.parse().map_err(|e| alloc::format!("bad int: {}", e))?;
            Ok(CppToken { kind: CppTokenKind::IntLit(val), span })
        }
    }

    fn lex_ident(&mut self, span: Span) -> Result<CppToken, String> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' { self.advance(); } else { break; }
        }
        let word: String = self.source[start..self.pos].iter().map(|&c| c as char).collect();

        let kind = match word.as_str() {
            "class" => CppTokenKind::Class,
            "public" => CppTokenKind::Public,
            "private" => CppTokenKind::Private,
            "protected" => CppTokenKind::Protected,
            "virtual" => CppTokenKind::Virtual,
            "override" => CppTokenKind::Override,
            "final" => CppTokenKind::Final,
            "template" => CppTokenKind::Template,
            "typename" => CppTokenKind::Typename,
            "namespace" => CppTokenKind::Namespace,
            "using" => CppTokenKind::Using,
            "new" => CppTokenKind::New,
            "delete" => CppTokenKind::Delete,
            "throw" => CppTokenKind::Throw,
            "try" => CppTokenKind::Try,
            "catch" => CppTokenKind::Catch,
            "nullptr" => CppTokenKind::Nullptr,
            "auto" => CppTokenKind::Auto,
            "constexpr" => CppTokenKind::Constexpr,
            "static_assert" => CppTokenKind::StaticAssert,
            "operator" => CppTokenKind::Operator,
            "bool" => CppTokenKind::Bool,
            "true" => CppTokenKind::True,
            "false" => CppTokenKind::False,
            "this" => CppTokenKind::This,
            "friend" => CppTokenKind::Friend,
            "mutable" => CppTokenKind::Mutable,
            "explicit" => CppTokenKind::Explicit,
            "noexcept" => CppTokenKind::Noexcept,
            _ => CppTokenKind::Ident(word),
        };
        Ok(CppToken { kind, span })
    }

    fn lex_string(&mut self, span: Span) -> Result<CppToken, String> {
        self.advance();
        let mut s = Vec::new();
        loop {
            match self.advance() {
                Some(b'"') => break,
                Some(b'\\') => match self.advance() {
                    Some(b'n') => s.push(b'\n'),
                    Some(b't') => s.push(b'\t'),
                    Some(b'\\') => s.push(b'\\'),
                    Some(b'"') => s.push(b'"'),
                    Some(b'0') => s.push(0),
                    Some(ch) => { s.push(b'\\'); s.push(ch); }
                    None => return Err(String::from("unterminated string")),
                },
                Some(ch) => s.push(ch),
                None => return Err(String::from("unterminated string")),
            }
        }
        Ok(CppToken { kind: CppTokenKind::StringLit(s), span })
    }

    fn lex_char(&mut self, span: Span) -> Result<CppToken, String> {
        self.advance();
        let ch = match self.advance() {
            Some(b'\\') => match self.advance() {
                Some(b'n') => b'\n',
                Some(b't') => b'\t',
                Some(b'\\') => b'\\',
                Some(b'\'') => b'\'',
                Some(b'0') => 0,
                Some(ch) => ch,
                None => return Err(String::from("unterminated char")),
            },
            Some(ch) => ch,
            None => return Err(String::from("unterminated char")),
        };
        if self.advance() != Some(b'\'') {
            return Err(String::from("unterminated char literal"));
        }
        Ok(CppToken { kind: CppTokenKind::CharLit(ch), span })
    }

    fn lex_operator(&mut self, span: Span) -> Result<CppToken, String> {
        let ch = self.advance().unwrap();
        let kind = match ch {
            b':' => {
                if self.peek() == Some(b':') { self.advance(); CppTokenKind::ScopeRes }
                else { CppTokenKind::Colon }
            }
            b'+' => match self.peek() {
                Some(b'+') => { self.advance(); CppTokenKind::PlusPlus }
                Some(b'=') => { self.advance(); CppTokenKind::PlusAssign }
                _ => CppTokenKind::Plus,
            },
            b'-' => match self.peek() {
                Some(b'-') => { self.advance(); CppTokenKind::MinusMinus }
                Some(b'=') => { self.advance(); CppTokenKind::MinusAssign }
                Some(b'>') => {
                    self.advance();
                    if self.peek() == Some(b'*') { self.advance(); CppTokenKind::ArrowStar }
                    else { CppTokenKind::Arrow }
                }
                _ => CppTokenKind::Minus,
            },
            b'*' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::StarAssign }
                _ => CppTokenKind::Star,
            },
            b'/' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::SlashAssign }
                _ => CppTokenKind::Slash,
            },
            b'%' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::PercentAssign }
                _ => CppTokenKind::Percent,
            },
            b'&' => match self.peek() {
                Some(b'&') => { self.advance(); CppTokenKind::AmpAmp }
                Some(b'=') => { self.advance(); CppTokenKind::AmpAssign }
                _ => CppTokenKind::Amp,
            },
            b'|' => match self.peek() {
                Some(b'|') => { self.advance(); CppTokenKind::PipePipe }
                Some(b'=') => { self.advance(); CppTokenKind::PipeAssign }
                _ => CppTokenKind::Pipe,
            },
            b'^' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::CaretAssign }
                _ => CppTokenKind::Caret,
            },
            b'<' => match self.peek() {
                Some(b'<') => { self.advance(); if self.peek() == Some(b'=') { self.advance(); CppTokenKind::ShlAssign } else { CppTokenKind::Shl } }
                Some(b'=') => { self.advance(); CppTokenKind::Le }
                _ => CppTokenKind::Lt,
            },
            b'>' => match self.peek() {
                Some(b'>') => { self.advance(); if self.peek() == Some(b'=') { self.advance(); CppTokenKind::ShrAssign } else { CppTokenKind::Shr } }
                Some(b'=') => { self.advance(); CppTokenKind::Ge }
                _ => CppTokenKind::Gt,
            },
            b'=' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::EqEq }
                _ => CppTokenKind::Assign,
            },
            b'!' => match self.peek() {
                Some(b'=') => { self.advance(); CppTokenKind::Ne }
                _ => CppTokenKind::Bang,
            },
            b'~' => CppTokenKind::Tilde,
            b'?' => CppTokenKind::Question,
            b'.' => {
                if self.peek() == Some(b'.') && self.peek_ahead(1) == Some(b'.') {
                    self.advance(); self.advance(); CppTokenKind::Ellipsis
                } else if self.peek() == Some(b'*') {
                    self.advance(); CppTokenKind::DotStar
                } else {
                    CppTokenKind::Dot
                }
            }
            b'(' => CppTokenKind::LParen,
            b')' => CppTokenKind::RParen,
            b'{' => CppTokenKind::LBrace,
            b'}' => CppTokenKind::RBrace,
            b'[' => CppTokenKind::LBracket,
            b']' => CppTokenKind::RBracket,
            b';' => CppTokenKind::Semicolon,
            b',' => CppTokenKind::Comma,
            _ => return Err(alloc::format!("{}:{}: unexpected char '{}'", span.line, span.col, ch as char)),
        };
        Ok(CppToken { kind, span })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpp_keywords() {
        let tokens = tokenize_cpp("class Foo : public Bar").unwrap();
        assert!(matches!(tokens[0].kind, CppTokenKind::Class));
        assert!(matches!(tokens[2].kind, CppTokenKind::Colon));
        assert!(matches!(tokens[3].kind, CppTokenKind::Public));
    }

    #[test]
    fn test_scope_resolution() {
        let tokens = tokenize_cpp("std::cout").unwrap();
        assert!(matches!(tokens[0].kind, CppTokenKind::Ident(ref s) if s == "std"));
        assert!(matches!(tokens[1].kind, CppTokenKind::ScopeRes));
    }

    #[test]
    fn test_template() {
        let tokens = tokenize_cpp("template<typename T>").unwrap();
        assert!(matches!(tokens[0].kind, CppTokenKind::Template));
        assert!(matches!(tokens[2].kind, CppTokenKind::Typename));
    }

    #[test]
    fn test_nullptr() {
        let tokens = tokenize_cpp("nullptr").unwrap();
        assert!(matches!(tokens[0].kind, CppTokenKind::Nullptr));
    }
}
