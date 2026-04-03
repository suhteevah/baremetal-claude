//! C tokenizer (lexer) for cc-lite.
//!
//! Converts C source code into a stream of [`Token`]s following C11 lexical rules.
//! The tokenizer operates as a single-pass, byte-oriented scanner that tracks
//! line/column for error reporting. It handles:
//!
//! - **Keywords**: All C11 keywords (`int`, `if`, `return`, `struct`, etc.)
//! - **Identifiers**: `[a-zA-Z_][a-zA-Z0-9_]*`
//! - **Numeric literals**: Decimal, hex (`0x`), octal (`0`-prefix), binary (`0b`),
//!   and floating-point with optional exponent and suffix
//! - **String literals**: Double-quoted with full escape sequence support
//!   (`\n`, `\t`, `\xHH`, `\\`, `\"`, etc.)
//! - **Character literals**: Single-quoted with escape support
//! - **Operators**: All C operators including compound assignment (`+=`, `<<=`)
//!   and multi-character tokens (`->`, `++`, `&&`, `||`)
//! - **Preprocessor directives**: `#include`, `#define`, `#ifdef`, etc.
//!   (simplified: captures the directive and its argument as a single token)
//! - **Comments**: Both line (`//`) and block (`/* */`) comments are stripped
//!
//! The tokenizer uses a maximal munch strategy: at each position it tries to
//! match the longest possible token. For multi-character operators like `<<=`,
//! it peeks ahead to distinguish `<` from `<<` from `<<=`.

use alloc::string::String;
use alloc::vec::Vec;

/// Source location for error reporting.
///
/// Tracks the 1-based line and column where a token begins in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
}

/// A C token with its source location.
///
/// Each token carries both its [`TokenKind`] (what it is) and its [`Span`]
/// (where it appeared in the source).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// All possible C token types produced by the lexer.
///
/// Organized into categories:
/// - **Keywords**: Reserved words in C11 (`int`, `if`, `struct`, ...)
/// - **Identifiers & Literals**: Names, numbers, strings, characters
/// - **Operators**: Arithmetic, bitwise, logical, comparison, assignment
/// - **Delimiters**: Parentheses, braces, brackets, semicolons, commas
/// - **Preprocessor**: `#include`, `#define`, conditional compilation
/// - **Special**: Built-in macros (`__FILE__`, `__LINE__`, `__func__`)
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // === Keywords ===
    Auto, Break, Case, Char, Const, Continue, Default, Do, Double, Else,
    Enum, Extern, Float, For, Goto, If, Inline, Int, Long, Register,
    Return, Short, Signed, Sizeof, Static, Struct, Switch, Typedef,
    Union, Unsigned, Void, Volatile, While, Bool,

    // === Identifiers & Literals ===
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    CharLit(u8),
    StringLit(Vec<u8>),

    // === Operators ===
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    Percent,      // %
    Amp,          // &
    Pipe,         // |
    Caret,        // ^
    Tilde,        // ~
    Bang,         // !
    Assign,       // =
    Lt,           // <
    Gt,           // >
    Question,     // ?
    Dot,          // .
    Arrow,        // ->
    PlusPlus,     // ++
    MinusMinus,   // --
    Shl,          // <<
    Shr,          // >>
    Le,           // <=
    Ge,           // >=
    EqEq,         // ==
    Ne,           // !=
    AmpAmp,       // &&
    PipePipe,     // ||
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign,// %=
    AmpAssign,    // &=
    PipeAssign,   // |=
    CaretAssign,  // ^=
    ShlAssign,    // <<=
    ShrAssign,    // >>=

    // === Delimiters ===
    LParen,       // (
    RParen,       // )
    LBrace,       // {
    RBrace,       // }
    LBracket,     // [
    RBracket,     // ]
    Semicolon,    // ;
    Comma,        // ,
    Colon,        // :
    Ellipsis,     // ...
    Hash,         // #

    // === Preprocessor (simplified) ===
    PpInclude(String),
    PpDefine(String, String),
    PpIfdef(String),
    PpIfndef(String),
    PpEndif,
    PpIf,
    PpElif,
    PpElse,
    PpPragma(String),
    PpError(String),

    // === Special ===
    MacroFile,    // __FILE__
    MacroLine,    // __LINE__
    MacroFunc,    // __func__

    Eof,
}

