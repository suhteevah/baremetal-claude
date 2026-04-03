//! Lua tokenizer: keywords, operators, numbers, strings, comments.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

/// Token types for Lua 5.4.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Integer(i64),
    Number(f64),
    Str(String),
    True,
    False,
    Nil,

    // Identifiers
    Ident(String),

    // Keywords
    And,
    Break,
    Do,
    Else,
    ElseIf,
    End,
    For,
    Function,
    Goto,
    If,
    In,
    Local,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    Until,
    While,

    // Operators
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Percent,    // %
    Caret,      // ^
    Hash,       // #
    Ampersand,  // &
    Tilde,      // ~
    Pipe,       // |
    LessLess,   // <<
    GreatGreat, // >>
    SlashSlash, // //
    EqEq,       // ==
    TildeEq,    // ~=
    LessEq,     // <=
    GreatEq,    // >=
    Less,       // <
    Great,      // >
    Eq,         // =
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    DblColon,   // ::
    Semi,       // ;
    Colon,      // :
    Comma,      // ,
    Dot,        // .
    DotDot,     // ..
    DotDotDot,  // ...

    // End of input
    Eof,
}

impl Token {
    pub fn is_eof(&self) -> bool {
        matches!(self, Token::Eof)
    }
}

/// A token with source position info.
#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub token: Token,
    pub line: usize,
}

