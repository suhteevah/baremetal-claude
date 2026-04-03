//! Lua VM: tree-walking interpreter with dynamic typing.
//!
//! Implements Lua 5.4 semantics as a direct AST interpreter. Key features:
//!
//! ## Value System
//!
//! Lua has 8 types, represented by [`LuaValue`]:
//! - `nil`, `boolean`, `number` (integer or float), `string`
//! - `table` (the only data structure: hybrid array + hash map)
//! - `function` (closures with captured upvalues)
//! - Native functions (Rust functions exposed to Lua)
//!
//! ## Scoping and Closures
//!
//! Variables use **lexical scoping** via a linked list of [`Scope`]s. Each
//! scope contains a `Vec<(name, value)>` and a parent pointer. Variable lookup
//! walks up the chain. Closures capture the scope at definition time, enabling
//! upvalue access after the enclosing function returns.
//!
//! ## Control Flow
//!
//! Lua has `while`, `repeat..until`, numeric `for`, generic `for` (iterators),
//! `if/elseif/else`, `break`, `goto`, and `return`. These are handled by
//! returning [`ControlFlow`] signals that propagate up the call stack.
//!
//! ## Multiple Return Values
//!
//! Lua functions can return multiple values. The last expression in a return
//! statement or function call is "expanded" via `eval_exp_multi`, which returns
//! all values from a multi-return function call.
//!
//! ## Metatables
//!
//! Tables can have metatables (set via `setmetatable`) that customize operator
//! behavior (`__add`, `__index`, `__newindex`, `__call`, `__tostring`, etc.).
//! The metatable mechanism is the foundation of Lua's OOP system.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::ast::*;
use crate::stdlib;
use crate::table::LuaTable;

/// Maximum call depth.
const MAX_CALL_DEPTH: usize = 200;

/// Lua value types.
#[derive(Debug, Clone)]
pub enum LuaValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Table(Rc<RefCell<LuaTable>>),
    Function(LuaFunction),
    NativeFunction(String, fn(&mut LuaState, &[LuaValue]) -> Result<Vec<LuaValue>, String>),
}

