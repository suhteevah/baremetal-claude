//! Tree-walking JavaScript evaluator for js-lite.
//!
//! Evaluates the AST produced by the parser. Supports:
//! - Variables, scoping (function scope + block scope for let/const)
//! - Functions (declarations, expressions, arrows, closures)
//! - Objects and arrays with methods
//! - String/Math/Array/Object builtins
//! - parseInt, parseFloat, encodeURIComponent, decodeURIComponent
//! - btoa, atob (base64)
//! - setTimeout/setInterval stubs (execute immediately)
//! - document.cookie (get/set)
//! - Regex .test() and .exec() (basic)
//! - Bitwise operators
//! - try/catch/finally, throw
//! - for-in, for-of
//! - switch/case
//! - typeof, instanceof, delete, void

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::parser::*;

// ---------------------------------------------------------------------------
// Value type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Value {
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
    Function(JsFunction),
    Regex(String, String),
}

#[derive(Debug, Clone)]
pub struct JsFunction {
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    /// Captured scope for closures (simplified: just variable names and values)
    pub closure: BTreeMap<String, Value>,
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Bool(b) => *b,
            Value::Null | Value::Undefined => false,
            Value::Array(_) | Value::Object(_) | Value::Function(_) | Value::Regex(..) => true,
        }
    }

    pub fn to_number(&self) -> f64 {
        match self {
            Value::Number(n) => *n,
            Value::Str(s) => {
                let s = s.trim();
                if s.is_empty() { return 0.0; }
                if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    u64::from_str_radix(rest, 16).map(|v| v as f64).unwrap_or(f64::NAN)
                } else {
                    parse_float_simple(s)
                }
            }
            Value::Bool(true) => 1.0,
            Value::Bool(false) => 0.0,
            Value::Null => 0.0,
            Value::Undefined => f64::NAN,
            _ => f64::NAN,
        }
    }

    pub fn to_string_val(&self) -> String {
        match self {
            Value::Number(n) => format_number(*n),
            Value::Str(s) => s.clone(),
            Value::Bool(true) => String::from("true"),
            Value::Bool(false) => String::from("false"),
            Value::Null => String::from("null"),
            Value::Undefined => String::from("undefined"),
            Value::Array(arr) => {
                let parts: Vec<String> = arr.iter().map(|v| v.to_string_val()).collect();
                parts.join(",")
            }
            Value::Object(_) => String::from("[object Object]"),
            Value::Function(f) => {
                format!("function {}() {{ [native code] }}", f.name.as_deref().unwrap_or("anonymous"))
            }
            Value::Regex(p, f) => format!("/{}/{}", p, f),
        }
    }

    pub fn to_i32(&self) -> i32 {
        let n = self.to_number();
        if n.is_nan() || n.is_infinite() { return 0; }
        n as i64 as i32
    }

    pub fn to_u32(&self) -> u32 {
        let n = self.to_number();
        if n.is_nan() || n.is_infinite() { return 0; }
        n as i64 as u32
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Number(_) => "number",
            Value::Str(_) => "string",
            Value::Bool(_) => "boolean",
            Value::Null => "object", // yes, typeof null === "object" in JS
            Value::Undefined => "undefined",
            Value::Array(_) => "object",
            Value::Object(_) => "object",
            Value::Function(_) => "function",
            Value::Regex(..) => "object",
        }
    }

    pub fn strict_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Undefined, Value::Undefined) => true,
            _ => false,
        }
    }

    pub fn loose_eq(&self, other: &Value) -> bool {
        if self.strict_eq(other) {
            return true;
        }
        match (self, other) {
            (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
            (Value::Number(a), Value::Str(b)) => *a == parse_float_simple(b),
            (Value::Str(a), Value::Number(b)) => parse_float_simple(a) == *b,
            (Value::Bool(_), _) => Value::Number(self.to_number()).loose_eq(other),
            (_, Value::Bool(_)) => self.loose_eq(&Value::Number(other.to_number())),
            _ => false,
        }
    }
}

fn char_to_string(c: char) -> String {
    let mut s = String::new();
    s.push(c);
    s
}

fn format_number(n: f64) -> String {
    if n.is_nan() { return String::from("NaN"); }
    if n.is_infinite() {
        return if n > 0.0 { String::from("Infinity") } else { String::from("-Infinity") };
    }
    if n == 0.0 {
        return String::from("0");
    }
    // Check if it's an integer
    if n == (n as i64 as f64) && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    format!("{}", n)
}

fn parse_float_simple(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() { return f64::NAN; }
    if s == "Infinity" || s == "+Infinity" { return f64::INFINITY; }
    if s == "-Infinity" { return f64::NEG_INFINITY; }

    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    // Hex
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return match u64::from_str_radix(hex, 16) {
            Ok(v) => if neg { -(v as f64) } else { v as f64 },
            Err(_) => f64::NAN,
        };
    }

    let (mantissa_str, exp_str) = if let Some(pos) = s.find(|c: char| c == 'e' || c == 'E') {
        (&s[..pos], Some(&s[pos + 1..]))
    } else {
        (s, None)
    };

    let (int_part, frac_part) = if let Some(dot_pos) = mantissa_str.find('.') {
        (&mantissa_str[..dot_pos], Some(&mantissa_str[dot_pos + 1..]))
    } else {
        (mantissa_str, None)
    };

    let mut val: f64 = 0.0;
    for b in int_part.bytes() {
        if b.is_ascii_digit() {
            val = val * 10.0 + (b - b'0') as f64;
        } else {
            return f64::NAN;
        }
    }

    if let Some(frac) = frac_part {
        let mut mul = 0.1;
        for b in frac.bytes() {
            if b.is_ascii_digit() {
                val += (b - b'0') as f64 * mul;
                mul *= 0.1;
            } else {
                break;
            }
        }
    }

    if let Some(exp) = exp_str {
        let (exp_neg, exp_digits) = if let Some(rest) = exp.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = exp.strip_prefix('+') {
            (false, rest)
        } else {
            (false, exp)
        };
        let mut exp_val: i32 = 0;
        for b in exp_digits.bytes() {
            if b.is_ascii_digit() {
                exp_val = exp_val * 10 + (b - b'0') as i32;
            } else { break; }
        }
        if exp_neg { exp_val = -exp_val; }
        val *= pow10(exp_val);
    }

    if neg { -val } else { val }
}

fn pow10(exp: i32) -> f64 {
    if exp >= 0 {
        let mut r = 1.0;
        for _ in 0..exp { r *= 10.0; }
        r
    } else {
        let mut r = 1.0;
        for _ in 0..(-exp) { r /= 10.0; }
        r
    }
}

// ---------------------------------------------------------------------------
// Control flow signals
// ---------------------------------------------------------------------------

pub(crate) enum Signal {
    None,
    Return(Value),
    Break,
    Continue,
    Throw(Value),
}

// ---------------------------------------------------------------------------
// Environment (scope chain)
// ---------------------------------------------------------------------------

struct Scope {
    vars: BTreeMap<String, Value>,
}

impl Scope {
    fn new() -> Self {
        Self { vars: BTreeMap::new() }
    }
}

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

pub struct Interpreter {
    scopes: Vec<Scope>,
    output: String,
    /// document.cookie value
    pub cookie: String,
    /// Call depth for stack overflow protection
    call_depth: usize,
    /// Simple RNG state for Math.random()
    rng_state: u64,
}

const MAX_CALL_DEPTH: usize = 256;
const MAX_LOOP_ITERATIONS: usize = 1_000_000;

impl Interpreter {
    pub fn new() -> Self {
        let mut interp = Self {
            scopes: vec![Scope::new()],
            output: String::new(),
            cookie: String::new(),
            call_depth: 0,
            rng_state: 0x12345678_ABCDEF01,
        };
        interp.init_globals();
        interp
    }

    pub fn take_output(&mut self) -> String {
        core::mem::take(&mut self.output)
    }

    pub fn get_cookie(&self) -> &str {
        &self.cookie
    }

    /// Take a variable from the global scope (removing it). Used for context setup.
    pub fn take_var(&mut self, name: &str) -> Option<Value> {
        if let Some(scope) = self.scopes.first_mut() {
            scope.vars.remove(name)
        } else {
            None
        }
    }

    /// Set a variable in the global (bottom) scope.
    pub fn set_global(&mut self, name: &str, val: Value) {
        if let Some(scope) = self.scopes.first_mut() {
            scope.vars.insert(String::from(name), val);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn set_var(&mut self, name: &str, val: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.vars.insert(String::from(name), val);
        }
    }

    fn get_var(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.vars.get(name) {
                return Some(val);
            }
        }
        None
    }