/// Tokenize C source code into a vector of tokens.
///
/// Scans the input byte-by-byte, maintaining a cursor position (`i`), current
/// line number, and column number. At each iteration of the main loop, the
/// scanner inspects the current byte and dispatches to the appropriate handler:
///
/// 1. **Whitespace / newlines**: Consumed silently, updating line/col.
/// 2. **Comments**: `//` skips to end-of-line; `/* */` skips to closing `*/`.
/// 3. **Preprocessor**: `#` reads the directive name and rest-of-line argument.
/// 4. **String/char literals**: Scans until closing quote, interpreting escapes.
/// 5. **Numbers**: Delegates to [`lex_number`] for integer/float parsing.
/// 6. **Identifiers/keywords**: Scans `[a-zA-Z_][a-zA-Z0-9_]*`, then checks
///    against the keyword table. Unknown words become `Ident` tokens.
/// 7. **Operators**: Uses lookahead to resolve multi-character operators.
///
/// Returns `Err` if an unexpected byte is encountered.
pub fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;       // byte cursor into the source
    let mut line: u32 = 1;
    let mut col: u32 = 1;

    while i < bytes.len() {
        let start_line = line;
        let start_col = col;

        match bytes[i] {
            // Whitespace: spaces and tabs advance the column
            b' ' | b'\t' => {
                col += 1;
                i += 1;
            }
            b'\r' => {
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                line += 1;
                col = 1;
            }
            b'\n' => {
                i += 1;
                line += 1;
                col = 1;
            }
            // Line comment
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                col += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        col += 2;
                        break;
                    }
                    if bytes[i] == b'\n' {
                        line += 1;
                        col = 1;
                    } else {
                        col += 1;
                    }
                    i += 1;
                }
            }
            // Preprocessor directive
            b'#' => {
                let span = Span { line: start_line, col: start_col };
                i += 1;
                col += 1;
                // Skip whitespace
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                    col += 1;
                }
                // Read directive name
                let dir_start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                    col += 1;
                }
                let directive = core::str::from_utf8(&bytes[dir_start..i]).unwrap_or("");
                // Skip whitespace
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                    col += 1;
                }
                // Read rest of line
                let rest_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let rest = core::str::from_utf8(&bytes[rest_start..i]).unwrap_or("").trim_end();
                let rest_str = String::from(rest);

                match directive {
                    "include" => tokens.push(Token { kind: TokenKind::PpInclude(rest_str), span }),
                    "define" => {
                        let (name, val) = split_define(rest);
                        tokens.push(Token { kind: TokenKind::PpDefine(name, val), span });
                    }
                    "ifdef" => tokens.push(Token { kind: TokenKind::PpIfdef(rest_str), span }),
                    "ifndef" => tokens.push(Token { kind: TokenKind::PpIfndef(rest_str), span }),
                    "endif" => tokens.push(Token { kind: TokenKind::PpEndif, span }),
                    "if" => tokens.push(Token { kind: TokenKind::PpIf, span }),
                    "elif" => tokens.push(Token { kind: TokenKind::PpElif, span }),
                    "else" => tokens.push(Token { kind: TokenKind::PpElse, span }),
                    "pragma" => tokens.push(Token { kind: TokenKind::PpPragma(rest_str), span }),
                    "error" => tokens.push(Token { kind: TokenKind::PpError(rest_str), span }),
                    _ => {} // Ignore unknown preprocessor directives
                }
            }
            // String literal
            b'"' => {
                let span = Span { line: start_line, col: start_col };
                i += 1;
                col += 1;
                let mut bytes_vec = Vec::new();
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                        col += 1;
                        match bytes[i] {
                            b'n' => bytes_vec.push(b'\n'),
                            b'r' => bytes_vec.push(b'\r'),
                            b't' => bytes_vec.push(b'\t'),
                            b'0' => bytes_vec.push(0),
                            b'\\' => bytes_vec.push(b'\\'),
                            b'"' => bytes_vec.push(b'"'),
                            b'\'' => bytes_vec.push(b'\''),
                            b'a' => bytes_vec.push(0x07),
                            b'b' => bytes_vec.push(0x08),
                            b'f' => bytes_vec.push(0x0C),
                            b'v' => bytes_vec.push(0x0B),
                            b'x' => {
                                // Hex escape
                                i += 1;
                                let mut val = 0u8;
                                for _ in 0..2 {
                                    if i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                                        val = val * 16 + hex_digit(bytes[i]);
                                        i += 1;
                                        col += 1;
                                    }
                                }
                                bytes_vec.push(val);
                                continue;
                            }
                            other => bytes_vec.push(other),
                        }
                    } else {
                        bytes_vec.push(bytes[i]);
                    }
                    i += 1;
                    col += 1;
                }
                if i < bytes.len() {
                    i += 1;
                    col += 1;
                }
                tokens.push(Token { kind: TokenKind::StringLit(bytes_vec), span });
            }
            // Char literal
            b'\'' => {
                let span = Span { line: start_line, col: start_col };
                i += 1;
                col += 1;
                let ch = if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    col += 1;
                    match bytes[i] {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'0' => 0,
                        b'\\' => b'\\',
                        b'\'' => b'\'',
                        other => other,
                    }
                } else {
                    bytes[i]
                };
                i += 1;
                col += 1;
                if i < bytes.len() && bytes[i] == b'\'' {
                    i += 1;
                    col += 1;
                }
                tokens.push(Token { kind: TokenKind::CharLit(ch), span });
            }
            // Number
            b'0'..=b'9' => {
                let span = Span { line: start_line, col: start_col };
                let (tok, adv) = lex_number(&bytes[i..]);
                tokens.push(Token { kind: tok, span });
                col += adv as u32;
                i += adv;
            }
            // Identifier or keyword
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let span = Span { line: start_line, col: start_col };
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                    col += 1;
                }
                let word = core::str::from_utf8(&bytes[start..i]).unwrap();
                let kind = match word {
                    "auto" => TokenKind::Auto,
                    "break" => TokenKind::Break,
                    "case" => TokenKind::Case,
                    "char" => TokenKind::Char,
                    "const" => TokenKind::Const,
                    "continue" => TokenKind::Continue,
                    "default" => TokenKind::Default,
                    "do" => TokenKind::Do,
                    "double" => TokenKind::Double,
                    "else" => TokenKind::Else,
                    "enum" => TokenKind::Enum,
                    "extern" => TokenKind::Extern,
                    "float" => TokenKind::Float,
                    "for" => TokenKind::For,
                    "goto" => TokenKind::Goto,
                    "if" => TokenKind::If,
                    "inline" => TokenKind::Inline,
                    "int" => TokenKind::Int,
                    "long" => TokenKind::Long,
                    "register" => TokenKind::Register,
                    "return" => TokenKind::Return,
                    "short" => TokenKind::Short,
                    "signed" => TokenKind::Signed,
                    "sizeof" => TokenKind::Sizeof,
                    "static" => TokenKind::Static,
                    "struct" => TokenKind::Struct,
                    "switch" => TokenKind::Switch,
                    "typedef" => TokenKind::Typedef,
                    "union" => TokenKind::Union,
                    "unsigned" => TokenKind::Unsigned,
                    "void" => TokenKind::Void,
                    "volatile" => TokenKind::Volatile,
                    "while" => TokenKind::While,
                    "_Bool" => TokenKind::Bool,
                    "__FILE__" => TokenKind::MacroFile,
                    "__LINE__" => TokenKind::MacroLine,
                    "__func__" => TokenKind::MacroFunc,
                    _ => TokenKind::Ident(String::from(word)),
                };
                tokens.push(Token { kind, span });
            }
            // Operators and delimiters
            b'+' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'+' {
                    tokens.push(Token { kind: TokenKind::PlusPlus, span });
                    i += 1; col += 1;
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::PlusAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Plus, span });
                }
            }
            b'-' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'-' {
                    tokens.push(Token { kind: TokenKind::MinusMinus, span });
                    i += 1; col += 1;
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::MinusAssign, span });
                    i += 1; col += 1;
                } else if i < bytes.len() && bytes[i] == b'>' {
                    tokens.push(Token { kind: TokenKind::Arrow, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Minus, span });
                }
            }
            b'*' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::StarAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Star, span });
                }
            }
            b'/' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::SlashAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Slash, span });
                }
            }
            b'%' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::PercentAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Percent, span });
                }
            }
            b'&' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'&' {
                    tokens.push(Token { kind: TokenKind::AmpAmp, span });
                    i += 1; col += 1;
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::AmpAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Amp, span });
                }
            }
            b'|' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'|' {
                    tokens.push(Token { kind: TokenKind::PipePipe, span });
                    i += 1; col += 1;
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::PipeAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Pipe, span });
                }
            }
            b'^' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::CaretAssign, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Caret, span });
                }
            }
            b'~' => {
                tokens.push(Token { kind: TokenKind::Tilde, span: Span { line: start_line, col: start_col } });
                i += 1; col += 1;
            }
            b'!' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::Ne, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Bang, span });
                }
            }
            b'=' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::EqEq, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Assign, span });
                }
            }
            b'<' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'<' {
                    i += 1; col += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        tokens.push(Token { kind: TokenKind::ShlAssign, span });
                        i += 1; col += 1;
                    } else {
                        tokens.push(Token { kind: TokenKind::Shl, span });
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::Le, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Lt, span });
                }
            }
            b'>' => {
                let span = Span { line: start_line, col: start_col };
                i += 1; col += 1;
                if i < bytes.len() && bytes[i] == b'>' {
                    i += 1; col += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        tokens.push(Token { kind: TokenKind::ShrAssign, span });
                        i += 1; col += 1;
                    } else {
                        tokens.push(Token { kind: TokenKind::Shr, span });
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    tokens.push(Token { kind: TokenKind::Ge, span });
                    i += 1; col += 1;
                } else {
                    tokens.push(Token { kind: TokenKind::Gt, span });
                }
            }
            b'?' => {
                tokens.push(Token { kind: TokenKind::Question, span: Span { line: start_line, col: start_col } });
                i += 1; col += 1;
            }
            b'.' => {
                let span = Span { line: start_line, col: start_col };
                if i + 2 < bytes.len() && bytes[i + 1] == b'.' && bytes[i + 2] == b'.' {
                    tokens.push(Token { kind: TokenKind::Ellipsis, span });
                    i += 3; col += 3;
                } else {
                    tokens.push(Token { kind: TokenKind::Dot, span });
                    i += 1; col += 1;
                }
            }
            b'(' => { tokens.push(Token { kind: TokenKind::LParen, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b')' => { tokens.push(Token { kind: TokenKind::RParen, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b'{' => { tokens.push(Token { kind: TokenKind::LBrace, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b'}' => { tokens.push(Token { kind: TokenKind::RBrace, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b'[' => { tokens.push(Token { kind: TokenKind::LBracket, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b']' => { tokens.push(Token { kind: TokenKind::RBracket, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b';' => { tokens.push(Token { kind: TokenKind::Semicolon, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b',' => { tokens.push(Token { kind: TokenKind::Comma, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }
            b':' => { tokens.push(Token { kind: TokenKind::Colon, span: Span { line: start_line, col: start_col } }); i += 1; col += 1; }

            other => {
                return Err(alloc::format!(
                    "{}:{}: unexpected character '{}' (0x{:02x})",
                    line, col, other as char, other
                ));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span { line, col },
    });
    Ok(tokens)
}

/// Convert an ASCII hex digit to its numeric value (0-15).
///
/// Returns 0 for non-hex characters (caller ensures valid input).
fn hex_digit(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Lex a numeric literal starting at the given byte slice.
///
/// Returns `(token_kind, bytes_consumed)`. Handles all C numeric literal forms:
///
/// - **Hex**: `0x` or `0X` prefix, digits `[0-9a-fA-F]`, plus optional `U`/`L` suffix
/// - **Binary**: `0b` or `0B` prefix, digits `[01]` (GCC extension)
/// - **Octal**: Leading `0` followed by `[0-7]`
/// - **Decimal integer**: `[0-9]+` with optional `U`/`L`/`LL` suffix
/// - **Floating-point**: Digits with `.` and/or `[eE][+-]?[0-9]+`, optional `f`/`F`/`l`/`L` suffix
///
/// Integer values are accumulated using `wrapping_mul`/`wrapping_add` to handle
/// overflow gracefully (matching C's unsigned wrapping semantics).
fn lex_number(bytes: &[u8]) -> (TokenKind, usize) {
    let mut i = 0;
    let mut is_float = false;

    // Check for hex (0x), binary (0b), or octal (0nnn) prefix
    if bytes.len() >= 2 && bytes[0] == b'0' {
        match bytes[1] {
            b'x' | b'X' => {
                i = 2;
                let mut val: i64 = 0;
                while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    val = val.wrapping_mul(16).wrapping_add(hex_digit(bytes[i]) as i64);
                    i += 1;
                }
                // Skip suffixes (U, L, LL, etc.)
                while i < bytes.len() && matches!(bytes[i], b'u' | b'U' | b'l' | b'L') {
                    i += 1;
                }
                return (TokenKind::IntLit(val), i);
            }
            b'b' | b'B' => {
                i = 2;
                let mut val: i64 = 0;
                while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1') {
                    val = val * 2 + (bytes[i] - b'0') as i64;
                    i += 1;
                }
                return (TokenKind::IntLit(val), i);
            }
            b'0'..=b'7' => {
                // Octal
                i = 1;
                let mut val: i64 = 0;
                while i < bytes.len() && bytes[i] >= b'0' && bytes[i] <= b'7' {
                    val = val * 8 + (bytes[i] - b'0') as i64;
                    i += 1;
                }
                return (TokenKind::IntLit(val), i);
            }
            b'.' => {
                is_float = true;
            }
            _ => {}
        }
    }

    // Decimal integer or float
    if !is_float {
        i = 0;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        is_float = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        is_float = true;
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }

    let text = core::str::from_utf8(&bytes[..i]).unwrap_or("0");

    if is_float {
        // Simple float parse (no std)
        let val = parse_float(text);
        // Skip float suffix
        if i < bytes.len() && (bytes[i] == b'f' || bytes[i] == b'F' || bytes[i] == b'l' || bytes[i] == b'L') {
            i += 1;
        }
        (TokenKind::FloatLit(val), i)
    } else {
        let mut val: i64 = 0;
        for b in text.bytes() {
            if b.is_ascii_digit() {
                val = val.wrapping_mul(10).wrapping_add((b - b'0') as i64);
            }
        }
        // Skip integer suffixes
        while i < bytes.len() && matches!(bytes[i], b'u' | b'U' | b'l' | b'L') {
            i += 1;
        }
        (TokenKind::IntLit(val), i)
    }
}

/// Minimal floating-point parser for `no_std` environments.
///
/// Parses a decimal float string like `"3.14"`, `"1e10"`, or `"2.5E-3"`.
/// Processes three phases: integer part, fractional part, exponent.
/// Not IEEE-754 bit-exact for all inputs, but sufficient for C constant expressions.
fn parse_float(s: &str) -> f64 {
    // Hand-rolled parser since we have no std::str::parse
    let mut result: f64 = 0.0;
    let mut frac: f64 = 0.0;
    let mut frac_div: f64 = 1.0;
    let mut exp: i32 = 0;
    let mut exp_neg = false;
    let mut in_frac = false;
    let mut in_exp = false;

    for b in s.bytes() {
        if in_exp {
            if b == b'-' {
                exp_neg = true;
            } else if b == b'+' {
                // skip
            } else if b.is_ascii_digit() {
                exp = exp * 10 + (b - b'0') as i32;
            }
        } else if b == b'.' {
            in_frac = true;
        } else if b == b'e' || b == b'E' {
            in_exp = true;
        } else if b.is_ascii_digit() {
            if in_frac {
                frac_div *= 10.0;
                frac += (b - b'0') as f64 / frac_div;
            } else {
                result = result * 10.0 + (b - b'0') as f64;
            }
        }
    }

    result += frac;
    if exp_neg {
        exp = -exp;
    }
    // Apply exponent
    if exp > 0 {
        for _ in 0..exp {
            result *= 10.0;
        }
    } else if exp < 0 {
        for _ in 0..(-exp) {
            result /= 10.0;
        }
    }
    result
}

/// Split a `#define` directive's argument into (name, value).
///
/// Given `"FOO 42"`, returns `("FOO", "42")`.
/// Given `"BAR"` (no value), returns `("BAR", "")`.
fn split_define(rest: &str) -> (String, String) {
    let trimmed = rest.trim();
    if let Some(pos) = trimmed.find(|c: char| c == ' ' || c == '\t') {
        let name = String::from(&trimmed[..pos]);
        let val = String::from(trimmed[pos..].trim());
        (name, val)
    } else {
        (String::from(trimmed), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let tokens = tokenize("int main() { return 0; }").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int));
        assert!(matches!(tokens[1].kind, TokenKind::Ident(_)));
        assert!(matches!(tokens[2].kind, TokenKind::LParen));
    }

    #[test]
    fn test_operators() {
        let tokens = tokenize("a += b->c").unwrap();
        assert!(matches!(tokens[1].kind, TokenKind::PlusAssign));
        assert!(matches!(tokens[3].kind, TokenKind::Arrow));
    }

    #[test]
    fn test_string_literal() {
        let tokens = tokenize("\"hello\\n\"").unwrap();
        if let TokenKind::StringLit(ref bytes) = tokens[0].kind {
            assert_eq!(bytes, &[b'h', b'e', b'l', b'l', b'o', b'\n']);
        } else {
            panic!("expected string literal");
        }
    }

    #[test]
    fn test_hex_literal() {
        let tokens = tokenize("0xFF").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::IntLit(255));
    }

    #[test]
    fn test_preprocessor() {
        let tokens = tokenize("#include <stdio.h>\n").unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::PpInclude(_)));
    }
}