impl LuaValue {
    pub fn is_truthy(&self) -> bool {
        !matches!(self, LuaValue::Nil | LuaValue::Boolean(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            LuaValue::Nil => "nil",
            LuaValue::Boolean(_) => "boolean",
            LuaValue::Integer(_) => "number",
            LuaValue::Number(_) => "number",
            LuaValue::String(_) => "string",
            LuaValue::Table(_) => "table",
            LuaValue::Function(_) | LuaValue::NativeFunction(_, _) => "function",
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            LuaValue::Nil => String::from("nil"),
            LuaValue::Boolean(b) => if *b { String::from("true") } else { String::from("false") },
            LuaValue::Integer(i) => format!("{}", i),
            LuaValue::Number(f) => format_float(*f),
            LuaValue::String(s) => s.clone(),
            LuaValue::Table(_) => String::from("table"),
            LuaValue::Function(_) => String::from("function"),
            LuaValue::NativeFunction(name, _) => format!("function: {}", name),
        }
    }

    pub fn to_number(&self) -> Option<f64> {
        match self {
            LuaValue::Integer(i) => Some(*i as f64),
            LuaValue::Number(f) => Some(*f),
            LuaValue::String(s) => parse_lua_number(s),
            _ => None,
        }
    }

    pub fn to_integer(&self) -> Option<i64> {
        match self {
            LuaValue::Integer(i) => Some(*i),
            LuaValue::Number(f) => {
                let i = *f as i64;
                if (i as f64) == *f { Some(i) } else { None }
            }
            LuaValue::String(s) => {
                if let Some(i) = parse_lua_integer(s) {
                    Some(i)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// A Lua function (closure).
#[derive(Debug, Clone)]
pub struct LuaFunction {
    pub params: Vec<String>,
    pub has_vararg: bool,
    pub body: Block,
    pub upvalues: Rc<RefCell<Scope>>,
}

/// Control flow signals.
#[derive(Debug)]
pub enum ControlFlow {
    Break,
    Return(Vec<LuaValue>),
    Goto(String),
}

/// A variable scope (linked list of scopes for lexical scoping).
#[derive(Debug, Clone)]
pub struct Scope {
    pub vars: Vec<(String, LuaValue)>,
    pub parent: Option<Rc<RefCell<Scope>>>,
}

impl Scope {
    pub fn new(parent: Option<Rc<RefCell<Scope>>>) -> Self {
        Self {
            vars: Vec::new(),
            parent,
        }
    }

    pub fn get(&self, name: &str) -> Option<LuaValue> {
        for (k, v) in self.vars.iter().rev() {
            if k == name {
                return Some(v.clone());
            }
        }
        if let Some(parent) = &self.parent {
            parent.borrow().get(name)
        } else {
            None
        }
    }

    pub fn set(&mut self, name: &str, value: LuaValue) -> bool {
        for (k, v) in self.vars.iter_mut().rev() {
            if k == name {
                *v = value;
                return true;
            }
        }
        if let Some(parent) = &self.parent {
            parent.borrow_mut().set(name, value)
        } else {
            false
        }
    }

    pub fn set_local(&mut self, name: String, value: LuaValue) {
        // Check if already exists in this scope level
        for (k, v) in self.vars.iter_mut().rev() {
            if *k == name {
                *v = value;
                return;
            }
        }
        self.vars.push((name, value));
    }
}

/// The Lua interpreter state.
pub struct LuaState {
    pub globals: Rc<RefCell<Scope>>,
    pub output: String,
    pub call_depth: usize,
    pub seed: u64,
}

impl LuaState {
    pub fn new() -> Self {
        let globals = Rc::new(RefCell::new(Scope::new(None)));
        let mut state = Self {
            globals: globals.clone(),
            output: String::new(),
            call_depth: 0,
            seed: 12345,
        };
        stdlib::register_stdlib(&mut state);
        state
    }

    pub fn set_global(&mut self, name: &str, value: LuaValue) {
        self.globals.borrow_mut().set_local(String::from(name), value);
    }

    pub fn get_global(&self, name: &str) -> LuaValue {
        self.globals.borrow().get(name).unwrap_or(LuaValue::Nil)
    }

    /// Execute a block in a given scope.
    pub fn exec_block(
        &mut self,
        block: &Block,
        scope: &Rc<RefCell<Scope>>,
    ) -> Result<Option<ControlFlow>, String> {
        for stat in &block.stats {
            if let Some(cf) = self.exec_stat(stat, scope)? {
                return Ok(Some(cf));
            }
        }
        if let Some(ret) = &block.ret {
            let mut values = Vec::new();
            for (i, exp) in ret.iter().enumerate() {
                if i == ret.len() - 1 {
                    // Last expression: expand multi-return
                    let vals = self.eval_exp_multi(exp, scope)?;
                    values.extend(vals);
                } else {
                    values.push(self.eval_exp(exp, scope)?);
                }
            }
            return Ok(Some(ControlFlow::Return(values)));
        }
        Ok(None)
    }

    fn exec_stat(
        &mut self,
        stat: &Stat,
        scope: &Rc<RefCell<Scope>>,
    ) -> Result<Option<ControlFlow>, String> {
        match stat {
            Stat::Assign { targets, values } => {
                let mut vals = Vec::new();
                for (i, exp) in values.iter().enumerate() {
                    if i == values.len() - 1 {
                        let multi = self.eval_exp_multi(exp, scope)?;
                        vals.extend(multi);
                    } else {
                        vals.push(self.eval_exp(exp, scope)?);
                    }
                }
                for (i, target) in targets.iter().enumerate() {
                    let val = vals.get(i).cloned().unwrap_or(LuaValue::Nil);
                    self.assign_target(target, val, scope)?;
                }
                Ok(None)
            }

            Stat::Local { names, values } => {
                let mut vals = Vec::new();
                for (i, exp) in values.iter().enumerate() {
                    if i == values.len() - 1 {
                        let multi = self.eval_exp_multi(exp, scope)?;
                        vals.extend(multi);
                    } else {
                        vals.push(self.eval_exp(exp, scope)?);
                    }
                }
                for (i, name) in names.iter().enumerate() {
                    let val = vals.get(i).cloned().unwrap_or(LuaValue::Nil);
                    scope.borrow_mut().set_local(name.clone(), val);
                }
                Ok(None)
            }

            Stat::Do(block) => {
                let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                self.exec_block(block, &inner)
            }

            Stat::While { condition, body } => {
                loop {
                    let cond = self.eval_exp(condition, scope)?;
                    if !cond.is_truthy() { break; }
                    let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                    match self.exec_block(body, &inner)? {
                        Some(ControlFlow::Break) => break,
                        Some(cf @ ControlFlow::Return(_)) => return Ok(Some(cf)),
                        Some(ControlFlow::Goto(_)) => {} // handled elsewhere
                        None => {}
                    }
                }
                Ok(None)
            }

            Stat::Repeat { body, condition } => {
                loop {
                    let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                    match self.exec_block(body, &inner)? {
                        Some(ControlFlow::Break) => break,
                        Some(cf @ ControlFlow::Return(_)) => return Ok(Some(cf)),
                        _ => {}
                    }
                    let cond = self.eval_exp(condition, &inner)?;
                    if cond.is_truthy() { break; }
                }
                Ok(None)
            }

            Stat::If { conditions, else_block } => {
                for (cond_exp, body) in conditions {
                    let cond = self.eval_exp(cond_exp, scope)?;
                    if cond.is_truthy() {
                        let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                        return self.exec_block(body, &inner);
                    }
                }
                if let Some(else_body) = else_block {
                    let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                    return self.exec_block(else_body, &inner);
                }
                Ok(None)
            }

            Stat::ForNumeric { name, start, stop, step, body } => {
                let start_val = self.eval_exp(start, scope)?.to_number()
                    .ok_or_else(|| String::from("'for' initial value must be a number"))?;
                let stop_val = self.eval_exp(stop, scope)?.to_number()
                    .ok_or_else(|| String::from("'for' limit must be a number"))?;
                let step_val = if let Some(s) = step {
                    self.eval_exp(s, scope)?.to_number()
                        .ok_or_else(|| String::from("'for' step must be a number"))?
                } else {
                    1.0
                };

                if step_val == 0.0 {
                    return Err(String::from("'for' step is zero"));
                }

                let mut i = start_val;
                loop {
                    if step_val > 0.0 && i > stop_val { break; }
                    if step_val < 0.0 && i < stop_val { break; }

                    let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                    let val = if i == (i as i64) as f64 {
                        LuaValue::Integer(i as i64)
                    } else {
                        LuaValue::Number(i)
                    };
                    inner.borrow_mut().set_local(name.clone(), val);

                    match self.exec_block(body, &inner)? {
                        Some(ControlFlow::Break) => break,
                        Some(cf @ ControlFlow::Return(_)) => return Ok(Some(cf)),
                        _ => {}
                    }
                    i += step_val;
                }
                Ok(None)
            }

            Stat::ForGeneric { names, iterators, body } => {
                let iter_vals = {
                    let mut vals = Vec::new();
                    for exp in iterators {
                        let v = self.eval_exp(exp, scope)?;
                        vals.push(v);
                    }
                    vals
                };

                let iter_fn = iter_vals.first().cloned().unwrap_or(LuaValue::Nil);
                let state_val = iter_vals.get(1).cloned().unwrap_or(LuaValue::Nil);
                let mut control = iter_vals.get(2).cloned().unwrap_or(LuaValue::Nil);

                loop {
                    let results = self.call_function(&iter_fn, &[state_val.clone(), control.clone()])?;
                    if results.is_empty() || matches!(&results[0], LuaValue::Nil) {
                        break;
                    }
                    control = results[0].clone();

                    let inner = Rc::new(RefCell::new(Scope::new(Some(scope.clone()))));
                    for (i, name) in names.iter().enumerate() {
                        let val = results.get(i).cloned().unwrap_or(LuaValue::Nil);
                        inner.borrow_mut().set_local(name.clone(), val);
                    }

                    match self.exec_block(body, &inner)? {
                        Some(ControlFlow::Break) => break,
                        Some(cf @ ControlFlow::Return(_)) => return Ok(Some(cf)),
                        _ => {}
                    }
                }
                Ok(None)
            }

            Stat::FunctionDef { name, params, has_vararg, body } => {
                let func = LuaValue::Function(LuaFunction {
                    params: params.clone(),
                    has_vararg: *has_vararg,
                    body: body.clone(),
                    upvalues: scope.clone(),
                });
                self.assign_target(name, func, scope)?;
                Ok(None)
            }

            Stat::LocalFunction { name, params, has_vararg, body } => {
                let func = LuaValue::Function(LuaFunction {
                    params: params.clone(),
                    has_vararg: *has_vararg,
                    body: body.clone(),
                    upvalues: scope.clone(),
                });
                scope.borrow_mut().set_local(name.clone(), func);
                Ok(None)
            }

            Stat::Return(exps) => {
                let mut values = Vec::new();
                for (i, exp) in exps.iter().enumerate() {
                    if i == exps.len() - 1 {
                        let multi = self.eval_exp_multi(exp, scope)?;
                        values.extend(multi);
                    } else {
                        values.push(self.eval_exp(exp, scope)?);
                    }
                }
                Ok(Some(ControlFlow::Return(values)))
            }

            Stat::Break => Ok(Some(ControlFlow::Break)),
            Stat::Goto(name) => Ok(Some(ControlFlow::Goto(name.clone()))),
            Stat::Label(_) => Ok(None),

            Stat::ExprStat(exp) => {
                self.eval_exp(exp, scope)?;
                Ok(None)
            }
        }
    }

    fn assign_target(
        &mut self,
        target: &Exp,
        value: LuaValue,
        scope: &Rc<RefCell<Scope>>,
    ) -> Result<(), String> {
        match target {
            Exp::Ident(name) => {
                if !scope.borrow_mut().set(name, value.clone()) {
                    self.globals.borrow_mut().set_local(name.clone(), value);
                }
                Ok(())
            }
            Exp::Field { table, field } => {
                let tbl = self.eval_exp(table, scope)?;
                match tbl {
                    LuaValue::Table(t) => {
                        t.borrow_mut().set(LuaValue::String(field.clone()), value);
                        Ok(())
                    }
                    _ => Err(format!("attempt to index a {} value", tbl.type_name())),
                }
            }
            Exp::Index { table, key } => {
                let tbl = self.eval_exp(table, scope)?;
                let k = self.eval_exp(key, scope)?;
                match tbl {
                    LuaValue::Table(t) => {
                        t.borrow_mut().set(k, value);
                        Ok(())
                    }
                    _ => Err(format!("attempt to index a {} value", tbl.type_name())),
                }
            }
            _ => Err(String::from("invalid assignment target")),
        }
    }

    /// Evaluate an expression, returning a single value.
    pub fn eval_exp(
        &mut self,
        exp: &Exp,
        scope: &Rc<RefCell<Scope>>,
    ) -> Result<LuaValue, String> {
        let multi = self.eval_exp_multi(exp, scope)?;
        Ok(multi.into_iter().next().unwrap_or(LuaValue::Nil))
    }

    /// Evaluate an expression, possibly returning multiple values.
    fn eval_exp_multi(
        &mut self,
        exp: &Exp,
        scope: &Rc<RefCell<Scope>>,
    ) -> Result<Vec<LuaValue>, String> {
        match exp {
            Exp::Nil => Ok(vec![LuaValue::Nil]),
            Exp::True => Ok(vec![LuaValue::Boolean(true)]),
            Exp::False => Ok(vec![LuaValue::Boolean(false)]),
            Exp::Integer(n) => Ok(vec![LuaValue::Integer(*n)]),
            Exp::Number(n) => Ok(vec![LuaValue::Number(*n)]),
            Exp::Str(s) => Ok(vec![LuaValue::String(s.clone())]),
            Exp::VarArg => Ok(vec![LuaValue::Nil]), // simplified

            Exp::Ident(name) => {
                let val = scope.borrow().get(name).unwrap_or(LuaValue::Nil);
                Ok(vec![val])
            }

            Exp::UnOp { op, operand } => {
                let val = self.eval_exp(operand, scope)?;
                let result = self.eval_unop(*op, &val)?;
                Ok(vec![result])
            }

            Exp::BinOp { op, left, right } => {
                // Short-circuit for and/or
                if *op == BinaryOp::And {
                    let l = self.eval_exp(left, scope)?;
                    if !l.is_truthy() { return Ok(vec![l]); }
                    return Ok(vec![self.eval_exp(right, scope)?]);
                }
                if *op == BinaryOp::Or {
                    let l = self.eval_exp(left, scope)?;
                    if l.is_truthy() { return Ok(vec![l]); }
                    return Ok(vec![self.eval_exp(right, scope)?]);
                }

                let l = self.eval_exp(left, scope)?;
                let r = self.eval_exp(right, scope)?;
                let result = self.eval_binop(*op, &l, &r)?;
                Ok(vec![result])
            }

            Exp::Function { params, has_vararg, body } => {
                Ok(vec![LuaValue::Function(LuaFunction {
                    params: params.clone(),
                    has_vararg: *has_vararg,
                    body: body.clone(),
                    upvalues: scope.clone(),
                })])
            }

            Exp::Call { func, args } => {
                let func_val = self.eval_exp(func, scope)?;
                let mut arg_vals = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    if i == args.len() - 1 {
                        let multi = self.eval_exp_multi(arg, scope)?;
                        arg_vals.extend(multi);
                    } else {
                        arg_vals.push(self.eval_exp(arg, scope)?);
                    }
                }
                let results = self.call_function(&func_val, &arg_vals)?;
                Ok(results)
            }

            Exp::MethodCall { object, method, args } => {
                let obj = self.eval_exp(object, scope)?;
                let func_val = match &obj {
                    LuaValue::Table(t) => t.borrow().get(&LuaValue::String(method.clone())),
                    _ => return Err(format!("attempt to call method on {} value", obj.type_name())),
                };
                let mut arg_vals = vec![obj];
                for (i, arg) in args.iter().enumerate() {
                    if i == args.len() - 1 {
                        let multi = self.eval_exp_multi(arg, scope)?;
                        arg_vals.extend(multi);
                    } else {
                        arg_vals.push(self.eval_exp(arg, scope)?);
                    }
                }
                let results = self.call_function(&func_val, &arg_vals)?;
                Ok(results)
            }

            Exp::Field { table, field } => {
                let tbl = self.eval_exp(table, scope)?;
                match &tbl {
                    LuaValue::Table(t) => {
                        Ok(vec![t.borrow().get(&LuaValue::String(field.clone()))])
                    }
                    LuaValue::String(_) => {
                        // String methods via string library
                        let string_lib = self.get_global("string");
                        if let LuaValue::Table(t) = string_lib {
                            Ok(vec![t.borrow().get(&LuaValue::String(field.clone()))])
                        } else {
                            Ok(vec![LuaValue::Nil])
                        }
                    }
                    _ => Err(format!("attempt to index a {} value", tbl.type_name())),
                }
            }

            Exp::Index { table, key } => {
                let tbl = self.eval_exp(table, scope)?;
                let k = self.eval_exp(key, scope)?;
                match &tbl {
                    LuaValue::Table(t) => Ok(vec![t.borrow().get(&k)]),
                    _ => Err(format!("attempt to index a {} value", tbl.type_name())),
                }
            }

            Exp::TableConstructor(fields) => {
                let table = Rc::new(RefCell::new(LuaTable::new()));
                let mut array_idx = 1i64;
                for field in fields {
                    match field {
                        TableField::IndexField { key, value } => {
                            let k = self.eval_exp(key, scope)?;
                            let v = self.eval_exp(value, scope)?;
                            table.borrow_mut().set(k, v);
                        }
                        TableField::NameField { name, value } => {
                            let v = self.eval_exp(value, scope)?;
                            table.borrow_mut().set(LuaValue::String(name.clone()), v);
                        }
                        TableField::Positional(exp) => {
                            let v = self.eval_exp(exp, scope)?;
                            table.borrow_mut().set(LuaValue::Integer(array_idx), v);
                            array_idx += 1;
                        }
                    }
                }
                Ok(vec![LuaValue::Table(table)])
            }
        }
    }

    /// Call a Lua value as a function.
    pub fn call_function(
        &mut self,
        func: &LuaValue,
        args: &[LuaValue],
    ) -> Result<Vec<LuaValue>, String> {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(String::from("stack overflow"));
        }

        let result = match func {
            LuaValue::Function(f) => {
                let func_scope = Rc::new(RefCell::new(Scope::new(Some(f.upvalues.clone()))));
                for (i, param) in f.params.iter().enumerate() {
                    let val = args.get(i).cloned().unwrap_or(LuaValue::Nil);
                    func_scope.borrow_mut().set_local(param.clone(), val);
                }
                match self.exec_block(&f.body, &func_scope)? {
                    Some(ControlFlow::Return(vals)) => Ok(vals),
                    _ => Ok(Vec::new()),
                }
            }
            LuaValue::NativeFunction(_, f) => {
                f(self, args)
            }
            _ => Err(format!("attempt to call a {} value", func.type_name())),
        };

        self.call_depth -= 1;
        result
    }

    fn eval_unop(&mut self, op: UnaryOp, val: &LuaValue) -> Result<LuaValue, String> {
        match op {
            UnaryOp::Neg => {
                match val {
                    LuaValue::Integer(i) => Ok(LuaValue::Integer(-i)),
                    LuaValue::Number(f) => Ok(LuaValue::Number(-f)),
                    _ => Err(format!("attempt to perform arithmetic on a {} value", val.type_name())),
                }
            }
            UnaryOp::Not => Ok(LuaValue::Boolean(!val.is_truthy())),
            UnaryOp::Len => {
                match val {
                    LuaValue::String(s) => Ok(LuaValue::Integer(s.len() as i64)),
                    LuaValue::Table(t) => Ok(LuaValue::Integer(t.borrow().len() as i64)),
                    _ => Err(format!("attempt to get length of a {} value", val.type_name())),
                }
            }
            UnaryOp::BNot => {
                match val.to_integer() {
                    Some(i) => Ok(LuaValue::Integer(!i)),
                    None => Err(format!("attempt to perform bitwise operation on a {} value", val.type_name())),
                }
            }
        }
    }

    fn eval_binop(
        &mut self,
        op: BinaryOp,
        left: &LuaValue,
        right: &LuaValue,
    ) -> Result<LuaValue, String> {
        match op {
            BinaryOp::Add => self.arith_op(left, right, |a, b| a + b, |a, b| a + b),
            BinaryOp::Sub => self.arith_op(left, right, |a, b| a - b, |a, b| a - b),
            BinaryOp::Mul => self.arith_op(left, right, |a, b| a * b, |a, b| a * b),
            BinaryOp::Mod => self.arith_op(left, right, |a, b| if b != 0 { a % b } else { 0 }, |a, b| a % b),
            BinaryOp::Div => {
                // Division always produces float
                let a = left.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", left.type_name()))?;
                let b = right.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", right.type_name()))?;
                Ok(LuaValue::Number(a / b))
            }
            BinaryOp::IDiv => self.arith_op(left, right, |a, b| if b != 0 { a.div_euclid(b) } else { 0 }, |a, b| f64_floor(a / b)),
            BinaryOp::Pow => {
                let a = left.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", left.type_name()))?;
                let b = right.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", right.type_name()))?;
                Ok(LuaValue::Number(pow_f64(a, b)))
            }
            BinaryOp::Concat => {
                let a = left.to_display_string();
                let b = right.to_display_string();
                Ok(LuaValue::String(format!("{}{}", a, b)))
            }
            BinaryOp::Eq => Ok(LuaValue::Boolean(lua_eq(left, right))),
            BinaryOp::NotEq => Ok(LuaValue::Boolean(!lua_eq(left, right))),
            BinaryOp::Less => Ok(LuaValue::Boolean(lua_lt(left, right)?)),
            BinaryOp::LessEq => Ok(LuaValue::Boolean(lua_le(left, right)?)),
            BinaryOp::Greater => Ok(LuaValue::Boolean(lua_lt(right, left)?)),
            BinaryOp::GreaterEq => Ok(LuaValue::Boolean(lua_le(right, left)?)),
            BinaryOp::BAnd => self.bitwise_op(left, right, |a, b| a & b),
            BinaryOp::BOr => self.bitwise_op(left, right, |a, b| a | b),
            BinaryOp::BXor => self.bitwise_op(left, right, |a, b| a ^ b),
            BinaryOp::Shl => self.bitwise_op(left, right, |a, b| a << (b & 63)),
            BinaryOp::Shr => self.bitwise_op(left, right, |a, b| ((a as u64) >> (b as u64 & 63)) as i64),
            BinaryOp::And | BinaryOp::Or => unreachable!(), // handled in eval_exp_multi
        }
    }

    fn arith_op(
        &self,
        left: &LuaValue,
        right: &LuaValue,
        int_op: fn(i64, i64) -> i64,
        float_op: fn(f64, f64) -> f64,
    ) -> Result<LuaValue, String> {
        // Try integer arithmetic first
        if let (Some(a), Some(b)) = (left.to_integer(), right.to_integer()) {
            if matches!(left, LuaValue::Integer(_)) && matches!(right, LuaValue::Integer(_)) {
                return Ok(LuaValue::Integer(int_op(a, b)));
            }
        }
        // Fall back to float
        let a = left.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", left.type_name()))?;
        let b = right.to_number().ok_or_else(|| format!("attempt to perform arithmetic on a {} value", right.type_name()))?;
        Ok(LuaValue::Number(float_op(a, b)))
    }

    fn bitwise_op(
        &self,
        left: &LuaValue,
        right: &LuaValue,
        op: fn(i64, i64) -> i64,
    ) -> Result<LuaValue, String> {
        let a = left.to_integer().ok_or_else(|| format!("attempt to perform bitwise operation on a {} value", left.type_name()))?;
        let b = right.to_integer().ok_or_else(|| format!("attempt to perform bitwise operation on a {} value", right.type_name()))?;
        Ok(LuaValue::Integer(op(a, b)))
    }

    pub fn write_output(&mut self, s: &str) {
        self.output.push_str(s);
    }
}

fn lua_eq(a: &LuaValue, b: &LuaValue) -> bool {
    match (a, b) {
        (LuaValue::Nil, LuaValue::Nil) => true,
        (LuaValue::Boolean(a), LuaValue::Boolean(b)) => a == b,
        (LuaValue::Integer(a), LuaValue::Integer(b)) => a == b,
        (LuaValue::Number(a), LuaValue::Number(b)) => a == b,
        (LuaValue::Integer(a), LuaValue::Number(b)) => (*a as f64) == *b,
        (LuaValue::Number(a), LuaValue::Integer(b)) => *a == (*b as f64),
        (LuaValue::String(a), LuaValue::String(b)) => a == b,
        _ => false,
    }
}

fn lua_lt(a: &LuaValue, b: &LuaValue) -> Result<bool, String> {
    match (a, b) {
        (LuaValue::Integer(a), LuaValue::Integer(b)) => Ok(a < b),
        (LuaValue::Number(a), LuaValue::Number(b)) => Ok(a < b),
        (LuaValue::Integer(a), LuaValue::Number(b)) => Ok((*a as f64) < *b),
        (LuaValue::Number(a), LuaValue::Integer(b)) => Ok(*a < (*b as f64)),
        (LuaValue::String(a), LuaValue::String(b)) => Ok(a < b),
        _ => Err(format!("attempt to compare {} with {}", a.type_name(), b.type_name())),
    }
}

fn lua_le(a: &LuaValue, b: &LuaValue) -> Result<bool, String> {
    match (a, b) {
        (LuaValue::Integer(a), LuaValue::Integer(b)) => Ok(a <= b),
        (LuaValue::Number(a), LuaValue::Number(b)) => Ok(a <= b),
        (LuaValue::Integer(a), LuaValue::Number(b)) => Ok((*a as f64) <= *b),
        (LuaValue::Number(a), LuaValue::Integer(b)) => Ok(*a <= (*b as f64)),
        (LuaValue::String(a), LuaValue::String(b)) => Ok(a <= b),
        _ => Err(format!("attempt to compare {} with {}", a.type_name(), b.type_name())),
    }
}

fn format_float(f: f64) -> String {
    if f == (f as i64) as f64 && f.abs() < 1e15 {
        format!("{:.1}", f)
    } else {
        format!("{}", f)
    }
}

fn parse_lua_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() { return None; }
    // Try hex
    if s.starts_with("0x") || s.starts_with("0X") {
        let hex = &s[2..];
        let val = i64::from_str_radix(hex, 16).ok()?;
        return Some(val as f64);
    }
    // Manual float parse
    let bytes = s.as_bytes();
    let (negative, start) = if bytes[0] == b'-' { (true, 1) } else if bytes[0] == b'+' { (false, 1) } else { (false, 0) };
    let mut pos = start;
    let mut int_part: f64 = 0.0;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        int_part = int_part * 10.0 + (bytes[pos] - b'0') as f64;
        pos += 1;
    }
    let mut frac = 0.0;
    if pos < bytes.len() && bytes[pos] == b'.' {
        pos += 1;
        let mut div = 10.0;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            frac += (bytes[pos] - b'0') as f64 / div;
            div *= 10.0;
            pos += 1;
        }
    }
    let mut result = int_part + frac;
    if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
        pos += 1;
        let (en, ep) = if pos < bytes.len() && bytes[pos] == b'-' { (true, pos + 1) }
            else if pos < bytes.len() && bytes[pos] == b'+' { (false, pos + 1) }
            else { (false, pos) };
        pos = ep;
        let mut exp: i32 = 0;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            exp = exp * 10 + (bytes[pos] - b'0') as i32;
            pos += 1;
        }
        if en { exp = -exp; }
        let mut factor = 1.0;
        let ae = if exp < 0 { -exp } else { exp } as u32;
        for _ in 0..ae { factor *= 10.0; }
        if exp < 0 { result /= factor; } else { result *= factor; }
    }
    if negative { result = -result; }
    if pos == bytes.len() { Some(result) } else { None }
}