    fn update_var(&mut self, name: &str, val: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.vars.contains_key(name) {
                scope.vars.insert(String::from(name), val);
                return true;
            }
        }
        false
    }

    fn init_globals(&mut self) {
        self.set_var("NaN", Value::Number(f64::NAN));
        self.set_var("Infinity", Value::Number(f64::INFINITY));

        // Math object
        let mut math = BTreeMap::new();
        math.insert(String::from("PI"), Value::Number(core::f64::consts::PI));
        math.insert(String::from("E"), Value::Number(core::f64::consts::E));
        math.insert(String::from("LN2"), Value::Number(core::f64::consts::LN_2));
        math.insert(String::from("LN10"), Value::Number(core::f64::consts::LN_10));
        math.insert(String::from("SQRT2"), Value::Number(core::f64::consts::SQRT_2));
        math.insert(String::from("LOG2E"), Value::Number(core::f64::consts::LOG2_E));
        math.insert(String::from("LOG10E"), Value::Number(core::f64::consts::LOG10_E));
        self.set_var("Math", Value::Object(math));

        self.set_var("console", Value::Object(BTreeMap::new()));

        // document with cookie
        let mut doc = BTreeMap::new();
        doc.insert(String::from("cookie"), Value::Str(String::new()));
        self.set_var("document", Value::Object(doc));

        self.set_var("window", Value::Object(BTreeMap::new()));

        let mut nav = BTreeMap::new();
        nav.insert(String::from("userAgent"), Value::Str(String::from(
            "Mozilla/5.0 (X86_64; ClaudioOS) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36"
        )));
        nav.insert(String::from("language"), Value::Str(String::from("en-US")));
        nav.insert(String::from("cookieEnabled"), Value::Bool(true));
        self.set_var("navigator", Value::Object(nav));

        let mut loc = BTreeMap::new();
        loc.insert(String::from("href"), Value::Str(String::new()));
        loc.insert(String::from("hostname"), Value::Str(String::new()));
        loc.insert(String::from("pathname"), Value::Str(String::from("/")));
        loc.insert(String::from("protocol"), Value::Str(String::from("https:")));
        loc.insert(String::from("search"), Value::Str(String::new()));
        loc.insert(String::from("hash"), Value::Str(String::new()));
        self.set_var("location", Value::Object(loc));

        let mut screen = BTreeMap::new();
        screen.insert(String::from("width"), Value::Number(1920.0));
        screen.insert(String::from("height"), Value::Number(1080.0));
        self.set_var("screen", Value::Object(screen));
    }

    fn next_random(&mut self) -> f64 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        (self.rng_state as f64) / (u64::MAX as f64)
    }

    // -------------------------------------------------------------------
    // Statement execution
    // -------------------------------------------------------------------

    pub(crate) fn exec_block(&mut self, stmts: &[Stmt]) -> Result<Signal, String> {
        for stmt in stmts {
            let signal = self.exec_stmt(stmt)?;
            match signal {
                Signal::None => {}
                other => return Ok(other),
            }
        }
        Ok(Signal::None)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Signal, String> {
        match stmt {
            Stmt::Empty => Ok(Signal::None),
            Stmt::Expr(expr) => {
                self.eval_expr(expr)?;
                Ok(Signal::None)
            }
            Stmt::VarDecl(_, decls) => {
                for (name, init) in decls {
                    let val = if let Some(expr) = init {
                        self.eval_expr(expr)?
                    } else {
                        Value::Undefined
                    };
                    self.set_var(name, val);
                }
                Ok(Signal::None)
            }
            Stmt::Block(stmts) => {
                self.push_scope();
                let result = self.exec_block(stmts);
                self.pop_scope();
                result
            }
            Stmt::If(cond, then, else_branch) => {
                let val = self.eval_expr(cond)?;
                if val.is_truthy() {
                    self.exec_stmt(then)
                } else if let Some(el) = else_branch {
                    self.exec_stmt(el)
                } else {
                    Ok(Signal::None)
                }
            }
            Stmt::While(cond, body) => {
                let mut iterations = 0;
                loop {
                    let val = self.eval_expr(cond)?;
                    if !val.is_truthy() { break; }
                    let signal = self.exec_stmt(body)?;
                    match signal {
                        Signal::Break => break,
                        Signal::Continue => {}
                        Signal::Return(v) => return Ok(Signal::Return(v)),
                        Signal::Throw(v) => return Ok(Signal::Throw(v)),
                        Signal::None => {}
                    }
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        return Err(String::from("infinite loop detected"));
                    }
                }
                Ok(Signal::None)
            }
            Stmt::DoWhile(body, cond) => {
                let mut iterations = 0;
                loop {
                    let signal = self.exec_stmt(body)?;
                    match signal {
                        Signal::Break => break,
                        Signal::Continue => {}
                        Signal::Return(v) => return Ok(Signal::Return(v)),
                        Signal::Throw(v) => return Ok(Signal::Throw(v)),
                        Signal::None => {}
                    }
                    let val = self.eval_expr(cond)?;
                    if !val.is_truthy() { break; }
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        return Err(String::from("infinite loop detected"));
                    }
                }
                Ok(Signal::None)
            }
            Stmt::For(init, cond, update, body) => {
                self.push_scope();
                if let Some(init_stmt) = init {
                    self.exec_stmt(init_stmt)?;
                }
                let mut iterations = 0;
                loop {
                    if let Some(cond_expr) = cond {
                        let val = self.eval_expr(cond_expr)?;
                        if !val.is_truthy() { break; }
                    }
                    let signal = self.exec_stmt(body)?;
                    match signal {
                        Signal::Break => break,
                        Signal::Continue => {}
                        Signal::Return(v) => { self.pop_scope(); return Ok(Signal::Return(v)); }
                        Signal::Throw(v) => { self.pop_scope(); return Ok(Signal::Throw(v)); }
                        Signal::None => {}
                    }
                    if let Some(update_expr) = update {
                        self.eval_expr(update_expr)?;
                    }
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        self.pop_scope();
                        return Err(String::from("infinite loop detected"));
                    }
                }
                self.pop_scope();
                Ok(Signal::None)
            }
            Stmt::ForIn(_, name, obj_expr, body) => {
                let obj = self.eval_expr(obj_expr)?;
                match obj {
                    Value::Object(map) => {
                        let keys: Vec<String> = map.keys().cloned().collect();
                        for key in keys {
                            self.set_var(name, Value::Str(key));
                            let signal = self.exec_stmt(body)?;
                            match signal {
                                Signal::Break => break,
                                Signal::Continue => continue,
                                Signal::Return(v) => return Ok(Signal::Return(v)),
                                Signal::Throw(v) => return Ok(Signal::Throw(v)),
                                Signal::None => {}
                            }
                        }
                    }
                    Value::Array(arr) => {
                        for i in 0..arr.len() {
                            self.set_var(name, Value::Number(i as f64));
                            let signal = self.exec_stmt(body)?;
                            match signal {
                                Signal::Break => break,
                                Signal::Continue => continue,
                                Signal::Return(v) => return Ok(Signal::Return(v)),
                                Signal::Throw(v) => return Ok(Signal::Throw(v)),
                                Signal::None => {}
                            }
                        }
                    }
                    _ => {}
                }
                Ok(Signal::None)
            }
            Stmt::ForOf(_, name, iter_expr, body) => {
                let iter = self.eval_expr(iter_expr)?;
                let items: Vec<Value> = match iter {
                    Value::Array(arr) => arr,
                    Value::Str(s) => s.chars().map(|c| Value::Str(char_to_string(c))).collect(),
                    _ => vec![],
                };
                for item in items {
                    self.set_var(name, item);
                    let signal = self.exec_stmt(body)?;
                    match signal {
                        Signal::Break => break,
                        Signal::Continue => continue,
                        Signal::Return(v) => return Ok(Signal::Return(v)),
                        Signal::Throw(v) => return Ok(Signal::Throw(v)),
                        Signal::None => {}
                    }
                }
                Ok(Signal::None)
            }
            Stmt::FunctionDecl(name, params, body_stmts) => {
                let closure = self.capture_scope();
                let func = Value::Function(JsFunction {
                    name: Some(name.clone()),
                    params: params.clone(),
                    body: body_stmts.clone(),
                    closure,
                });
                self.set_var(name, func);
                Ok(Signal::None)
            }
            Stmt::Return(expr) => {
                let val = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    Value::Undefined
                };
                Ok(Signal::Return(val))
            }
            Stmt::Break => Ok(Signal::Break),
            Stmt::Continue => Ok(Signal::Continue),
            Stmt::Switch(disc, cases) => {
                let disc_val = self.eval_expr(disc)?;
                let mut matched = false;
                let mut fell_through = false;

                for case in cases {
                    if !matched && !fell_through {
                        if let Some(test) = &case.test {
                            let test_val = self.eval_expr(test)?;
                            if disc_val.strict_eq(&test_val) {
                                matched = true;
                            }
                        }
                    }

                    if matched || fell_through {
                        let signal = self.exec_block(&case.body)?;
                        match signal {
                            Signal::Break => return Ok(Signal::None),
                            Signal::Return(v) => return Ok(Signal::Return(v)),
                            Signal::Throw(v) => return Ok(Signal::Throw(v)),
                            Signal::Continue => return Ok(Signal::Continue),
                            Signal::None => { fell_through = true; }
                        }
                    }
                }

                if !matched && !fell_through {
                    for case in cases {
                        if case.test.is_none() {
                            let signal = self.exec_block(&case.body)?;
                            match signal {
                                Signal::Break => return Ok(Signal::None),
                                other => return Ok(other),
                            }
                        }
                    }
                }

                Ok(Signal::None)
            }
            Stmt::TryCatch(try_body, catch, finally) => {
                let result = self.exec_block(try_body);

                // Determine if we need to run the catch block
                let needs_catch = match &result {
                    Ok(Signal::Throw(_)) => true,
                    Err(_) => true,
                    _ => false,
                };

                let signal = if needs_catch && catch.is_some() {
                    let (param, catch_body) = catch.as_ref().unwrap();
                    self.push_scope();
                    let thrown = match result {
                        Ok(Signal::Throw(v)) => v,
                        Err(e) => Value::Str(e),
                        _ => Value::Undefined,
                    };
                    if let Some(name) = param {
                        self.set_var(name, thrown);
                    }
                    let sig = self.exec_block(catch_body);
                    self.pop_scope();
                    sig?
                } else {
                    match result {
                        Ok(s) => s,
                        Err(e) => {
                            if let Some(finally_body) = finally {
                                let _ = self.exec_block(finally_body);
                            }
                            return Err(e);
                        }
                    }
                };

                if let Some(finally_body) = finally {
                    let _ = self.exec_block(finally_body);
                }

                Ok(signal)
            }
            Stmt::Throw(expr) => {
                let val = self.eval_expr(expr)?;
                Ok(Signal::Throw(val))
            }
        }
    }

    fn capture_scope(&self) -> BTreeMap<String, Value> {
        let mut captured = BTreeMap::new();
        for scope in &self.scopes {
            for (k, v) in &scope.vars {
                captured.insert(k.clone(), v.clone());
            }
        }
        captured
    }

    // -------------------------------------------------------------------
    // Expression evaluation
    // -------------------------------------------------------------------

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Undefined => Ok(Value::Undefined),
            Expr::This => Ok(Value::Undefined),

            Expr::Ident(name) => {
                match self.get_var(name) {
                    Some(v) => Ok(v.clone()),
                    None => Ok(Value::Undefined),
                }
            }

            Expr::TemplateLiteral(parts) => {
                let mut result = String::new();
                for part in parts {
                    match part {
                        TemplateExprPart::Str(s) => result.push_str(s),
                        TemplateExprPart::Expr(e) => {
                            let val = self.eval_expr(e)?;
                            result.push_str(&val.to_string_val());
                        }
                    }
                }
                Ok(Value::Str(result))
            }

            Expr::Array(elements) => {
                let mut arr = Vec::new();
                for el in elements {
                    if let Expr::Spread(inner) = el {
                        let val = self.eval_expr(inner)?;
                        if let Value::Array(items) = val {
                            arr.extend(items);
                        } else {
                            arr.push(val);
                        }
                    } else {
                        arr.push(self.eval_expr(el)?);
                    }
                }
                Ok(Value::Array(arr))
            }

            Expr::Object(props) => {
                let mut map = BTreeMap::new();
                for (key, val_expr) in props {
                    let key_str = match key {
                        PropKey::Ident(n) => n.clone(),
                        PropKey::Str(s) => s.clone(),
                        PropKey::Number(n) => format_number(*n),
                        PropKey::Computed(expr) => {
                            let v = self.eval_expr(expr)?;
                            v.to_string_val()
                        }
                    };
                    let val = self.eval_expr(val_expr)?;
                    map.insert(key_str, val);
                }
                Ok(Value::Object(map))
            }

            Expr::Binary(left, op, right) => {
                match op {
                    BinOp::And => {
                        let l = self.eval_expr(left)?;
                        if !l.is_truthy() { return Ok(l); }
                        return self.eval_expr(right);
                    }
                    BinOp::Or => {
                        let l = self.eval_expr(left)?;
                        if l.is_truthy() { return Ok(l); }
                        return self.eval_expr(right);
                    }
                    BinOp::NullishCoalesce => {
                        let l = self.eval_expr(left)?;
                        if !matches!(l, Value::Null | Value::Undefined) { return Ok(l); }
                        return self.eval_expr(right);
                    }
                    _ => {}
                }

                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;
                self.eval_binop(&l, *op, &r)
            }

            Expr::Unary(op, inner) => {
                let val = self.eval_expr(inner)?;
                match op {
                    UnaryOp::Neg => Ok(Value::Number(-val.to_number())),
                    UnaryOp::Pos => Ok(Value::Number(val.to_number())),
                    UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
                    UnaryOp::BitNot => Ok(Value::Number((!val.to_i32()) as f64)),
                    UnaryOp::Typeof => Ok(Value::Str(String::from(val.type_of()))),
                    UnaryOp::Void => Ok(Value::Undefined),
                    UnaryOp::Delete => Ok(Value::Bool(true)),
                }
            }

            Expr::Typeof(inner) => {
                let val = self.eval_expr(inner)?;
                Ok(Value::Str(String::from(val.type_of())))
            }

            Expr::Void(inner) => {
                self.eval_expr(inner)?;
                Ok(Value::Undefined)
            }

            Expr::Delete(inner) => {
                self.eval_expr(inner)?;
                Ok(Value::Bool(true))
            }

            Expr::Postfix(inner, op) => {
                let val = self.eval_expr(inner)?;
                let num = val.to_number();
                let new_val = match op {
                    PostfixOp::Inc => Value::Number(num + 1.0),
                    PostfixOp::Dec => Value::Number(num - 1.0),
                };
                self.assign_to(inner, new_val)?;
                Ok(Value::Number(num))
            }

            Expr::Assign(target, op, value) => {
                let rhs = self.eval_expr(value)?;
                let final_val = match op {
                    AssignOp::Assign => rhs,
                    _ => {
                        let lhs = self.eval_expr(target)?;
                        match op {
                            AssignOp::AddAssign => self.eval_binop(&lhs, BinOp::Add, &rhs)?,
                            AssignOp::SubAssign => self.eval_binop(&lhs, BinOp::Sub, &rhs)?,
                            AssignOp::MulAssign => self.eval_binop(&lhs, BinOp::Mul, &rhs)?,
                            AssignOp::DivAssign => self.eval_binop(&lhs, BinOp::Div, &rhs)?,
                            AssignOp::ModAssign => self.eval_binop(&lhs, BinOp::Mod, &rhs)?,
                            AssignOp::BitAndAssign => self.eval_binop(&lhs, BinOp::BitAnd, &rhs)?,
                            AssignOp::BitOrAssign => self.eval_binop(&lhs, BinOp::BitOr, &rhs)?,
                            AssignOp::BitXorAssign => self.eval_binop(&lhs, BinOp::BitXor, &rhs)?,
                            AssignOp::ShlAssign => self.eval_binop(&lhs, BinOp::Shl, &rhs)?,
                            AssignOp::ShrAssign => self.eval_binop(&lhs, BinOp::Shr, &rhs)?,
                            AssignOp::UshrAssign => self.eval_binop(&lhs, BinOp::Ushr, &rhs)?,
                            AssignOp::Assign => unreachable!(),
                        }
                    }
                };
                self.assign_to(target, final_val.clone())?;
                Ok(final_val)
            }

            Expr::Member(obj, prop) => {
                let obj_val = self.eval_expr(obj)?;
                self.get_property(&obj_val, prop)
            }

            Expr::OptionalMember(obj, prop) => {
                let obj_val = self.eval_expr(obj)?;
                if matches!(obj_val, Value::Null | Value::Undefined) {
                    return Ok(Value::Undefined);
                }
                self.get_property(&obj_val, prop)
            }

            Expr::Index(obj, idx) => {
                let obj_val = self.eval_expr(obj)?;
                let idx_val = self.eval_expr(idx)?;
                let key = idx_val.to_string_val();
                self.get_property(&obj_val, &key)
            }

            Expr::Call(callee, args) => self.eval_call(callee, args),

            Expr::New(callee, args) => self.eval_new(callee, args),

            Expr::Ternary(cond, then, else_expr) => {
                let val = self.eval_expr(cond)?;
                if val.is_truthy() {
                    self.eval_expr(then)
                } else {
                    self.eval_expr(else_expr)
                }
            }

            Expr::Arrow(params, body) => {
                let closure = self.capture_scope();
                Ok(Value::Function(JsFunction {
                    name: None,
                    params: params.clone(),
                    body: match body.as_ref() {
                        Stmt::Block(stmts) => stmts.clone(),
                        Stmt::Return(Some(expr)) => vec![Stmt::Return(Some(expr.clone()))],
                        other => vec![other.clone()],
                    },
                    closure,
                }))
            }

            Expr::FunctionExpr(name, params, body) => {
                let closure = self.capture_scope();
                Ok(Value::Function(JsFunction {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    closure,
                }))
            }

            Expr::Regex(pattern, flags) => Ok(Value::Regex(pattern.clone(), flags.clone())),

            Expr::Sequence(exprs) => {
                let mut last = Value::Undefined;
                for e in exprs {
                    last = self.eval_expr(e)?;
                }
                Ok(last)
            }

            Expr::Spread(_) => {
                Err(String::from("spread not supported in this context"))
            }
        }
    }

    fn assign_to(&mut self, target: &Expr, val: Value) -> Result<(), String> {
        match target {
            Expr::Ident(name) => {
                if !self.update_var(name, val.clone()) {
                    self.set_var(name, val);
                }
                Ok(())
            }
            Expr::Member(obj_expr, prop) => {
                self.set_object_property(obj_expr, prop, val)
            }
            Expr::Index(obj_expr, idx_expr) => {
                let idx = self.eval_expr(idx_expr)?;
                let key = idx.to_string_val();
                self.set_object_property(obj_expr, &key, val)
            }
            _ => Err(String::from("invalid assignment target")),
        }
    }

    fn set_object_property(&mut self, obj_expr: &Expr, prop: &str, val: Value) -> Result<(), String> {
        // Special case: document.cookie
        if let Expr::Ident(name) = obj_expr {
            if name == "document" && prop == "cookie" {
                if let Value::Str(s) = &val {
                    let cookie_part = s.split(';').next().unwrap_or("");
                    if !self.cookie.is_empty() {
                        self.cookie.push_str("; ");
                    }
                    self.cookie.push_str(cookie_part);
                }
                if let Some(doc) = self.get_var("document").cloned() {
                    if let Value::Object(mut map) = doc {
                        map.insert(String::from("cookie"), Value::Str(self.cookie.clone()));
                        self.update_var("document", Value::Object(map));
                    }
                }
                return Ok(());
            }
        }

        let mut obj = self.eval_expr(obj_expr)?;
        match &mut obj {
            Value::Object(map) => {
                map.insert(String::from(prop), val);
            }
            Value::Array(arr) => {
                if let Ok(idx) = prop.parse::<usize>() {
                    while arr.len() <= idx {
                        arr.push(Value::Undefined);
                    }
                    arr[idx] = val;
                } else if prop == "length" {
                    if let Value::Number(n) = &val {
                        let new_len = *n as usize;
                        arr.truncate(new_len);
                    }
                }
            }
            _ => {}
        }
        self.assign_to(obj_expr, obj)?;
        Ok(())
    }

    fn get_property(&self, obj: &Value, prop: &str) -> Result<Value, String> {
        match obj {
            Value::Object(map) => {
                Ok(map.get(prop).cloned().unwrap_or(Value::Undefined))
            }
            Value::Array(arr) => {
                if prop == "length" {
                    return Ok(Value::Number(arr.len() as f64));
                }
                if let Ok(idx) = prop.parse::<usize>() {
                    return Ok(arr.get(idx).cloned().unwrap_or(Value::Undefined));
                }
                Ok(Value::Undefined)
            }
            Value::Str(s) => {
                if prop == "length" {
                    return Ok(Value::Number(s.len() as f64));
                }
                if let Ok(idx) = prop.parse::<usize>() {
                    return Ok(s.chars().nth(idx)
                        .map(|c| Value::Str(char_to_string(c)))
                        .unwrap_or(Value::Undefined));
                }
                Ok(Value::Undefined)
            }
            _ => Ok(Value::Undefined),
        }
    }

    fn eval_binop(&self, l: &Value, op: BinOp, r: &Value) -> Result<Value, String> {
        match op {
            BinOp::Add => {
                if matches!(l, Value::Str(_)) || matches!(r, Value::Str(_)) {
                    let ls = l.to_string_val();
                    let rs = r.to_string_val();
                    Ok(Value::Str(format!("{}{}", ls, rs)))
                } else {
                    Ok(Value::Number(l.to_number() + r.to_number()))
                }
            }
            BinOp::Sub => Ok(Value::Number(l.to_number() - r.to_number())),
            BinOp::Mul => Ok(Value::Number(l.to_number() * r.to_number())),
            BinOp::Div => Ok(Value::Number(l.to_number() / r.to_number())),
            BinOp::Mod => Ok(Value::Number(l.to_number() % r.to_number())),
            BinOp::Pow => Ok(Value::Number(pow(l.to_number(), r.to_number()))),
            BinOp::Eq => Ok(Value::Bool(l.loose_eq(r))),
            BinOp::StrictEq => Ok(Value::Bool(l.strict_eq(r))),
            BinOp::NotEq => Ok(Value::Bool(!l.loose_eq(r))),
            BinOp::StrictNotEq => Ok(Value::Bool(!l.strict_eq(r))),
            BinOp::Lt => Ok(Value::Bool(l.to_number() < r.to_number())),
            BinOp::Gt => Ok(Value::Bool(l.to_number() > r.to_number())),
            BinOp::LtEq => Ok(Value::Bool(l.to_number() <= r.to_number())),
            BinOp::GtEq => Ok(Value::Bool(l.to_number() >= r.to_number())),
            BinOp::BitAnd => Ok(Value::Number((l.to_i32() & r.to_i32()) as f64)),
            BinOp::BitOr => Ok(Value::Number((l.to_i32() | r.to_i32()) as f64)),
            BinOp::BitXor => Ok(Value::Number((l.to_i32() ^ r.to_i32()) as f64)),
            BinOp::Shl => Ok(Value::Number(((l.to_i32()) << (r.to_u32() & 0x1f)) as f64)),
            BinOp::Shr => Ok(Value::Number(((l.to_i32()) >> (r.to_u32() & 0x1f)) as f64)),
            BinOp::Ushr => Ok(Value::Number(((l.to_u32()) >> (r.to_u32() & 0x1f)) as f64)),
            BinOp::Instanceof => Ok(Value::Bool(false)),
            BinOp::In => {
                let key = l.to_string_val();
                match r {
                    Value::Object(map) => Ok(Value::Bool(map.contains_key(&key))),
                    Value::Array(arr) => {
                        if let Ok(idx) = key.parse::<usize>() {
                            Ok(Value::Bool(idx < arr.len()))
                        } else {
                            Ok(Value::Bool(false))
                        }
                    }
                    _ => Ok(Value::Bool(false)),
                }
            }
            BinOp::And | BinOp::Or | BinOp::NullishCoalesce => unreachable!(),
        }
    }

    // -------------------------------------------------------------------
    // Function calls
    // -------------------------------------------------------------------

    fn eval_call(&mut self, callee: &Expr, arg_exprs: &[Expr]) -> Result<Value, String> {
        let mut args = Vec::new();
        for a in arg_exprs {
            if let Expr::Spread(inner) = a {
                let val = self.eval_expr(inner)?;
                if let Value::Array(items) = val {
                    args.extend(items);
                } else {
                    args.push(val);
                }
            } else {
                args.push(self.eval_expr(a)?);
            }
        }

        match callee {
            Expr::Member(obj, method) => {
                if let Expr::Ident(obj_name) = obj.as_ref() {
                    return self.eval_method_call(obj_name, method, &args, obj);
                }
                let obj_val = self.eval_expr(obj)?;
                return self.eval_value_method_call(&obj_val, method, &args);
            }
            Expr::Ident(name) => {
                return self.eval_global_call(name, &args);
            }
            _ => {}
        }

        let func_val = self.eval_expr(callee)?;
        self.call_function(&func_val, &args)
    }

    fn eval_global_call(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
        match name {
            "parseInt" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let radix = args.get(1).map(|v| v.to_number() as u32).unwrap_or(0);
                Ok(Value::Number(js_parse_int(&s, radix)))
            }
            "parseFloat" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Number(js_parse_float(&s)))
            }
            "isNaN" => {
                let n = args.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
                Ok(Value::Bool(n.is_nan()))
            }
            "isFinite" => {
                let n = args.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
                Ok(Value::Bool(n.is_finite()))
            }
            "Number" => {
                let n = args.first().map(|v| v.to_number()).unwrap_or(0.0);
                Ok(Value::Number(n))
            }
            "String" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(s))
            }
            "Boolean" => {
                let b = args.first().map(|v| v.is_truthy()).unwrap_or(false);
                Ok(Value::Bool(b))
            }
            "Array" => {
                if args.len() == 1 {
                    if let Value::Number(n) = &args[0] {
                        let len = *n as usize;
                        return Ok(Value::Array(vec![Value::Undefined; len]));
                    }
                }
                Ok(Value::Array(args.to_vec()))
            }
            "Object" => {
                if let Some(Value::Object(m)) = args.first() {
                    Ok(Value::Object(m.clone()))
                } else {
                    Ok(Value::Object(BTreeMap::new()))
                }
            }
            "encodeURIComponent" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(encode_uri_component(&s)))
            }
            "decodeURIComponent" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(decode_uri_component(&s)))
            }
            "encodeURI" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(encode_uri(&s)))
            }
            "decodeURI" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(decode_uri_component(&s)))
            }
            "btoa" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(btoa(&s)))
            }
            "atob" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(atob(&s)))
            }
            "setTimeout" | "setInterval" => {
                if let Some(func) = args.first() {
                    if let Value::Function(_) = func {
                        self.call_function(func, &[])?;
                    }
                }
                Ok(Value::Number(0.0))
            }
            "clearTimeout" | "clearInterval" => Ok(Value::Undefined),
            "eval" => {
                if let Some(Value::Str(code)) = args.first() {
                    let tokens = crate::tokenizer::tokenize(code)?;
                    let ast = crate::parser::parse(tokens)?;
                    let mut last = Value::Undefined;
                    for stmt in &ast {
                        match self.exec_stmt(stmt)? {
                            Signal::Return(v) => return Ok(v),
                            Signal::Throw(v) => return Err(format!("Uncaught: {}", v.to_string_val())),
                            _ => {
                                if let Stmt::Expr(e) = stmt {
                                    last = self.eval_expr(e)?;
                                }
                            }
                        }
                    }
                    Ok(last)
                } else {
                    Ok(args.first().cloned().unwrap_or(Value::Undefined))
                }
            }
            "Date" => {
                Ok(Value::Str(String::from("Tue Jan 01 2030 00:00:00 GMT+0000")))
            }
            "RegExp" => {
                let pattern = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let flags = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Regex(pattern, flags))
            }
            "escape" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(encode_uri_component(&s)))
            }
            "unescape" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(decode_uri_component(&s)))
            }
            _ => {
                match self.get_var(name).cloned() {
                    Some(func) => self.call_function(&func, args),
                    None => Err(format!("{} is not a function", name)),
                }
            }
        }
    }

    fn eval_method_call(&mut self, obj_name: &str, method: &str, args: &[Value], _obj_expr: &Expr) -> Result<Value, String> {
        match obj_name {
            "console" => {
                match method {
                    "log" | "info" | "warn" | "error" | "debug" => {
                        let parts: Vec<String> = args.iter().map(|a| a.to_string_val()).collect();
                        self.output.push_str(&parts.join(" "));
                        self.output.push('\n');
                        Ok(Value::Undefined)
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "Math" => self.eval_math_call(method, args),
            "JSON" => {
                match method {
                    "stringify" => {
                        let val = args.first().cloned().unwrap_or(Value::Undefined);
                        Ok(Value::Str(json_stringify(&val)))
                    }
                    "parse" => {
                        let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                        json_parse(&s)
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "Object" => {
                match method {
                    "keys" => {
                        if let Some(Value::Object(map)) = args.first() {
                            let keys: Vec<Value> = map.keys().map(|k| Value::Str(k.clone())).collect();
                            Ok(Value::Array(keys))
                        } else {
                            Ok(Value::Array(vec![]))
                        }
                    }
                    "values" => {
                        if let Some(Value::Object(map)) = args.first() {
                            let vals: Vec<Value> = map.values().cloned().collect();
                            Ok(Value::Array(vals))
                        } else {
                            Ok(Value::Array(vec![]))
                        }
                    }
                    "entries" => {
                        if let Some(Value::Object(map)) = args.first() {
                            let entries: Vec<Value> = map.iter()
                                .map(|(k, v)| Value::Array(vec![Value::Str(k.clone()), v.clone()]))
                                .collect();
                            Ok(Value::Array(entries))
                        } else {
                            Ok(Value::Array(vec![]))
                        }
                    }
                    "assign" => {
                        let mut target = match args.first() {
                            Some(Value::Object(m)) => m.clone(),
                            _ => BTreeMap::new(),
                        };
                        for src in args.iter().skip(1) {
                            if let Value::Object(m) = src {
                                for (k, v) in m {
                                    target.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        Ok(Value::Object(target))
                    }
                    "freeze" | "seal" | "create" | "defineProperty" => {
                        Ok(args.first().cloned().unwrap_or(Value::Object(BTreeMap::new())))
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "Array" => {
                match method {
                    "isArray" => {
                        Ok(Value::Bool(matches!(args.first(), Some(Value::Array(_)))))
                    }
                    "from" => {
                        match args.first() {
                            Some(Value::Array(arr)) => Ok(Value::Array(arr.clone())),
                            Some(Value::Str(s)) => {
                                let chars: Vec<Value> = s.chars()
                                    .map(|c| Value::Str(char_to_string(c)))
                                    .collect();
                                Ok(Value::Array(chars))
                            }
                            _ => Ok(Value::Array(vec![])),
                        }
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "String" => {
                match method {
                    "fromCharCode" => {
                        let mut s = String::new();
                        for a in args {
                            let code = a.to_number() as u32;
                            if let Some(c) = char::from_u32(code) {
                                s.push(c);
                            }
                        }
                        Ok(Value::Str(s))
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "Number" => {
                match method {
                    "isInteger" => {
                        let n = args.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
                        Ok(Value::Bool(n == (n as i64 as f64) && n.is_finite()))
                    }
                    "isFinite" => {
                        let n = args.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
                        Ok(Value::Bool(n.is_finite()))
                    }
                    "isNaN" => {
                        let n = args.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
                        Ok(Value::Bool(n.is_nan()))
                    }
                    "parseInt" => {
                        let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                        let radix = args.get(1).map(|v| v.to_number() as u32).unwrap_or(10);
                        Ok(Value::Number(js_parse_int(&s, radix)))
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            "Date" => {
                match method {
                    "now" => Ok(Value::Number(1700000000000.0)),
                    _ => Ok(Value::Undefined),
                }
            }
            "document" => {
                match method {
                    "createElement" => {
                        let tag = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                        let mut el = BTreeMap::new();
                        el.insert(String::from("tagName"), Value::Str(tag));
                        el.insert(String::from("innerHTML"), Value::Str(String::new()));
                        el.insert(String::from("style"), Value::Object(BTreeMap::new()));
                        el.insert(String::from("className"), Value::Str(String::new()));
                        el.insert(String::from("id"), Value::Str(String::new()));
                        Ok(Value::Object(el))
                    }
                    "getElementById" | "querySelector" => Ok(Value::Null),
                    "getElementsByTagName" | "getElementsByClassName" | "querySelectorAll" => {
                        Ok(Value::Array(vec![]))
                    }
                    "write" | "writeln" => {
                        for a in args {
                            self.output.push_str(&a.to_string_val());
                        }
                        Ok(Value::Undefined)
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            _ => {
                let obj_val = self.get_var(obj_name).cloned().unwrap_or(Value::Undefined);
                self.eval_value_method_call(&obj_val, method, args)
            }
        }
    }

    fn eval_value_method_call(&mut self, obj: &Value, method: &str, args: &[Value]) -> Result<Value, String> {
        match obj {
            Value::Str(s) => self.eval_string_method(s, method, args),
            Value::Array(arr) => self.eval_array_method(arr, method, args),
            Value::Object(map) => {
                if let Some(Value::Function(func)) = map.get(method) {
                    let func = func.clone();
                    return self.call_js_function(&func, args);
                }
                if method == "hasOwnProperty" {
                    let key = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                    return Ok(Value::Bool(map.contains_key(&key)));
                }
                if method == "toString" {
                    return Ok(Value::Str(String::from("[object Object]")));
                }
                Ok(Value::Undefined)
            }
            Value::Regex(pattern, _flags) => self.eval_regex_method(pattern, _flags, method, args),
            Value::Number(n) => {
                match method {
                    "toString" => {
                        let radix = args.first().map(|v| v.to_number() as u32).unwrap_or(10);
                        Ok(Value::Str(number_to_string(*n, radix)))
                    }
                    "toFixed" => {
                        let digits = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                        Ok(Value::Str(to_fixed(*n, digits)))
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            Value::Function(func) => {
                match method {
                    "call" => {
                        let call_args = if args.len() > 1 { &args[1..] } else { &[] };
                        self.call_js_function(func, call_args)
                    }
                    "apply" => {
                        let call_args = match args.get(1) {
                            Some(Value::Array(a)) => a.clone(),
                            _ => vec![],
                        };
                        self.call_js_function(func, &call_args)
                    }
                    "bind" => {
                        Ok(Value::Function(func.clone()))
                    }
                    _ => Ok(Value::Undefined),
                }
            }
            _ => Ok(Value::Undefined),
        }
    }

    fn eval_string_method(&self, s: &str, method: &str, args: &[Value]) -> Result<Value, String> {
        match method {
            "charAt" => {
                let idx = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                Ok(Value::Str(s.chars().nth(idx).map(|c| char_to_string(c)).unwrap_or_default()))
            }
            "charCodeAt" => {
                let idx = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                Ok(Value::Number(s.chars().nth(idx).map(|c| c as u32 as f64).unwrap_or(f64::NAN)))
            }
            "codePointAt" => {
                let idx = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                Ok(Value::Number(s.chars().nth(idx).map(|c| c as u32 as f64).unwrap_or(f64::NAN)))
            }
            "indexOf" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let from = args.get(1).map(|v| v.to_number() as usize).unwrap_or(0);
                let search_str = if from < s.len() { &s[from..] } else { "" };
                Ok(Value::Number(search_str.find(&*needle).map(|i| (i + from) as f64).unwrap_or(-1.0)))
            }
            "lastIndexOf" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Number(s.rfind(&*needle).map(|i| i as f64).unwrap_or(-1.0)))
            }
            "includes" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Bool(s.contains(&*needle)))
            }
            "startsWith" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Bool(s.starts_with(&*needle)))
            }
            "endsWith" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Bool(s.ends_with(&*needle)))
            }
            "substring" | "slice" => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let mut start = args.first().map(|v| v.to_number() as i64).unwrap_or(0);
                let mut end = args.get(1).map(|v| v.to_number() as i64).unwrap_or(len);

                if method == "slice" {
                    if start < 0 { start = (len + start).max(0); }
                    if end < 0 { end = (len + end).max(0); }
                } else {
                    if start < 0 { start = 0; }
                    if end < 0 { end = 0; }
                }

                let start = start as usize;
                let end = end.min(len) as usize;
                if start >= end {
                    return Ok(Value::Str(String::new()));
                }
                let result: String = chars[start..end].iter().collect();
                Ok(Value::Str(result))
            }
            "substr" => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let mut start = args.first().map(|v| v.to_number() as i64).unwrap_or(0);
                if start < 0 { start = (len + start).max(0); }
                let count = args.get(1).map(|v| v.to_number() as usize).unwrap_or(chars.len());
                let start = start as usize;
                let end = (start + count).min(chars.len());
                let result: String = chars[start..end].iter().collect();
                Ok(Value::Str(result))
            }
            "toLowerCase" | "toLocaleLowerCase" => {
                let mut result = String::new();
                for c in s.chars() {
                    for lc in c.to_lowercase() { result.push(lc); }
                }
                Ok(Value::Str(result))
            }
            "toUpperCase" | "toLocaleUpperCase" => {
                let mut result = String::new();
                for c in s.chars() {
                    for uc in c.to_uppercase() { result.push(uc); }
                }
                Ok(Value::Str(result))
            }
            "trim" => Ok(Value::Str(String::from(s.trim()))),
            "trimStart" | "trimLeft" => Ok(Value::Str(String::from(s.trim_start()))),
            "trimEnd" | "trimRight" => Ok(Value::Str(String::from(s.trim_end()))),
            "split" => {
                let sep = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let limit = args.get(1).map(|v| v.to_number() as usize);
                let parts: Vec<Value> = if sep.is_empty() {
                    s.chars().map(|c| Value::Str(char_to_string(c))).collect()
                } else {
                    s.split(&*sep).map(|p| Value::Str(String::from(p))).collect()
                };
                let parts = if let Some(lim) = limit {
                    parts.into_iter().take(lim).collect()
                } else {
                    parts
                };
                Ok(Value::Array(parts))
            }
            "replace" => {
                let pattern = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let replacement = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(replace_first(s, &pattern, &replacement)))
            }
            "replaceAll" => {
                let pattern = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let replacement = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Str(s.replace(&*pattern, &replacement)))
            }
            "repeat" => {
                let count = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                let mut result = String::new();
                for _ in 0..count.min(10_000) {
                    result.push_str(s);
                }
                Ok(Value::Str(result))
            }
            "padStart" => {
                let target_len = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                let pad_str = args.get(1).map(|v| v.to_string_val()).unwrap_or_else(|| String::from(" "));
                let current_len = s.chars().count();
                if current_len >= target_len {
                    return Ok(Value::Str(String::from(s)));
                }
                let pad_needed = target_len - current_len;
                let mut prefix = String::new();
                let pad_chars: Vec<char> = pad_str.chars().collect();
                if !pad_chars.is_empty() {
                    let mut i = 0;
                    while prefix.chars().count() < pad_needed {
                        prefix.push(pad_chars[i % pad_chars.len()]);
                        i += 1;
                    }
                }
                prefix.push_str(s);
                Ok(Value::Str(prefix))
            }
            "padEnd" => {
                let target_len = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                let pad_str = args.get(1).map(|v| v.to_string_val()).unwrap_or_else(|| String::from(" "));
                let mut result = String::from(s);
                let pad_chars: Vec<char> = pad_str.chars().collect();
                if !pad_chars.is_empty() {
                    let mut i = 0;
                    while result.chars().count() < target_len {
                        result.push(pad_chars[i % pad_chars.len()]);
                        i += 1;
                    }
                }
                Ok(Value::Str(result))
            }
            "match" => {
                if let Some(Value::Regex(pattern, _flags)) = args.first() {
                    if let Some(matched) = simple_regex_match(s, pattern) {
                        Ok(Value::Array(vec![Value::Str(matched)]))
                    } else {
                        Ok(Value::Null)
                    }
                } else {
                    let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                    if s.contains(&*needle) {
                        Ok(Value::Array(vec![Value::Str(needle)]))
                    } else {
                        Ok(Value::Null)
                    }
                }
            }
            "search" => {
                let needle = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                Ok(Value::Number(s.find(&*needle).map(|i| i as f64).unwrap_or(-1.0)))
            }
            "concat" => {
                let mut result = String::from(s);
                for a in args {
                    result.push_str(&a.to_string_val());
                }
                Ok(Value::Str(result))
            }
            "toString" | "valueOf" => Ok(Value::Str(String::from(s))),
            _ => Ok(Value::Undefined),
        }
    }

    fn eval_array_method(&mut self, arr: &[Value], method: &str, args: &[Value]) -> Result<Value, String> {
        match method {
            "push" => {
                let mut new_arr = arr.to_vec();
                for a in args { new_arr.push(a.clone()); }
                Ok(Value::Number(new_arr.len() as f64))
            }
            "pop" => {
                Ok(arr.last().cloned().unwrap_or(Value::Undefined))
            }
            "shift" => {
                Ok(arr.first().cloned().unwrap_or(Value::Undefined))
            }
            "unshift" => Ok(Value::Number((arr.len() + args.len()) as f64)),
            "join" => {
                let sep = args.first().map(|v| v.to_string_val()).unwrap_or_else(|| String::from(","));
                let parts: Vec<String> = arr.iter().map(|v| v.to_string_val()).collect();
                Ok(Value::Str(parts.join(&sep)))
            }
            "indexOf" => {
                let needle = args.first().cloned().unwrap_or(Value::Undefined);
                for (i, item) in arr.iter().enumerate() {
                    if item.strict_eq(&needle) {
                        return Ok(Value::Number(i as f64));
                    }
                }
                Ok(Value::Number(-1.0))
            }
            "includes" => {
                let needle = args.first().cloned().unwrap_or(Value::Undefined);
                for item in arr {
                    if item.strict_eq(&needle) {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            "slice" => {
                let len = arr.len() as i64;
                let mut start = args.first().map(|v| v.to_number() as i64).unwrap_or(0);
                let mut end = args.get(1).map(|v| v.to_number() as i64).unwrap_or(len);
                if start < 0 { start = (len + start).max(0); }
                if end < 0 { end = (len + end).max(0); }
                let start = start as usize;
                let end = (end as usize).min(arr.len());
                if start >= end {
                    return Ok(Value::Array(vec![]));
                }
                Ok(Value::Array(arr[start..end].to_vec()))
            }
            "concat" => {
                let mut result = arr.to_vec();
                for a in args {
                    if let Value::Array(other) = a {
                        result.extend(other.iter().cloned());
                    } else {
                        result.push(a.clone());
                    }
                }
                Ok(Value::Array(result))
            }
            "reverse" => {
                let mut result = arr.to_vec();
                result.reverse();
                Ok(Value::Array(result))
            }
            "map" => {
                if let Some(func) = args.first() {
                    let mut result = Vec::new();
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        result.push(val);
                    }
                    Ok(Value::Array(result))
                } else {
                    Ok(Value::Array(arr.to_vec()))
                }
            }
            "filter" => {
                if let Some(func) = args.first() {
                    let mut result = Vec::new();
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        if val.is_truthy() {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::Array(result))
                } else {
                    Ok(Value::Array(arr.to_vec()))
                }
            }
            "find" => {
                if let Some(func) = args.first() {
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        if val.is_truthy() {
                            return Ok(item.clone());
                        }
                    }
                }
                Ok(Value::Undefined)
            }
            "findIndex" => {
                if let Some(func) = args.first() {
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        if val.is_truthy() {
                            return Ok(Value::Number(i as f64));
                        }
                    }
                }
                Ok(Value::Number(-1.0))
            }
            "forEach" => {
                if let Some(func) = args.first() {
                    for (i, item) in arr.iter().enumerate() {
                        self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                    }
                }
                Ok(Value::Undefined)
            }
            "reduce" => {
                let func = args.first().cloned().unwrap_or(Value::Undefined);
                let mut acc = if args.len() > 1 {
                    args[1].clone()
                } else if !arr.is_empty() {
                    arr[0].clone()
                } else {
                    return Err(String::from("Reduce of empty array with no initial value"));
                };
                let start_idx = if args.len() > 1 { 0 } else { 1 };
                for (i, item) in arr.iter().enumerate().skip(start_idx) {
                    acc = self.call_function(&func, &[acc, item.clone(), Value::Number(i as f64)])?;
                }
                Ok(acc)
            }
            "some" => {
                if let Some(func) = args.first() {
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        if val.is_truthy() { return Ok(Value::Bool(true)); }
                    }
                }
                Ok(Value::Bool(false))
            }
            "every" => {
                if let Some(func) = args.first() {
                    for (i, item) in arr.iter().enumerate() {
                        let val = self.call_function(func, &[item.clone(), Value::Number(i as f64)])?;
                        if !val.is_truthy() { return Ok(Value::Bool(false)); }
                    }
                }
                Ok(Value::Bool(true))
            }
            "flat" => {
                let depth = args.first().map(|v| v.to_number() as usize).unwrap_or(1);
                Ok(Value::Array(flatten_array(arr, depth)))
            }
            "sort" => {
                let mut result = arr.to_vec();
                result.sort_by(|a, b| a.to_string_val().cmp(&b.to_string_val()));
                Ok(Value::Array(result))
            }
            "splice" => {
                let start = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                let delete_count = args.get(1).map(|v| v.to_number() as usize).unwrap_or(arr.len());
                let end = (start + delete_count).min(arr.len());
                let removed: Vec<Value> = if start < arr.len() { arr[start..end].to_vec() } else { vec![] };
                Ok(Value::Array(removed))
            }
            "toString" => {
                let parts: Vec<String> = arr.iter().map(|v| v.to_string_val()).collect();
                Ok(Value::Str(parts.join(",")))
            }
            "fill" => {
                let fill_val = args.first().cloned().unwrap_or(Value::Undefined);
                let mut result = arr.to_vec();
                for item in result.iter_mut() {
                    *item = fill_val.clone();
                }
                Ok(Value::Array(result))
            }
            _ => Ok(Value::Undefined),
        }
    }

    fn eval_regex_method(&self, pattern: &str, _flags: &str, method: &str, args: &[Value]) -> Result<Value, String> {
        match method {
            "test" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                let matched = simple_regex_match(&s, pattern).is_some();
                Ok(Value::Bool(matched))
            }
            "exec" => {
                let s = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                match simple_regex_match(&s, pattern) {
                    Some(m) => Ok(Value::Array(vec![Value::Str(m)])),
                    None => Ok(Value::Null),
                }
            }
            _ => Ok(Value::Undefined),
        }
    }

    fn eval_math_call(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        let a = args.first().map(|v| v.to_number()).unwrap_or(0.0);
        let b = args.get(1).map(|v| v.to_number()).unwrap_or(0.0);

        match method {
            "floor" => Ok(Value::Number(floor(a))),
            "ceil" => Ok(Value::Number(ceil(a))),
            "round" => Ok(Value::Number(round(a))),
            "abs" => Ok(Value::Number(if a < 0.0 { -a } else { a })),
            "min" => {
                if args.is_empty() { return Ok(Value::Number(f64::INFINITY)); }
                let mut min_val = args[0].to_number();
                for v in args.iter().skip(1) {
                    let n = v.to_number();
                    if n < min_val { min_val = n; }
                }
                Ok(Value::Number(min_val))
            }
            "max" => {
                if args.is_empty() { return Ok(Value::Number(f64::NEG_INFINITY)); }
                let mut max_val = args[0].to_number();
                for v in args.iter().skip(1) {
                    let n = v.to_number();
                    if n > max_val { max_val = n; }
                }
                Ok(Value::Number(max_val))
            }
            "pow" => Ok(Value::Number(pow(a, b))),
            "sqrt" => Ok(Value::Number(sqrt(a))),
            "log" => Ok(Value::Number(ln(a))),
            "log2" => Ok(Value::Number(ln(a) / core::f64::consts::LN_2)),
            "log10" => Ok(Value::Number(ln(a) / core::f64::consts::LN_10)),
            "sin" => Ok(Value::Number(sin(a))),
            "cos" => Ok(Value::Number(cos(a))),
            "tan" => {
                let c = cos(a);
                if c == 0.0 { Ok(Value::Number(f64::INFINITY)) }
                else { Ok(Value::Number(sin(a) / c)) }
            }
            "atan2" => Ok(Value::Number(atan2(a, b))),
            "random" => Ok(Value::Number(self.next_random())),
            "sign" => {
                if a > 0.0 { Ok(Value::Number(1.0)) }
                else if a < 0.0 { Ok(Value::Number(-1.0)) }
                else { Ok(Value::Number(0.0)) }
            }
            "trunc" => Ok(Value::Number(trunc(a))),
            "cbrt" => Ok(Value::Number(cbrt(a))),
            "exp" => Ok(Value::Number(exp_fn(a))),
            "clz32" => {
                let n = a as i32 as u32;
                Ok(Value::Number(n.leading_zeros() as f64))
            }
            "imul" => {
                let x = a as i32;
                let y = b as i32;
                Ok(Value::Number(x.wrapping_mul(y) as f64))
            }
            "fround" => Ok(Value::Number((a as f32) as f64)),
            "hypot" => {
                let sum: f64 = args.iter().map(|v| { let n = v.to_number(); n * n }).sum();
                Ok(Value::Number(sqrt(sum)))
            }
            _ => Ok(Value::Number(f64::NAN)),
        }
    }

    fn eval_new(&mut self, callee: &Expr, arg_exprs: &[Expr]) -> Result<Value, String> {
        let mut args = Vec::new();
        for a in arg_exprs {
            args.push(self.eval_expr(a)?);
        }

        if let Expr::Ident(name) = callee {
            match name.as_str() {
                "Date" => {
                    let mut obj = BTreeMap::new();
                    obj.insert(String::from("getTime"), Value::Function(JsFunction {
                        name: Some(String::from("getTime")),
                        params: vec![],
                        body: vec![Stmt::Return(Some(Expr::Number(1700000000000.0)))],
                        closure: BTreeMap::new(),
                    }));
                    return Ok(Value::Object(obj));
                }
                "RegExp" => {
                    let pattern = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                    let flags = args.get(1).map(|v| v.to_string_val()).unwrap_or_default();
                    return Ok(Value::Regex(pattern, flags));
                }
                "Array" => {
                    if args.len() == 1 {
                        if let Value::Number(n) = &args[0] {
                            return Ok(Value::Array(vec![Value::Undefined; *n as usize]));
                        }
                    }
                    return Ok(Value::Array(args));
                }
                "Object" => return Ok(Value::Object(BTreeMap::new())),
                "Error" | "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError" => {
                    let msg = args.first().map(|v| v.to_string_val()).unwrap_or_default();
                    let mut obj = BTreeMap::new();
                    obj.insert(String::from("name"), Value::Str(name.clone()));
                    obj.insert(String::from("message"), Value::Str(msg));
                    obj.insert(String::from("stack"), Value::Str(String::new()));
                    return Ok(Value::Object(obj));
                }
                _ => {}
            }
        }

        let func_val = self.eval_expr(callee)?;
        match func_val {
            Value::Function(func) => {
                let result = self.call_js_function(&func, &args)?;
                match result {
                    Value::Object(_) => Ok(result),
                    _ => Ok(Value::Object(BTreeMap::new())),
                }
            }
            _ => Ok(Value::Object(BTreeMap::new())),
        }
    }

    fn call_function(&mut self, func: &Value, args: &[Value]) -> Result<Value, String> {
        match func {
            Value::Function(f) => self.call_js_function(f, args),
            _ => Err(format!("not a function: {:?}", func.type_of())),
        }
    }

    fn call_js_function(&mut self, func: &JsFunction, args: &[Value]) -> Result<Value, String> {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(String::from("Maximum call stack size exceeded"));
        }

        let saved_scopes = core::mem::take(&mut self.scopes);

        let mut closure_scope = Scope::new();
        for (k, v) in &func.closure {
            closure_scope.vars.insert(k.clone(), v.clone());
        }
        self.scopes = vec![closure_scope];

        self.push_scope();
        for (i, param) in func.params.iter().enumerate() {
            let val = args.get(i).cloned().unwrap_or(Value::Undefined);
            self.set_var(param, val);
        }
        self.set_var("arguments", Value::Array(args.to_vec()));

        let result = self.exec_block(&func.body);

        self.scopes = saved_scopes;
        self.call_depth -= 1;

        match result {
            Ok(Signal::Return(val)) => Ok(val),
            Ok(Signal::Throw(val)) => Err(format!("Uncaught: {}", val.to_string_val())),
            Ok(_) => Ok(Value::Undefined),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in helpers
// ---------------------------------------------------------------------------

fn js_parse_int(s: &str, radix: u32) -> f64 {
    let s = s.trim();
    if s.is_empty() { return f64::NAN; }

    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let radix = if radix == 0 {
        if s.starts_with("0x") || s.starts_with("0X") { 16 } else { 10 }
    } else {
        radix
    };

    let s = if radix == 16 {
        s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s)
    } else {
        s
    };

    let mut val: i64 = 0;
    let mut found_digit = false;
    for ch in s.chars() {
        let digit = match ch {
            '0'..='9' => (ch as u32 - '0' as u32),
            'a'..='f' => (ch as u32 - 'a' as u32 + 10),
            'A'..='F' => (ch as u32 - 'A' as u32 + 10),
            _ => break,
        };
        if digit >= radix { break; }
        found_digit = true;
        val = val * radix as i64 + digit as i64;
    }

    if !found_digit { return f64::NAN; }
    if neg { -(val as f64) } else { val as f64 }
}

fn js_parse_float(s: &str) -> f64 {
    parse_float_simple(s.trim())
}

fn replace_first(s: &str, from: &str, to: &str) -> String {
    if let Some(pos) = s.find(from) {
        let mut result = String::from(&s[..pos]);
        result.push_str(to);
        result.push_str(&s[pos + from.len()..]);
        result
    } else {
        String::from(s)
    }
}

fn flatten_array(arr: &[Value], depth: usize) -> Vec<Value> {
    let mut result = Vec::new();
    for item in arr {
        if depth > 0 {
            if let Value::Array(inner) = item {
                result.extend(flatten_array(inner, depth - 1));
                continue;
            }
        }
        result.push(item.clone());
    }
    result
}

// ---------------------------------------------------------------------------
// URI encoding/decoding
// ---------------------------------------------------------------------------

fn encode_uri_component(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            result.push(b as char);
        } else {
            result.push('%');
            result.push(hex_digit(b >> 4));
            result.push(hex_digit(b & 0xf));
        }
    }
    result
}

fn encode_uri(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
                | b';' | b',' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b'#')
        {
            result.push(b as char);
        } else {
            result.push('%');
            result.push(hex_digit(b >> 4));
            result.push(hex_digit(b & 0xf));
        }
    }
    result
}

fn decode_uri_component(s: &str) -> String {
    let mut result = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = from_hex_digit(bytes[i + 1]);
            let l = from_hex_digit(bytes[i + 2]);
            if let (Some(h), Some(l)) = (h, l) {
                result.push((h * 16 + l) as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + n - 10) as char,
        _ => '0',
    }
}

fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Base64 (btoa/atob)
// ---------------------------------------------------------------------------

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn btoa(s: &str) -> String {
    let input = s.as_bytes();
    let mut result = String::new();
    let mut i = 0;

    while i < input.len() {
        let b0 = input[i] as u32;
        let b1 = if i + 1 < input.len() { input[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if i + 1 < input.len() {
            result.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if i + 2 < input.len() {
            result.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        i += 3;
    }

    result
}

fn atob(s: &str) -> String {
    let mut result = Vec::new();
    let input: Vec<u8> = s.bytes().filter(|b| *b != b'\n' && *b != b'\r' && *b != b' ').collect();

    let mut i = 0;
    while i + 3 < input.len() {
        let a = b64_decode_char(input[i]);
        let b = b64_decode_char(input[i + 1]);
        let c = b64_decode_char(input[i + 2]);
        let d = b64_decode_char(input[i + 3]);

        result.push(((a << 2) | (b >> 4)) as u8);
        if input[i + 2] != b'=' {
            result.push((((b & 0xF) << 4) | (c >> 2)) as u8);
        }
        if input[i + 3] != b'=' {
            result.push((((c & 0x3) << 6) | d) as u8);
        }

        i += 4;
    }

    String::from_utf8(result).unwrap_or_default()
}

fn b64_decode_char(c: u8) -> u32 {
    match c {
        b'A'..=b'Z' => (c - b'A') as u32,
        b'a'..=b'z' => (c - b'a' + 26) as u32,
        b'0'..=b'9' => (c - b'0' + 52) as u32,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// JSON stringify/parse (minimal)
// ---------------------------------------------------------------------------

fn json_stringify(val: &Value) -> String {
    match val {
        Value::Number(n) => format_number(*n),
        Value::Str(s) => {
            let mut result = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => result.push_str("\\\""),
                    '\\' => result.push_str("\\\\"),
                    '\n' => result.push_str("\\n"),
                    '\r' => result.push_str("\\r"),
                    '\t' => result.push_str("\\t"),
                    _ => result.push(c),
                }
            }
            result.push('"');
            result
        }
        Value::Bool(true) => String::from("true"),
        Value::Bool(false) => String::from("false"),
        Value::Null => String::from("null"),
        Value::Undefined => String::from("undefined"),
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(json_stringify).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map.iter()
                .map(|(k, v)| format!("\"{}\":{}", k, json_stringify(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Function(_) => String::from("undefined"),
        Value::Regex(p, f) => format!("\"/{}/{}\"", p, f),
    }
}

fn json_parse(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if s.is_empty() { return Err(String::from("unexpected end of JSON")); }
    let (val, _) = json_parse_value(s)?;
    Ok(val)
}

fn json_parse_value(s: &str) -> Result<(Value, usize), String> {
    let orig_len = s.len();
    let s = s.trim_start();
    let skipped = orig_len - s.len();
    if s.is_empty() { return Err(String::from("unexpected end of JSON")); }

    let (val, consumed) = match s.as_bytes()[0] {
        b'"' => {
            let (string, c) = json_parse_string(s)?;
            (Value::Str(string), c)
        }
        b'{' => json_parse_object(s)?,
        b'[' => json_parse_array(s)?,
        b't' if s.starts_with("true") => (Value::Bool(true), 4),
        b'f' if s.starts_with("false") => (Value::Bool(false), 5),
        b'n' if s.starts_with("null") => (Value::Null, 4),
        b'-' | b'0'..=b'9' => json_parse_number(s)?,
        _ => return Err(format!("unexpected character in JSON: {:?}", s.chars().next())),
    };
    Ok((val, consumed + skipped))
}

fn json_parse_string(s: &str) -> Result<(String, usize), String> {
    if !s.starts_with('"') { return Err(String::from("expected '\"'")); }
    let mut result = String::new();
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Ok((result, i + 1)),
            b'\\' => {
                i += 1;
                if i >= bytes.len() { return Err(String::from("unterminated string")); }
                match bytes[i] {
                    b'"' => result.push('"'),
                    b'\\' => result.push('\\'),
                    b'/' => result.push('/'),
                    b'n' => result.push('\n'),
                    b'r' => result.push('\r'),
                    b't' => result.push('\t'),
                    b'u' => {
                        if i + 4 < bytes.len() {
                            let hex = &s[i+1..i+5];
                            if let Ok(code) = u32::from_str_radix(hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    result.push(c);
                                }
                            }
                            i += 4;
                        }
                    }
                    _ => { result.push('\\'); result.push(bytes[i] as char); }
                }
            }
            ch => {
                if ch < 0x80 {
                    result.push(ch as char);
                } else {
                    let remaining = &s[i..];
                    if let Some(c) = remaining.chars().next() {
                        result.push(c);
                        i += c.len_utf8() - 1;
                    }
                }
            }
        }
        i += 1;
    }
    Err(String::from("unterminated JSON string"))
}

fn json_parse_number(s: &str) -> Result<(Value, usize), String> {
    let mut end = 0;
    let bytes = s.as_bytes();
    if end < bytes.len() && bytes[end] == b'-' { end += 1; }
    while end < bytes.len() && bytes[end].is_ascii_digit() { end += 1; }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() { end += 1; }
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') { end += 1; }
        while end < bytes.len() && bytes[end].is_ascii_digit() { end += 1; }
    }
    let num_str = &s[..end];
    let n = parse_float_simple(num_str);
    Ok((Value::Number(n), end))
}

fn json_parse_object(s: &str) -> Result<(Value, usize), String> {
    let mut map = BTreeMap::new();
    let mut i = 1;
    loop {
        while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() { i += 1; }
        if i >= s.len() { return Err(String::from("unterminated object")); }
        if s.as_bytes()[i] == b'}' { return Ok((Value::Object(map), i + 1)); }
        if !map.is_empty() {
            if s.as_bytes()[i] != b',' { return Err(String::from("expected ',' in object")); }
            i += 1;
            while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() { i += 1; }
        }
        if i < s.len() && s.as_bytes()[i] == b'}' { return Ok((Value::Object(map), i + 1)); }
        let (key, consumed) = json_parse_string(&s[i..])?;
        i += consumed;
        while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() { i += 1; }
        if i >= s.len() || s.as_bytes()[i] != b':' { return Err(String::from("expected ':' in object")); }
        i += 1;
        let (val, consumed) = json_parse_value(&s[i..])?;
        i += consumed;
        map.insert(key, val);
    }
}

fn json_parse_array(s: &str) -> Result<(Value, usize), String> {
    let mut arr = Vec::new();
    let mut i = 1;
    loop {
        while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() { i += 1; }
        if i >= s.len() { return Err(String::from("unterminated array")); }
        if s.as_bytes()[i] == b']' { return Ok((Value::Array(arr), i + 1)); }
        if !arr.is_empty() {
            if s.as_bytes()[i] != b',' { return Err(String::from("expected ',' in array")); }
            i += 1;
        }
        let (val, consumed) = json_parse_value(&s[i..])?;
        i += consumed;
        arr.push(val);
    }
}

// ---------------------------------------------------------------------------
// Simple regex matching
// ---------------------------------------------------------------------------

fn simple_regex_match(text: &str, pattern: &str) -> Option<String> {
    let anchored_start = pattern.starts_with('^');
    let pattern = if anchored_start { &pattern[1..] } else { pattern };
    let anchored_end = pattern.ends_with('$') && !pattern.ends_with("\\$");
    let pattern = if anchored_end { &pattern[..pattern.len()-1] } else { pattern };

    if anchored_start {
        if let Some(end) = regex_match_at(text, 0, pattern) {
            if anchored_end && end != text.len() { return None; }
            return Some(String::from(&text[..end]));
        }
        None
    } else {
        for start in 0..=text.len() {
            if let Some(end) = regex_match_at(text, start, pattern) {
                if anchored_end && end != text.len() { continue; }
                return Some(String::from(&text[start..end]));
            }
        }
        None
    }
}

fn regex_match_at(text: &str, text_pos: usize, pattern: &str) -> Option<usize> {
    regex_match_inner(text.as_bytes(), text_pos, pattern.as_bytes(), 0)
}

fn regex_match_inner(text: &[u8], mut tp: usize, pat: &[u8], pp: usize) -> Option<usize> {
    if pp >= pat.len() {
        return Some(tp);
    }

    let (matcher, next_pp) = parse_pattern_element(pat, pp)?;

    let quantifier = if next_pp < pat.len() {
        match pat[next_pp] {
            b'*' => Some((0usize, 10000usize, next_pp + 1)),
            b'+' => Some((1, 10000, next_pp + 1)),
            b'?' => Some((0, 1, next_pp + 1)),
            _ => None,
        }
    } else {
        None
    };

    if let Some((min, max, after_quant)) = quantifier {
        let mut count = 0;
        let mut positions = vec![tp];
        while count < max && tp < text.len() && matches_element(text[tp], &matcher) {
            tp += 1;
            count += 1;
            positions.push(tp);
        }
        while positions.len() > min {
            let try_pos = positions.pop().unwrap();
            if let Some(result) = regex_match_inner(text, try_pos, pat, after_quant) {
                return Some(result);
            }
        }
        if count >= min {
            if let Some(result) = regex_match_inner(text, positions.last().copied().unwrap_or(tp), pat, after_quant) {
                return Some(result);
            }
        }
        None
    } else {
        if tp < text.len() && matches_element(text[tp], &matcher) {
            regex_match_inner(text, tp + 1, pat, next_pp)
        } else if matches!(matcher, PatElement::Group(_)) {
            regex_match_inner(text, tp, pat, next_pp)
        } else {
            None
        }
    }
}

enum PatElement {
    Literal(u8),
    Dot,
    Digit,
    NonDigit,
    Word,
    NonWord,
    Space,
    NonSpace,
    CharClass(Vec<(u8, u8)>, bool),
    Group(Vec<u8>),
}

fn parse_pattern_element(pat: &[u8], pp: usize) -> Option<(PatElement, usize)> {
    if pp >= pat.len() { return None; }

    match pat[pp] {
        b'.' => Some((PatElement::Dot, pp + 1)),
        b'\\' => {
            if pp + 1 >= pat.len() { return Some((PatElement::Literal(b'\\'), pp + 1)); }
            match pat[pp + 1] {
                b'd' => Some((PatElement::Digit, pp + 2)),
                b'D' => Some((PatElement::NonDigit, pp + 2)),
                b'w' => Some((PatElement::Word, pp + 2)),
                b'W' => Some((PatElement::NonWord, pp + 2)),
                b's' => Some((PatElement::Space, pp + 2)),
                b'S' => Some((PatElement::NonSpace, pp + 2)),
                ch => Some((PatElement::Literal(ch), pp + 2)),
            }
        }
        b'[' => {
            let negated = pp + 1 < pat.len() && pat[pp + 1] == b'^';
            let start = if negated { pp + 2 } else { pp + 1 };
            let mut ranges = Vec::new();
            let mut i = start;
            while i < pat.len() && pat[i] != b']' {
                if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
                    ranges.push((pat[i], pat[i + 2]));
                    i += 3;
                } else if pat[i] == b'\\' && i + 1 < pat.len() {
                    ranges.push((pat[i + 1], pat[i + 1]));
                    i += 2;
                } else {
                    ranges.push((pat[i], pat[i]));
                    i += 1;
                }
            }
            let end = if i < pat.len() { i + 1 } else { i };
            Some((PatElement::CharClass(ranges, negated), end))
        }
        b'(' => {
            let mut depth = 1;
            let mut i = pp + 1;
            while i < pat.len() && depth > 0 {
                match pat[i] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b'\\' => { i += 1; }
                    _ => {}
                }
                if depth > 0 { i += 1; }
            }
            if depth == 0 {
                let inner = pat[pp+1..i].to_vec();
                Some((PatElement::Group(inner), i + 1))
            } else {
                Some((PatElement::Literal(b'('), pp + 1))
            }
        }
        b'|' => {
            Some((PatElement::Dot, pp + 1))
        }
        ch => Some((PatElement::Literal(ch), pp + 1)),
    }
}

fn matches_element(ch: u8, elem: &PatElement) -> bool {
    match elem {
        PatElement::Literal(expected) => ch == *expected,
        PatElement::Dot => ch != b'\n',
        PatElement::Digit => ch.is_ascii_digit(),
        PatElement::NonDigit => !ch.is_ascii_digit(),
        PatElement::Word => ch.is_ascii_alphanumeric() || ch == b'_',
        PatElement::NonWord => !ch.is_ascii_alphanumeric() && ch != b'_',
        PatElement::Space => matches!(ch, b' ' | b'\t' | b'\n' | b'\r'),
        PatElement::NonSpace => !matches!(ch, b' ' | b'\t' | b'\n' | b'\r'),
        PatElement::CharClass(ranges, negated) => {
            let in_class = ranges.iter().any(|(lo, hi)| ch >= *lo && ch <= *hi);
            if *negated { !in_class } else { in_class }
        }
        PatElement::Group(_) => true,
    }
}

// ---------------------------------------------------------------------------
// Number formatting helpers
// ---------------------------------------------------------------------------

fn number_to_string(n: f64, radix: u32) -> String {
    if radix == 10 { return format_number(n); }
    if n.is_nan() { return String::from("NaN"); }
    if n.is_infinite() { return if n > 0.0 { String::from("Infinity") } else { String::from("-Infinity") }; }

    let neg = n < 0.0;
    let mut val = if neg { -n } else { n } as u64;
    if val == 0 { return String::from("0"); }

    let mut digits = Vec::new();
    while val > 0 {
        let d = (val % radix as u64) as u8;
        digits.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        val /= radix as u64;
    }
    digits.reverse();

    let mut result = String::new();
    if neg { result.push('-'); }
    for d in digits { result.push(d as char); }
    result
}

fn to_fixed(n: f64, digits: usize) -> String {
    if digits == 0 {
        return format!("{}", round(n) as i64);
    }
    let factor = pow10(digits as i32);
    let rounded = round(n * factor) / factor;
    let int_part = trunc(rounded) as i64;
    let frac = (rounded - int_part as f64).abs();
    let frac_scaled = round(frac * factor) as u64;
    let mut frac_str = format!("{}", frac_scaled);
    while frac_str.len() < digits {
        frac_str.insert(0, '0');
    }
    format!("{}.{}", int_part, frac_str)
}

// ---------------------------------------------------------------------------
// Math functions (no libm in no_std)
// ---------------------------------------------------------------------------

fn floor(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() { return x; }
    let i = x as i64;
    if x < 0.0 && x != i as f64 { (i - 1) as f64 } else { i as f64 }
}

fn ceil(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() { return x; }
    let i = x as i64;
    if x > 0.0 && x != i as f64 { (i + 1) as f64 } else { i as f64 }
}

fn round(x: f64) -> f64 {
    floor(x + 0.5)
}

fn trunc(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() { return x; }
    x as i64 as f64
}

fn pow(base: f64, exp: f64) -> f64 {
    if exp == 0.0 { return 1.0; }
    if base == 0.0 { return if exp > 0.0 { 0.0 } else { f64::INFINITY }; }
    if exp == 1.0 { return base; }

    let exp_int = exp as i64;
    if exp == exp_int as f64 && exp_int.abs() < 100 {
        let mut result = 1.0;
        let mut b = base;
        let mut e = exp_int.unsigned_abs();
        while e > 0 {
            if e & 1 == 1 { result *= b; }
            b *= b;
            e >>= 1;
        }
        if exp_int < 0 { 1.0 / result } else { result }
    } else {
        exp_fn(exp * ln(base))
    }
}

fn sqrt(x: f64) -> f64 {
    if x < 0.0 { return f64::NAN; }
    if x == 0.0 { return 0.0; }
    let mut guess = x / 2.0;
    if guess == 0.0 { guess = 1.0; }
    for _ in 0..64 {
        let new_guess = (guess + x / guess) / 2.0;
        if (new_guess - guess).abs() < 1e-15 * guess.abs() { break; }
        guess = new_guess;
    }
    guess
}

fn cbrt(x: f64) -> f64 {
    if x == 0.0 { return 0.0; }
    let neg = x < 0.0;
    let x = if neg { -x } else { x };
    let mut guess = x / 3.0;
    if guess == 0.0 { guess = 1.0; }
    for _ in 0..64 {
        let new_guess = (2.0 * guess + x / (guess * guess)) / 3.0;
        if (new_guess - guess).abs() < 1e-15 * guess.abs() { break; }
        guess = new_guess;
    }
    if neg { -guess } else { guess }
}

fn ln(x: f64) -> f64 {
    if x <= 0.0 { return f64::NAN; }
    if x == 1.0 { return 0.0; }

    let mut k: i64 = 0;
    let mut f = x;
    while f >= 2.0 { f /= 2.0; k += 1; }
    while f < 1.0 { f *= 2.0; k -= 1; }

    let t = f - 1.0;
    let mut result = 0.0;
    let mut term = t;
    for n in 1..100 {
        result += term / n as f64;
        term *= -t;
        if term.abs() < 1e-15 { break; }
    }

    result + k as f64 * core::f64::consts::LN_2
}

fn exp_fn(x: f64) -> f64 {
    if x == 0.0 { return 1.0; }
    if x > 709.0 { return f64::INFINITY; }
    if x < -709.0 { return 0.0; }

    let mut result = 1.0;
    let mut term = 1.0;
    for n in 1..100 {
        term *= x / n as f64;
        result += term;
        if term.abs() < 1e-15 { break; }
    }
    result
}

fn sin(x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    let mut x = x % (2.0 * pi);
    if x > pi { x -= 2.0 * pi; }
    if x < -pi { x += 2.0 * pi; }

    let mut result = 0.0;
    let mut term = x;
    for n in 0..50 {
        result += term;
        term *= -x * x / ((2 * n + 2) as f64 * (2 * n + 3) as f64);
        if term.abs() < 1e-15 { break; }
    }
    result
}

fn cos(x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    let mut x = x % (2.0 * pi);
    if x > pi { x -= 2.0 * pi; }
    if x < -pi { x += 2.0 * pi; }

    let mut result = 0.0;
    let mut term = 1.0;
    for n in 0..50 {
        result += term;
        term *= -x * x / ((2 * n + 1) as f64 * (2 * n + 2) as f64);
        if term.abs() < 1e-15 { break; }
    }
    result
}

fn atan2(y: f64, x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    if x == 0.0 {
        if y > 0.0 { return pi / 2.0; }
        if y < 0.0 { return -pi / 2.0; }
        return 0.0;
    }
    let at = atan_approx(y / x);
    if x > 0.0 { at }
    else if y >= 0.0 { at + pi }
    else { at - pi }
}

fn atan_approx(x: f64) -> f64 {
    if x.abs() <= 1.0 {
        let mut result = 0.0;
        let mut term = x;
        let x2 = x * x;
        for n in 0..100 {
            result += term / (2 * n + 1) as f64;
            term *= -x2;
            if term.abs() < 1e-15 { break; }
        }
        result
    } else {
        let pi_2 = core::f64::consts::FRAC_PI_2;
        if x > 0.0 { pi_2 - atan_approx(1.0 / x) }
        else { -pi_2 - atan_approx(1.0 / x) }
    }
}