/// Tokenize Lua source code.
pub fn tokenize(source: &str) -> Result<Vec<SpannedToken>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut pos = 0;
    let mut line = 1;

    while pos < len {
        let ch = chars[pos];

        // Whitespace
        if ch.is_whitespace() {
            if ch == '\n' {
                line += 1;
            }
            pos += 1;
            continue;
        }

        // Comments
        if ch == '-' && pos + 1 < len && chars[pos + 1] == '-' {
            pos += 2;
            // Long comment --[[ ... ]]
            if pos + 1 < len && chars[pos] == '[' {
                let level = count_long_bracket(&chars, pos);
                if level >= 0 {
                    pos += (level as usize) + 2; // skip [=*[
                    let close = find_long_bracket_close(&chars, pos, level);
                    match close {
                        Some(end) => {
                            // Count newlines in the comment
                            for i in pos..end {
                                if chars[i] == '\n' { line += 1; }
                            }
                            pos = end + (level as usize) + 2; // skip ]=*]
                            continue;
                        }
                        None => {
                            return Err(format!("line {}: unterminated long comment", line));
                        }
                    }
                }
            }
            // Short comment
            while pos < len && chars[pos] != '\n' {
                pos += 1;
            }
            continue;
        }

        // Long strings
        if ch == '[' {
            let level = count_long_bracket(&chars, pos);
            if level >= 0 {
                pos += (level as usize) + 2;
                let close = find_long_bracket_close(&chars, pos, level);
                match close {
                    Some(end) => {
                        let mut s = String::new();
                        let start = pos;
                        // Skip leading newline
                        let start = if start < end && chars[start] == '\n' {
                            line += 1;
                            start + 1
                        } else {
                            start
                        };
                        for i in start..end {
                            if chars[i] == '\n' { line += 1; }
                            s.push(chars[i]);
                        }
                        pos = end + (level as usize) + 2;
                        tokens.push(SpannedToken { token: Token::Str(s), line });
                        continue;
                    }
                    None => {
                        return Err(format!("line {}: unterminated long string", line));
                    }
                }
            }
        }

        // Numbers
        if ch.is_ascii_digit() || (ch == '.' && pos + 1 < len && chars[pos + 1].is_ascii_digit()) {
            let (tok, new_pos) = lex_number(&chars, pos, line)?;
            tokens.push(SpannedToken { token: tok, line });
            pos = new_pos;
            continue;
        }

        // Strings
        if ch == '"' || ch == '\'' {
            let (s, new_pos, new_line) = lex_string(&chars, pos, line)?;
            tokens.push(SpannedToken { token: Token::Str(s), line });
            pos = new_pos;
            line = new_line;
            continue;
        }

        // Identifiers and keywords
        if ch.is_alphabetic() || ch == '_' {
            let start = pos;
            while pos < len && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                pos += 1;
            }
            let word: String = chars[start..pos].iter().collect();
            let tok = match word.as_str() {
                "and" => Token::And,
                "break" => Token::Break,
                "do" => Token::Do,
                "else" => Token::Else,
                "elseif" => Token::ElseIf,
                "end" => Token::End,
                "false" => Token::False,
                "for" => Token::For,
                "function" => Token::Function,
                "goto" => Token::Goto,
                "if" => Token::If,
                "in" => Token::In,
                "local" => Token::Local,
                "nil" => Token::Nil,
                "not" => Token::Not,
                "or" => Token::Or,
                "repeat" => Token::Repeat,
                "return" => Token::Return,
                "then" => Token::Then,
                "true" => Token::True,
                "until" => Token::Until,
                "while" => Token::While,
                _ => Token::Ident(word),
            };
            tokens.push(SpannedToken { token: tok, line });
            continue;
        }

        // Multi-char operators
        let tok = match ch {
            '=' => {
                if pos + 1 < len && chars[pos + 1] == '=' {
                    pos += 2;
                    Token::EqEq
                } else {
                    pos += 1;
                    Token::Eq
                }
            }
            '~' => {
                if pos + 1 < len && chars[pos + 1] == '=' {
                    pos += 2;
                    Token::TildeEq
                } else {
                    pos += 1;
                    Token::Tilde
                }
            }
            '<' => {
                if pos + 1 < len && chars[pos + 1] == '=' {
                    pos += 2;
                    Token::LessEq
                } else if pos + 1 < len && chars[pos + 1] == '<' {
                    pos += 2;
                    Token::LessLess
                } else {
                    pos += 1;
                    Token::Less
                }
            }
            '>' => {
                if pos + 1 < len && chars[pos + 1] == '=' {
                    pos += 2;
                    Token::GreatEq
                } else if pos + 1 < len && chars[pos + 1] == '>' {
                    pos += 2;
                    Token::GreatGreat
                } else {
                    pos += 1;
                    Token::Great
                }
            }
            ':' => {
                if pos + 1 < len && chars[pos + 1] == ':' {
                    pos += 2;
                    Token::DblColon
                } else {
                    pos += 1;
                    Token::Colon
                }
            }
            '/' => {
                if pos + 1 < len && chars[pos + 1] == '/' {
                    pos += 2;
                    Token::SlashSlash
                } else {
                    pos += 1;
                    Token::Slash
                }
            }
            '.' => {
                if pos + 1 < len && chars[pos + 1] == '.' {
                    if pos + 2 < len && chars[pos + 2] == '.' {
                        pos += 3;
                        Token::DotDotDot
                    } else {
                        pos += 2;
                        Token::DotDot
                    }
                } else {
                    pos += 1;
                    Token::Dot
                }
            }
            '+' => { pos += 1; Token::Plus }
            '-' => { pos += 1; Token::Minus }
            '*' => { pos += 1; Token::Star }
            '%' => { pos += 1; Token::Percent }
            '^' => { pos += 1; Token::Caret }
            '#' => { pos += 1; Token::Hash }
            '&' => { pos += 1; Token::Ampersand }
            '|' => { pos += 1; Token::Pipe }
            '(' => { pos += 1; Token::LParen }
            ')' => { pos += 1; Token::RParen }
            '{' => { pos += 1; Token::LBrace }
            '}' => { pos += 1; Token::RBrace }
            '[' => { pos += 1; Token::LBracket }
            ']' => { pos += 1; Token::RBracket }
            ';' => { pos += 1; Token::Semi }
            ',' => { pos += 1; Token::Comma }
            _ => {
                return Err(format!("line {}: unexpected character '{}'", line, ch));
            }
        };

        tokens.push(SpannedToken { token: tok, line });
    }

    tokens.push(SpannedToken { token: Token::Eof, line });
    Ok(tokens)
}

