//! Tokenizer for python-lite.
//!
//! Converts source text into a flat stream of tokens, with explicit INDENT /
//! DEDENT tokens derived from leading whitespace (Python-style significant
//! indentation).

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Int(i64),
    Float(f64),
    Str(String),
    FStr(String), // f-string literal (raw template)
    True,
    False,
    None,

    // Identifier
    Ident(String),

    // Keywords
    If,
    Elif,
    Else,
    For,
    While,
    In,
    Def,
    Return,
    And,
    Or,
    Not,
    Break,
    Continue,
    Class,
    Try,
    Except,
    Finally,
    Raise,
    Import,
    From,
    As,
    With,
    Lambda,
    Del,
    Global,
    Nonlocal,
    Yield,
    Pass,
    Is,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    DoubleSlash,
    Percent,
    DoubleStar,
    Eq,        // ==
    NotEq,     // !=
    Lt,
    Gt,
    LtEq,
    GtEq,
    Assign,    // =
    PlusEq,    // +=
    MinusEq,   // -=
    StarEq,    // *=
    SlashEq,   // /=
    PercentEq, // %=
    DoubleStarEq, // **=
    DoubleSlashEq, // //=
    Walrus,    // :=
    At,        // @
    Pipe,      // |
    Ampersand, // &
    Caret,     // ^
    Tilde,     // ~

    // Delimiters
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Semicolon,
    Arrow,     // ->

    // Structure
    Newline,
    Indent,
    Dedent,
    Eof,
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut indent_stack: Vec<usize> = Vec::new();
    indent_stack.push(0);

    let lines = split_logical_lines(source);

    for line in &lines {
        // Skip blank lines and comment-only lines.
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Compute indentation level (number of leading spaces).
        let indent = line.len() - line.trim_start().len();
        let current = *indent_stack.last().unwrap();

        if indent > current {
            indent_stack.push(indent);
            tokens.push(Token::Indent);
        } else {
            while indent < *indent_stack.last().unwrap() {
                indent_stack.pop();
                tokens.push(Token::Dedent);
            }
            if indent != *indent_stack.last().unwrap() {
                return Err(String::from("inconsistent indentation"));
            }
        }

        // Tokenize the content of this line.
        tokenize_line(trimmed, &mut tokens)?;
        tokens.push(Token::Newline);
    }

    // Emit remaining DEDENTs.
    while indent_stack.len() > 1 {
        indent_stack.pop();
        tokens.push(Token::Dedent);
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

fn split_logical_lines(source: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0i32;
    let mut in_string = false;
    let mut string_char = '"';

    for ch in source.chars() {
        if in_string {
            current.push(ch);
            if ch == string_char {
                in_string = false;
            } else if ch == '\\' {
                // skip next char in string
                // handled by pushing next char normally
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = true;
            string_char = ch;
            current.push(ch);
            continue;
        }
        if ch == '(' || ch == '[' || ch == '{' {
            paren_depth += 1;
        } else if ch == ')' || ch == ']' || ch == '}' {
            paren_depth -= 1;
            if paren_depth < 0 {
                paren_depth = 0;
            }
        }
        if ch == '\n' {
            if paren_depth > 0 {
                current.push(' ');
            } else {
                lines.push(core::mem::take(&mut current));
            }
        } else if ch == '\\' {
            // line continuation -- skip this char and the next newline
            // (handled implicitly: next iteration will see \n and paren_depth logic)
            // Actually we should just skip it
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn tokenize_line(line: &str, tokens: &mut Vec<Token>) -> Result<(), String> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Skip whitespace (already handled indentation).
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }

        // Comment — skip rest of line.
        if c == '#' {
            break;
        }

        // f-string literals
        if (c == 'f' || c == 'F') && i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\'') {
            let quote = chars[i + 1];
            i += 2; // skip f and opening quote
            let mut s = String::new();
            while i < chars.len() {
                let ch = chars[i];
                if ch == '\\' && i + 1 < chars.len() {
                    i += 1;
                    let esc = match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '\\' => '\\',
                        '\'' => '\'',
                        '"' => '"',
                        '{' => '{',
                        '}' => '}',
                        other => other,
                    };
                    s.push(esc);
                    i += 1;
                } else if ch == quote {
                    i += 1;
                    break;
                } else {
                    s.push(ch);
                    i += 1;
                }
            }
            tokens.push(Token::FStr(s));
            continue;
        }

        // String literals.
        if c == '"' || c == '\'' {
            let (s, end) = read_string(&chars, i)?;
            tokens.push(Token::Str(s));
            i = end;
            continue;
        }

        // Numbers.
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let (tok, end) = read_number(&chars, i)?;
            tokens.push(tok);
            i = end;
            continue;
        }

        // Identifiers and keywords.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let tok = match word.as_str() {
                "if" => Token::If,
                "elif" => Token::Elif,
                "else" => Token::Else,
                "for" => Token::For,
                "while" => Token::While,
                "in" => Token::In,
                "def" => Token::Def,
                "return" => Token::Return,
                "and" => Token::And,
                "or" => Token::Or,
                "not" => Token::Not,
                "True" => Token::True,
                "False" => Token::False,
                "None" => Token::None,
                "break" => Token::Break,
                "continue" => Token::Continue,
                "class" => Token::Class,
                "try" => Token::Try,
                "except" => Token::Except,
                "finally" => Token::Finally,
                "raise" => Token::Raise,
                "import" => Token::Import,
                "from" => Token::From,
                "as" => Token::As,
                "with" => Token::With,
                "lambda" => Token::Lambda,
                "del" => Token::Del,
                "global" => Token::Global,
                "nonlocal" => Token::Nonlocal,
                "yield" => Token::Yield,
                "pass" => Token::Pass,
                "is" => Token::Is,
                _ => Token::Ident(word),
            };
            tokens.push(tok);
            continue;
        }

        // Multi-character operators.
        let next = if i + 1 < chars.len() { Some(chars[i + 1]) } else { Option::None };
        let next2 = if i + 2 < chars.len() { Some(chars[i + 2]) } else { Option::None };

        match (c, next, next2) {
            ('*', Some('*'), Some('=')) => { tokens.push(Token::DoubleStarEq); i += 3; }
            ('/', Some('/'), Some('=')) => { tokens.push(Token::DoubleSlashEq); i += 3; }
            ('*', Some('*'), _) => { tokens.push(Token::DoubleStar); i += 2; }
            ('*', Some('='), _) => { tokens.push(Token::StarEq); i += 2; }
            ('/', Some('/'), _) => { tokens.push(Token::DoubleSlash); i += 2; }
            ('/', Some('='), _) => { tokens.push(Token::SlashEq); i += 2; }
            ('+', Some('='), _) => { tokens.push(Token::PlusEq); i += 2; }
            ('-', Some('='), _) => { tokens.push(Token::MinusEq); i += 2; }
            ('-', Some('>'), _) => { tokens.push(Token::Arrow); i += 2; }
            ('%', Some('='), _) => { tokens.push(Token::PercentEq); i += 2; }
            ('=', Some('='), _) => { tokens.push(Token::Eq); i += 2; }
            ('!', Some('='), _) => { tokens.push(Token::NotEq); i += 2; }
            ('<', Some('='), _) => { tokens.push(Token::LtEq); i += 2; }
            ('>', Some('='), _) => { tokens.push(Token::GtEq); i += 2; }
            (':', Some('='), _) => { tokens.push(Token::Walrus); i += 2; }
            _ => {
                // Single-character tokens.
                let tok = match c {
                    '+' => Token::Plus,
                    '-' => Token::Minus,
                    '*' => Token::Star,
                    '/' => Token::Slash,
                    '%' => Token::Percent,
                    '=' => Token::Assign,
                    '<' => Token::Lt,
                    '>' => Token::Gt,
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    '[' => Token::LBracket,
                    ']' => Token::RBracket,
                    '{' => Token::LBrace,
                    '}' => Token::RBrace,
                    ',' => Token::Comma,
                    ':' => Token::Colon,
                    '.' => Token::Dot,
                    ';' => Token::Semicolon,
                    '@' => Token::At,
                    '|' => Token::Pipe,
                    '&' => Token::Ampersand,
                    '^' => Token::Caret,
                    '~' => Token::Tilde,
                    _ => return Err(alloc::format!("unexpected character: '{}'", c)),
                };
                tokens.push(tok);
                i += 1;
            }
        }
    }

    Ok(())
}

