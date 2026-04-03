//! TypeScript lexer extensions on top of JS tokens.
//!
//! Adds type annotations (: type), generics (<T>), interface, enum, as,
//! implements, readonly, access modifiers, abstract, declare, namespace,
//! keyof, typeof in type position, never, unknown, any.

use alloc::string::String;
use alloc::vec::Vec;

/// TypeScript-specific token (wraps or extends JS tokens).
#[derive(Debug, Clone, PartialEq)]
pub enum TsToken {
    // Re-exported JS tokens
    Js(js_lite::tokenizer::Token),

    // TypeScript-specific keywords
    Interface,
    Enum,
    As,
    Implements,
    Readonly,
    Private,
    Public,
    Protected,
    Abstract,
    Declare,
    Namespace,
    Module,
    Type,       // type keyword (for type aliases)
    Keyof,
    Infer,
    Is,         // type predicate: x is T
    Satisfies,
    Override,
    Accessor,

    // TypeScript type keywords
    Any,
    Unknown,
    Never,
    Void,       // already in JS but special in TS types

    // Operators specific to TS context
    QuestionDot,   // ?.
    NonNull,       // ! (postfix non-null assertion)
    Colon,         // : (type annotation)
    LAngle,        // < (generic open)
    RAngle,        // > (generic close)

    // Misc
    Ident(String),
    StringLit(String),
    NumberLit(f64),
    Eof,
}

/// Source location.
#[derive(Debug, Clone, Copy, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A positioned TS token.
#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub token: TsToken,
    pub span: Span,
}

/// Tokenize TypeScript source, producing a stream of TsTokens.
///
/// Strategy: first tokenize as JS, then post-process to identify TS-specific
/// tokens (interface, enum, type annotations, etc.).
pub fn tokenize_ts(source: &str) -> Result<Vec<SpannedToken>, String> {
    // Use the JS tokenizer as a base
    let js_tokens = js_lite::tokenizer::tokenize(source)?;

    let mut result = Vec::new();
    let mut line = 1u32;
    let mut col = 1u32;

    for tok in js_tokens {
        let span = Span { line, col };
        col += 1; // simplified tracking

        let ts_tok = match &tok {
            js_lite::tokenizer::Token::Ident(ref name) => {
                match name.as_str() {
                    "interface" => TsToken::Interface,
                    "enum" => TsToken::Enum,
                    "as" => TsToken::As,
                    "implements" => TsToken::Implements,
                    "readonly" => TsToken::Readonly,
                    "private" => TsToken::Private,
                    "public" => TsToken::Public,
                    "protected" => TsToken::Protected,
                    "abstract" => TsToken::Abstract,
                    "declare" => TsToken::Declare,
                    "namespace" => TsToken::Namespace,
                    "module" => TsToken::Module,
                    "type" => TsToken::Type,
                    "keyof" => TsToken::Keyof,
                    "infer" => TsToken::Infer,
                    "is" => TsToken::Is,
                    "satisfies" => TsToken::Satisfies,
                    "override" => TsToken::Override,
                    "any" => TsToken::Any,
                    "unknown" => TsToken::Unknown,
                    "never" => TsToken::Never,
                    _ => TsToken::Js(tok.clone()),
                }
            }
            _ => TsToken::Js(tok.clone()),
        };

        result.push(SpannedToken { token: ts_tok, span });
    }

    result.push(SpannedToken { token: TsToken::Eof, span: Span { line, col } });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_keywords() {
        let tokens = tokenize_ts("interface Foo { x: number }").unwrap();
        assert!(matches!(tokens[0].token, TsToken::Interface));
    }

    #[test]
    fn test_enum_keyword() {
        let tokens = tokenize_ts("enum Color { Red, Green, Blue }").unwrap();
        assert!(matches!(tokens[0].token, TsToken::Enum));
    }

    #[test]
    fn test_type_keyword() {
        let tokens = tokenize_ts("type Alias = string | number").unwrap();
        assert!(matches!(tokens[0].token, TsToken::Type));
    }

    #[test]
    fn test_access_modifiers() {
        let tokens = tokenize_ts("private readonly x").unwrap();
        assert!(matches!(tokens[0].token, TsToken::Private));
        assert!(matches!(tokens[1].token, TsToken::Readonly));
    }
}
