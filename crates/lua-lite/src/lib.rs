//! claudio-lua-lite: A no_std Lua 5.4 interpreter for ClaudioOS.
//!
//! This is a tree-walking interpreter supporting a substantial subset of Lua 5.4:
//! variables, control flow, functions (closures), tables, metatables (basic),
//! string/table/math/io/os standard libraries.
//!
//! It is `#![no_std]` and runs directly on bare metal with only `alloc`.
//!
//! # Usage
//! ```ignore
//! let mut state = claudio_lua_lite::LuaState::new();
//! let output = state.dostring("print('Hello from Lua!')").unwrap();
//! assert_eq!(output, "Hello from Lua!\n");
//! ```

#![no_std]

extern crate alloc;

pub mod lexer;
pub mod ast;
pub mod parser;
pub mod table;
pub mod vm;
pub mod stdlib;
pub mod compiler;
pub mod driver;

pub use vm::{LuaState, LuaValue};

use alloc::string::String;

/// Execute Lua source code and return captured print() output.
///
/// This is the main entry point for tool integration.
pub fn execute(source: &str) -> Result<String, String> {
    let mut state = LuaState::new();
    state.dostring(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_world() {
        let output = execute("print('Hello, World!')").unwrap();
        assert_eq!(output, "Hello, World!\n");
    }

    #[test]
    fn test_arithmetic() {
        let output = execute("print(2 + 3 * 4)").unwrap();
        assert_eq!(output, "14\n");
    }

    #[test]
    fn test_variables() {
        let output = execute("local x = 10\nlocal y = 20\nprint(x + y)").unwrap();
        assert_eq!(output, "30\n");
    }

    #[test]
    fn test_if_else() {
        let output = execute(r#"
            local x = 10
            if x > 5 then
                print("big")
            else
                print("small")
            end
        "#).unwrap();
        assert_eq!(output, "big\n");
    }

    #[test]
    fn test_while_loop() {
        let output = execute(r#"
            local sum = 0
            local i = 1
            while i <= 10 do
                sum = sum + i
                i = i + 1
            end
            print(sum)
        "#).unwrap();
        assert_eq!(output, "55\n");
    }

    #[test]
    fn test_for_numeric() {
        let output = execute(r#"
            local sum = 0
            for i = 1, 5 do
                sum = sum + i
            end
            print(sum)
        "#).unwrap();
        assert_eq!(output, "15\n");
    }

    #[test]
    fn test_functions() {
        let output = execute(r#"
            local function factorial(n)
                if n <= 1 then return 1 end
                return n * factorial(n - 1)
            end
            print(factorial(10))
        "#).unwrap();
        assert_eq!(output, "3628800\n");
    }

    #[test]
    fn test_tables() {
        let output = execute(r#"
            local t = {10, 20, 30}
            print(#t)
            print(t[2])
        "#).unwrap();
        assert_eq!(output, "3\n20\n");
    }

    #[test]
    fn test_string_ops() {
        let output = execute(r#"
            local s = "Hello, World!"
            print(#s)
            print(string.upper(s))
            print(string.sub(s, 1, 5))
        "#).unwrap();
        assert_eq!(output, "13\nHELLO, WORLD!\nHello\n");
    }

    #[test]
    fn test_table_insert_and_concat() {
        let output = execute(r#"
            local t = {}
            table.insert(t, "a")
            table.insert(t, "b")
            table.insert(t, "c")
            print(table.concat(t, ", "))
        "#).unwrap();
        assert_eq!(output, "a, b, c\n");
    }

    #[test]
    fn test_closures() {
        let output = execute(r#"
            local function counter(start)
                local n = start
                return function()
                    n = n + 1
                    return n
                end
            end
            local c = counter(0)
            print(c())
            print(c())
            print(c())
        "#).unwrap();
        assert_eq!(output, "1\n2\n3\n");
    }

    #[test]
    fn test_string_format() {
        let output = execute(r#"
            print(string.format("Hello %s, you are %d", "Lua", 54))
        "#).unwrap();
        assert_eq!(output, "Hello Lua, you are 54\n");
    }

    #[test]
    fn test_pcall() {
        let output = execute(r#"
            local ok, err = pcall(function() error("oops") end)
            print(ok)
            print(err)
        "#).unwrap();
        assert_eq!(output, "false\noops\n");
    }

    #[test]
    fn test_repeat_until() {
        let output = execute(r#"
            local i = 1
            repeat
                i = i * 2
            until i > 100
            print(i)
        "#).unwrap();
        assert_eq!(output, "128\n");
    }

    #[test]
    fn test_boolean_logic() {
        let output = execute(r#"
            print(true and "yes" or "no")
            print(false and "yes" or "no")
            print(nil or "default")
        "#).unwrap();
        assert_eq!(output, "yes\nno\ndefault\n");
    }

    #[test]
    fn test_table_pairs() {
        let output = execute(r#"
            local t = {a=1, b=2, c=3}
            local sum = 0
            for k, v in pairs(t) do
                sum = sum + v
            end
            print(sum)
        "#).unwrap();
        assert_eq!(output, "6\n");
    }

    #[test]
    fn test_ipairs() {
        let output = execute(r#"
            local t = {10, 20, 30}
            local sum = 0
            for i, v in ipairs(t) do
                sum = sum + v
            end
            print(sum)
        "#).unwrap();
        assert_eq!(output, "60\n");
    }

    #[test]
    fn test_multiline_string() {
        let output = execute(r#"
            local s = "hello" .. " " .. "world"
            print(s)
        "#).unwrap();
        assert_eq!(output, "hello world\n");
    }

    #[test]
    fn test_for_step() {
        let output = execute(r#"
            local s = ""
            for i = 10, 1, -3 do
                s = s .. tostring(i) .. " "
            end
            print(s)
        "#).unwrap();
        assert_eq!(output, "10 7 4 1 \n");
    }

    #[test]
    fn test_nested_tables() {
        let output = execute(r#"
            local t = {inner = {value = 42}}
            print(t.inner.value)
        "#).unwrap();
        assert_eq!(output, "42\n");
    }

    #[test]
    fn test_method_call() {
        let output = execute(r#"
            local obj = {
                name = "test",
                greet = function(self)
                    return "Hello, " .. self.name
                end
            }
            print(obj:greet())
        "#).unwrap();
        assert_eq!(output, "Hello, test\n");
    }

    #[test]
    fn test_math_library() {
        let output = execute(r#"
            print(math.abs(-42))
            print(math.max(1, 5, 3))
            print(math.min(1, 5, 3))
            print(math.floor(3.7))
            print(math.ceil(3.2))
        "#).unwrap();
        assert_eq!(output, "42\n5\n1\n3\n4\n");
    }

    #[test]
    fn test_type_function() {
        let output = execute(r#"
            print(type(42))
            print(type("hello"))
            print(type(true))
            print(type(nil))
            print(type({}))
            print(type(print))
        "#).unwrap();
        assert_eq!(output, "number\nstring\nboolean\nnil\ntable\nfunction\n");
    }

    #[test]
    fn test_integer_division() {
        let output = execute(r#"
            print(7 // 2)
            print(7 % 2)
        "#).unwrap();
        assert_eq!(output, "3\n1\n");
    }

    #[test]
    fn test_string_find() {
        let output = execute(r#"
            local s, e = string.find("hello world", "world")
            print(s, e)
        "#).unwrap();
        assert_eq!(output, "7\t11\n");
    }

    #[test]
    fn test_break_in_for() {
        let output = execute(r#"
            local result = 0
            for i = 1, 100 do
                if i > 5 then break end
                result = result + i
            end
            print(result)
        "#).unwrap();
        assert_eq!(output, "15\n");
    }

    #[test]
    fn test_local_function_recursion() {
        let output = execute(r#"
            local function fib(n)
                if n < 2 then return n end
                return fib(n-1) + fib(n-2)
            end
            print(fib(10))
        "#).unwrap();
        assert_eq!(output, "55\n");
    }
}