fn lex_number(chars: &[char], mut pos: usize, line: usize) -> Result<(Token, usize), String> {
    let start = pos;
    let len = chars.len();
    let mut is_float = false;

    // Hex
    if pos + 1 < len && chars[pos] == '0' && (chars[pos + 1] == 'x' || chars[pos + 1] == 'X') {
        pos += 2;
        let hex_start = pos;
        while pos < len && chars[pos].is_ascii_hexdigit() {
            pos += 1;
        }
        if pos == hex_start {
            return Err(format!("line {}: invalid hex literal", line));
        }
        let s: String = chars[hex_start..pos].iter().collect();
        let val = i64::from_str_radix(&s, 16)
            .map_err(|_| format!("line {}: invalid hex number", line))?;
        return Ok((Token::Integer(val), pos));
    }

    // Decimal
    while pos < len && chars[pos].is_ascii_digit() {
        pos += 1;
    }

    if pos < len && chars[pos] == '.' && (pos + 1 >= len || chars[pos + 1] != '.') {
        is_float = true;
        pos += 1;
        while pos < len && chars[pos].is_ascii_digit() {
            pos += 1;
        }
    }

    // Exponent
    if pos < len && (chars[pos] == 'e' || chars[pos] == 'E') {
        is_float = true;
        pos += 1;
        if pos < len && (chars[pos] == '+' || chars[pos] == '-') {
            pos += 1;
        }
        while pos < len && chars[pos].is_ascii_digit() {
            pos += 1;
        }
    }

    let s: String = chars[start..pos].iter().collect();
    if is_float {
        let val = parse_f64(&s)
            .ok_or_else(|| format!("line {}: invalid number '{}'", line, s))?;
        Ok((Token::Number(val), pos))
    } else {
        match parse_i64(&s) {
            Some(val) => Ok((Token::Integer(val), pos)),
            None => {
                let val = parse_f64(&s)
                    .ok_or_else(|| format!("line {}: invalid number '{}'", line, s))?;
                Ok((Token::Number(val), pos))
            }
        }
    }
}

fn lex_string(
    chars: &[char],
    mut pos: usize,
    mut line: usize,
) -> Result<(String, usize, usize), String> {
    let quote = chars[pos];
    pos += 1;
    let mut s = String::new();
    let len = chars.len();

    while pos < len && chars[pos] != quote {
        if chars[pos] == '\n' {
            return Err(format!("line {}: unterminated string", line));
        }
        if chars[pos] == '\\' {
            pos += 1;
            if pos >= len {
                return Err(format!("line {}: unterminated escape", line));
            }
            match chars[pos] {
                'a' => { s.push('\x07'); pos += 1; }
                'b' => { s.push('\x08'); pos += 1; }
                'f' => { s.push('\x0C'); pos += 1; }
                'n' => { s.push('\n'); pos += 1; }
                'r' => { s.push('\r'); pos += 1; }
                't' => { s.push('\t'); pos += 1; }
                'v' => { s.push('\x0B'); pos += 1; }
                '\\' => { s.push('\\'); pos += 1; }
                '\'' => { s.push('\''); pos += 1; }
                '"' => { s.push('"'); pos += 1; }
                '\n' => { s.push('\n'); pos += 1; line += 1; }
                'x' => {
                    pos += 1;
                    let mut hex = String::new();
                    for _ in 0..2 {
                        if pos < len && chars[pos].is_ascii_hexdigit() {
                            hex.push(chars[pos]);
                            pos += 1;
                        }
                    }
                    let code = u8::from_str_radix(&hex, 16)
                        .map_err(|_| format!("line {}: invalid hex escape", line))?;
                    s.push(code as char);
                }
                c if c.is_ascii_digit() => {
                    let mut num = String::new();
                    for _ in 0..3 {
                        if pos < len && chars[pos].is_ascii_digit() {
                            num.push(chars[pos]);
                            pos += 1;
                        } else {
                            break;
                        }
                    }
                    let code: u32 = parse_u32(&num)
                        .ok_or_else(|| format!("line {}: invalid decimal escape", line))?;
                    if code > 255 {
                        return Err(format!("line {}: escape value too large", line));
                    }
                    s.push(code as u8 as char);
                }
                c => {
                    s.push(c);
                    pos += 1;
                }
            }
        } else {
            s.push(chars[pos]);
            pos += 1;
        }
    }

    if pos >= len {
        return Err(format!("line {}: unterminated string", line));
    }
    pos += 1; // skip closing quote

    Ok((s, pos, line))
}

