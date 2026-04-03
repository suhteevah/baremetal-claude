//! Tree-walking evaluator for python-lite AST.
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use crate::parser::*;

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64), Float(f64), Str(String), Bool(bool),
    List(Vec<Value>), Dict(Vec<(Value, Value)>), Set(Vec<Value>), Tuple(Vec<Value>),
    None,
    Func { name: String, params: Vec<Param>, body: Vec<Stmt>, closure: BTreeMap<String, Value> },
    Class { name: String, bases: Vec<String>, methods: BTreeMap<String, Value>, class_attrs: BTreeMap<String, Value> },
    Instance { class_name: String, attrs: BTreeMap<String, Value> },
    Type(String),
    Generator(Vec<Value>),
    Module { name: String, attrs: BTreeMap<String, Value> },
    Exception { exc_type: String, message: String },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::None, Value::None) => true,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            _ => false,
        }
    }
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
            Value::List(items) => { let inner: Vec<String> = items.iter().map(|v| v.repr()).collect(); format!("[{}]", inner.join(", ")) }
            Value::Dict(pairs) => { let inner: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", k.repr(), v.repr())).collect(); format!("{{{}}}", inner.join(", ")) }
            Value::Set(items) => { if items.is_empty() { return String::from("set()"); } let inner: Vec<String> = items.iter().map(|v| v.repr()).collect(); format!("{{{}}}", inner.join(", ")) }
            Value::Tuple(items) => { if items.len() == 1 { format!("({},)", items[0].repr()) } else { let inner: Vec<String> = items.iter().map(|v| v.repr()).collect(); format!("({})", inner.join(", ")) } }
            Value::Func { name, .. } => format!("<function {}>", name),
            Value::Class { name, .. } => format!("<class '{}'>", name),
            Value::Instance { class_name, attrs } => { if let Some(s) = attrs.get("__str_cache__") { return s.display(); } format!("<{} instance>", class_name) }
            Value::Type(name) => format!("<class '{}'>", name),
            Value::Generator(_) => String::from("<generator object>"),
            Value::Module { name, .. } => format!("<module '{}'>", name),
            Value::Exception { exc_type, message } => { if message.is_empty() { exc_type.clone() } else { format!("{}: {}", exc_type, message) } }
        }
    }
    pub fn repr(&self) -> String { match self { Value::Str(s) => format!("'{}'", s), other => other.display() } }
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b, Value::Int(n) => *n != 0, Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(), Value::List(items) | Value::Set(items) => !items.is_empty(),
            Value::Dict(pairs) => !pairs.is_empty(), Value::Tuple(items) => !items.is_empty(),
            Value::None => false, _ => true,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int", Value::Float(_) => "float", Value::Str(_) => "str",
            Value::Bool(_) => "bool", Value::List(_) => "list", Value::Dict(_) => "dict",
            Value::Set(_) => "set", Value::Tuple(_) => "tuple", Value::None => "NoneType",
            Value::Func { .. } => "function", Value::Class { .. } => "type",
            Value::Instance { .. } => "object", Value::Type(_) => "type",
            Value::Generator(_) => "generator", Value::Module { .. } => "module",
            Value::Exception { .. } => "Exception",
        }
    }
    fn as_int(&self) -> Result<i64, String> {
        match self { Value::Int(n) => Ok(*n), Value::Float(f) => Ok(*f as i64), Value::Bool(b) => Ok(if *b { 1 } else { 0 }), _ => Err(format!("cannot convert {} to int", self.type_name())) }
    }
    fn as_float(&self) -> Result<f64, String> {
        match self { Value::Float(f) => Ok(*f), Value::Int(n) => Ok(*n as f64), Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }), _ => Err(format!("cannot convert {} to float", self.type_name())) }
    }
    fn key_string(&self) -> String {
        match self { Value::Int(n) => format!("i:{}", n), Value::Str(s) => format!("s:{}", s), Value::Bool(b) => format!("b:{}", b), Value::None => String::from("n:"), Value::Tuple(items) => { let inner: Vec<String> = items.iter().map(|v| v.key_string()).collect(); format!("t:({})", inner.join(",")) } _ => format!("o:{}", self.display()) }
    }
    fn get_class_name(&self) -> &str {
        match self { Value::Instance { class_name, .. } => class_name.as_str(), Value::Exception { exc_type, .. } => exc_type.as_str(), _ => self.type_name() }
    }
}

fn format_float(f: f64) -> String { if f.is_nan() { return String::from("nan"); } if f.is_infinite() { return if f > 0.0 { String::from("inf") } else { String::from("-inf") }; } format!("{}", f) }
fn floor_f64(x: f64) -> f64 { let i = x as i64; let fi = i as f64; if x < fi { fi - 1.0 } else { fi } }
fn ceil_f64(x: f64) -> f64 { let f = floor_f64(x); if x == f { f } else { f + 1.0 } }
fn abs_f64(x: f64) -> f64 { if x < 0.0 { -x } else { x } }

enum ControlFlow { Return(Value), Break, Continue, Exception(Value) }
type EvalResult = Result<Value, String>;
type StmtResult = Result<Option<ControlFlow>, String>;

pub struct Interpreter { globals: BTreeMap<String, Value>, output: String, call_depth: usize }
const MAX_CALL_DEPTH: usize = 256;
const MAX_ITERATIONS: usize = 100_000;

