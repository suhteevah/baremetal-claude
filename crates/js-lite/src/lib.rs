//! js-lite: A minimal JavaScript evaluator for ClaudioOS.
//!
//! This is a `no_std` + `alloc` tree-walking interpreter that supports a useful
//! subset of JavaScript syntax: variables, functions, objects, arrays, control flow,
//! try/catch, and builtins needed for Cloudflare challenge solving.
//!
//! It is NOT spec-compliant. It is "JavaScript-shaped" enough that Cloudflare
//! challenge scripts (math + string manipulation + cookie setting) can execute.

#![no_std]

#[macro_use]
extern crate alloc;

pub mod tokenizer;
pub mod parser;
pub mod eval;

pub use eval::{Interpreter, Value};

use alloc::string::String;

/// Execute JavaScript source code and return captured console.log() output.
///
/// This is the main entry point for tool integration.
pub fn execute(source: &str) -> Result<String, String> {
    let tokens = tokenizer::tokenize(source)?;
    let ast = parser::parse(tokens)?;
    let mut interp = Interpreter::new();
    run_block(&mut interp, &ast)?;
    Ok(interp.take_output())
}

/// Execute JavaScript source code and return the resulting document.cookie value.
///
/// This is the entry point for Cloudflare challenge solving: the challenge JS
/// computes a cookie value and sets it via `document.cookie = "..."`.
pub fn execute_for_cookie(source: &str) -> Result<String, String> {
    let tokens = tokenizer::tokenize(source)?;
    let ast = parser::parse(tokens)?;
    let mut interp = Interpreter::new();
    run_block(&mut interp, &ast)?;
    Ok(String::from(interp.get_cookie()))
}

/// Execute JavaScript with pre-set document.cookie and location values.
///
/// Cloudflare challenges often read existing cookies and the current URL.
pub fn execute_with_context(
    source: &str,
    initial_cookie: &str,
    hostname: &str,
    path: &str,
) -> Result<(String, String), String> {
    let tokens = tokenizer::tokenize(source)?;
    let ast = parser::parse(tokens)?;
    let mut interp = Interpreter::new();

    // Set initial context
    interp.cookie = String::from(initial_cookie);

    // Update document.cookie
    if let Some(eval::Value::Object(mut doc)) = interp.take_var("document") {
        doc.insert(String::from("cookie"), eval::Value::Str(String::from(initial_cookie)));
        interp.set_global("document", eval::Value::Object(doc));
    }

    // Update location
    if let Some(eval::Value::Object(mut loc)) = interp.take_var("location") {
        loc.insert(String::from("hostname"), eval::Value::Str(String::from(hostname)));
        loc.insert(String::from("pathname"), eval::Value::Str(String::from(path)));
        loc.insert(
            String::from("href"),
            eval::Value::Str(alloc::format!("https://{}{}", hostname, path)),
        );
        interp.set_global("location", eval::Value::Object(loc));
    }

    run_block(&mut interp, &ast)?;

    let output = interp.take_output();
    let cookie = String::from(interp.get_cookie());
    Ok((output, cookie))
}

