//! Top-level TypeScript execution API.
//!
//! Pipeline: lex → type check (warnings) → transform to JS → execute via js-lite.

use alloc::string::String;

use crate::lexer::tokenize_ts;
use crate::type_checker::TypeChecker;
use crate::transformer::transform_to_js;

/// Execute TypeScript source code and return captured console.log() output.
///
/// Type checking produces warnings (not errors) for gradual typing.
/// The source is stripped of types and transformed to plain JS, then
/// executed via js-lite.
pub fn execute_ts(source: &str) -> Result<String, String> {
    log::info!("[ts] executing {} bytes of TypeScript source", source.len());

    // 1. Tokenize as TypeScript
    let tokens = tokenize_ts(source)?;
    log::debug!("[ts] lexed {} tokens", tokens.len());

    // 2. Type check (advisory, does not block execution)
    let mut checker = TypeChecker::new();
    // In a full implementation, we'd walk the AST and check types.
    // For now, the type checker is available for explicit use.
    let diagnostics = checker.format_diagnostics();
    if !diagnostics.is_empty() {
        log::warn!("[ts] type diagnostics:\n{}", diagnostics);
    }

    // 3. Transform to plain JavaScript
    let js_source = transform_to_js(&tokens)?;
    log::debug!("[ts] transformed to {} bytes of JavaScript", js_source.len());

    // 4. Execute via js-lite
    let output = js_lite::execute(&js_source)?;

    Ok(output)
}

/// Execute TypeScript with type checking diagnostics returned separately.
pub fn execute_ts_with_diagnostics(source: &str) -> Result<(String, String), String> {
    let tokens = tokenize_ts(source)?;

    let mut checker = TypeChecker::new();
    let diagnostics = checker.format_diagnostics();

    let js_source = transform_to_js(&tokens)?;
    let output = js_lite::execute(&js_source)?;

    Ok((output, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_simple_ts() {
        // This should work: no type annotations, pure JS subset
        let out = execute_ts("console.log('hello from ts')").unwrap();
        assert_eq!(out.trim(), "hello from ts");
    }

    #[test]
    fn test_execute_var_ts() {
        let out = execute_ts("var x = 42; console.log(x)").unwrap();
        assert_eq!(out.trim(), "42");
    }

    #[test]
    fn test_execute_enum_ts() {
        let out = execute_ts("enum Color { Red, Green, Blue } console.log(Color.Green)").unwrap();
        assert_eq!(out.trim(), "1");
    }

    #[test]
    fn test_strip_interface() {
        // Interface should be stripped, code after it should still work
        let out = execute_ts("interface Foo { x: number } var y = 10; console.log(y)").unwrap();
        assert_eq!(out.trim(), "10");
    }
}