impl Interpreter {
    pub fn new() -> Self { Interpreter { globals: BTreeMap::new(), output: String::new(), call_depth: 0 } }
    pub fn take_output(&mut self) -> String { core::mem::take(&mut self.output) }
    pub fn exec_block(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        let mut scope = BTreeMap::new();
        for stmt in stmts {
            if let Some(cf) = self.exec_stmt(stmt, &mut scope)? {
                match cf {
                    ControlFlow::Return(_) => return Err(String::from("'return' outside function")),
                    ControlFlow::Break => return Err(String::from("'break' outside loop")),
                    ControlFlow::Continue => return Err(String::from("'continue' outside loop")),
                    ControlFlow::Exception(exc) => return Err(format!("Unhandled exception: {}", exc.display())),
                }
            }
        }
        Ok(())
    }
    fn exec_stmt(&mut self, stmt: &Stmt, scope: &mut BTreeMap<String, Value>) -> StmtResult {
        match stmt {
            Stmt::Expr(expr) => { self.eval_expr(expr, scope)?; Ok(None) }
            Stmt::Assign { target, value } => { let val = self.eval_expr(value, scope)?; self.assign(target, val, scope)?; Ok(None) }
            Stmt::AugAssign { target, op, value } => { let cur = self.eval_assign_target_value(target, scope)?; let rhs = self.eval_expr(value, scope)?; let result = eval_binop(*op, &cur, &rhs)?; self.assign(target, result, scope)?; Ok(None) }
            Stmt::If { condition, body, elif_clauses, else_body } => {
                if self.eval_expr(condition, scope)?.is_truthy() { return self.exec_stmts(body, scope); }
                for (ec, eb) in elif_clauses { if self.eval_expr(ec, scope)?.is_truthy() { return self.exec_stmts(eb, scope); } }
                if let Some(eb) = else_body { return self.exec_stmts(eb, scope); }
                Ok(None)
            }
            Stmt::While { condition, body } => {
                let mut iters = 0;
                loop {
                    if !self.eval_expr(condition, scope)?.is_truthy() { break; }
                    iters += 1; if iters > MAX_ITERATIONS { return Err(String::from("maximum loop iterations exceeded")); }
                    match self.exec_stmts(body, scope)? { Some(ControlFlow::Break) => break, Some(ControlFlow::Continue) => continue, Some(cf) => return Ok(Some(cf)), None => {} }
                }
                Ok(None)
            }
            Stmt::For { var, var_tuple, iterable, body } => {
                let iter_val = self.eval_expr(iterable, scope)?;
                let items = self.to_iterable(&iter_val)?;
                let mut iters = 0;
                for item in items {
                    iters += 1; if iters > MAX_ITERATIONS { return Err(String::from("maximum loop iterations exceeded")); }
                    if let Some(vars) = var_tuple {
                        let unpacked = self.to_iterable(&item)?;
                        for (v, val) in vars.iter().zip(unpacked.into_iter()) { scope.insert(v.clone(), val); }
                    } else { scope.insert(var.clone(), item); }
                    match self.exec_stmts(body, scope)? { Some(ControlFlow::Break) => break, Some(ControlFlow::Continue) => continue, Some(cf) => return Ok(Some(cf)), None => {} }
                }
                Ok(None)
            }
            Stmt::FuncDef { name, params, body, decorators } => {
                let mut func = Value::Func { name: name.clone(), params: params.clone(), body: body.clone(), closure: scope.clone() };
                for dec in decorators.iter().rev() { let d = self.eval_expr(dec, scope)?; func = self.call_value(&d, &[func], &[], scope)?; }
                scope.insert(name.clone(), func.clone()); self.globals.insert(name.clone(), func); Ok(None)
            }
            Stmt::ClassDef { name, bases, body, .. } => {
                let base_names: Vec<String> = bases.iter().map(|b| if let Expr::Name(n) = b { n.clone() } else { String::from("object") }).collect();
                let mut cs = BTreeMap::new();
                for (k, v) in self.globals.iter() { cs.insert(k.clone(), v.clone()); }
                for s in body { self.exec_stmt(s, &mut cs)?; }
                let mut methods = BTreeMap::new(); let mut cattrs = BTreeMap::new();
                for (k, v) in &cs {
                    if self.globals.contains_key(k) && !matches!(v, Value::Func { .. }) { continue; }
                    match v { Value::Func { .. } => { methods.insert(k.clone(), v.clone()); } _ => { cattrs.insert(k.clone(), v.clone()); } }
                }
                for bn in &base_names { if let Ok(bv) = self.lookup(bn, scope) { if let Value::Class { methods: bm, class_attrs: ba, .. } = &bv { for (k, v) in bm { if !methods.contains_key(k) { methods.insert(k.clone(), v.clone()); } } for (k, v) in ba { if !cattrs.contains_key(k) { cattrs.insert(k.clone(), v.clone()); } } } } }
                let class = Value::Class { name: name.clone(), bases: base_names, methods, class_attrs: cattrs };
                scope.insert(name.clone(), class.clone()); self.globals.insert(name.clone(), class); Ok(None)
            }
            Stmt::Return(expr) => Ok(Some(ControlFlow::Return(match expr { Some(e) => self.eval_expr(e, scope)?, None => Value::None }))),
            Stmt::Break => Ok(Some(ControlFlow::Break)),
            Stmt::Continue => Ok(Some(ControlFlow::Continue)),
            Stmt::Pass => Ok(None),
            Stmt::Del(expr) => {
                match expr { Expr::Name(n) => { scope.remove(n); self.globals.remove(n); } Expr::Index { obj, index } => { if let Expr::Name(n) = obj.as_ref() { let idx = self.eval_expr(index, scope)?; let c = self.lookup_mut(n, scope)?; match c { Value::Dict(pairs) => { let ks = idx.key_string(); pairs.retain(|(k, _)| k.key_string() != ks); } Value::List(items) => { let i = idx.as_int()? as usize; items.remove(i); } _ => {} } } } _ => {} }
                Ok(None)
            }
            Stmt::Raise(expr) => {
                let exc = if let Some(e) = expr { let v = self.eval_expr(e, scope)?; match &v { Value::Exception { .. } => v, _ => Value::Exception { exc_type: String::from("Exception"), message: v.display() } } } else { Value::Exception { exc_type: String::from("RuntimeError"), message: String::from("No active exception") } };
                Ok(Some(ControlFlow::Exception(exc)))
            }
            Stmt::Try { body, handlers, else_body, finally_body } => {
                let result = self.exec_stmts(body, scope);
                let mut handled = false;
                match result {
                    Ok(Some(ControlFlow::Exception(exc))) => {
                        let et = match &exc { Value::Exception { exc_type, .. } => exc_type.clone(), _ => String::from("Exception") };
                        for h in handlers { let m = match &h.exc_type { None => true, Some(t) => exception_matches(&et, t) }; if m { if let Some(n) = &h.name { scope.insert(n.clone(), exc.clone()); } let r = self.exec_stmts(&h.body, scope)?; if let Some(cf) = r { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Ok(Some(cf)); } handled = true; break; } }
                        if !handled { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Ok(Some(ControlFlow::Exception(exc))); }
                    }
                    Ok(Some(cf)) => { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Ok(Some(cf)); }
                    Ok(None) => { if let Some(eb) = else_body { let r = self.exec_stmts(eb, scope)?; if let Some(cf) = r { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Ok(Some(cf)); } } }
                    Err(e) => {
                        for h in handlers { let m = match &h.exc_type { None => true, Some(t) => exception_matches("RuntimeError", t) }; if m { let exc2 = Value::Exception { exc_type: String::from("RuntimeError"), message: e.clone() }; if let Some(n) = &h.name { scope.insert(n.clone(), exc2); } let r = self.exec_stmts(&h.body, scope)?; if let Some(cf) = r { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Ok(Some(cf)); } handled = true; break; } }
                        if !handled { if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; } return Err(e); }
                    }
                }
                if let Some(fb) = finally_body { self.exec_stmts(fb, scope)?; }
                Ok(None)
            }
            Stmt::With { context, var, body } => { let ctx = self.eval_expr(context, scope)?; if let Some(n) = var { scope.insert(n.clone(), ctx); } self.exec_stmts(body, scope) }
            Stmt::Import { module, alias } => { let mv = self.create_module(module)?; let n = alias.as_ref().unwrap_or(module); let an = if let Some(d) = n.find('.') { &n[..d] } else { n.as_str() }; scope.insert(String::from(an), mv.clone()); self.globals.insert(String::from(an), mv); Ok(None) }
            Stmt::FromImport { module, names } => {
                let mv = self.create_module(module)?;
                if let Value::Module { attrs, .. } = &mv { for (name, alias) in names { if name == "*" { for (k, v) in attrs { scope.insert(k.clone(), v.clone()); self.globals.insert(k.clone(), v.clone()); } } else { let val = attrs.get(name).cloned().unwrap_or(Value::None); let t = alias.as_ref().unwrap_or(name); scope.insert(t.clone(), val.clone()); self.globals.insert(t.clone(), val); } } }
                Ok(None)
            }
            Stmt::Global(names) => { for n in names { if let Some(v) = self.globals.get(n).cloned() { scope.insert(n.clone(), v); } } Ok(None) }
            Stmt::Nonlocal(_) | Stmt::YieldStmt(_) | Stmt::YieldFromStmt(_) => Ok(None),
        }
    }
    fn exec_stmts(&mut self, stmts: &[Stmt], scope: &mut BTreeMap<String, Value>) -> StmtResult { for s in stmts { if let Some(cf) = self.exec_stmt(s, scope)? { return Ok(Some(cf)); } } Ok(None) }
    fn assign(&mut self, target: &AssignTarget, value: Value, scope: &mut BTreeMap<String, Value>) -> Result<(), String> {
        match target {
            AssignTarget::Name(name) => { scope.insert(name.clone(), value.clone()); self.globals.insert(name.clone(), value); Ok(()) }
            AssignTarget::Index { obj, index } => {
                if let Expr::Name(name) = obj { let idx = self.eval_expr(index, scope)?; let c = self.lookup_mut(name, scope)?; match c { Value::List(items) => { let i = idx.as_int()?; let ai = if i < 0 { (items.len() as i64 + i) as usize } else { i as usize }; if ai >= items.len() { return Err(format!("list index out of range")); } items[ai] = value; } Value::Dict(pairs) => { let ks = idx.key_string(); for p in pairs.iter_mut() { if p.0.key_string() == ks { p.1 = value; return Ok(()); } } pairs.push((idx, value)); } _ => return Err(format!("not subscriptable")) } Ok(()) } else { Err(String::from("complex index assignment not supported")) }
            }
            AssignTarget::Attr { obj, attr } => {
                if let Expr::Name(name) = obj { let c = self.lookup_mut(name, scope)?; match c { Value::Instance { attrs, .. } | Value::Module { attrs, .. } => { attrs.insert(attr.clone(), value); } _ => return Err(format!("cannot set attribute")) } Ok(()) } else { Err(String::from("complex attr assignment not supported")) }
            }
            AssignTarget::Tuple(names) => {
                let items = self.to_iterable(&value)?;
                if items.len() != names.len() { return Err(format!("not enough values to unpack (expected {}, got {})", names.len(), items.len())); }
                for (n, v) in names.iter().zip(items.into_iter()) { scope.insert(n.clone(), v.clone()); self.globals.insert(n.clone(), v); }
                Ok(())
            }
        }
    }
    fn eval_assign_target_value(&mut self, target: &AssignTarget, scope: &mut BTreeMap<String, Value>) -> EvalResult {
        match target {
            AssignTarget::Name(n) => self.lookup(n, scope),
            AssignTarget::Index { obj, index } => { let o = self.eval_expr(obj, scope)?; let i = self.eval_expr(index, scope)?; eval_index(&o, &i) }
            AssignTarget::Attr { obj, attr } => { let o = self.eval_expr(obj, scope)?; match &o { Value::Instance { attrs, .. } => attrs.get(attr).cloned().ok_or_else(|| format!("no attribute '{}'", attr)), _ => Err(format!("no attribute '{}'", attr)) } }
            AssignTarget::Tuple(_) => Err(String::from("cannot augmented-assign to tuple")),
        }
    }
    fn lookup(&self, name: &str, scope: &BTreeMap<String, Value>) -> EvalResult {
        if let Some(v) = scope.get(name) { Ok(v.clone()) } else if let Some(v) = self.globals.get(name) { Ok(v.clone()) } else { Err(format!("name '{}' is not defined", name)) }
    }
    fn lookup_mut<'a>(&'a mut self, name: &str, scope: &'a mut BTreeMap<String, Value>) -> Result<&'a mut Value, String> {
        if scope.contains_key(name) { Ok(scope.get_mut(name).unwrap()) } else if self.globals.contains_key(name) { Ok(self.globals.get_mut(name).unwrap()) } else { Err(format!("name '{}' is not defined", name)) }
    }
    fn to_iterable(&self, val: &Value) -> Result<Vec<Value>, String> {
        match val { Value::List(i) => Ok(i.clone()), Value::Tuple(i) => Ok(i.clone()), Value::Set(i) => Ok(i.clone()), Value::Str(s) => Ok(s.chars().map(|c| Value::Str(String::from(c.to_string()))).collect()), Value::Dict(p) => Ok(p.iter().map(|(k, _)| k.clone()).collect()), Value::Generator(items) => Ok(items.clone()), _ => Err(format!("'{}' is not iterable", val.type_name())) }
    }

    fn eval_expr(&mut self, expr: &Expr, scope: &mut BTreeMap<String, Value>) -> EvalResult {
        match expr {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::FStr(parts) => { let mut r = String::new(); for p in parts { match p { FStrPart::Literal(s) => r.push_str(s), FStrPart::Expr(e) => r.push_str(&self.eval_expr(e, scope)?.display()) } } Ok(Value::Str(r)) }
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::None => Ok(Value::None),
            Expr::Name(name) => self.lookup(name, scope),
            Expr::List(elts) => { let mut items = Vec::new(); for e in elts { items.push(self.eval_expr(e, scope)?); } Ok(Value::List(items)) }
            Expr::Dict(pairs) => { let mut r = Vec::new(); for (k, v) in pairs { r.push((self.eval_expr(k, scope)?, self.eval_expr(v, scope)?)); } Ok(Value::Dict(r)) }
            Expr::Set(elts) => { let mut items = Vec::new(); for e in elts { let v = self.eval_expr(e, scope)?; let k = v.key_string(); if !items.iter().any(|x: &Value| x.key_string() == k) { items.push(v); } } Ok(Value::Set(items)) }
            Expr::Tuple(elts) => { let mut items = Vec::new(); for e in elts { items.push(self.eval_expr(e, scope)?); } Ok(Value::Tuple(items)) }
            Expr::BinOp { left, op, right } => { let l = self.eval_expr(left, scope)?; let r = self.eval_expr(right, scope)?; eval_binop(*op, &l, &r) }
            Expr::UnaryOp { op, operand } => { let v = self.eval_expr(operand, scope)?; match op { UnaryOp::Neg => match v { Value::Int(n) => Ok(Value::Int(-n)), Value::Float(f) => Ok(Value::Float(-f)), _ => Err(format!("bad operand for unary -")) }, UnaryOp::Not => Ok(Value::Bool(!v.is_truthy())), UnaryOp::BitNot => match v { Value::Int(n) => Ok(Value::Int(!n)), _ => Err(format!("bad operand for ~")) } } }
            Expr::Compare { left, ops, comparators } => {
                let mut cur = self.eval_expr(left, scope)?;
                for (op, ce) in ops.iter().zip(comparators.iter()) { let right = self.eval_expr(ce, scope)?; if !eval_compare(*op, &cur, &right)?.is_truthy() { return Ok(Value::Bool(false)); } cur = right; }
                Ok(Value::Bool(true))
            }
            Expr::BoolOp { left, op, right } => { let l = self.eval_expr(left, scope)?; match op { BoolOpKind::And => if !l.is_truthy() { Ok(l) } else { self.eval_expr(right, scope) }, BoolOpKind::Or => if l.is_truthy() { Ok(l) } else { self.eval_expr(right, scope) } } }
            Expr::Call { func, args, kwargs, .. } => {
                let mut av = Vec::new(); for a in args { av.push(self.eval_expr(a, scope)?); }
                let mut kv = Vec::new(); for (k, v) in kwargs { kv.push((k.clone(), self.eval_expr(v, scope)?)); }
                if let Expr::Attribute { obj, attr } = func.as_ref() { let ov = self.eval_expr(obj, scope)?; return self.call_method(obj, &ov, attr, &av, &kv, scope); }
                if let Expr::Name(name) = func.as_ref() { if let Some(r) = self.try_builtin(name, &av, &kv, scope)? { return Ok(r); } }
                let fv = self.eval_expr(func, scope)?;
                self.call_value(&fv, &av, &kv, scope)
            }
            Expr::Index { obj, index } => { let o = self.eval_expr(obj, scope)?; let i = self.eval_expr(index, scope)?; eval_index(&o, &i) }
            Expr::Slice { obj, lower, upper, step } => {
                let o = self.eval_expr(obj, scope)?;
                let lo = match lower { Some(e) => Some(self.eval_expr(e, scope)?.as_int()?), None => None };
                let hi = match upper { Some(e) => Some(self.eval_expr(e, scope)?.as_int()?), None => None };
                let st = match step { Some(e) => Some(self.eval_expr(e, scope)?.as_int()?), None => None };
                eval_slice(&o, lo, hi, st)
            }
            Expr::Attribute { obj, attr } => { let o = self.eval_expr(obj, scope)?; self.get_attribute(&o, attr) }
            Expr::Lambda { params, body } => {
                let ps: Vec<Param> = params.iter().map(|n| Param { name: n.clone(), default: None, is_args: false, is_kwargs: false }).collect();
                Ok(Value::Func { name: String::from("<lambda>"), params: ps, body: vec![Stmt::Return(Some(body.as_ref().clone()))], closure: scope.clone() })
            }
            Expr::IfExpr { body, test, orelse } => { if self.eval_expr(test, scope)?.is_truthy() { self.eval_expr(body, scope) } else { self.eval_expr(orelse, scope) } }
            Expr::ListComp { elt, generators } => { let mut r = Vec::new(); self.eval_comp(elt, generators, 0, scope, &mut r)?; Ok(Value::List(r)) }
            Expr::SetComp { elt, generators } => { let mut r = Vec::new(); self.eval_comp(elt, generators, 0, scope, &mut r)?; let mut u = Vec::new(); for v in r { let k = v.key_string(); if !u.iter().any(|x: &Value| x.key_string() == k) { u.push(v); } } Ok(Value::Set(u)) }
            Expr::DictComp { key, value, generators } => { let mut ks = Vec::new(); let mut vs = Vec::new(); self.eval_dict_comp(key, value, generators, 0, scope, &mut ks, &mut vs)?; Ok(Value::Dict(ks.into_iter().zip(vs.into_iter()).collect())) }
            Expr::GeneratorExp { elt, generators } => { let mut r = Vec::new(); self.eval_comp(elt, generators, 0, scope, &mut r)?; Ok(Value::Generator(r)) }
            Expr::Yield(e) => match e { Some(ex) => self.eval_expr(ex, scope), None => Ok(Value::None) },
            Expr::YieldFrom(e) => self.eval_expr(e, scope),
            Expr::Walrus { target, value } => { let v = self.eval_expr(value, scope)?; scope.insert(target.clone(), v.clone()); self.globals.insert(target.clone(), v.clone()); Ok(v) }
            Expr::Starred(e) => self.eval_expr(e, scope),
        }
    }
    fn eval_comp(&mut self, elt: &Expr, gens: &[Comprehension], gi: usize, scope: &mut BTreeMap<String, Value>, results: &mut Vec<Value>) -> Result<(), String> {
        if gi >= gens.len() { results.push(self.eval_expr(elt, scope)?); return Ok(()); }
        let g = &gens[gi]; let iv = self.eval_expr(&g.iter, scope)?; let items = self.to_iterable(&iv)?;
        for item in items {
            if let Some(vars) = &g.target_tuple { let u = self.to_iterable(&item)?; for (v, val) in vars.iter().zip(u.into_iter()) { scope.insert(v.clone(), val); } } else { scope.insert(g.target.clone(), item); }
            let mut pass = true; for ie in &g.ifs { if !self.eval_expr(ie, scope)?.is_truthy() { pass = false; break; } }
            if pass { self.eval_comp(elt, gens, gi + 1, scope, results)?; }
        }
        Ok(())
    }
    fn eval_dict_comp(&mut self, ke: &Expr, ve: &Expr, gens: &[Comprehension], gi: usize, scope: &mut BTreeMap<String, Value>, keys: &mut Vec<Value>, vals: &mut Vec<Value>) -> Result<(), String> {
        if gi >= gens.len() { keys.push(self.eval_expr(ke, scope)?); vals.push(self.eval_expr(ve, scope)?); return Ok(()); }
        let g = &gens[gi]; let iv = self.eval_expr(&g.iter, scope)?; let items = self.to_iterable(&iv)?;
        for item in items {
            if let Some(vars) = &g.target_tuple { let u = self.to_iterable(&item)?; for (v, val) in vars.iter().zip(u.into_iter()) { scope.insert(v.clone(), val); } } else { scope.insert(g.target.clone(), item); }
            let mut pass = true; for ie in &g.ifs { if !self.eval_expr(ie, scope)?.is_truthy() { pass = false; break; } }
            if pass { self.eval_dict_comp(ke, ve, gens, gi + 1, scope, keys, vals)?; }
        }
        Ok(())
    }
    fn get_attribute(&self, obj: &Value, attr: &str) -> EvalResult {
        match obj {
            Value::Instance { class_name, attrs } => { if let Some(v) = attrs.get(attr) { return Ok(v.clone()); } if let Some(c) = self.globals.get(class_name) { if let Value::Class { methods, class_attrs, .. } = c { if let Some(m) = methods.get(attr) { return Ok(m.clone()); } if let Some(a) = class_attrs.get(attr) { return Ok(a.clone()); } } } Err(format!("'{}' has no attribute '{}'", class_name, attr)) }
            Value::Module { attrs, .. } => attrs.get(attr).cloned().ok_or_else(|| format!("module has no attribute '{}'", attr)),
            Value::Class { methods, class_attrs, .. } => { if let Some(m) = methods.get(attr) { return Ok(m.clone()); } if let Some(a) = class_attrs.get(attr) { return Ok(a.clone()); } Err(format!("type has no attribute '{}'", attr)) }
            Value::Exception { exc_type, message } => match attr { "args" => Ok(Value::Tuple(vec![Value::Str(message.clone())])), "__class__" => Ok(Value::Type(exc_type.clone())), _ => Err(format!("Exception has no attribute '{}'", attr)) },
            _ => Err(format!("'{}' has no attribute '{}'", obj.type_name(), attr)),
        }
    }
    fn try_builtin(&mut self, name: &str, args: &[Value], kwargs: &[(String, Value)], scope: &mut BTreeMap<String, Value>) -> Result<Option<Value>, String> {
        match name {
            "print" => { let mut sep = String::from(" "); let mut end = String::from("\n"); for (k, v) in kwargs { match k.as_str() { "sep" => sep = v.display(), "end" => end = v.display(), _ => {} } } let parts: Vec<String> = args.iter().map(|v| v.display()).collect(); self.output.push_str(&parts.join(&sep)); self.output.push_str(&end); Ok(Some(Value::None)) }
            "len" => { if args.len() != 1 { return Err(String::from("len() takes exactly 1 argument")); } match &args[0] { Value::Str(s) => Ok(Some(Value::Int(s.chars().count() as i64))), Value::List(i) | Value::Set(i) => Ok(Some(Value::Int(i.len() as i64))), Value::Dict(p) => Ok(Some(Value::Int(p.len() as i64))), Value::Tuple(i) => Ok(Some(Value::Int(i.len() as i64))), _ => Err(format!("object of type '{}' has no len()", args[0].type_name())) } }
            "range" => { let (start, stop, step) = match args.len() { 1 => (0i64, args[0].as_int()?, 1i64), 2 => (args[0].as_int()?, args[1].as_int()?, 1i64), 3 => (args[0].as_int()?, args[1].as_int()?, args[2].as_int()?), _ => return Err(String::from("range() takes 1 to 3 arguments")) }; if step == 0 { return Err(String::from("range() step must not be zero")); } let mut items = Vec::new(); let mut i = start; if step > 0 { while i < stop { items.push(Value::Int(i)); i += step; if items.len() > MAX_ITERATIONS { return Err(String::from("range() too large")); } } } else { while i > stop { items.push(Value::Int(i)); i += step; if items.len() > MAX_ITERATIONS { return Err(String::from("range() too large")); } } } Ok(Some(Value::List(items))) }
            "str" => { if args.len() != 1 { return Err(String::from("str() takes exactly 1 argument")); } Ok(Some(Value::Str(args[0].display()))) }
            "int" => { if args.is_empty() { return Ok(Some(Value::Int(0))); } match &args[0] { Value::Int(n) => Ok(Some(Value::Int(*n))), Value::Float(f) => Ok(Some(Value::Int(*f as i64))), Value::Bool(b) => Ok(Some(Value::Int(if *b { 1 } else { 0 }))), Value::Str(s) => { let n: i64 = s.trim().parse().map_err(|_| format!("invalid literal for int(): '{}'", s))?; Ok(Some(Value::Int(n))) } _ => Err(format!("int() argument must be a string or number, not '{}'", args[0].type_name())) } }
            "float" => { if args.is_empty() { return Ok(Some(Value::Float(0.0))); } match &args[0] { Value::Float(f) => Ok(Some(Value::Float(*f))), Value::Int(n) => Ok(Some(Value::Float(*n as f64))), Value::Str(s) => { let f: f64 = s.trim().parse().map_err(|_| format!("could not convert string to float: '{}'", s))?; Ok(Some(Value::Float(f))) } _ => Err(String::from("float() argument must be a string or number")) } }
            "bool" => { if args.is_empty() { return Ok(Some(Value::Bool(false))); } Ok(Some(Value::Bool(args[0].is_truthy()))) }
            "list" => { if args.is_empty() { return Ok(Some(Value::List(Vec::new()))); } Ok(Some(Value::List(self.to_iterable(&args[0])?))) }
            "tuple" => { if args.is_empty() { return Ok(Some(Value::Tuple(Vec::new()))); } Ok(Some(Value::Tuple(self.to_iterable(&args[0])?))) }
            "set" => { if args.is_empty() { return Ok(Some(Value::Set(Vec::new()))); } let items = self.to_iterable(&args[0])?; let mut u = Vec::new(); for v in items { let k = v.key_string(); if !u.iter().any(|x: &Value| x.key_string() == k) { u.push(v); } } Ok(Some(Value::Set(u))) }
            "dict" => { if args.is_empty() && kwargs.is_empty() { return Ok(Some(Value::Dict(Vec::new()))); } let mut pairs = Vec::new(); if !args.is_empty() { let items = self.to_iterable(&args[0])?; for item in items { let p = self.to_iterable(&item)?; if p.len() == 2 { pairs.push((p[0].clone(), p[1].clone())); } } } for (k, v) in kwargs { pairs.push((Value::Str(k.clone()), v.clone())); } Ok(Some(Value::Dict(pairs))) }
            "type" => { if args.len() != 1 { return Err(String::from("type() takes exactly 1 argument")); } let tn = match &args[0] { Value::Instance { class_name, .. } => class_name.clone(), o => String::from(o.type_name()) }; Ok(Some(Value::Str(tn))) }
            "abs" => { if args.len() != 1 { return Err(String::from("abs() takes exactly 1 argument")); } match &args[0] { Value::Int(n) => Ok(Some(Value::Int(n.abs()))), Value::Float(f) => Ok(Some(Value::Float(abs_f64(*f)))), _ => Err(format!("bad operand type for abs()")) } }
            "min" => { if args.is_empty() { return Err(String::from("min() requires at least 1 argument")); } let items = if args.len() == 1 { self.to_iterable(&args[0])? } else { args.to_vec() }; if items.is_empty() { return Err(String::from("min() arg is an empty sequence")); } let mut best = items[0].clone(); for i in &items[1..] { if eval_compare(CmpOp::Lt, i, &best)? == Value::Bool(true) { best = i.clone(); } } Ok(Some(best)) }
            "max" => { if args.is_empty() { return Err(String::from("max() requires at least 1 argument")); } let items = if args.len() == 1 { self.to_iterable(&args[0])? } else { args.to_vec() }; if items.is_empty() { return Err(String::from("max() arg is an empty sequence")); } let mut best = items[0].clone(); for i in &items[1..] { if eval_compare(CmpOp::Gt, i, &best)? == Value::Bool(true) { best = i.clone(); } } Ok(Some(best)) }
            "sum" => { if args.is_empty() { return Err(String::from("sum() requires at least 1 argument")); } let items = self.to_iterable(&args[0])?; let mut total = Value::Int(0); for i in &items { total = eval_binop(BinOp::Add, &total, i)?; } Ok(Some(total)) }
            "sorted" => { if args.is_empty() { return Err(String::from("sorted() takes at least 1 argument")); } let mut s = self.to_iterable(&args[0])?; let rev = kwargs.iter().find(|(k, _)| k == "reverse").map(|(_, v)| v.is_truthy()).unwrap_or(false); for i in 1..s.len() { let mut j = i; while j > 0 { if eval_compare(CmpOp::Lt, &s[j], &s[j-1])? == Value::Bool(true) { s.swap(j, j-1); j -= 1; } else { break; } } } if rev { s.reverse(); } Ok(Some(Value::List(s))) }
            "reversed" => { if args.len() != 1 { return Err(String::from("reversed() takes exactly 1 argument")); } let mut i = self.to_iterable(&args[0])?; i.reverse(); Ok(Some(Value::List(i))) }
            "enumerate" => { if args.is_empty() { return Err(String::from("enumerate() requires at least 1 argument")); } let start = if args.len() > 1 { args[1].as_int()? } else { 0 }; let items = self.to_iterable(&args[0])?; Ok(Some(Value::List(items.iter().enumerate().map(|(i, v)| Value::Tuple(vec![Value::Int(i as i64 + start), v.clone()])).collect()))) }
            "zip" => { if args.is_empty() { return Ok(Some(Value::List(Vec::new()))); } let mut iters: Vec<Vec<Value>> = Vec::new(); for a in args { iters.push(self.to_iterable(a)?); } let ml = iters.iter().map(|v| v.len()).min().unwrap_or(0); let mut r = Vec::new(); for i in 0..ml { let mut t = Vec::new(); for it in &iters { t.push(it[i].clone()); } r.push(Value::Tuple(t)); } Ok(Some(Value::List(r))) }
            "map" => { if args.len() < 2 { return Err(String::from("map() requires at least 2 arguments")); } let f = args[0].clone(); let items = self.to_iterable(&args[1])?; let mut r = Vec::new(); for i in items { r.push(self.call_value(&f, &[i], &[], scope)?); } Ok(Some(Value::List(r))) }
            "filter" => { if args.len() != 2 { return Err(String::from("filter() takes exactly 2 arguments")); } let f = &args[0]; let items = self.to_iterable(&args[1])?; let mut r = Vec::new(); for i in items { let keep = if matches!(f, Value::None) { i.is_truthy() } else { self.call_value(f, &[i.clone()], &[], scope)?.is_truthy() }; if keep { r.push(i); } } Ok(Some(Value::List(r))) }
            "any" => { if args.len() != 1 { return Err(String::from("any() takes exactly 1 argument")); } let items = self.to_iterable(&args[0])?; Ok(Some(Value::Bool(items.iter().any(|v| v.is_truthy())))) }
            "all" => { if args.len() != 1 { return Err(String::from("all() takes exactly 1 argument")); } let items = self.to_iterable(&args[0])?; Ok(Some(Value::Bool(items.iter().all(|v| v.is_truthy())))) }
            "isinstance" => { if args.len() != 2 { return Err(String::from("isinstance() takes exactly 2 arguments")); } let r = match &args[1] { Value::Type(tn) => check_isinstance(&args[0], tn, &self.globals), Value::Str(tn) => args[0].type_name() == tn.as_str(), Value::Class { name, .. } => args[0].get_class_name() == name.as_str(), Value::Tuple(types) => types.iter().any(|t| match t { Value::Type(tn) => check_isinstance(&args[0], tn, &self.globals), Value::Class { name, .. } => args[0].get_class_name() == name.as_str(), _ => false }), _ => false }; Ok(Some(Value::Bool(r))) }
            "hasattr" => { if args.len() != 2 { return Err(String::from("hasattr() takes exactly 2 arguments")); } let a = args[1].display(); let r = match &args[0] { Value::Instance { attrs, .. } | Value::Module { attrs, .. } => attrs.contains_key(&a), _ => false }; Ok(Some(Value::Bool(r))) }
            "getattr" => { if args.len() < 2 { return Err(String::from("getattr() takes 2 or 3 arguments")); } let a = args[1].display(); let r = match &args[0] { Value::Instance { attrs, .. } | Value::Module { attrs, .. } => attrs.get(&a).cloned(), _ => None }; match r { Some(v) => Ok(Some(v)), None => if args.len() == 3 { Ok(Some(args[2].clone())) } else { Err(format!("no attribute '{}'", a)) } } }
            "repr" => { if args.len() != 1 { return Err(String::from("repr() takes exactly 1 argument")); } Ok(Some(Value::Str(args[0].repr()))) }
            "chr" => { if args.len() != 1 { return Err(String::from("chr() takes exactly 1 argument")); } let n = args[0].as_int()? as u32; let c = char::from_u32(n).ok_or_else(|| String::from("chr() arg not in range"))?; Ok(Some(Value::Str(String::from(c.to_string())))) }
            "ord" => { if args.len() != 1 { return Err(String::from("ord() takes exactly 1 argument")); } if let Value::Str(s) = &args[0] { if s.chars().count() != 1 { return Err(String::from("ord() expected a character")); } Ok(Some(Value::Int(s.chars().next().unwrap() as i64))) } else { Err(String::from("ord() expected string")) } }
            "hex" => { if args.len() != 1 { return Err(String::from("hex() takes exactly 1 argument")); } let n = args[0].as_int()?; Ok(Some(Value::Str(if n < 0 { format!("-0x{:x}", -n) } else { format!("0x{:x}", n) }))) }
            "bin" => { if args.len() != 1 { return Err(String::from("bin() takes exactly 1 argument")); } let n = args[0].as_int()?; Ok(Some(Value::Str(if n < 0 { format!("-0b{:b}", -n) } else { format!("0b{:b}", n) }))) }
            "round" => { if args.is_empty() { return Err(String::from("round() takes 1 or 2 arguments")); } let nd = if args.len() == 2 { args[1].as_int()? } else { 0 }; let f = args[0].as_float()?; let fac = pow_float(10.0, nd as f64); let r = floor_f64(f * fac + 0.5) / fac; if nd <= 0 { Ok(Some(Value::Int(r as i64))) } else { Ok(Some(Value::Float(r))) } }
            "divmod" => { if args.len() != 2 { return Err(String::from("divmod() takes exactly 2 arguments")); } let a = args[0].as_int()?; let b = args[1].as_int()?; if b == 0 { return Err(String::from("division by zero")); } let q = if (a ^ b) < 0 && a % b != 0 { a / b - 1 } else { a / b }; Ok(Some(Value::Tuple(vec![Value::Int(q), Value::Int(a - q * b)]))) }
            "pow" => { if args.len() < 2 { return Err(String::from("pow() takes 2 or 3 arguments")); } if args.len() == 3 { let m = args[2].as_int()?; if m == 0 { return Err(String::from("pow() 3rd argument cannot be 0")); } return Ok(Some(Value::Int(pow_mod(args[0].as_int()?, args[1].as_int()?, m)))); } eval_binop(BinOp::Pow, &args[0], &args[1]).map(Some) }
            "input" => { if args.len() == 1 { self.output.push_str(&args[0].display()); } Ok(Some(Value::Str(String::new()))) }
            "callable" => { if args.len() != 1 { return Err(String::from("callable() takes exactly 1 argument")); } Ok(Some(Value::Bool(matches!(&args[0], Value::Func { .. } | Value::Class { .. } | Value::Type(_))))) }
            "id" => Ok(Some(Value::Int(0))),
            "hash" => { if args.len() != 1 { return Err(String::from("hash() takes exactly 1 argument")); } match &args[0] { Value::Int(n) => Ok(Some(Value::Int(*n))), Value::Str(s) => { let mut h: i64 = 0; for c in s.chars() { h = h.wrapping_mul(31).wrapping_add(c as i64); } Ok(Some(Value::Int(h))) } _ => Err(format!("unhashable type: '{}'", args[0].type_name())) } }
            "next" => { if args.is_empty() { return Err(String::from("next() requires at least 1 argument")); } match &args[0] { Value::Generator(items) => { if items.is_empty() { if args.len() > 1 { Ok(Some(args[1].clone())) } else { Err(String::from("StopIteration")) } } else { Ok(Some(items[0].clone())) } } _ => Err(format!("'{}' is not an iterator", args[0].type_name())) } }
            "iter" => { Ok(Some(args[0].clone())) }
            "format" => { if args.is_empty() { return Err(String::from("format() takes at least 1 argument")); } Ok(Some(Value::Str(args[0].display()))) }
            "super" => Ok(Some(Value::None)),
            "Exception" | "ValueError" | "TypeError" | "KeyError" | "IndexError" | "AttributeError" | "RuntimeError" | "StopIteration" | "FileNotFoundError" | "ZeroDivisionError" | "NotImplementedError" | "OverflowError" | "IOError" | "NameError" | "ImportError" => { let msg = if args.is_empty() { String::new() } else { args[0].display() }; Ok(Some(Value::Exception { exc_type: String::from(name), message: msg })) }
            _ => Ok(None),
        }
    }

    fn call_method(&mut self, obj_expr: &Expr, obj_val: &Value, method: &str, args: &[Value], _kwargs: &[(String, Value)], scope: &mut BTreeMap<String, Value>) -> EvalResult {
        match (obj_val, method) {
            (Value::List(_), "append") => { if args.len() != 1 { return Err(String::from("append() takes exactly 1 argument")); } if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { items.push(args[0].clone()); return Ok(Value::None); } } Err(String::from("cannot append")) }
            (Value::List(_), "pop") => { if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { if items.is_empty() { return Err(String::from("pop from empty list")); } return Ok(if args.is_empty() { items.pop().unwrap() } else { let idx = args[0].as_int()? as usize; if idx >= items.len() { return Err(format!("pop index out of range")); } items.remove(idx) }); } } Err(String::from("cannot pop")) }
            (Value::List(_), "insert") => { if args.len() != 2 { return Err(String::from("insert() takes exactly 2 arguments")); } if let Expr::Name(n) = obj_expr { let idx = args[0].as_int()? as usize; let val = args[1].clone(); let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { let pos = if idx > items.len() { items.len() } else { idx }; items.insert(pos, val); return Ok(Value::None); } } Err(String::from("cannot insert")) }
            (Value::List(_), "extend") => { if args.len() != 1 { return Err(String::from("extend() takes exactly 1 argument")); } let ni = self.to_iterable(&args[0])?; if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { items.extend(ni); return Ok(Value::None); } } Err(String::from("cannot extend")) }
            (Value::List(items), "index") => { for (i, item) in items.iter().enumerate() { if item == &args[0] { return Ok(Value::Int(i as i64)); } } Err(format!("{} is not in list", args[0].repr())) }
            (Value::List(items), "count") => { Ok(Value::Int(items.iter().filter(|i| *i == &args[0]).count() as i64)) }
            (Value::List(_), "reverse") => { if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { items.reverse(); return Ok(Value::None); } } Err(String::from("cannot reverse")) }
            (Value::List(_), "sort") => { if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { for i in 1..items.len() { let mut j = i; while j > 0 { if let Ok(Value::Bool(true)) = eval_compare(CmpOp::Lt, &items[j], &items[j-1]) { items.swap(j, j-1); j -= 1; } else { break; } } } return Ok(Value::None); } } Err(String::from("cannot sort")) }
            (Value::List(_), "clear") => { if let Expr::Name(n) = obj_expr { let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { items.clear(); return Ok(Value::None); } } Err(String::from("cannot clear")) }
            (Value::List(items), "copy") => Ok(Value::List(items.clone())),
            (Value::List(_), "remove") => { if args.len() != 1 { return Err(String::from("remove() takes exactly 1 argument")); } if let Expr::Name(n) = obj_expr { let tgt = args[0].clone(); let l = self.lookup_mut(n, scope)?; if let Value::List(items) = l { for i in 0..items.len() { if items[i] == tgt { items.remove(i); return Ok(Value::None); } } return Err(format!("{} not in list", tgt.repr())); } } Err(String::from("cannot remove")) }
            // String methods
            (Value::Str(s), "upper") => Ok(Value::Str(s.to_uppercase())),
            (Value::Str(s), "lower") => Ok(Value::Str(s.to_lowercase())),
            (Value::Str(s), "strip") => Ok(Value::Str(String::from(s.trim()))),
            (Value::Str(s), "lstrip") => Ok(Value::Str(String::from(s.trim_start()))),
            (Value::Str(s), "rstrip") => Ok(Value::Str(String::from(s.trim_end()))),
            (Value::Str(s), "capitalize") => { let mut c = s.chars(); Ok(Value::Str(match c.next() { None => String::new(), Some(f) => format!("{}{}", f.to_uppercase().collect::<String>(), c.collect::<String>().to_lowercase()) })) }
            (Value::Str(s), "title") => { let mut r = String::new(); let mut cap = true; for c in s.chars() { if c.is_whitespace() || !c.is_alphanumeric() { r.push(c); cap = true; } else if cap { for uc in c.to_uppercase() { r.push(uc); } cap = false; } else { for lc in c.to_lowercase() { r.push(lc); } } } Ok(Value::Str(r)) }
            (Value::Str(s), "startswith") => { if let Value::Str(p) = &args[0] { Ok(Value::Bool(s.starts_with(p.as_str()))) } else { Err(String::from("startswith() argument must be str")) } }
            (Value::Str(s), "endswith") => { if let Value::Str(p) = &args[0] { Ok(Value::Bool(s.ends_with(p.as_str()))) } else { Err(String::from("endswith() argument must be str")) } }
            (Value::Str(s), "find") => { if let Value::Str(sub) = &args[0] { Ok(Value::Int(s.find(sub.as_str()).map(|p| p as i64).unwrap_or(-1))) } else { Err(String::from("find() argument must be str")) } }
            (Value::Str(s), "rfind") => { if let Value::Str(sub) = &args[0] { Ok(Value::Int(s.rfind(sub.as_str()).map(|p| p as i64).unwrap_or(-1))) } else { Err(String::from("rfind() argument must be str")) } }
            (Value::Str(s), "replace") => { if let (Value::Str(old), Value::Str(new)) = (&args[0], &args[1]) { Ok(Value::Str(s.replace(old.as_str(), new.as_str()))) } else { Err(String::from("replace() arguments must be strings")) } }
            (Value::Str(s), "split") => { let parts: Vec<Value> = if args.is_empty() { s.split_whitespace().map(|p| Value::Str(String::from(p))).collect() } else if let Value::Str(sep) = &args[0] { s.split(sep.as_str()).map(|p| Value::Str(String::from(p))).collect() } else { return Err(String::from("split() argument must be str")); }; Ok(Value::List(parts)) }
            (Value::Str(s), "join") => { if args.len() != 1 { return Err(String::from("join() takes exactly 1 argument")); } let items = self.to_iterable(&args[0])?; let strs: Result<Vec<String>, String> = items.iter().map(|v| match v { Value::Str(s) => Ok(s.clone()), _ => Err(format!("join() item must be str")) }).collect(); Ok(Value::Str(strs?.join(s.as_str()))) }
            (Value::Str(s), "isdigit") => Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))),
            (Value::Str(s), "isalpha") => Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic()))),
            (Value::Str(s), "isalnum") => Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric()))),
            (Value::Str(s), "count") => { if let Value::Str(sub) = &args[0] { Ok(Value::Int(s.matches(sub.as_str()).count() as i64)) } else { Err(String::from("count() argument must be str")) } }
            (Value::Str(s), "format") => { let mut result = String::new(); let chars: Vec<char> = s.chars().collect(); let mut i = 0; let mut ai = 0; while i < chars.len() { if chars[i] == '{' { if i + 1 < chars.len() && chars[i+1] == '{' { result.push('{'); i += 2; } else { let mut j = i+1; while j < chars.len() && chars[j] != '}' { j += 1; } let spec: String = chars[i+1..j].iter().collect(); let idx = if spec.is_empty() { let x = ai; ai += 1; x } else if let Ok(n) = spec.parse::<usize>() { n } else { ai }; if idx < args.len() { result.push_str(&args[idx].display()); } i = j + 1; } } else if chars[i] == '}' && i + 1 < chars.len() && chars[i+1] == '}' { result.push('}'); i += 2; } else { result.push(chars[i]); i += 1; } } Ok(Value::Str(result)) }
            (Value::Str(s), "encode") => Ok(Value::List(s.as_bytes().iter().map(|b| Value::Int(*b as i64)).collect())),
            (Value::Str(s), "splitlines") => Ok(Value::List(s.lines().map(|l| Value::Str(String::from(l))).collect())),
            // Dict methods
            (Value::Dict(pairs), "keys") => Ok(Value::List(pairs.iter().map(|(k, _)| k.clone()).collect())),
            (Value::Dict(pairs), "values") => Ok(Value::List(pairs.iter().map(|(_, v)| v.clone()).collect())),
            (Value::Dict(pairs), "items") => Ok(Value::List(pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect())),
            (Value::Dict(pairs), "get") => { if args.is_empty() { return Err(String::from("get() takes at least 1 argument")); } let ks = args[0].key_string(); let def = if args.len() > 1 { args[1].clone() } else { Value::None }; for (k, v) in pairs { if k.key_string() == ks { return Ok(v.clone()); } } Ok(def) }
            (Value::Dict(_), "pop") => { if args.is_empty() { return Err(String::from("pop() takes at least 1 argument")); } if let Expr::Name(n) = obj_expr { let ks = args[0].key_string(); let def = if args.len() > 1 { Some(args[1].clone()) } else { None }; let d = self.lookup_mut(n, scope)?; if let Value::Dict(pairs) = d { for i in 0..pairs.len() { if pairs[i].0.key_string() == ks { let (_, v) = pairs.remove(i); return Ok(v); } } if let Some(dv) = def { return Ok(dv); } return Err(format!("KeyError: {}", args[0].repr())); } } Err(String::from("cannot pop")) }
            (Value::Dict(_), "update") => { if args.len() != 1 { return Err(String::from("update() takes exactly 1 argument")); } if let Expr::Name(n) = obj_expr { if let Value::Dict(np) = &args[0] { let npc = np.clone(); let d = self.lookup_mut(n, scope)?; if let Value::Dict(pairs) = d { for (nk, nv) in npc { let ks = nk.key_string(); let mut found = false; for p in pairs.iter_mut() { if p.0.key_string() == ks { p.1 = nv.clone(); found = true; break; } } if !found { pairs.push((nk, nv)); } } return Ok(Value::None); } } } Err(String::from("cannot update")) }
            (Value::Dict(_), "setdefault") => { if args.is_empty() { return Err(String::from("setdefault() takes at least 1 argument")); } if let Expr::Name(n) = obj_expr { let key = args[0].clone(); let ks = key.key_string(); let def = if args.len() > 1 { args[1].clone() } else { Value::None }; let d = self.lookup_mut(n, scope)?; if let Value::Dict(pairs) = d { for (k, v) in pairs.iter() { if k.key_string() == ks { return Ok(v.clone()); } } pairs.push((key, def.clone())); return Ok(def); } } Err(String::from("cannot setdefault")) }
            (Value::Dict(pairs), "copy") => Ok(Value::Dict(pairs.clone())),
            (Value::Dict(_), "clear") => { if let Expr::Name(n) = obj_expr { let d = self.lookup_mut(n, scope)?; if let Value::Dict(pairs) = d { pairs.clear(); return Ok(Value::None); } } Err(String::from("cannot clear")) }
            // Set methods
            (Value::Set(_), "add") => { if args.len() != 1 { return Err(String::from("add() takes exactly 1 argument")); } if let Expr::Name(n) = obj_expr { let val = args[0].clone(); let key = val.key_string(); let s = self.lookup_mut(n, scope)?; if let Value::Set(items) = s { if !items.iter().any(|v| v.key_string() == key) { items.push(val); } return Ok(Value::None); } } Err(String::from("cannot add")) }
            (Value::Set(_), "remove") | (Value::Set(_), "discard") => { if args.len() != 1 { return Err(String::from("remove()/discard() takes exactly 1 argument")); } if let Expr::Name(n) = obj_expr { let key = args[0].key_string(); let is_rem = method == "remove"; let s = self.lookup_mut(n, scope)?; if let Value::Set(items) = s { let pos = items.iter().position(|v| v.key_string() == key); if let Some(i) = pos { items.remove(i); } else if is_rem { return Err(format!("KeyError: {}", args[0].repr())); } return Ok(Value::None); } } Err(String::from("cannot remove")) }
            (Value::Set(items), "union") => { let other = self.to_iterable(&args[0])?; let mut r = items.clone(); for v in other { let k = v.key_string(); if !r.iter().any(|x| x.key_string() == k) { r.push(v); } } Ok(Value::Set(r)) }
            (Value::Set(items), "intersection") => { let other = self.to_iterable(&args[0])?; let ok: Vec<String> = other.iter().map(|v| v.key_string()).collect(); Ok(Value::Set(items.iter().filter(|v| ok.contains(&v.key_string())).cloned().collect())) }
            (Value::Set(items), "difference") => { let other = self.to_iterable(&args[0])?; let ok: Vec<String> = other.iter().map(|v| v.key_string()).collect(); Ok(Value::Set(items.iter().filter(|v| !ok.contains(&v.key_string())).cloned().collect())) }
            (Value::Set(items), "copy") => Ok(Value::Set(items.clone())),
            (Value::Tuple(items), "count") => { Ok(Value::Int(items.iter().filter(|i| *i == &args[0]).count() as i64)) }
            (Value::Tuple(items), "index") => { for (i, item) in items.iter().enumerate() { if item == &args[0] { return Ok(Value::Int(i as i64)); } } Err(format!("{} is not in tuple", args[0].repr())) }
            // Instance method calls
            (Value::Instance { class_name, attrs }, _) => {
                if let Some(cv) = self.globals.get(class_name).cloned() { if let Value::Class { methods, .. } = &cv { if let Some(mv) = methods.get(method) { let mut aa = vec![obj_val.clone()]; aa.extend(args.iter().cloned()); return self.call_value(mv, &aa, &[], scope); } } }
                if let Some(av) = attrs.get(method) { return self.call_value(av, args, &[], scope); }
                Err(format!("'{}' has no method '{}'", class_name, method))
            }
            (Value::Module { attrs, .. }, _) => { if let Some(av) = attrs.get(method) { return self.call_value(av, args, &[], scope); } Err(format!("module has no attribute '{}'", method)) }
            _ => Err(format!("'{}' has no method '{}'", obj_val.type_name(), method)),
        }
    }
    fn call_value(&mut self, fv: &Value, args: &[Value], kwargs: &[(String, Value)], scope: &mut BTreeMap<String, Value>) -> EvalResult {
        match fv {
            Value::Func { name, params, body, closure } => {
                if body.is_empty() && !name.is_empty() { if let Some(r) = call_module_func(self, name, args, scope)? { return Ok(r); } }
                self.call_func(params, body, args, kwargs, Some(closure))
            }
            Value::Class { name, methods, class_attrs, .. } => {
                let mut ia = BTreeMap::new(); for (k, v) in class_attrs { ia.insert(k.clone(), v.clone()); }
                let mut inst = Value::Instance { class_name: name.clone(), attrs: ia };
                if let Some(init) = methods.get("__init__") { let mut aa = vec![inst.clone()]; aa.extend(args.iter().cloned()); let r = self.call_value(init, &aa, kwargs, scope)?; if let Value::Instance { .. } = &r { inst = r; } }
                Ok(inst)
            }
            Value::Type(tn) => match tn.as_str() {
                "int" => self.try_builtin("int", args, kwargs, scope).map(|o| o.unwrap_or(Value::Int(0))),
                "float" => self.try_builtin("float", args, kwargs, scope).map(|o| o.unwrap_or(Value::Float(0.0))),
                "str" => self.try_builtin("str", args, kwargs, scope).map(|o| o.unwrap_or(Value::Str(String::new()))),
                "bool" => self.try_builtin("bool", args, kwargs, scope).map(|o| o.unwrap_or(Value::Bool(false))),
                "list" => self.try_builtin("list", args, kwargs, scope).map(|o| o.unwrap_or(Value::List(Vec::new()))),
                "dict" => self.try_builtin("dict", args, kwargs, scope).map(|o| o.unwrap_or(Value::Dict(Vec::new()))),
                "tuple" => self.try_builtin("tuple", args, kwargs, scope).map(|o| o.unwrap_or(Value::Tuple(Vec::new()))),
                "set" => self.try_builtin("set", args, kwargs, scope).map(|o| o.unwrap_or(Value::Set(Vec::new()))),
                _ => Err(format!("cannot call type '{}'", tn)),
            },
            _ => Err(format!("'{}' is not callable", fv.type_name())),
        }
    }
    fn call_func(&mut self, params: &[Param], body: &[Stmt], args: &[Value], kwargs: &[(String, Value)], closure: Option<&BTreeMap<String, Value>>) -> EvalResult {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH { self.call_depth -= 1; return Err(String::from("maximum recursion depth exceeded")); }
        let mut ls = BTreeMap::new();
        if let Some(cl) = closure { for (k, v) in cl { ls.insert(k.clone(), v.clone()); } }
        for (k, v) in self.globals.iter() { if !ls.contains_key(k) { ls.insert(k.clone(), v.clone()); } }
        let mut ai = 0;
        for p in params {
            if p.is_kwargs { let mut kd = Vec::new(); for (k, v) in kwargs { kd.push((Value::Str(k.clone()), v.clone())); } ls.insert(p.name.clone(), Value::Dict(kd)); }
            else if p.is_args { ls.insert(p.name.clone(), Value::Tuple(args[ai..].to_vec())); ai = args.len(); }
            else {
                let val = if ai < args.len() { let v = args[ai].clone(); ai += 1; v }
                else if let Some((_, v)) = kwargs.iter().find(|(k, _)| *k == p.name) { v.clone() }
                else if let Some(def) = &p.default { let mut tmp = ls.clone(); self.eval_expr(def, &mut tmp)? }
                else { self.call_depth -= 1; return Err(format!("missing required argument: '{}'", p.name)); };
                ls.insert(p.name.clone(), val);
            }
        }
        let result = self.exec_stmts(body, &mut ls);
        if let Some(sp) = params.first() {
            if sp.name == "self" { if let Some(Value::Instance { attrs, class_name }) = ls.get("self") { let updated = Value::Instance { attrs: attrs.clone(), class_name: class_name.clone() }; self.call_depth -= 1; return match result { Ok(Some(ControlFlow::Return(v))) => Ok(v), Ok(None) => Ok(updated), Err(e) => Err(e), _ => Ok(updated) }; } }
        }
        self.call_depth -= 1;
        match result { Ok(Some(ControlFlow::Return(v))) => Ok(v), Ok(Some(ControlFlow::Break)) => Err(String::from("'break' outside loop")), Ok(Some(ControlFlow::Continue)) => Err(String::from("'continue' outside loop")), Ok(Some(ControlFlow::Exception(e))) => Err(format!("Unhandled exception: {}", e.display())), Ok(None) => Ok(Value::None), Err(e) => Err(e) }
    }
    fn create_module(&self, name: &str) -> Result<Value, String> {
        let mut attrs = BTreeMap::new();
        match name {
            "math" => {
                attrs.insert(String::from("pi"), Value::Float(core::f64::consts::PI));
                attrs.insert(String::from("e"), Value::Float(core::f64::consts::E));
                attrs.insert(String::from("inf"), Value::Float(f64::INFINITY));
                attrs.insert(String::from("tau"), Value::Float(core::f64::consts::PI * 2.0));
                for f in &["sqrt","sin","cos","tan","exp","log","log2","log10","floor","ceil","fabs","pow","asin","acos","atan","atan2","sinh","cosh","tanh","degrees","radians","isnan","isinf","factorial","gcd"] { attrs.insert(String::from(*f), Value::Func { name: format!("math.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); }
            }
            "random" => { for f in &["random","randint","choice","shuffle","uniform","seed"] { attrs.insert(String::from(*f), Value::Func { name: format!("random.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); } }
            "string" => { attrs.insert(String::from("ascii_letters"), Value::Str(String::from("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"))); attrs.insert(String::from("digits"), Value::Str(String::from("0123456789"))); attrs.insert(String::from("ascii_lowercase"), Value::Str(String::from("abcdefghijklmnopqrstuvwxyz"))); attrs.insert(String::from("ascii_uppercase"), Value::Str(String::from("ABCDEFGHIJKLMNOPQRSTUVWXYZ"))); }
            "json" => { for f in &["dumps","loads"] { attrs.insert(String::from(*f), Value::Func { name: format!("json.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); } }
            "os" | "os.path" => {
                let mut pa = BTreeMap::new();
                for f in &["join","exists","basename","dirname","splitext","isfile","isdir"] { pa.insert(String::from(*f), Value::Func { name: format!("os.path.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); }
                attrs.insert(String::from("path"), Value::Module { name: String::from("os.path"), attrs: pa });
                attrs.insert(String::from("sep"), Value::Str(String::from("/")));
                for f in &["getcwd","listdir","getenv"] { attrs.insert(String::from(*f), Value::Func { name: format!("os.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); }
            }
            "sys" => { attrs.insert(String::from("argv"), Value::List(vec![Value::Str(String::from("python-lite"))])); attrs.insert(String::from("platform"), Value::Str(String::from("claudioos"))); attrs.insert(String::from("version"), Value::Str(String::from("0.1.0"))); attrs.insert(String::from("maxsize"), Value::Int(i64::MAX)); attrs.insert(String::from("exit"), Value::Func { name: String::from("sys.exit"), params: vec![], body: Vec::new(), closure: BTreeMap::new() }); }
            "time" => { for f in &["time","sleep","monotonic"] { attrs.insert(String::from(*f), Value::Func { name: format!("time.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); } }
            "collections" => { for f in &["OrderedDict","defaultdict","Counter","deque"] { attrs.insert(String::from(*f), Value::Func { name: format!("collections.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); } }
            "functools" => { for f in &["reduce","partial","lru_cache","wraps"] { attrs.insert(String::from(*f), Value::Func { name: format!("functools.{}", f), params: vec![Param { name: String::from("x"), default: None, is_args: true, is_kwargs: false }], body: Vec::new(), closure: BTreeMap::new() }); } }
            _ => return Err(format!("ModuleNotFoundError: No module named '{}'", name)),
        }
        Ok(Value::Module { name: String::from(name), attrs })
    }
} // end impl Interpreter

fn exception_matches(et: &str, ht: &str) -> bool { if ht == "Exception" || ht == "BaseException" { return true; } et == ht }
fn check_isinstance(val: &Value, tn: &str, globals: &BTreeMap<String, Value>) -> bool {
    match tn { "int" => matches!(val, Value::Int(_)), "float" => matches!(val, Value::Float(_)), "str" => matches!(val, Value::Str(_)), "bool" => matches!(val, Value::Bool(_)), "list" => matches!(val, Value::List(_)), "dict" => matches!(val, Value::Dict(_)), "tuple" => matches!(val, Value::Tuple(_)), "set" => matches!(val, Value::Set(_)), "NoneType" => matches!(val, Value::None), "object" => true, _ => { if let Value::Instance { class_name, .. } = val { if class_name == tn { return true; } if let Some(Value::Class { bases, .. }) = globals.get(class_name) { return bases.iter().any(|b| b == tn); } } false } }
}

fn eval_binop(op: BinOp, left: &Value, right: &Value) -> EvalResult {
    if let (BinOp::Add, Value::Str(a), Value::Str(b)) = (op, left, right) { return Ok(Value::Str(format!("{}{}", a, b))); }
    if let (BinOp::Mul, Value::Str(s), Value::Int(n)) = (op, left, right) { let n = *n; if n <= 0 { return Ok(Value::Str(String::new())); } let mut r = String::new(); for _ in 0..n { r.push_str(s); } return Ok(Value::Str(r)); }
    if let (BinOp::Mul, Value::Int(n), Value::Str(s)) = (op, left, right) { let n = *n; if n <= 0 { return Ok(Value::Str(String::new())); } let mut r = String::new(); for _ in 0..n { r.push_str(s); } return Ok(Value::Str(r)); }
    if let (BinOp::Add, Value::List(a), Value::List(b)) = (op, left, right) { let mut r = a.clone(); r.extend(b.iter().cloned()); return Ok(Value::List(r)); }
    if let (BinOp::Mul, Value::List(items), Value::Int(n)) = (op, left, right) { let n = *n; if n <= 0 { return Ok(Value::List(Vec::new())); } let mut r = Vec::new(); for _ in 0..n { r.extend(items.iter().cloned()); } return Ok(Value::List(r)); }
    if let (BinOp::Add, Value::Tuple(a), Value::Tuple(b)) = (op, left, right) { let mut r = a.clone(); r.extend(b.iter().cloned()); return Ok(Value::Tuple(r)); }
    if let (BinOp::BitOr, Value::Set(a), Value::Set(b)) = (op, left, right) { let mut r = a.clone(); for v in b { let k = v.key_string(); if !r.iter().any(|x| x.key_string() == k) { r.push(v.clone()); } } return Ok(Value::Set(r)); }
    if let (BinOp::BitAnd, Value::Set(a), Value::Set(b)) = (op, left, right) { let bk: Vec<String> = b.iter().map(|v| v.key_string()).collect(); return Ok(Value::Set(a.iter().filter(|v| bk.contains(&v.key_string())).cloned().collect())); }
    if let (BinOp::Sub, Value::Set(a), Value::Set(b)) = (op, left, right) { let bk: Vec<String> = b.iter().map(|v| v.key_string()).collect(); return Ok(Value::Set(a.iter().filter(|v| !bk.contains(&v.key_string())).cloned().collect())); }
    if let (BinOp::BitOr, Value::Dict(a), Value::Dict(b)) = (op, left, right) { let mut r = a.clone(); for (k, v) in b { let ks = k.key_string(); let mut f = false; for p in r.iter_mut() { if p.0.key_string() == ks { p.1 = v.clone(); f = true; break; } } if !f { r.push((k.clone(), v.clone())); } } return Ok(Value::Dict(r)); }
    // String % formatting
    if let (BinOp::Mod, Value::Str(fmt), rv) = (op, left, right) {
        let fa = match rv { Value::Tuple(items) => items.clone(), o => vec![o.clone()] };
        let mut result = String::new(); let chars: Vec<char> = fmt.chars().collect(); let mut i = 0; let mut ai = 0;
        while i < chars.len() { if chars[i] == '%' && i + 1 < chars.len() { i += 1; if chars[i] == '%' { result.push('%'); i += 1; } else { while i < chars.len() && (chars[i] == '-' || chars[i] == '+' || chars[i] == '0' || chars[i].is_ascii_digit() || chars[i] == '.') { i += 1; } if i < chars.len() && ai < fa.len() { match chars[i] { 's' => result.push_str(&fa[ai].display()), 'd' | 'i' => { if let Ok(n) = fa[ai].as_int() { result.push_str(&format!("{}", n)); } } 'f' => { if let Ok(f) = fa[ai].as_float() { result.push_str(&format!("{:.6}", f)); } } 'r' => result.push_str(&fa[ai].repr()), 'x' => { if let Ok(n) = fa[ai].as_int() { result.push_str(&format!("{:x}", n)); } } _ => {} } ai += 1; i += 1; } } } else { result.push(chars[i]); i += 1; } }
        return Ok(Value::Str(result));
    }
    let uf = matches!(left, Value::Float(_)) || matches!(right, Value::Float(_));
    if uf {
        let a = left.as_float()?; let b = right.as_float()?;
        let r = match op { BinOp::Add => a + b, BinOp::Sub => a - b, BinOp::Mul => a * b, BinOp::Div => { if b == 0.0 { return Err(String::from("division by zero")); } a / b } BinOp::FloorDiv => { if b == 0.0 { return Err(String::from("division by zero")); } floor_f64(a / b) } BinOp::Mod => { if b == 0.0 { return Err(String::from("modulo by zero")); } let r = a % b; if r != 0.0 && ((r < 0.0) != (b < 0.0)) { r + b } else { r } } BinOp::Pow => pow_float(a, b), _ => return Err(String::from("bitwise ops not supported on floats")) };
        Ok(Value::Float(r))
    } else {
        let a = left.as_int()?; let b = right.as_int()?;
        let r = match op { BinOp::Add => a.checked_add(b).ok_or("integer overflow")?, BinOp::Sub => a.checked_sub(b).ok_or("integer overflow")?, BinOp::Mul => a.checked_mul(b).ok_or("integer overflow")?, BinOp::Div => { if b == 0 { return Err(String::from("division by zero")); } return Ok(Value::Float(a as f64 / b as f64)); } BinOp::FloorDiv => { if b == 0 { return Err(String::from("division by zero")); } let d = a.wrapping_div(b); if (a ^ b) < 0 && d * b != a { d - 1 } else { d } } BinOp::Mod => { if b == 0 { return Err(String::from("modulo by zero")); } ((a % b) + b) % b } BinOp::Pow => { if b < 0 { return Ok(Value::Float(pow_float(a as f64, b as f64))); } pow_int(a, b as u64) } BinOp::BitOr => a | b, BinOp::BitAnd => a & b, BinOp::BitXor => a ^ b };
        Ok(Value::Int(r))
    }
}

fn eval_compare(op: CmpOp, left: &Value, right: &Value) -> EvalResult {
    match (left, right, op) {
        (_, Value::List(items), CmpOp::In) => return Ok(Value::Bool(items.iter().any(|v| v == left))),
        (_, Value::List(items), CmpOp::NotIn) => return Ok(Value::Bool(!items.iter().any(|v| v == left))),
        (_, Value::Tuple(items), CmpOp::In) => return Ok(Value::Bool(items.iter().any(|v| v == left))),
        (_, Value::Tuple(items), CmpOp::NotIn) => return Ok(Value::Bool(!items.iter().any(|v| v == left))),
        (_, Value::Set(items), CmpOp::In) => return Ok(Value::Bool(items.iter().any(|v| v.key_string() == left.key_string()))),
        (_, Value::Set(items), CmpOp::NotIn) => return Ok(Value::Bool(!items.iter().any(|v| v.key_string() == left.key_string()))),
        (Value::Str(sub), Value::Str(s), CmpOp::In) => return Ok(Value::Bool(s.contains(sub.as_str()))),
        (Value::Str(sub), Value::Str(s), CmpOp::NotIn) => return Ok(Value::Bool(!s.contains(sub.as_str()))),
        (_, Value::Dict(pairs), CmpOp::In) => { let ks = left.key_string(); return Ok(Value::Bool(pairs.iter().any(|(k, _)| k.key_string() == ks))); }
        (_, Value::Dict(pairs), CmpOp::NotIn) => { let ks = left.key_string(); return Ok(Value::Bool(!pairs.iter().any(|(k, _)| k.key_string() == ks))); }
        (_, _, CmpOp::Is) => return Ok(Value::Bool(matches!((left, right), (Value::None, Value::None)))),
        (_, _, CmpOp::IsNot) => return Ok(Value::Bool(!matches!((left, right), (Value::None, Value::None)))),
        _ => {}
    }
    let r = match (left, right) {
        (Value::Int(a), Value::Int(b)) => match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, CmpOp::Lt => a < b, CmpOp::Gt => a > b, CmpOp::LtEq => a <= b, CmpOp::GtEq => a >= b, _ => false },
        (Value::Float(a), Value::Float(b)) => cmp_f(*a, *b, op),
        (Value::Int(a), Value::Float(b)) => cmp_f(*a as f64, *b, op),
        (Value::Float(a), Value::Int(b)) => cmp_f(*a, *b as f64, op),
        (Value::Str(a), Value::Str(b)) => match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, CmpOp::Lt => a < b, CmpOp::Gt => a > b, CmpOp::LtEq => a <= b, CmpOp::GtEq => a >= b, _ => false },
        (Value::Bool(a), Value::Bool(b)) => match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, _ => return Err(String::from("cannot order booleans")) },
        (Value::None, Value::None) => match op { CmpOp::Eq => true, CmpOp::NotEq => false, _ => return Err(String::from("cannot order None")) },
        (Value::List(a), Value::List(b)) => match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, _ => false },
        (Value::Tuple(a), Value::Tuple(b)) => match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, _ => false },
        _ => match op { CmpOp::Eq => false, CmpOp::NotEq => true, _ => return Err(format!("cannot compare '{}' and '{}'", left.type_name(), right.type_name())) },
    };
    Ok(Value::Bool(r))
}
fn cmp_f(a: f64, b: f64, op: CmpOp) -> bool { match op { CmpOp::Eq => a == b, CmpOp::NotEq => a != b, CmpOp::Lt => a < b, CmpOp::Gt => a > b, CmpOp::LtEq => a <= b, CmpOp::GtEq => a >= b, _ => false } }

fn eval_index(obj: &Value, index: &Value) -> EvalResult {
    match (obj, index) {
        (Value::List(items), Value::Int(i)) => { let idx = if *i < 0 { (items.len() as i64 + *i) as usize } else { *i as usize }; items.get(idx).cloned().ok_or_else(|| format!("list index {} out of range", i)) }
        (Value::Tuple(items), Value::Int(i)) => { let idx = if *i < 0 { (items.len() as i64 + *i) as usize } else { *i as usize }; items.get(idx).cloned().ok_or_else(|| format!("tuple index {} out of range", i)) }
        (Value::Str(s), Value::Int(i)) => { let chars: Vec<char> = s.chars().collect(); let idx = if *i < 0 { (chars.len() as i64 + *i) as usize } else { *i as usize }; chars.get(idx).map(|c| Value::Str(String::from(c.to_string()))).ok_or_else(|| format!("string index {} out of range", i)) }
        (Value::Dict(pairs), key) => { let ks = key.key_string(); for (k, v) in pairs { if k.key_string() == ks { return Ok(v.clone()); } } Err(format!("KeyError: {}", key.repr())) }
        _ => Err(format!("'{}' object is not subscriptable", obj.type_name())),
    }
}

fn eval_slice(obj: &Value, lo: Option<i64>, hi: Option<i64>, step: Option<i64>) -> EvalResult {
    let step = step.unwrap_or(1); if step == 0 { return Err(String::from("slice step cannot be zero")); }
    match obj {
        Value::List(items) => Ok(Value::List(slice_vec(items, lo, hi, step))),
        Value::Tuple(items) => Ok(Value::Tuple(slice_vec(items, lo, hi, step))),
        Value::Str(s) => { let chars: Vec<Value> = s.chars().map(|c| Value::Str(String::from(c.to_string()))).collect(); Ok(Value::Str(slice_vec(&chars, lo, hi, step).into_iter().map(|v| v.display()).collect())) }
        _ => Err(format!("'{}' is not subscriptable", obj.type_name())),
    }
}
fn slice_vec(items: &[Value], lo: Option<i64>, hi: Option<i64>, step: i64) -> Vec<Value> {
    let len = items.len() as i64;
    let ni = |idx: i64| -> i64 { if idx < 0 { (idx + len).max(0) } else { idx } };
    if step > 0 {
        let s = match lo { Some(l) => ni(l).max(0), None => 0 }; let e = match hi { Some(h) => ni(h).min(len), None => len };
        let mut r = Vec::new(); let mut i = s; while i < e { r.push(items[i as usize].clone()); i += step; } r
    } else {
        let s = match lo { Some(l) => ni(l).min(len - 1), None => len - 1 }; let e = match hi { Some(h) => ni(h), None => -1 };
        let mut r = Vec::new(); let mut i = s; while i > e { if i >= 0 && (i as usize) < items.len() { r.push(items[i as usize].clone()); } i += step; } r
    }
}

// Math helpers
fn pow_int(base: i64, exp: u64) -> i64 { let mut r: i64 = 1; let mut b = base; let mut e = exp; while e > 0 { if e & 1 == 1 { r = r.wrapping_mul(b); } b = b.wrapping_mul(b); e >>= 1; } r }
fn pow_mod(base: i64, exp: i64, m: i64) -> i64 { if m == 1 { return 0; } let mut r: i64 = 1; let mut b = ((base % m) + m) % m; let mut e = exp; while e > 0 { if e & 1 == 1 { r = r.wrapping_mul(b) % m; } b = b.wrapping_mul(b) % m; e >>= 1; } r }
fn pow_float(base: f64, exp: f64) -> f64 {
    if exp == 0.0 { return 1.0; } if base == 0.0 { return if exp > 0.0 { 0.0 } else { f64::INFINITY }; } if base == 1.0 { return 1.0; } if exp == 1.0 { return base; }
    if exp == floor_f64(exp) && abs_f64(exp) < 1000.0 { let e = exp as i64; if e >= 0 { let mut r = 1.0; let mut b = base; let mut n = e as u64; while n > 0 { if n & 1 == 1 { r *= b; } b *= b; n >>= 1; } return r; } else { let mut r = 1.0; let mut b = base; let mut n = (-e) as u64; while n > 0 { if n & 1 == 1 { r *= b; } b *= b; n >>= 1; } return 1.0 / r; } }
    if base < 0.0 { return f64::NAN; } exp_f64(exp * ln_f64(base))
}
fn ln_f64(x: f64) -> f64 { if x <= 0.0 { return f64::NAN; } if x == 1.0 { return 0.0; } let mut v = x; let mut ea: i64 = 0; while v > 2.0 { v /= 2.0; ea += 1; } while v < 0.5 { v *= 2.0; ea -= 1; } let y = (v - 1.0) / (v + 1.0); let y2 = y * y; let mut t = y; let mut s = y; for n in 1..50 { t *= y2; let c = t / (2 * n + 1) as f64; s += c; if abs_f64(c) < 1e-15 { break; } } 2.0 * s + ea as f64 * 0.6931471805599453 }
fn exp_f64(x: f64) -> f64 { if x == 0.0 { return 1.0; } if x > 709.0 { return f64::INFINITY; } if x < -709.0 { return 0.0; } let ln2 = 0.6931471805599453; let k = floor_f64(x / ln2); let r = x - k * ln2; let mut s = 1.0; let mut t = 1.0; for n in 1..100 { t *= r / n as f64; s += t; if abs_f64(t) < 1e-15 { break; } } let mut res = s; let ki = k as i64; if ki >= 0 { for _ in 0..ki.min(1023) { res *= 2.0; } } else { for _ in 0..(-ki).min(1023) { res /= 2.0; } } res }
fn sqrt_f64(x: f64) -> f64 { if x < 0.0 { return f64::NAN; } if x == 0.0 { return 0.0; } let mut g = if x > 1.0 { x / 2.0 } else { 1.0 }; for _ in 0..100 { let n = (g + x / g) / 2.0; if abs_f64(n - g) < 1e-15 * abs_f64(n) { return n; } g = n; } g }
fn sin_f64(x: f64) -> f64 { let pi = core::f64::consts::PI; let tp = 2.0 * pi; let mut v = x % tp; if v > pi { v -= tp; } if v < -pi { v += tp; } let mut s = 0.0; let mut t = v; s += t; for n in 1..30 { t *= -v * v / ((2*n) as f64 * (2*n+1) as f64); s += t; if abs_f64(t) < 1e-15 { break; } } s }
fn cos_f64(x: f64) -> f64 { let pi = core::f64::consts::PI; let tp = 2.0 * pi; let mut v = x % tp; if v > pi { v -= tp; } if v < -pi { v += tp; } let mut s = 0.0; let mut t = 1.0; s += t; for n in 1..30 { t *= -v * v / ((2*n-1) as f64 * (2*n) as f64); s += t; if abs_f64(t) < 1e-15 { break; } } s }
fn tan_f64(x: f64) -> f64 { let c = cos_f64(x); if abs_f64(c) < 1e-15 { f64::INFINITY } else { sin_f64(x) / c } }
fn asin_f64(x: f64) -> f64 { if abs_f64(x) > 1.0 { return f64::NAN; } if abs_f64(x) == 1.0 { return if x > 0.0 { core::f64::consts::FRAC_PI_2 } else { -core::f64::consts::FRAC_PI_2 }; } atan_f64(x / sqrt_f64(1.0 - x*x)) }
fn acos_f64(x: f64) -> f64 { core::f64::consts::FRAC_PI_2 - asin_f64(x) }
fn atan_f64(x: f64) -> f64 { let pi = core::f64::consts::PI; if x > 1.0 { return pi/2.0 - atan_f64(1.0/x); } if x < -1.0 { return -pi/2.0 - atan_f64(1.0/x); } let x2 = x*x; let mut s = 0.0; let mut t = x; s += t; for n in 1..200 { t *= -x2; s += t / (2*n+1) as f64; if abs_f64(t / (2*n+1) as f64) < 1e-15 { break; } } s }
fn atan2_f64(y: f64, x: f64) -> f64 { let pi = core::f64::consts::PI; if x > 0.0 { atan_f64(y/x) } else if x < 0.0 && y >= 0.0 { atan_f64(y/x) + pi } else if x < 0.0 && y < 0.0 { atan_f64(y/x) - pi } else if x == 0.0 && y > 0.0 { pi/2.0 } else if x == 0.0 && y < 0.0 { -pi/2.0 } else { 0.0 } }
fn sinh_f64(x: f64) -> f64 { (exp_f64(x) - exp_f64(-x)) / 2.0 }
fn cosh_f64(x: f64) -> f64 { (exp_f64(x) + exp_f64(-x)) / 2.0 }
fn tanh_f64(x: f64) -> f64 { if x > 20.0 { return 1.0; } if x < -20.0 { return -1.0; } let e = exp_f64(2.0*x); (e - 1.0) / (e + 1.0) }
fn factorial(n: i64) -> i64 { let mut r: i64 = 1; for i in 2..=n { r = r.wrapping_mul(i); } r }
fn gcd(a: i64, b: i64) -> i64 { let mut a = a.abs(); let mut b = b.abs(); while b != 0 { let t = b; b = a % b; a = t; } a }

pub fn call_module_func(interp: &mut Interpreter, name: &str, args: &[Value], scope: &mut BTreeMap<String, Value>) -> Result<Option<Value>, String> {
    match name {
        "math.sqrt" => Ok(Some(Value::Float(sqrt_f64(args[0].as_float()?)))),
        "math.sin" => Ok(Some(Value::Float(sin_f64(args[0].as_float()?)))),
        "math.cos" => Ok(Some(Value::Float(cos_f64(args[0].as_float()?)))),
        "math.tan" => Ok(Some(Value::Float(tan_f64(args[0].as_float()?)))),
        "math.exp" => Ok(Some(Value::Float(exp_f64(args[0].as_float()?)))),
        "math.log" => { let x = args[0].as_float()?; if args.len() > 1 { Ok(Some(Value::Float(ln_f64(x) / ln_f64(args[1].as_float()?)))) } else { Ok(Some(Value::Float(ln_f64(x)))) } }
        "math.log2" => Ok(Some(Value::Float(ln_f64(args[0].as_float()?) / 0.6931471805599453))),
        "math.log10" => Ok(Some(Value::Float(ln_f64(args[0].as_float()?) / 2.302585092994046))),
        "math.floor" => Ok(Some(Value::Int(floor_f64(args[0].as_float()?) as i64))),
        "math.ceil" => Ok(Some(Value::Int(ceil_f64(args[0].as_float()?) as i64))),
        "math.fabs" => Ok(Some(Value::Float(abs_f64(args[0].as_float()?)))),
        "math.pow" => Ok(Some(Value::Float(pow_float(args[0].as_float()?, args[1].as_float()?)))),
        "math.asin" => Ok(Some(Value::Float(asin_f64(args[0].as_float()?)))),
        "math.acos" => Ok(Some(Value::Float(acos_f64(args[0].as_float()?)))),
        "math.atan" => Ok(Some(Value::Float(atan_f64(args[0].as_float()?)))),
        "math.atan2" => Ok(Some(Value::Float(atan2_f64(args[0].as_float()?, args[1].as_float()?)))),
        "math.sinh" => Ok(Some(Value::Float(sinh_f64(args[0].as_float()?)))),
        "math.cosh" => Ok(Some(Value::Float(cosh_f64(args[0].as_float()?)))),
        "math.tanh" => Ok(Some(Value::Float(tanh_f64(args[0].as_float()?)))),
        "math.degrees" => Ok(Some(Value::Float(args[0].as_float()? * 180.0 / core::f64::consts::PI))),
        "math.radians" => Ok(Some(Value::Float(args[0].as_float()? * core::f64::consts::PI / 180.0))),
        "math.isnan" => Ok(Some(Value::Bool(args[0].as_float()?.is_nan()))),
        "math.isinf" => Ok(Some(Value::Bool(args[0].as_float()?.is_infinite()))),
        "math.factorial" => { let n = args[0].as_int()?; if n < 0 { return Err(String::from("factorial() not defined for negative values")); } Ok(Some(Value::Int(factorial(n)))) }
        "math.gcd" => Ok(Some(Value::Int(gcd(args[0].as_int()?, args[1].as_int()?)))),
        "random.random" => Ok(Some(Value::Float(0.5))),
        "random.randint" => Ok(Some(Value::Int(args[0].as_int()?))),
        "random.choice" => { let items = interp.to_iterable(&args[0])?; if items.is_empty() { return Err(String::from("cannot choose from empty sequence")); } Ok(Some(items[0].clone())) }
        "random.shuffle" | "random.seed" => Ok(Some(Value::None)),
        "random.uniform" => Ok(Some(Value::Float((args[0].as_float()? + args[1].as_float()?) / 2.0))),
        "json.dumps" => { if args.is_empty() { return Err(String::from("json.dumps() takes 1 argument")); } Ok(Some(Value::Str(json_encode(&args[0])))) }
        "json.loads" => { if let Value::Str(s) = &args[0] { Ok(Some(json_decode(s)?)) } else { Err(String::from("json.loads() argument must be str")) } }
        "os.getcwd" => Ok(Some(Value::Str(String::from("/")))),
        "os.listdir" => Ok(Some(Value::List(Vec::new()))),
        "os.getenv" => Ok(Some(Value::None)),
        "os.path.join" => { let mut r = String::new(); for (i, a) in args.iter().enumerate() { if i > 0 && !r.ends_with('/') { r.push('/'); } r.push_str(&a.display()); } Ok(Some(Value::Str(r))) }
        "os.path.exists" | "os.path.isfile" | "os.path.isdir" => Ok(Some(Value::Bool(false))),
        "os.path.basename" => { let s = args[0].display(); Ok(Some(Value::Str(String::from(s.rsplit('/').next().unwrap_or(&s))))) }
        "os.path.dirname" => { let s = args[0].display(); Ok(Some(Value::Str(if let Some(p) = s.rfind('/') { String::from(&s[..p]) } else { String::new() }))) }
        "sys.exit" => Err(String::from("SystemExit")),
        "time.time" | "time.monotonic" => Ok(Some(Value::Float(0.0))),
        "time.sleep" => Ok(Some(Value::None)),
        "collections.OrderedDict" | "collections.defaultdict" => Ok(Some(Value::Dict(Vec::new()))),
        "collections.Counter" => { if args.is_empty() { return Ok(Some(Value::Dict(Vec::new()))); } let items = interp.to_iterable(&args[0])?; let mut pairs: Vec<(Value, Value)> = Vec::new(); for item in items { let ks = item.key_string(); let mut f = false; for p in pairs.iter_mut() { if p.0.key_string() == ks { if let Value::Int(n) = &p.1 { p.1 = Value::Int(n + 1); } f = true; break; } } if !f { pairs.push((item, Value::Int(1))); } } Ok(Some(Value::Dict(pairs))) }
        "functools.reduce" => { if args.len() < 2 { return Err(String::from("reduce() requires at least 2 arguments")); } let func = &args[0]; let items = interp.to_iterable(&args[1])?; let mut acc = if args.len() > 2 { args[2].clone() } else { if items.is_empty() { return Err(String::from("reduce() of empty sequence with no initial value")); } items[0].clone() }; let start = if args.len() > 2 { 0 } else { 1 }; for item in &items[start..] { acc = interp.call_value(func, &[acc, item.clone()], &[], scope)?; } Ok(Some(acc)) }
        _ => Ok(None),
    }
}

fn json_encode(val: &Value) -> String {
    match val { Value::None => String::from("null"), Value::Bool(true) => String::from("true"), Value::Bool(false) => String::from("false"), Value::Int(n) => format!("{}", n), Value::Float(f) => format_float(*f), Value::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")), Value::List(items) | Value::Tuple(items) => { let i: Vec<String> = items.iter().map(json_encode).collect(); format!("[{}]", i.join(", ")) } Value::Dict(pairs) => { let i: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", json_encode(k), json_encode(v))).collect(); format!("{{{}}}", i.join(", ")) } _ => format!("\"{}\"", val.display()) }
}
fn json_decode(s: &str) -> Result<Value, String> {
    let t = s.trim();
    if t == "null" { return Ok(Value::None); } if t == "true" { return Ok(Value::Bool(true)); } if t == "false" { return Ok(Value::Bool(false)); }
    if t.starts_with('"') && t.ends_with('"') { return Ok(Value::Str(t[1..t.len()-1].replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\"))); }
    if let Ok(n) = t.parse::<i64>() { return Ok(Value::Int(n)); } if let Ok(f) = t.parse::<f64>() { return Ok(Value::Float(f)); }
    if t.starts_with('[') && t.ends_with(']') { let inner = t[1..t.len()-1].trim(); if inner.is_empty() { return Ok(Value::List(Vec::new())); } let mut items = Vec::new(); let mut d = 0; let mut start = 0; let chars: Vec<char> = inner.chars().collect(); for (i, &c) in chars.iter().enumerate() { match c { '[' | '{' => d += 1, ']' | '}' => d -= 1, ',' if d == 0 => { let p: String = chars[start..i].iter().collect(); items.push(json_decode(p.trim())?); start = i + 1; } _ => {} } } let last: String = chars[start..].iter().collect(); if !last.trim().is_empty() { items.push(json_decode(last.trim())?); } return Ok(Value::List(items)); }
    if t.starts_with('{') && t.ends_with('}') { let inner = t[1..t.len()-1].trim(); if inner.is_empty() { return Ok(Value::Dict(Vec::new())); } let mut pairs = Vec::new(); let mut d = 0; let mut start = 0; let chars: Vec<char> = inner.chars().collect(); for (i, &c) in chars.iter().enumerate() { match c { '[' | '{' => d += 1, ']' | '}' => d -= 1, ',' if d == 0 => { let p: String = chars[start..i].iter().collect(); let kv = parse_json_kv(p.trim())?; pairs.push(kv); start = i + 1; } _ => {} } } let last: String = chars[start..].iter().collect(); if !last.trim().is_empty() { pairs.push(parse_json_kv(last.trim())?); } return Ok(Value::Dict(pairs)); }
    Err(format!("json.loads: cannot parse"))
}
fn parse_json_kv(s: &str) -> Result<(Value, Value), String> {
    let chars: Vec<char> = s.chars().collect(); let mut ins = false; let mut cp = None;
    for (i, &c) in chars.iter().enumerate() { if c == '"' { ins = !ins; } if c == ':' && !ins { cp = Some(i); break; } }
    if let Some(pos) = cp { let k: String = chars[..pos].iter().collect(); let v: String = chars[pos+1..].iter().collect(); Ok((json_decode(k.trim())?, json_decode(v.trim())?)) }
    else { Err(format!("json: invalid key-value pair")) }
}
