//! x86_64 code generation for go-lite via Cranelift.
//!
//! Translates Go AST to Cranelift IR and compiles to native x86_64 machine code.
//! This module handles Go-specific features that differ from C:
//!
//! - **Multiple return values**: Go functions can return multiple values. Each
//!   return type becomes a separate `AbiParam` in the Cranelift signature.
//! - **Fat types as pointers**: Slices (`ptr, len, cap`), strings (`ptr, len`),
//!   maps, channels, and interfaces are all represented as `I64` pointers to
//!   their runtime structures.
//! - **Short variable declarations** (`:=`): Create new Cranelift variables
//!   on-the-fly without explicit type annotations.
//! - **Go-specific statements**: `i++`/`i--` are statements (not expressions),
//!   compiled as load-add/sub-store sequences.
//! - **For loops with init/post**: Go's `for init; cond; post { body }` maps
//!   to the same header/body/post/exit block pattern as C's `for`.
//! - **If with init statement**: `if x := f(); x > 0 { ... }` compiles the
//!   init statement before the condition check.
//! - **`&^` (and-not)**: Go's bit-clear operator, compiled as `band(l, bnot(r))`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use cranelift_codegen::ir::types::{I8, I16, I32, I64, F32, F64};
use cranelift_codegen::ir::{
    AbiParam, Block as ClifBlock, Function, InstBuilder, Signature,
    Type as ClifType, UserFuncName, Value,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};

use crate::ast::*;
use crate::types::GoType;

/// Compiled function: machine code bytes.
pub struct CompiledFunc {
    pub name: String,
    pub code: Vec<u8>,
}

/// Code generation context.
pub struct CodeGen {
    /// Variable counter for Cranelift.
    var_counter: u32,
    /// Map from Go variable name to Cranelift Variable.
    var_map: BTreeMap<String, (Variable, ClifType)>,
    /// Break target block stack.
    break_targets: Vec<ClifBlock>,
    /// Continue target block stack.
    continue_targets: Vec<ClifBlock>,
    /// Defer stack (simplified: store function pointers).
    defer_stack: Vec<Value>,
    /// Function signatures.
    func_sigs: BTreeMap<String, Vec<GoType>>,
}

impl CodeGen {
    pub fn new() -> Self {
        Self {
            var_counter: 0,
            var_map: BTreeMap::new(),
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            defer_stack: Vec::new(),
            func_sigs: BTreeMap::new(),
        }
    }

    fn new_var(&mut self) -> Variable {
        let v = Variable::from_u32(self.var_counter);
        self.var_counter += 1;
        v
    }

    /// Map a Go type to a Cranelift IR type.
    pub fn gotype_to_clif(ty: &GoType) -> ClifType {
        match ty {
            GoType::Bool => I8,
            GoType::Int8 | GoType::Uint8 | GoType::Byte => I8,
            GoType::Int16 | GoType::Uint16 => I16,
            GoType::Int32 | GoType::Uint32 | GoType::Rune | GoType::Float32 => {
                match ty {
                    GoType::Float32 => F32,
                    _ => I32,
                }
            }
            GoType::Int64 | GoType::Uint64 | GoType::Float64 => {
                match ty {
                    GoType::Float64 => F64,
                    _ => I64,
                }
            }
            GoType::Int | GoType::Uint | GoType::Uintptr => I64,
            GoType::Complex64 => I64,   // two f32 packed
            GoType::Complex128 => I64,  // pointer to two f64
            GoType::String => I64,      // pointer to (ptr, len) pair
            GoType::Slice(_) => I64,    // pointer to (ptr, len, cap) triple
            GoType::Array(_, _) => I64, // pointer
            GoType::Map(_, _) => I64,   // pointer to runtime map
            GoType::Chan(_, _) => I64,  // pointer to runtime channel
            GoType::Pointer(_) => I64,
            GoType::Func { .. } => I64, // function pointer
            GoType::Interface(_) => I64, // pointer to (type_id, data_ptr)
            GoType::Struct(_) => I64,   // pointer
            GoType::Named(_) | GoType::Qualified(_, _) => I64,
            GoType::Void => I64,
        }
    }