fn read_string(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let quote = chars[start];
    let mut s = String::new();
    let mut i = start + 1;

    // Check for triple-quoted string
    if i + 1 < chars.len() && chars[i] == quote && chars[i + 1] == quote {
        // Triple-quoted string -- but since we join logical lines, this is limited.
        // Just consume the two extra quotes and read until triple-quote end.
        i += 2;
        while i + 2 < chars.len() {
            if chars[i] == quote && chars[i + 1] == quote && chars[i + 2] == quote {
                return Ok((s, i + 3));
            }
            if chars[i] == '\\' && i + 1 < chars.len() {
                i += 1;
                let esc = match chars[i] {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '\\' => '\\',
                    '\'' => '\'',
                    '"' => '"',
                    '0' => '\0',
                    other => other,
                };
                s.push(esc);
                i += 1;
            } else {
                s.push(chars[i]);
                i += 1;
            }
        }
        // Consume remaining chars
        while i < chars.len() {
            s.push(chars[i]);
            i += 1;
        }
        return Ok((s, i));
    }

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            i += 1;
            let esc = match chars[i] {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                '0' => '\0',
                other => other,
            };
            s.push(esc);
            i += 1;
        } else if c == quote {
            i += 1;
            return Ok((s, i));
        } else {
            s.push(c);
            i += 1;
        }
    }

    Err(String::from("unterminated string literal"))
}

fn read_number(chars: &[char], start: usize) -> Result<(Token, usize), String> {
    let mut i = start;
    let mut has_dot = false;

    // Handle hex
    if i + 1 < chars.len() && chars[i] == '0' && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
        i += 2;
        let hex_start = i;
        while i < chars.len() && chars[i].is_ascii_hexdigit() {
            i += 1;
        }
        let hex_str: String = chars[hex_start..i].iter().collect();
        let val = i64::from_str_radix(&hex_str, 16)
            .map_err(|_| alloc::format!("invalid hex: 0x{}", hex_str))?;
        return Ok((Token::Int(val), i));
    }

    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == 'e' || chars[i] == 'E') {
        if chars[i] == '.' {
            if has_dot {
                break;
            }
            // Check that next char is a digit or end (not an ident like .method)
            if i + 1 < chars.len() && !chars[i + 1].is_ascii_digit() && chars[i + 1] != 'e' && chars[i + 1] != 'E' {
                break;
            }
            has_dot = true;
        }
        if chars[i] == 'e' || chars[i] == 'E' {
            has_dot = true; // treat as float
            i += 1;
            if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    let num_str: String = chars[start..i].iter().collect();

    if has_dot {
        let val: f64 = num_str
            .parse()
            .map_err(|_| alloc::format!("invalid float: {}", num_str))?;
        Ok((Token::Float(val), i))
    } else {
        let val: i64 = num_str
            .parse()
            .map_err(|_| alloc::format!("invalid integer: {}", num_str))?;
        Ok((Token::Int(val), i))
    }
}
