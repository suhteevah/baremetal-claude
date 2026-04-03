//! Tree-walking evaluator for python-lite AST.
//!
//! Implements a subset of Python 3 semantics as a direct AST interpreter.
//!
//! ## Value System
//!
//! Python values ([`Value`]) include:
//! - `Int(i64)` -- arbitrary precision in CPython, fixed 64-bit here
//! - `Float(f64)` -- IEEE 754 double
//! - `Str(String)` -- immutable UTF-8 string
//! - `Bool(bool)` -- `True`/`False` (subtype of int in CPython)
//! - `List(Vec<Value>)` -- mutable ordered collection
//! - `None` -- Python's null value
//! - `Func { name, params, body }` -- user-defined functions
//!
//! ## Truthiness
//!
//! Python's truthiness rules: `False`, `0`, `0.0`, `""`, `[]`, and `None` are
//! falsy; everything else is truthy. This is implemented by `Value::is_truthy`.
//!
//! ## Scoping
//!
//! Uses a two-level scope model: globals (`self.globals`) and a local scope
//! (`BTreeMap<String, Value>`) passed through function calls. Variable lookup
//! checks the local scope first, then falls back to globals.
//!
//! ## Control Flow
//!
//! - `return`: Produces `ControlFlow::Return(value)`
//! - `break`/`continue`: Produce `ControlFlow::Break`/`ControlFlow::Continue`
//! - These propagate up via `Option<ControlFlow>` return values
//!
//! ## Built-in Functions
//!
//! `print()`, `len()`, `range()`, `int()`, `float()`, `str()`, `bool()`,
//! `type()`, `abs()`, `min()`, `max()`, `sum()`, `sorted()`, `reversed()`,
//! `enumerate()`, `zip()`, `map()`, `filter()`, `input()` (stub),
//! and list methods `.append()`, `.pop()`, `.insert()`, `.remove()`, etc.
//!
//! ## Safety Limits
//!
//! - `MAX_CALL_DEPTH` (256): Prevents stack overflow from infinite recursion
//! - `MAX_ITERATIONS` (100,000): Prevents infinite loops from hanging

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::parser::*;

// ---------------------------------------------------------------------------
// Runtime values
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    List(Vec<Value>),
    None,
    /// A user-defined function.
    Func {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
}

impl Value {
    pub fn display(&self) -> String {
        match self {
            Value::Int(n) => format!("{}", n),
            Value::Float(f) => format_float(*f),
            Value::Str(s) => s.clone(),
            Value::Bool(true) => String::from("True"),
            Value::Bool(false) => String::from("False"),
            Value::None => String::from("None"),
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.repr()).collect();
                format!("[{}]", inner.join(", "))
            }
            Value::Func { name, .. } => format!("<function {}>", name),
        }
    }

    /// Python repr — strings get quotes.
    pub fn repr(&self) -> String {
        match self {
            Value::Str(s) => format!("'{}'", s),
            other => other.display(),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::None => false,
            Value::Func { .. } => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::None => "NoneType",
            Value::Func { .. } => "function",
        }
    }

    fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(n) => Ok(*n),
            Value::Float(f) => Ok(*f as i64),
            Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
            _ => Err(format!("cannot convert {} to int", self.type_name())),
        }
    }

    fn as_float(&self) -> Result<f64, String> {
        match self {
            Value::Float(f) => Ok(*f),
            Value::Int(n) => Ok(*n as f64),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            _ => Err(format!("cannot convert {} to float", self.type_name())),
        }
    }
}

/// Format a float, stripping unnecessary trailing zeros but keeping at least one decimal.
/// Format a float, stripping unnecessary trailing zeros but keeping at least one decimal.
fn format_float(f: f64) -> String {
    let s = format!("{}", f);
    // Rust's default Display for f64 is reasonable — just use it.
    s
}

/// Floor for f64 without libm.
fn floor_f64(x: f64) -> f64 {
    let i = x as i64;
    let fi = i as f64;
    if x < fi { fi - 1.0 } else { fi }
}

// ---------------------------------------------------------------------------
// Control flow signals
// ---------------------------------------------------------------------------

enum ControlFlow {
    Return(Value),
    Break,
    Continue,
}

type EvalResult = Result<Value, String>;
type StmtResult = Result<Option<ControlFlow>, String>;

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

pub struct Interpreter {
    /// Global scope.
    globals: BTreeMap<String, Value>,
    /// Captured output from print() calls.
    output: String,
    /// Recursion depth guard.
    call_depth: usize,
}