fn count_long_bracket(chars: &[char], pos: usize) -> i32 {
    if pos >= chars.len() || chars[pos] != '[' { return -1; }
    let mut level = 0i32;
    let mut p = pos + 1;
    while p < chars.len() && chars[p] == '=' {
        level += 1;
        p += 1;
    }
    if p < chars.len() && chars[p] == '[' {
        level
    } else {
        -1
    }
}

fn find_long_bracket_close(chars: &[char], start: usize, level: i32) -> Option<usize> {
    let mut pos = start;
    let len = chars.len();
    while pos < len {
        if chars[pos] == ']' {
            let mut count = 0i32;
            let mut p = pos + 1;
            while p < len && chars[p] == '=' {
                count += 1;
                p += 1;
            }
            if count == level && p < len && chars[p] == ']' {
                return Some(pos);
            }
        }
        pos += 1;
    }
    None
}

// Manual number parsing for no_std
fn parse_i64(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return None; }
    let (negative, start) = if bytes[0] == b'-' { (true, 1) } else { (false, 0) };
    let mut result: i64 = 0;
    for &b in &bytes[start..] {
        if !b.is_ascii_digit() { return None; }
        result = result.checked_mul(10)?.checked_add((b - b'0') as i64)?;
    }
    Some(if negative { -result } else { result })
}

fn parse_u32(s: &str) -> Option<u32> {
    let mut result: u32 = 0;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() { return None; }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(result)
}

fn parse_f64(s: &str) -> Option<f64> {
    // Simple float parser for no_std
    let bytes = s.as_bytes();
    if bytes.is_empty() { return None; }

    let (negative, mut pos) = if bytes[0] == b'-' { (true, 1) } else { (false, 0) };

    let mut integer_part: f64 = 0.0;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        integer_part = integer_part * 10.0 + (bytes[pos] - b'0') as f64;
        pos += 1;
    }

    let mut frac_part: f64 = 0.0;
    if pos < bytes.len() && bytes[pos] == b'.' {
        pos += 1;
        let mut divisor = 10.0;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            frac_part += (bytes[pos] - b'0') as f64 / divisor;
            divisor *= 10.0;
            pos += 1;
        }
    }

    let mut result = integer_part + frac_part;

    // Exponent
    if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
        pos += 1;
        let (exp_neg, mut exp_pos) = if pos < bytes.len() && bytes[pos] == b'-' {
            (true, pos + 1)
        } else if pos < bytes.len() && bytes[pos] == b'+' {
            (false, pos + 1)
        } else {
            (false, pos)
        };
        let mut exp: i32 = 0;
        while exp_pos < bytes.len() && bytes[exp_pos].is_ascii_digit() {
            exp = exp * 10 + (bytes[exp_pos] - b'0') as i32;
            exp_pos += 1;
        }
        if exp_neg { exp = -exp; }
        // Apply exponent
        let mut factor = 1.0f64;
        let abs_exp = if exp < 0 { -exp } else { exp } as u32;
        for _ in 0..abs_exp {
            factor *= 10.0;
        }
        if exp < 0 {
            result /= factor;
        } else {
            result *= factor;
        }
    }

    if negative { result = -result; }
    Some(result)
}