    /// Compile a single Go function to machine code.
    pub fn compile_function(&mut self, func: &FuncDecl) -> Result<CompiledFunc, String> {
        log::debug!("[go] compiling function: {}", func.name);

        // Reset per-function state
        self.var_map.clear();
        self.var_counter = 0;
        self.break_targets.clear();
        self.continue_targets.clear();
        self.defer_stack.clear();

        // Set up Cranelift
        let mut flag_builder = settings::builder();
        flag_builder.set("opt_level", "speed").unwrap();
        let flags = settings::Flags::new(flag_builder);

        let isa = cranelift_codegen::isa::lookup_by_name("x86_64-unknown-none-elf")
            .map_err(|e| alloc::format!("ISA lookup failed: {}", e))?
            .finish(flags)
            .map_err(|e| alloc::format!("ISA build failed: {}", e))?;

        // Build function signature
        let mut sig = Signature::new(CallConv::SystemV);
        for param in &func.params {
            sig.params.push(AbiParam::new(Self::gotype_to_clif(&param.ty)));
        }
        for ret_ty in &func.returns {
            sig.returns.push(AbiParam::new(Self::gotype_to_clif(ret_ty)));
        }
        // Default to i64 return if no explicit returns (main returns int)
        if func.returns.is_empty() {
            sig.returns.push(AbiParam::new(I64));
        }

        let mut ir_func = Function::with_name_signature(UserFuncName::default(), sig);

        let mut func_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ir_func, &mut func_ctx);

        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        // Declare parameters as variables
        for (i, param) in func.params.iter().enumerate() {
            let var = self.new_var();
            let clif_ty = Self::gotype_to_clif(&param.ty);
            builder.declare_var(var, clif_ty);
            let param_val = builder.block_params(entry_block)[i];
            builder.def_var(var, param_val);
            self.var_map.insert(param.name.clone(), (var, clif_ty));
        }

        // Compile the body
        let mut has_return = false;
        for stmt in &func.body.stmts {
            self.compile_stmt(&mut builder, stmt, &mut has_return)?;
            if has_return {
                break;
            }
        }

        // Add default return if needed
        if !has_return {
            let zero = builder.ins().iconst(I64, 0);
            builder.ins().return_(&[zero]);
        }

        builder.finalize();

        // Compile to machine code
        let mut ctx = Context::for_function(ir_func);
        let code = ctx
            .compile(&*isa, &mut Default::default())
            .map_err(|e| alloc::format!("codegen error: {:?}", e))?;

        let code_bytes = code.code_buffer().to_vec();
        log::debug!("[go] compiled {}: {} bytes", func.name, code_bytes.len());