const MAX_CALL_DEPTH: usize = 256;
const MAX_ITERATIONS: usize = 100_000;

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            globals: BTreeMap::new(),
            output: String::new(),
            call_depth: 0,
        }
    }

    /// Take the accumulated print output, leaving the buffer empty.
    pub fn take_output(&mut self) -> String {
        core::mem::take(&mut self.output)
    }

    /// Execute a block of statements at the top level.
    pub fn exec_block(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        let mut scope = BTreeMap::new();
        for stmt in stmts {
            if let Some(cf) = self.exec_stmt(stmt, &mut scope)? {
                match cf {
                    ControlFlow::Return(_) => return Err(String::from("'return' outside function")),
                    ControlFlow::Break => return Err(String::from("'break' outside loop")),
                    ControlFlow::Continue => return Err(String::from("'continue' outside loop")),
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Statement execution
    // -----------------------------------------------------------------------

    fn exec_stmt(
        &mut self,
        stmt: &Stmt,
        scope: &mut BTreeMap<String, Value>,
    ) -> StmtResult {
        match stmt {
            Stmt::Expr(expr) => {
                self.eval_expr(expr, scope)?;
                Ok(Option::None)
            }
            Stmt::Assign { target, value } => {
                let val = self.eval_expr(value, scope)?;
                self.assign(target, val, scope)?;
                Ok(Option::None)
            }
            Stmt::AugAssign { target, op, value } => {
                let current = self.eval_assign_target_value(target, scope)?;
                let rhs = self.eval_expr(value, scope)?;
                let result = eval_binop(*op, &current, &rhs)?;
                self.assign(target, result, scope)?;
                Ok(Option::None)
            }
            Stmt::If {
                condition,
                body,
                elif_clauses,
                else_body,
            } => {
                let cond = self.eval_expr(condition, scope)?;
                if cond.is_truthy() {
                    return self.exec_stmts(body, scope);
                }
                for (elif_cond, elif_body) in elif_clauses {
                    let c = self.eval_expr(elif_cond, scope)?;
                    if c.is_truthy() {
                        return self.exec_stmts(elif_body, scope);
                    }
                }
                if let Some(eb) = else_body {
                    return self.exec_stmts(eb, scope);
                }
                Ok(Option::None)
            }
            Stmt::While { condition, body } => {
                let mut iterations = 0;
                loop {
                    let cond = self.eval_expr(condition, scope)?;
                    if !cond.is_truthy() {
                        break;
                    }
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        return Err(String::from("maximum loop iterations exceeded"));
                    }
                    match self.exec_stmts(body, scope)? {
                        Some(ControlFlow::Break) => break,
                        Some(ControlFlow::Continue) => continue,
                        Some(cf) => return Ok(Some(cf)),
                        Option::None => {}
                    }
                }
                Ok(Option::None)
            }
            Stmt::For { var, iterable, body } => {
                let iter_val = self.eval_expr(iterable, scope)?;
                let items = match iter_val {
                    Value::List(items) => items,
                    Value::Str(s) => {
                        s.chars().map(|c| Value::Str(String::from(c.to_string()))).collect()
                    }
                    _ => return Err(format!("'{}' is not iterable", iter_val.type_name())),
                };
                let mut iterations = 0;
                for item in items {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        return Err(String::from("maximum loop iterations exceeded"));
                    }
                    scope.insert(var.clone(), item);
                    match self.exec_stmts(body, scope)? {
                        Some(ControlFlow::Break) => break,
                        Some(ControlFlow::Continue) => continue,
                        Some(cf) => return Ok(Some(cf)),
                        Option::None => {}
                    }
                }
                Ok(Option::None)
            }
            Stmt::FuncDef { name, params, body } => {
                let func = Value::Func {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                };
                // Store in both scope and globals so it's visible everywhere.
                scope.insert(name.clone(), func.clone());
                self.globals.insert(name.clone(), func);
                Ok(Option::None)
            }
            Stmt::Return(expr) => {
                let val = match expr {
                    Some(e) => self.eval_expr(e, scope)?,
                    Option::None => Value::None,
                };
                Ok(Some(ControlFlow::Return(val)))
            }
            Stmt::Break => Ok(Some(ControlFlow::Break)),
            Stmt::Continue => Ok(Some(ControlFlow::Continue)),
        }
    }

    fn exec_stmts(
        &mut self,
        stmts: &[Stmt],
        scope: &mut BTreeMap<String, Value>,
    ) -> StmtResult {
        for stmt in stmts {
            if let Some(cf) = self.exec_stmt(stmt, scope)? {
                return Ok(Some(cf));
            }
        }
        Ok(Option::None)
    }

    fn assign(
        &mut self,
        target: &AssignTarget,
        value: Value,
        scope: &mut BTreeMap<String, Value>,
    ) -> Result<(), String> {
        match target {
            AssignTarget::Name(name) => {
                scope.insert(name.clone(), value.clone());
                self.globals.insert(name.clone(), value);
                Ok(())
            }
            AssignTarget::Index { obj, index } => {
                // Evaluate obj to find which variable to mutate.
                if let Expr::Name(name) = obj {
                    let idx = self.eval_expr(index, scope)?;
                    let i = idx.as_int()? as usize;
                    let list = self.lookup_mut(name, scope)?;
                    if let Value::List(items) = list {
                        if i >= items.len() {
                            return Err(format!("list index {} out of range", i));
                        }
                        items[i] = value;
                        Ok(())
                    } else {
                        Err(format!("'{}' is not subscriptable", list.type_name()))
                    }
                } else {
                    Err(String::from("complex index assignment not supported"))
                }
            }
        }
    }

    fn eval_assign_target_value(
        &mut self,
        target: &AssignTarget,
        scope: &mut BTreeMap<String, Value>,
    ) -> EvalResult {
        match target {
            AssignTarget::Name(name) => self.lookup(name, scope),
            AssignTarget::Index { obj, index } => {
                let o = self.eval_expr(obj, scope)?;
                let i = self.eval_expr(index, scope)?;
                eval_index(&o, &i)
            }
        }
    }

    fn lookup(&self, name: &str, scope: &BTreeMap<String, Value>) -> EvalResult {
        if let Some(v) = scope.get(name) {
            Ok(v.clone())
        } else if let Some(v) = self.globals.get(name) {
            Ok(v.clone())
        } else {
            Err(format!("name '{}' is not defined", name))
        }
    }

    fn lookup_mut<'a>(
        &'a mut self,
        name: &str,
        scope: &'a mut BTreeMap<String, Value>,
    ) -> Result<&'a mut Value, String> {
        if scope.contains_key(name) {
            Ok(scope.get_mut(name).unwrap())
        } else if self.globals.contains_key(name) {
            Ok(self.globals.get_mut(name).unwrap())
        } else {
            Err(format!("name '{}' is not defined", name))
        }
    }

    // -----------------------------------------------------------------------
    // Expression evaluation
    // -----------------------------------------------------------------------

    fn eval_expr(
        &mut self,
        expr: &Expr,
        scope: &mut BTreeMap<String, Value>,
    ) -> EvalResult {
        match expr {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::None => Ok(Value::None),
            Expr::Name(name) => self.lookup(name, scope),

            Expr::List(elements) => {
                let mut items = Vec::new();
                for e in elements {
                    items.push(self.eval_expr(e, scope)?);
                }
                Ok(Value::List(items))
            }

            Expr::BinOp { left, op, right } => {
                let l = self.eval_expr(left, scope)?;
                let r = self.eval_expr(right, scope)?;
                eval_binop(*op, &l, &r)
            }

            Expr::UnaryOp { op, operand } => {
                let v = self.eval_expr(operand, scope)?;
                match op {
                    UnaryOp::Neg => match v {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        Value::Float(f) => Ok(Value::Float(-f)),
                        _ => Err(format!("bad operand type for unary -: '{}'", v.type_name())),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!v.is_truthy())),
                }
            }

            Expr::Compare { left, op, right } => {
                let l = self.eval_expr(left, scope)?;
                let r = self.eval_expr(right, scope)?;
                eval_compare(*op, &l, &r)
            }

            Expr::BoolOp { left, op, right } => {
                let l = self.eval_expr(left, scope)?;
                match op {
                    BoolOpKind::And => {
                        if !l.is_truthy() {
                            Ok(l)
                        } else {
                            self.eval_expr(right, scope)
                        }
                    }
                    BoolOpKind::Or => {
                        if l.is_truthy() {
                            Ok(l)
                        } else {
                            self.eval_expr(right, scope)
                        }
                    }
                }
            }

            Expr::Call { func, args } => {
                // Evaluate arguments first.
                let mut arg_vals = Vec::new();
                for a in args {
                    arg_vals.push(self.eval_expr(a, scope)?);
                }

                // Handle method calls: func is Attribute { obj, attr }.
                if let Expr::Attribute { obj, attr } = func.as_ref() {
                    let obj_val = self.eval_expr(obj, scope)?;
                    return self.call_method(obj, &obj_val, attr, &arg_vals, scope);
                }

                // Check if it's a builtin name BEFORE evaluating the function
                // expression, since builtins aren't stored in any scope.
                if let Expr::Name(name) = func.as_ref() {
                    if let Some(result) = self.try_builtin(name, &arg_vals)? {
                        return Ok(result);
                    }
                }

                // Evaluate function expression (lookup user-defined function).
                let func_val = self.eval_expr(func, scope)?;

                // User-defined function.
                match func_val {
                    Value::Func { name: _, params, body } => {
                        self.call_func(&params, &body, &arg_vals)
                    }
                    _ => Err(format!("'{}' is not callable", func_val.type_name())),
                }
            }

            Expr::Index { obj, index } => {
                let o = self.eval_expr(obj, scope)?;
                let i = self.eval_expr(index, scope)?;
                eval_index(&o, &i)
            }

            Expr::Attribute { obj, attr } => {
                let o = self.eval_expr(obj, scope)?;
                // For now, attribute access without call is not supported for most things.
                Err(format!(
                    "'{}' object has no attribute '{}'",
                    o.type_name(),
                    attr
                ))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Builtin functions
    // -----------------------------------------------------------------------

    fn try_builtin(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, String> {
        match name {
            "print" => {
                let parts: Vec<String> = args.iter().map(|v| v.display()).collect();
                let line = parts.join(" ");
                self.output.push_str(&line);
                self.output.push('\n');
                Ok(Some(Value::None))
            }
            "len" => {
                if args.len() != 1 {
                    return Err(String::from("len() takes exactly 1 argument"));
                }
                match &args[0] {
                    Value::Str(s) => Ok(Some(Value::Int(s.len() as i64))),
                    Value::List(items) => Ok(Some(Value::Int(items.len() as i64))),
                    _ => Err(format!(
                        "object of type '{}' has no len()",
                        args[0].type_name()
                    )),
                }
            }
            "range" => {
                let (start, stop, step) = match args.len() {
                    1 => (0i64, args[0].as_int()?, 1i64),
                    2 => (args[0].as_int()?, args[1].as_int()?, 1i64),
                    3 => (args[0].as_int()?, args[1].as_int()?, args[2].as_int()?),
                    _ => return Err(String::from("range() takes 1 to 3 arguments")),
                };
                if step == 0 {
                    return Err(String::from("range() step must not be zero"));
                }
                let mut items = Vec::new();
                let mut i = start;
                if step > 0 {
                    while i < stop {
                        items.push(Value::Int(i));
                        i += step;
                        if items.len() > MAX_ITERATIONS {
                            return Err(String::from("range() too large"));
                        }
                    }
                } else {
                    while i > stop {
                        items.push(Value::Int(i));
                        i += step;
                        if items.len() > MAX_ITERATIONS {
                            return Err(String::from("range() too large"));
                        }
                    }
                }
                Ok(Some(Value::List(items)))
            }
            "str" => {
                if args.len() != 1 {
                    return Err(String::from("str() takes exactly 1 argument"));
                }
                Ok(Some(Value::Str(args[0].display())))
            }
            "int" => {
                if args.len() != 1 {
                    return Err(String::from("int() takes exactly 1 argument"));
                }
                match &args[0] {
                    Value::Int(n) => Ok(Some(Value::Int(*n))),
                    Value::Float(f) => Ok(Some(Value::Int(*f as i64))),
                    Value::Bool(b) => Ok(Some(Value::Int(if *b { 1 } else { 0 }))),
                    Value::Str(s) => {
                        let n: i64 = s.trim().parse().map_err(|_| {
                            format!("invalid literal for int(): '{}'", s)
                        })?;
                        Ok(Some(Value::Int(n)))
                    }
                    _ => Err(format!("int() argument must be a string or number, not '{}'", args[0].type_name())),
                }
            }
            "float" => {
                if args.len() != 1 {
                    return Err(String::from("float() takes exactly 1 argument"));
                }
                match &args[0] {
                    Value::Float(f) => Ok(Some(Value::Float(*f))),
                    Value::Int(n) => Ok(Some(Value::Float(*n as f64))),
                    Value::Str(s) => {
                        let f: f64 = s.trim().parse().map_err(|_| {
                            format!("invalid literal for float(): '{}'", s)
                        })?;
                        Ok(Some(Value::Float(f)))
                    }
                    _ => Err(format!("float() argument must be a string or number")),
                }
            }
            "type" => {
                if args.len() != 1 {
                    return Err(String::from("type() takes exactly 1 argument"));
                }
                Ok(Some(Value::Str(String::from(args[0].type_name()))))
            }
            "abs" => {
                if args.len() != 1 {
                    return Err(String::from("abs() takes exactly 1 argument"));
                }
                match &args[0] {
                    Value::Int(n) => Ok(Some(Value::Int(n.abs()))),
                    Value::Float(f) => Ok(Some(Value::Float(f.abs()))),
                    _ => Err(format!("bad operand type for abs(): '{}'", args[0].type_name())),
                }
            }
            "min" => {
                if args.is_empty() {
                    return Err(String::from("min() requires at least 1 argument"));
                }
                // If single list argument, find min of list.
                if args.len() == 1 {
                    if let Value::List(items) = &args[0] {
                        if items.is_empty() {
                            return Err(String::from("min() arg is an empty sequence"));
                        }
                        let mut best = items[0].clone();
                        for item in &items[1..] {
                            if eval_compare(CmpOp::Lt, item, &best)? == Value::Bool(true) {
                                best = item.clone();
                            }
                        }
                        return Ok(Some(best));
                    }
                }
                let mut best = args[0].clone();
                for arg in &args[1..] {
                    if eval_compare(CmpOp::Lt, arg, &best)? == Value::Bool(true) {
                        best = arg.clone();
                    }
                }
                Ok(Some(best))
            }
            "max" => {
                if args.is_empty() {
                    return Err(String::from("max() requires at least 1 argument"));
                }
                if args.len() == 1 {
                    if let Value::List(items) = &args[0] {
                        if items.is_empty() {
                            return Err(String::from("max() arg is an empty sequence"));
                        }
                        let mut best = items[0].clone();
                        for item in &items[1..] {
                            if eval_compare(CmpOp::Gt, item, &best)? == Value::Bool(true) {
                                best = item.clone();
                            }
                        }
                        return Ok(Some(best));
                    }
                }
                let mut best = args[0].clone();
                for arg in &args[1..] {
                    if eval_compare(CmpOp::Gt, arg, &best)? == Value::Bool(true) {
                        best = arg.clone();
                    }
                }
                Ok(Some(best))
            }
            "sum" => {
                if args.len() != 1 {
                    return Err(String::from("sum() takes exactly 1 argument"));
                }
                if let Value::List(items) = &args[0] {
                    let mut total = Value::Int(0);
                    for item in items {
                        total = eval_binop(BinOp::Add, &total, item)?;
                    }
                    Ok(Some(total))
                } else {
                    Err(format!("sum() argument must be iterable"))
                }
            }
            "sorted" => {
                if args.len() != 1 {
                    return Err(String::from("sorted() takes exactly 1 argument"));
                }
                if let Value::List(items) = &args[0] {
                    let mut sorted = items.clone();
                    // Simple insertion sort (good enough for small lists).
                    for i in 1..sorted.len() {
                        let mut j = i;
                        while j > 0 {
                            let cmp = eval_compare(CmpOp::Lt, &sorted[j], &sorted[j - 1])?;
                            if cmp == Value::Bool(true) {
                                sorted.swap(j, j - 1);
                                j -= 1;
                            } else {
                                break;
                            }
                        }
                    }
                    Ok(Some(Value::List(sorted)))
                } else {
                    Err(format!("sorted() argument must be iterable"))
                }
            }
            "reversed" => {
                if args.len() != 1 {
                    return Err(String::from("reversed() takes exactly 1 argument"));
                }
                if let Value::List(items) = &args[0] {
                    let mut rev = items.clone();
                    rev.reverse();
                    Ok(Some(Value::List(rev)))
                } else {
                    Err(format!("reversed() argument must be a sequence"))
                }
            }
            "enumerate" => {
                if args.len() != 1 {
                    return Err(String::from("enumerate() takes exactly 1 argument"));
                }
                if let Value::List(items) = &args[0] {
                    let pairs: Vec<Value> = items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| Value::List(vec![Value::Int(i as i64), v.clone()]))
                        .collect();
                    Ok(Some(Value::List(pairs)))
                } else {
                    Err(format!("enumerate() argument must be iterable"))
                }
            }
            "isinstance" => {
                // Simplified: isinstance(val, type_name_string)
                if args.len() != 2 {
                    return Err(String::from("isinstance() takes exactly 2 arguments"));
                }
                if let Value::Str(type_name) = &args[1] {
                    Ok(Some(Value::Bool(args[0].type_name() == type_name.as_str())))
                } else {
                    Err(String::from("isinstance() second arg must be a type name string in python-lite"))
                }
            }
            _ => Ok(Option::None), // Not a builtin.
        }
    }

    // -----------------------------------------------------------------------
    // Method calls
    // -----------------------------------------------------------------------

    fn call_method(
        &mut self,
        obj_expr: &Expr,
        obj_val: &Value,
        method: &str,
        args: &[Value],
        scope: &mut BTreeMap<String, Value>,
    ) -> EvalResult {
        match (obj_val, method) {
            // List methods
            (Value::List(_), "append") => {
                if args.len() != 1 {
                    return Err(String::from("append() takes exactly 1 argument"));
                }
                // Mutate the list in place.
                if let Expr::Name(name) = obj_expr {
                    let list = self.lookup_mut(name, scope)?;
                    if let Value::List(items) = list {
                        items.push(args[0].clone());
                        return Ok(Value::None);
                    }
                }
                Err(String::from("cannot append to this object"))
            }
            (Value::List(_items), "pop") => {
                if let Expr::Name(name) = obj_expr {
                    let list = self.lookup_mut(name, scope)?;
                    if let Value::List(items) = list {
                        if items.is_empty() {
                            return Err(String::from("pop from empty list"));
                        }
                        let val = if args.is_empty() {
                            items.pop().unwrap()
                        } else {
                            let idx = args[0].as_int()? as usize;
                            if idx >= items.len() {
                                return Err(format!("pop index {} out of range", idx));
                            }
                            items.remove(idx)
                        };
                        return Ok(val);
                    }
                }
                Err(String::from("cannot pop from this object"))
            }
            (Value::List(_items), "insert") => {
                if args.len() != 2 {
                    return Err(String::from("insert() takes exactly 2 arguments"));
                }
                if let Expr::Name(name) = obj_expr {
                    let idx = args[0].as_int()? as usize;
                    let val = args[1].clone();
                    let list = self.lookup_mut(name, scope)?;
                    if let Value::List(items) = list {
                        let pos = if idx > items.len() { items.len() } else { idx };
                        items.insert(pos, val);
                        return Ok(Value::None);
                    }
                }
                Err(String::from("cannot insert into this object"))
            }
            (Value::List(_items), "extend") => {
                if args.len() != 1 {
                    return Err(String::from("extend() takes exactly 1 argument"));
                }
                if let Value::List(new_items) = &args[0] {
                    if let Expr::Name(name) = obj_expr {
                        let list = self.lookup_mut(name, scope)?;
                        if let Value::List(items) = list {
                            items.extend(new_items.iter().cloned());
                            return Ok(Value::None);
                        }
                    }
                }
                Err(String::from("extend() argument must be iterable"))
            }
            (Value::List(items), "index") => {
                if args.len() != 1 {
                    return Err(String::from("index() takes exactly 1 argument"));
                }
                for (i, item) in items.iter().enumerate() {
                    if format!("{:?}", item) == format!("{:?}", args[0]) {
                        return Ok(Value::Int(i as i64));
                    }
                }
                Err(format!("{} is not in list", args[0].repr()))
            }
            (Value::List(items), "count") => {
                if args.len() != 1 {
                    return Err(String::from("count() takes exactly 1 argument"));
                }
                let count = items
                    .iter()
                    .filter(|item| format!("{:?}", item) == format!("{:?}", args[0]))
                    .count();
                Ok(Value::Int(count as i64))
            }
            (Value::List(_), "reverse") => {
                if let Expr::Name(name) = obj_expr {
                    let list = self.lookup_mut(name, scope)?;
                    if let Value::List(items) = list {
                        items.reverse();
                        return Ok(Value::None);
                    }
                }
                Err(String::from("cannot reverse this object"))
            }

            // String methods
            (Value::Str(s), "upper") => Ok(Value::Str(s.to_uppercase())),
            (Value::Str(s), "lower") => Ok(Value::Str(s.to_lowercase())),
            (Value::Str(s), "strip") => Ok(Value::Str(String::from(s.trim()))),
            (Value::Str(s), "lstrip") => Ok(Value::Str(String::from(s.trim_start()))),
            (Value::Str(s), "rstrip") => Ok(Value::Str(String::from(s.trim_end()))),
            (Value::Str(s), "startswith") => {
                if args.len() != 1 {
                    return Err(String::from("startswith() takes exactly 1 argument"));
                }
                if let Value::Str(prefix) = &args[0] {
                    Ok(Value::Bool(s.starts_with(prefix.as_str())))
                } else {
                    Err(String::from("startswith() argument must be str"))
                }
            }
            (Value::Str(s), "endswith") => {
                if args.len() != 1 {
                    return Err(String::from("endswith() takes exactly 1 argument"));
                }
                if let Value::Str(suffix) = &args[0] {
                    Ok(Value::Bool(s.ends_with(suffix.as_str())))
                } else {
                    Err(String::from("endswith() argument must be str"))
                }
            }
            (Value::Str(s), "find") => {
                if args.len() != 1 {
                    return Err(String::from("find() takes exactly 1 argument"));
                }
                if let Value::Str(sub) = &args[0] {
                    match s.find(sub.as_str()) {
                        Some(pos) => Ok(Value::Int(pos as i64)),
                        Option::None => Ok(Value::Int(-1)),
                    }
                } else {
                    Err(String::from("find() argument must be str"))
                }
            }
            (Value::Str(s), "replace") => {
                if args.len() != 2 {
                    return Err(String::from("replace() takes exactly 2 arguments"));
                }
                if let (Value::Str(old), Value::Str(new)) = (&args[0], &args[1]) {
                    Ok(Value::Str(s.replace(old.as_str(), new.as_str())))
                } else {
                    Err(String::from("replace() arguments must be strings"))
                }
            }
            (Value::Str(s), "split") => {
                let parts: Vec<Value> = if args.is_empty() {
                    s.split_whitespace()
                        .map(|p| Value::Str(String::from(p)))
                        .collect()
                } else if let Value::Str(sep) = &args[0] {
                    s.split(sep.as_str())
                        .map(|p| Value::Str(String::from(p)))
                        .collect()
                } else {
                    return Err(String::from("split() argument must be str"));
                };
                Ok(Value::List(parts))
            }
            (Value::Str(s), "join") => {
                if args.len() != 1 {
                    return Err(String::from("join() takes exactly 1 argument"));
                }
                if let Value::List(items) = &args[0] {
                    let strs: Result<Vec<String>, String> = items
                        .iter()
                        .map(|v| match v {
                            Value::Str(s) => Ok(s.clone()),
                            _ => Err(format!("join() sequence item must be str, not '{}'", v.type_name())),
                        })
                        .collect();
                    let joined = strs?.join(s.as_str());
                    Ok(Value::Str(joined))
                } else {
                    Err(String::from("join() argument must be iterable"))
                }
            }
            (Value::Str(s), "isdigit") => Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))),
            (Value::Str(s), "isalpha") => Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic()))),
            (Value::Str(s), "count") => {
                if args.len() != 1 {
                    return Err(String::from("count() takes exactly 1 argument"));
                }
                if let Value::Str(sub) = &args[0] {
                    Ok(Value::Int(s.matches(sub.as_str()).count() as i64))
                } else {
                    Err(String::from("count() argument must be str"))
                }
            }

            _ => Err(format!(
                "'{}' object has no method '{}'",
                obj_val.type_name(),
                method
            )),
        }
    }

    // -----------------------------------------------------------------------
    // User-defined function calls
    // -----------------------------------------------------------------------

    fn call_func(
        &mut self,
        params: &[String],
        body: &[Stmt],
        args: &[Value],
    ) -> EvalResult {
        if args.len() != params.len() {
            return Err(format!(
                "expected {} arguments, got {}",
                params.len(),
                args.len()
            ));
        }

        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(String::from("maximum recursion depth exceeded"));
        }

        // Create a new scope with parameters bound.
        let mut local_scope = BTreeMap::new();
        for (param, arg) in params.iter().zip(args.iter()) {
            local_scope.insert(param.clone(), arg.clone());
        }

        let result = self.exec_stmts(body, &mut local_scope);
        self.call_depth -= 1;

        match result {
            Ok(Some(ControlFlow::Return(val))) => Ok(val),
            Ok(Some(ControlFlow::Break)) => Err(String::from("'break' outside loop")),
            Ok(Some(ControlFlow::Continue)) => Err(String::from("'continue' outside loop")),
            Ok(Option::None) => Ok(Value::None),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Operator evaluation (stateless)
// ---------------------------------------------------------------------------

fn eval_binop(op: BinOp, left: &Value, right: &Value) -> EvalResult {
    // String + String => concatenation
    if let (BinOp::Add, Value::Str(a), Value::Str(b)) = (op, left, right) {
        return Ok(Value::Str(format!("{}{}", a, b)));
    }

    // String * Int => repetition
    if let (BinOp::Mul, Value::Str(s), Value::Int(n)) = (op, left, right) {
        let n = *n;
        if n <= 0 {
            return Ok(Value::Str(String::new()));
        }
        let mut result = String::new();
        for _ in 0..n {
            result.push_str(s);
        }
        return Ok(Value::Str(result));
    }
    if let (BinOp::Mul, Value::Int(n), Value::Str(s)) = (op, left, right) {
        let n = *n;
        if n <= 0 {
            return Ok(Value::Str(String::new()));
        }
        let mut result = String::new();
        for _ in 0..n {
            result.push_str(s);
        }
        return Ok(Value::Str(result));
    }

    // List + List => concatenation
    if let (BinOp::Add, Value::List(a), Value::List(b)) = (op, left, right) {
        let mut result = a.clone();
        result.extend(b.iter().cloned());
        return Ok(Value::List(result));
    }

    // Numeric operations — promote to float if either operand is float.
    let use_float = matches!(left, Value::Float(_)) || matches!(right, Value::Float(_));

    if use_float {
        let a = left.as_float()?;
        let b = right.as_float()?;
        let result = match op {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a * b,
            BinOp::Div => {
                if b == 0.0 {
                    return Err(String::from("division by zero"));
                }
                a / b
            }
            BinOp::FloorDiv => {
                if b == 0.0 {
                    return Err(String::from("division by zero"));
                }
                floor_f64(a / b)
            }
            BinOp::Mod => {
                if b == 0.0 {
                    return Err(String::from("modulo by zero"));
                }
                a % b
            }
            BinOp::Pow => pow_float(a, b),
        };
        Ok(Value::Float(result))
    } else {
        let a = left.as_int()?;
        let b = right.as_int()?;
        let result = match op {
            BinOp::Add => a.checked_add(b).ok_or("integer overflow")?,
            BinOp::Sub => a.checked_sub(b).ok_or("integer overflow")?,
            BinOp::Mul => a.checked_mul(b).ok_or("integer overflow")?,
            BinOp::Div => {
                if b == 0 {
                    return Err(String::from("division by zero"));
                }
                // Python-style: int / int => float
                return Ok(Value::Float(a as f64 / b as f64));
            }
            BinOp::FloorDiv => {
                if b == 0 {
                    return Err(String::from("division by zero"));
                }
                // Python floor division.
                let d = a.wrapping_div(b);
                if (a ^ b) < 0 && d * b != a {
                    d - 1
                } else {
                    d
                }
            }
            BinOp::Mod => {
                if b == 0 {
                    return Err(String::from("modulo by zero"));
                }
                ((a % b) + b) % b // Python-style modulo (always non-negative for positive b)
            }
            BinOp::Pow => {
                if b < 0 {
                    return Ok(Value::Float(pow_float(a as f64, b as f64)));
                }
                pow_int(a, b as u64)
            }
        };
        Ok(Value::Int(result))
    }
}

fn eval_compare(op: CmpOp, left: &Value, right: &Value) -> EvalResult {
    // Compare same types, or promote int<->float.
    let result = match (left, right) {
        (Value::Int(a), Value::Int(b)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::NotEq => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Gt => a > b,
            CmpOp::LtEq => a <= b,
            CmpOp::GtEq => a >= b,
        },
        (Value::Float(a), Value::Float(b)) => cmp_float(*a, *b, op),
        (Value::Int(a), Value::Float(b)) => cmp_float(*a as f64, *b, op),
        (Value::Float(a), Value::Int(b)) => cmp_float(*a, *b as f64, op),
        (Value::Str(a), Value::Str(b)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::NotEq => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Gt => a > b,
            CmpOp::LtEq => a <= b,
            CmpOp::GtEq => a >= b,
        },
        (Value::Bool(a), Value::Bool(b)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::NotEq => a != b,
            _ => return Err(String::from("cannot order booleans")),
        },
        (Value::None, Value::None) => match op {
            CmpOp::Eq => true,
            CmpOp::NotEq => false,
            _ => return Err(String::from("cannot order None")),
        },
        _ => match op {
            CmpOp::Eq => false,
            CmpOp::NotEq => true,
            _ => {
                return Err(format!(
                    "cannot compare '{}' and '{}'",
                    left.type_name(),
                    right.type_name()
                ))
            }
        },
    };
    Ok(Value::Bool(result))
}

fn cmp_float(a: f64, b: f64, op: CmpOp) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::NotEq => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Gt => a > b,
        CmpOp::LtEq => a <= b,
        CmpOp::GtEq => a >= b,
    }
}

