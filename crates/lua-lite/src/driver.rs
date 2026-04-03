//! LuaState driver: high-level API for executing Lua code.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::lexer;
use crate::parser;
use crate::compiler;
use crate::vm::{LuaState, LuaValue, Scope};

impl LuaState {
    /// Execute a string of Lua source code, returning captured output.
    pub fn dostring(&mut self, source: &str) -> Result<String, String> {
        let tokens = lexer::tokenize(source)?;
        let mut chunk = parser::parse(tokens)?;
        compiler::validate_chunk(&chunk)?;
        compiler::optimize_block(&mut chunk);

        let scope = Rc::new(RefCell::new(Scope::new(Some(self.globals.clone()))));
        self.exec_block(&chunk, &scope)?;

        let output = core::mem::take(&mut self.output);
        Ok(output)
    }

    /// Execute a Lua source and return the function's return values.
    pub fn execute_with_returns(&mut self, source: &str) -> Result<Vec<LuaValue>, String> {
        let tokens = lexer::tokenize(source)?;
        let mut chunk = parser::parse(tokens)?;
        compiler::optimize_block(&mut chunk);

        let scope = Rc::new(RefCell::new(Scope::new(Some(self.globals.clone()))));
        use crate::vm::ControlFlow;
        match self.exec_block(&chunk, &scope)? {
            Some(ControlFlow::Return(vals)) => Ok(vals),
            _ => Ok(Vec::new()),
        }
    }

    /// Register a native function accessible from Lua.
    pub fn register_function(
        &mut self,
        name: &str,
        func: fn(&mut LuaState, &[LuaValue]) -> Result<Vec<LuaValue>, String>,
    ) {
        self.set_global(name, LuaValue::NativeFunction(String::from(name), func));
    }
}