/// Run a block of statements on an interpreter, converting Signal to Result.
fn run_block(interp: &mut Interpreter, stmts: &[parser::Stmt]) -> Result<(), String> {
    use eval::Signal;
    match interp.exec_block(stmts) {
        Ok(Signal::None) => Ok(()),
        Ok(Signal::Return(_)) => Ok(()), // top-level return is fine
        Ok(Signal::Break) => Ok(()),
        Ok(Signal::Continue) => Ok(()),
        Ok(Signal::Throw(val)) => Err(alloc::format!("Uncaught: {}", val.to_string_val())),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_world() {
        let out = execute("console.log('hello world')").unwrap();
        assert_eq!(out.trim(), "hello world");
    }

    #[test]
    fn test_variables_and_math() {
        let out = execute("var x = 2 + 3; console.log(x)").unwrap();
        assert_eq!(out.trim(), "5");
    }

    #[test]
    fn test_string_concat() {
        let out = execute("var a = 'hello'; var b = ' world'; console.log(a + b)").unwrap();
        assert_eq!(out.trim(), "hello world");
    }

    #[test]
    fn test_if_else() {
        let out = execute("var x = 10; if (x > 5) { console.log('big'); } else { console.log('small'); }").unwrap();
        assert_eq!(out.trim(), "big");
    }

    #[test]
    fn test_for_loop() {
        let out = execute("for (var i = 0; i < 3; i++) { console.log(i); }").unwrap();
        assert_eq!(out.trim(), "0\n1\n2");
    }

    #[test]
    fn test_while_loop() {
        let out = execute("var x = 0; while (x < 3) { console.log(x); x++; }").unwrap();
        assert_eq!(out.trim(), "0\n1\n2");
    }

    #[test]
    fn test_function_def() {
        let out = execute("function add(a, b) { return a + b; } console.log(add(3, 4))").unwrap();
        assert_eq!(out.trim(), "7");
    }

    #[test]
    fn test_bitwise_ops() {
        let out = execute("console.log(0xFF & 0x0F); console.log(0x0F | 0xF0); console.log(5 ^ 3); console.log(1 << 4)").unwrap();
        assert_eq!(out.trim(), "15\n255\n6\n16");
    }

    #[test]
    fn test_parse_int() {
        let out = execute("console.log(parseInt('42')); console.log(parseInt('0xFF', 16)); console.log(parseInt('10', 2))").unwrap();
        assert_eq!(out.trim(), "42\n255\n2");
    }

    #[test]
    fn test_string_methods() {
        let out = execute("console.log('hello'.toUpperCase()); console.log('HELLO'.toLowerCase()); console.log('  hi  '.trim())").unwrap();
        assert_eq!(out.trim(), "HELLO\nhello\nhi");
    }

    #[test]
    fn test_array_methods() {
        let out = execute("var a = [1, 2, 3]; console.log(a.join('-')); console.log(a.map(function(x) { return x * 2; }).join(','))").unwrap();
        assert_eq!(out.trim(), "1-2-3\n2,4,6");
    }

    #[test]
    fn test_math_floor_ceil() {
        let out = execute("console.log(Math.floor(3.7)); console.log(Math.ceil(3.2)); console.log(Math.abs(-5))").unwrap();
        assert_eq!(out.trim(), "3\n4\n5");
    }

    #[test]
    fn test_btoa_atob() {
        let out = execute("var encoded = btoa('hello'); console.log(encoded); console.log(atob(encoded))").unwrap();
        assert_eq!(out.trim(), "aGVsbG8=\nhello");
    }

    #[test]
    fn test_encode_uri_component() {
        let out = execute("console.log(encodeURIComponent('hello world'))").unwrap();
        assert_eq!(out.trim(), "hello%20world");
    }

    #[test]
    fn test_document_cookie() {
        let cookie = execute_for_cookie("document.cookie = 'cf_clearance=abc123; path=/'").unwrap();
        assert_eq!(cookie, "cf_clearance=abc123");
    }

    #[test]
    fn test_ternary() {
        let out = execute("var x = 5; console.log(x > 3 ? 'yes' : 'no')").unwrap();
        assert_eq!(out.trim(), "yes");
    }

    #[test]
    fn test_arrow_function() {
        let out = execute("var double = x => x * 2; console.log(double(21))").unwrap();
        assert_eq!(out.trim(), "42");
    }

    #[test]
    fn test_template_literal() {
        let out = execute("var x = 42; console.log(`value is ${x}`)").unwrap();
        assert_eq!(out.trim(), "value is 42");
    }

    #[test]
    fn test_try_catch() {
        let out = execute("try { throw 'oops'; } catch(e) { console.log('caught: ' + e); }").unwrap();
        assert_eq!(out.trim(), "caught: oops");
    }

    #[test]
    fn test_typeof() {
        let out = execute("console.log(typeof 42); console.log(typeof 'hi'); console.log(typeof true)").unwrap();
        assert_eq!(out.trim(), "number\nstring\nboolean");
    }

    #[test]
    fn test_object_literal() {
        let out = execute("var o = {a: 1, b: 'hello'}; console.log(o.a); console.log(o.b)").unwrap();
        assert_eq!(out.trim(), "1\nhello");
    }

    #[test]
    fn test_json_stringify_parse() {
        let out = execute(r#"var o = {a: 1}; var s = JSON.stringify(o); console.log(s); var p = JSON.parse(s); console.log(p.a)"#).unwrap();
        assert_eq!(out.trim(), "{\"a\":1}\n1");
    }

    #[test]
    fn test_string_char_code() {
        let out = execute("console.log(String.fromCharCode(65, 66, 67))").unwrap();
        assert_eq!(out.trim(), "ABC");
    }

    #[test]
    fn test_regex_test() {
        let out = execute("var re = /\\d+/; console.log(re.test('abc123'))").unwrap();
        assert_eq!(out.trim(), "true");
    }

    #[test]
    fn test_switch() {
        let out = execute("var x = 2; switch(x) { case 1: console.log('one'); break; case 2: console.log('two'); break; default: console.log('other'); }").unwrap();
        assert_eq!(out.trim(), "two");
    }

    #[test]
    fn test_set_timeout_stub() {
        let out = execute("setTimeout(function() { console.log('fired'); }, 100)").unwrap();
        assert_eq!(out.trim(), "fired");
    }

    #[test]
    fn test_nullish_coalescing() {
        let out = execute("var x = null; console.log(x ?? 'default')").unwrap();
        assert_eq!(out.trim(), "default");
    }

    #[test]
    fn test_number_to_string_radix() {
        let out = execute("console.log((255).toString(16))").unwrap();
        assert_eq!(out.trim(), "ff");
    }

    #[test]
    fn test_cloudflare_style_math() {
        // Typical Cloudflare challenge pattern: accumulate values
        let code = r#"
            var t = 'example.com';
            var a = {};
            a.value = 47 * 31 + 16;
            a.value += 3 * 7;
            a.value = Math.round(a.value / 2);
            console.log(a.value);
        "#;
        let out = execute(code).unwrap();
        assert!(out.trim().parse::<f64>().is_ok());
    }
}