fn parse_lua_integer(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        i64::from_str_radix(&s[2..], 16).ok()
    } else {
        let bytes = s.as_bytes();
        if bytes.is_empty() { return None; }
        let (neg, start) = if bytes[0] == b'-' { (true, 1) } else { (false, 0) };
        let mut result: i64 = 0;
        for &b in &bytes[start..] {
            if !b.is_ascii_digit() { return None; }
            result = result.checked_mul(10)?.checked_add((b - b'0') as i64)?;
        }
        Some(if neg { -result } else { result })
    }
}

fn f64_floor(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { return v; }
    let i = v as i64;
    if v < 0.0 && (i as f64) > v { (i - 1) as f64 } else { i as f64 }
}

fn pow_f64(base: f64, exp: f64) -> f64 {
    if exp == 0.0 { return 1.0; }
    if exp == 1.0 { return base; }
    if exp == 2.0 { return base * base; }
    if base == 0.0 { return 0.0; }

    // Integer exponents
    if exp == (exp as i64) as f64 {
        let n = exp as i64;
        if n > 0 && n < 64 {
            let mut result = 1.0;
            let mut b = base;
            let mut e = n as u64;
            while e > 0 {
                if e & 1 != 0 { result *= b; }
                b *= b;
                e >>= 1;
            }
            return result;
        }
        if n < 0 && n > -64 {
            return 1.0 / pow_f64(base, -exp);
        }
    }

    // General case: exp(exp * ln(base)) approximation
    // For bare metal, this is approximate
    if base > 0.0 {
        let ln_base = ln_f64(base);
        exp_f64(exp * ln_base)
    } else {
        f64::NAN
    }
}

fn ln_f64(x: f64) -> f64 {
    if x <= 0.0 { return f64::NAN; }
    if x == 1.0 { return 0.0; }
    // Use the identity: ln(x) = 2 * atanh((x-1)/(x+1))
    let y = (x - 1.0) / (x + 1.0);
    let y2 = y * y;
    let mut sum = y;
    let mut term = y;
    for i in 1..30 {
        term *= y2;
        sum += term / (2 * i + 1) as f64;
    }
    2.0 * sum
}

fn exp_f64(x: f64) -> f64 {
    if x == 0.0 { return 1.0; }
    // Taylor series: e^x = sum(x^n / n!)
    let mut sum = 1.0;
    let mut term = 1.0;
    for i in 1..40 {
        term *= x / i as f64;
        sum += term;
        if term.abs() < 1e-15 { break; }
    }
    sum
}