        Ok(CompiledFunc {
            name: func.name.clone(),
            code: code_bytes,
        })
    }

    fn compile_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: &Stmt,
        has_return: &mut bool,
    ) -> Result<(), String> {
        match stmt {
            Stmt::VarDecl(decl) => {
                for (i, name) in decl.names.iter().enumerate() {
                    let clif_ty = decl.ty.as_ref()
                        .map(|t| Self::gotype_to_clif(t))
                        .unwrap_or(I64);
                    let var = self.new_var();
                    builder.declare_var(var, clif_ty);
                    if let Some(init) = decl.values.get(i) {
                        let val = self.compile_expr(builder, init)?;
                        builder.def_var(var, val);
                    } else {
                        let zero = builder.ins().iconst(clif_ty, 0);
                        builder.def_var(var, zero);
                    }
                    self.var_map.insert(name.clone(), (var, clif_ty));
                }
                Ok(())
            }
            Stmt::ShortDecl { names, values } => {
                for (i, name) in names.iter().enumerate() {
                    let var = self.new_var();
                    let clif_ty = I64;
                    builder.declare_var(var, clif_ty);
                    if let Some(init) = values.get(i) {
                        let val = self.compile_expr(builder, init)?;
                        builder.def_var(var, val);
                    } else {
                        let zero = builder.ins().iconst(clif_ty, 0);
                        builder.def_var(var, zero);
                    }
                    self.var_map.insert(name.clone(), (var, clif_ty));
                }
                Ok(())
            }
            Stmt::Assign { op, lhs, rhs } => {
                for (i, target) in lhs.iter().enumerate() {
                    if let Expr::Ident(name) = target {
                        if let Some(&(var, clif_ty)) = self.var_map.get(name) {
                            let rval = if let Some(rv) = rhs.get(i) {
                                self.compile_expr(builder, rv)?
                            } else {
                                builder.ins().iconst(clif_ty, 0)
                            };
                            let final_val = match op {
                                AssignOp::Assign => rval,
                                AssignOp::AddAssign => {
                                    let lval = builder.use_var(var);
                                    builder.ins().iadd(lval, rval)
                                }
                                AssignOp::SubAssign => {
                                    let lval = builder.use_var(var);
                                    builder.ins().isub(lval, rval)
                                }
                                AssignOp::MulAssign => {
                                    let lval = builder.use_var(var);
                                    builder.ins().imul(lval, rval)
                                }
                                _ => rval,
                            };
                            builder.def_var(var, final_val);
                        }
                    }
                }
                Ok(())
            }
            Stmt::Return(exprs) => {
                let vals: Vec<Value> = exprs
                    .iter()
                    .map(|e| self.compile_expr(builder, e))
                    .collect::<Result<_, _>>()?;
                if vals.is_empty() {
                    let zero = builder.ins().iconst(I64, 0);
                    builder.ins().return_(&[zero]);
                } else {
                    builder.ins().return_(&vals);
                }
                *has_return = true;
                Ok(())
            }
            Stmt::If { init, cond, body, else_body } => {
                if let Some(init_stmt) = init {
                    self.compile_stmt(builder, init_stmt, has_return)?;
                }
                let cond_val = self.compile_expr(builder, cond)?;
                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let merge_block = builder.create_block();

                builder.ins().brif(cond_val, then_block, &[], else_block, &[]);

                builder.switch_to_block(then_block);
                builder.seal_block(then_block);
                let mut then_returns = false;
                for s in &body.stmts {
                    self.compile_stmt(builder, s, &mut then_returns)?;
                    if then_returns { break; }
                }
                if !then_returns {
                    builder.ins().jump(merge_block, &[]);
                }

                builder.switch_to_block(else_block);
                builder.seal_block(else_block);
                let mut else_returns = false;
                if let Some(els) = else_body {
                    match els {
                        ElseClause::Block(block) => {
                            for s in &block.stmts {
                                self.compile_stmt(builder, s, &mut else_returns)?;
                                if else_returns { break; }
                            }
                        }
                        ElseClause::If(if_stmt) => {
                            self.compile_stmt(builder, if_stmt, &mut else_returns)?;
                        }
                    }
                }
                if !else_returns {
                    builder.ins().jump(merge_block, &[]);
                }

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);
                Ok(())
            }
            Stmt::For { init, cond, post, body } => {
                if let Some(init_stmt) = init {
                    self.compile_stmt(builder, init_stmt, has_return)?;
                }

                let header_block = builder.create_block();
                let body_block = builder.create_block();
                let post_block = builder.create_block();
                let exit_block = builder.create_block();

                self.break_targets.push(exit_block);
                self.continue_targets.push(post_block);

                builder.ins().jump(header_block, &[]);

                builder.switch_to_block(header_block);
                builder.seal_block(header_block);
                if let Some(c) = cond {
                    let cond_val = self.compile_expr(builder, c)?;
                    builder.ins().brif(cond_val, body_block, &[], exit_block, &[]);
                } else {
                    builder.ins().jump(body_block, &[]);
                }

                builder.switch_to_block(body_block);
                builder.seal_block(body_block);
                let mut body_returns = false;
                for s in &body.stmts {
                    self.compile_stmt(builder, s, &mut body_returns)?;
                    if body_returns { break; }
                }
                if !body_returns {
                    builder.ins().jump(post_block, &[]);
                }

                builder.switch_to_block(post_block);
                builder.seal_block(post_block);
                if let Some(post_stmt) = post {
                    self.compile_stmt(builder, post_stmt, has_return)?;
                }
                builder.ins().jump(header_block, &[]);

                builder.switch_to_block(exit_block);
                builder.seal_block(exit_block);

                self.break_targets.pop();
                self.continue_targets.pop();
                Ok(())
            }
            Stmt::Break(_) => {
                if let Some(&block) = self.break_targets.last() {
                    builder.ins().jump(block, &[]);
                }
                Ok(())
            }
            Stmt::Continue(_) => {
                if let Some(&block) = self.continue_targets.last() {
                    builder.ins().jump(block, &[]);
                }
                Ok(())
            }
            Stmt::Inc(expr) => {
                if let Expr::Ident(name) = expr {
                    if let Some(&(var, clif_ty)) = self.var_map.get(name) {
                        let val = builder.use_var(var);
                        let one = builder.ins().iconst(clif_ty, 1);
                        let result = builder.ins().iadd(val, one);
                        builder.def_var(var, result);
                    }
                }
                Ok(())
            }
            Stmt::Dec(expr) => {
                if let Expr::Ident(name) = expr {
                    if let Some(&(var, clif_ty)) = self.var_map.get(name) {
                        let val = builder.use_var(var);
                        let one = builder.ins().iconst(clif_ty, 1);
                        let result = builder.ins().isub(val, one);
                        builder.def_var(var, result);
                    }
                }
                Ok(())
            }
            Stmt::Expr(expr) => {
                let _ = self.compile_expr(builder, expr)?;
                Ok(())
            }
            Stmt::Block(block) => {
                for s in &block.stmts {
                    self.compile_stmt(builder, s, has_return)?;
                    if *has_return { break; }
                }
                Ok(())
            }
            Stmt::Empty | Stmt::Fallthrough => Ok(()),
            _ => {
                log::warn!("[go] unimplemented stmt: {:?}", core::mem::discriminant(stmt));
                Ok(())
            }
        }
    }

    fn compile_expr(&mut self, builder: &mut FunctionBuilder, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::IntLit(n) => Ok(builder.ins().iconst(I64, *n)),
            Expr::FloatLit(f) => Ok(builder.ins().f64const(*f)),
            Expr::BoolLit(b) => Ok(builder.ins().iconst(I8, if *b { 1 } else { 0 })),
            Expr::Ident(name) => {
                if let Some(&(var, _)) = self.var_map.get(name) {
                    Ok(builder.use_var(var))
                } else {
                    // Unknown variable, return zero
                    Ok(builder.ins().iconst(I64, 0))
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let l = self.compile_expr(builder, lhs)?;
                let r = self.compile_expr(builder, rhs)?;
                let result = match op {
                    BinOp::Add => builder.ins().iadd(l, r),
                    BinOp::Sub => builder.ins().isub(l, r),
                    BinOp::Mul => builder.ins().imul(l, r),
                    BinOp::Div => builder.ins().sdiv(l, r),
                    BinOp::Mod => builder.ins().srem(l, r),
                    BinOp::BitAnd => builder.ins().band(l, r),
                    BinOp::BitOr => builder.ins().bor(l, r),
                    BinOp::BitXor => builder.ins().bxor(l, r),
                    BinOp::Shl => builder.ins().ishl(l, r),
                    BinOp::Shr => builder.ins().sshr(l, r),
                    BinOp::AndNot => {
                        let not_r = builder.ins().bnot(r);
                        builder.ins().band(l, not_r)
                    }
                    BinOp::Eq => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, l, r),
                    BinOp::Ne => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, l, r),
                    BinOp::Lt => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, l, r),
                    BinOp::Le => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual, l, r),
                    BinOp::Gt => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThan, l, r),
                    BinOp::Ge => builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, l, r),
                    BinOp::LogAnd => builder.ins().band(l, r),
                    BinOp::LogOr => builder.ins().bor(l, r),
                };
                Ok(result)
            }
            Expr::Unary { op, operand } => {
                let val = self.compile_expr(builder, operand)?;
                let result = match op {
                    UnaryOp::Neg => builder.ins().ineg(val),
                    UnaryOp::BitNot => builder.ins().bnot(val),
                    UnaryOp::LogNot => {
                        let zero = builder.ins().iconst(I64, 0);
                        builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, val, zero)
                    }
                };
                Ok(result)
            }
            Expr::Call { func, args } => {
                // For now, calls return i64(0) — runtime dispatch needed
                let _ = args.iter()
                    .map(|a| self.compile_expr(builder, a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(builder.ins().iconst(I64, 0))
            }
            Expr::Nil => Ok(builder.ins().iconst(I64, 0)),
            Expr::StringLit(_) => {
                // Return pointer to string data (simplified: return 0)
                Ok(builder.ins().iconst(I64, 0))
            }
            _ => {
                // Unimplemented expressions return 0
                Ok(builder.ins().iconst(I64, 0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse_tokens;

    #[test]
    fn test_compile_simple_func() {
        let tokens = tokenize(r#"
            package main
            func main() int {
                return 42
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        let mut cg = CodeGen::new();
        if let TopLevelDecl::Func(ref f) = pkg.decls[0] {
            let result = cg.compile_function(f);
            assert!(result.is_ok());
            assert!(result.unwrap().code.len() > 0);
        }
    }

    #[test]
    fn test_compile_arithmetic() {
        let tokens = tokenize(r#"
            package main
            func add(a int, b int) int {
                return a + b
            }
        "#).unwrap();
        let pkg = parse_tokens(&tokens).unwrap();
        let mut cg = CodeGen::new();
        if let TopLevelDecl::Func(ref f) = pkg.decls[0] {
            let result = cg.compile_function(f);
            assert!(result.is_ok());
        }
    }
}