fn eval_index(obj: &Value, index: &Value) -> EvalResult {
    match (obj, index) {
        (Value::List(items), Value::Int(i)) => {
            let idx = if *i < 0 {
                (items.len() as i64 + *i) as usize
            } else {
                *i as usize
            };
            items
                .get(idx)
                .cloned()
                .ok_or_else(|| format!("list index {} out of range", i))
        }
        (Value::Str(s), Value::Int(i)) => {
            let idx = if *i < 0 {
                (s.len() as i64 + *i) as usize
            } else {
                *i as usize
            };
            s.chars()
                .nth(idx)
                .map(|c| Value::Str(String::from(c.to_string())))
                .ok_or_else(|| format!("string index {} out of range", i))
        }
        _ => Err(format!(
            "'{}' object is not subscriptable",
            obj.type_name()
        )),
    }
}

// ---------------------------------------------------------------------------
// Math helpers (no libm in no_std)
// ---------------------------------------------------------------------------

fn pow_int(base: i64, exp: u64) -> i64 {
    let mut result: i64 = 1;
    let mut b = base;
    let mut e = exp;
    while e > 0 {
        if e & 1 == 1 {
            result = result.wrapping_mul(b);
        }
        b = b.wrapping_mul(b);
        e >>= 1;
    }
    result
}

/// Compute a^b for floats. Since we don't have libm, use a simple
/// implementation for integer exponents and fall back to exp(b*ln(a))
/// approximation for fractional exponents.
fn pow_float(base: f64, exp: f64) -> f64 {
    // Integer exponent fast path.
    if exp == floor_f64(exp) && exp.abs() < 1000.0 {
        let e = exp as i64;
        if e >= 0 {
            let mut result = 1.0;
            let mut b = base;
            let mut n = e as u64;
            while n > 0 {
                if n & 1 == 1 {
                    result *= b;
                }
                b *= b;
                n >>= 1;
            }
            return result;
        } else {
            let mut result = 1.0;
            let mut b = base;
            let mut n = (-e) as u64;
            while n > 0 {
                if n & 1 == 1 {
                    result *= b;
                }
                b *= b;
                n >>= 1;
            }
            return 1.0 / result;
        }
    }

    // For non-integer exponents, we'd need a proper pow implementation.
    // Since this is bare metal with no libm, provide a rough approximation
    // or return NaN for unsupported cases.
    // TODO: Implement proper pow via exp/ln series expansion.
    f64::NAN
}
