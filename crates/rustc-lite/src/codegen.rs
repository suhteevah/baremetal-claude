//! Cranelift IR code generation for ClaudioOS rustc-lite.
//!
//! Lowers the typed AST to Cranelift IR, compiles to x86_64 machine code,
//! and produces executable functions that run on bare metal.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use cranelift_codegen::ir::types::{self, Type as CraneliftType};
use cranelift_codegen::ir::{
    AbiParam, Block as CrBlock, Function, InstBuilder, MemFlags, Signature,
    StackSlotData, StackSlotKind, UserFuncName, Value,
};
use cranelift_codegen::isa::{self, CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};

use crate::ast::*;
use crate::types::Type;
use crate::typeck::TypeEnv;

// ─── Compiled output ─────────────────────────────────────────────────────

/// A compiled function ready for execution.
pub struct CompiledFunction {
    pub name: String,
    pub code: Vec<u8>,
    pub ptr: Option<*const u8>,
}

/// Result of compiling a source file.
pub struct CompileResult {
    pub functions: Vec<CompiledFunction>,
    pub errors: Vec<String>,
}

// ─── Code generator ──────────────────────────────────────────────────────

/// Layout entry for a single field: (name, cranelift_type, byte_offset).
type FieldLayout = (String, CraneliftType, i32);

pub struct CodeGen {
    isa: Arc<dyn TargetIsa>,
    errors: Vec<String>,
    /// Signatures of all known functions, populated before compiling bodies.
    fn_sigs: BTreeMap<String, Signature>,
    /// Struct name -> vec of (field_name, cranelift_type, byte_offset).
    struct_layouts: BTreeMap<String, Vec<FieldLayout>>,
}

impl CodeGen {
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| format!("setting opt_level: {:?}", e))?;
        let flags = settings::Flags::new(flag_builder);
        let isa = isa::lookup_by_name("x86_64")
            .map_err(|e| format!("ISA lookup: {:?}", e))?
            .finish(flags)
            .map_err(|e| format!("ISA finish: {:?}", e))?;

        Ok(Self {
            isa,
            errors: Vec::new(),
            fn_sigs: BTreeMap::new(),
            struct_layouts: BTreeMap::new(),
        })
    }

    /// Build a Cranelift Signature from a FnDef's parameter and return types.
    fn sig_from_fndef(f: &FnDef) -> Signature {
        let mut sig = Signature::new(CallConv::SystemV);
        for param in &f.params {
            match param {
                FnParam::SelfParam { .. } => {
                    sig.params.push(AbiParam::new(types::I64));
                }
                FnParam::Typed { ty, .. } => {
                    sig.params.push(AbiParam::new(ast_type_to_cranelift(ty)));
                }
            }
        }
        if f.ret_type.is_some() {
            let ret_cl = f
                .ret_type
                .as_ref()
                .map(|t| ast_type_to_cranelift(t))
                .unwrap_or(types::I64);
            sig.returns.push(AbiParam::new(ret_cl));
        }
        sig
    }

    /// Compile a full source file to executable machine code.
    pub fn compile_file(
        &mut self,
        file: &SourceFile,
        env: &TypeEnv,
    ) -> CompileResult {
        // Phase 0: register struct layouts before compiling any function bodies.
        for item in &file.items {
            if let ItemKind::Struct(s) = &item.kind {
                self.register_struct_layout(s);
            }
        }

        // Phase 1: Pre-populate fn_sigs from all function definitions so that
        // every function body can reference callees by name.
        self.fn_sigs.clear();
        for item in &file.items {
            if let ItemKind::Function(f) = &item.kind {
                self.fn_sigs.insert(f.name.clone(), Self::sig_from_fndef(f));
            }
            if let ItemKind::Impl(imp) = &item.kind {
                for item in &imp.items {
                    if let ItemKind::Function(f) = &item.kind {
                        self.fn_sigs.insert(f.name.clone(), Self::sig_from_fndef(f));
                    }
                }
            }
        }

        // Phase 2: Compile all function bodies.
        let mut functions = Vec::new();

        for item in &file.items {
            if let ItemKind::Function(f) = &item.kind {
                match self.compile_fn(f, env) {
                    Ok(compiled) => functions.push(compiled),
                    Err(e) => self.errors.push(e),
                }
            }
            if let ItemKind::Impl(imp) = &item.kind {
                for item in &imp.items {
                    if let ItemKind::Function(f) = &item.kind {
                        match self.compile_fn(f, env) {
                            Ok(compiled) => functions.push(compiled),
                            Err(e) => self.errors.push(e),
                        }
                    }
                }
            }
        }

        CompileResult {
            functions,
            errors: core::mem::take(&mut self.errors),
        }
    }

    /// Compute and cache the field layout for a struct definition.
    fn register_struct_layout(&mut self, s: &StructDef) {
        let mut fields: Vec<FieldLayout> = Vec::new();
        let mut offset: i32 = 0;

        match &s.kind {
            StructKind::Named(field_defs) => {
                for fd in field_defs {
                    let cl_ty = ast_type_to_cranelift(&fd.ty);
                    let size = cranelift_type_size(cl_ty) as i32;
                    if size > 0 {
                        offset = (offset + size - 1) & !(size - 1);
                    }
                    fields.push((fd.name.clone(), cl_ty, offset));
                    offset += size;
                }
            }
            StructKind::Tuple(tuple_fields) => {
                for (i, tf) in tuple_fields.iter().enumerate() {
                    let cl_ty = ast_type_to_cranelift(&tf.ty);
                    let size = cranelift_type_size(cl_ty) as i32;
                    if size > 0 {
                        offset = (offset + size - 1) & !(size - 1);
                    }
                    fields.push((format!("{}", i), cl_ty, offset));
                    offset += size;
                }
            }
            StructKind::Unit => {}
        }

        self.struct_layouts.insert(s.name.clone(), fields);
    }

    fn compile_fn(
        &mut self,
        f: &FnDef,
        _env: &TypeEnv,
    ) -> Result<CompiledFunction, String> {
        let body = f
            .body
            .as_ref()
            .ok_or_else(|| format!("fn {} has no body", f.name))?;

        let mut sig = Signature::new(CallConv::SystemV);
        for param in &f.params {
            match param {
                FnParam::SelfParam { .. } => {
                    sig.params.push(AbiParam::new(types::I64));
                }
                FnParam::Typed { ty, .. } => {
                    sig.params.push(AbiParam::new(ast_type_to_cranelift(ty)));
                }
            }
        }
        let ret_cl = f
            .ret_type
            .as_ref()
            .map(|t| ast_type_to_cranelift(t))
            .unwrap_or(types::I64);
        if f.ret_type.is_some() {
            sig.returns.push(AbiParam::new(ret_cl));
        }

        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);

        let mut fn_ctx = FnCodegenCtx::new(
            &mut builder, entry, &self.fn_sigs, &self.struct_layouts,
        );

        for (i, param) in f.params.iter().enumerate() {
            match param {
                FnParam::SelfParam { .. } => {
                    let var = fn_ctx.new_variable(types::I64, &mut builder);
                    let val = builder.block_params(entry)[i];
                    builder.def_var(var, val);
                    fn_ctx.locals.insert("self".into(), var);
                }
                FnParam::Typed { pat, ty } => {
                    let cl_ty = ast_type_to_cranelift(ty);
                    if let Pattern::Ident { name, .. } = pat {
                        let var = fn_ctx.new_variable(cl_ty, &mut builder);
                        let val = builder.block_params(entry)[i];
                        builder.def_var(var, val);
                        fn_ctx.locals.insert(name.clone(), var);
                    }
                }
            }
        }

        let result = fn_ctx.compile_block(body, &mut builder)?;

        if f.ret_type.is_some() {
            if let Some(val) = result {
                builder.ins().return_(&[val]);
            } else {
                let zero = builder.ins().iconst(ret_cl, 0);
                builder.ins().return_(&[zero]);
            }
        } else {
            builder.ins().return_(&[]);
        }

        builder.seal_all_blocks();
        builder.finalize();

        let mut ctx = Context::for_function(func);
        let compiled = ctx
            .compile(&*self.isa, &mut Default::default())
            .map_err(|e| format!("compile fn {}: {:?}", f.name, e))?;

        let bytes = compiled.code_buffer().to_vec();
        log::info!(
            "[codegen] compiled fn {} -> {} bytes of x86_64",
            f.name,
            bytes.len()
        );

        Ok(CompiledFunction {
            name: f.name.clone(),
            code: bytes,
            ptr: None,
        })
    }
}

// ─── Closure definition (stored for inline expansion at call site) ──────

struct ClosureDef {
    params: Vec<ClosureParam>,
    body: Expr,
}

// ─── Function-level codegen context ──────────────────────────────────────

struct LoopContext {
    label: Option<String>,
    header_bb: CrBlock,
    exit_bb: CrBlock,
}

struct FnCodegenCtx<'a> {
    locals: BTreeMap<String, Variable>,
    var_counter: u32,
    entry_block: CrBlock,
    loop_stack: Vec<LoopContext>,
    fn_sigs: &'a BTreeMap<String, Signature>,
    struct_layouts: &'a BTreeMap<String, Vec<FieldLayout>>,
    closures: Vec<ClosureDef>,
}

impl<'a> FnCodegenCtx<'a> {
    fn new(
        _builder: &mut FunctionBuilder,
        entry: CrBlock,
        fn_sigs: &'a BTreeMap<String, Signature>,
        struct_layouts: &'a BTreeMap<String, Vec<FieldLayout>>,
    ) -> Self {
        Self {
            locals: BTreeMap::new(),
            var_counter: 0,
            entry_block: entry,
            loop_stack: Vec::new(),
            fn_sigs,
            struct_layouts,
            closures: Vec::new(),
        }
    }

    #[allow(dead_code)]
    fn get_closure(&self, id: usize) -> Option<&ClosureDef> {
        self.closures.get(id)
    }

    #[allow(dead_code)]
    fn call_closure(
        &mut self,
        id: usize,
        arg_vals: &[Value],
        builder: &mut FunctionBuilder,
    ) -> Result<Value, String> {
        let closure = self.closures.get(id)
            .ok_or_else(|| format!("closure id {} not found", id))?;
        let params: Vec<ClosureParam> = closure.params.clone();
        let body: Expr = closure.body.clone();

        for (i, param) in params.iter().enumerate() {
            if let Pattern::Ident { name, .. } = &param.pat {
                let cl_ty = param.ty.as_ref()
                    .map(ast_type_to_cranelift)
                    .unwrap_or(types::I64);
                let var = self.new_variable(cl_ty, builder);
                let val = if i < arg_vals.len() {
                    arg_vals[i]
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                builder.def_var(var, val);
                self.locals.insert(name.clone(), var);
            }
        }

        self.compile_expr(&body, builder)
    }

    fn new_variable(&mut self, ty: CraneliftType, builder: &mut FunctionBuilder) -> Variable {
        let var = Variable::from_u32(self.var_counter);
        self.var_counter += 1;
        builder.declare_var(var, ty);
        var
    }

    fn compile_block(
        &mut self,
        block: &crate::ast::Block,
        builder: &mut FunctionBuilder,
    ) -> Result<Option<Value>, String> {
        let mut last_val = None;

        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { pat, ty, init } => {
                    let cl_ty = ty
                        .as_ref()
                        .map(ast_type_to_cranelift)
                        .unwrap_or(types::I64);

                    if let Pattern::Ident { name, .. } = pat {
                        let var = self.new_variable(cl_ty, builder);
                        if let Some(init_expr) = init {
                            let val = self.compile_expr(init_expr, builder)?;
                            builder.def_var(var, val);
                        } else {
                            let zero = builder.ins().iconst(cl_ty, 0);
                            builder.def_var(var, zero);
                        }
                        self.locals.insert(name.clone(), var);
                    }
                    last_val = None;
                }
                Stmt::Expr(expr) => {
                    self.compile_expr(expr, builder)?;
                    last_val = None;
                }
                Stmt::ExprNoSemi(expr) => {
                    last_val = Some(self.compile_expr(expr, builder)?);
                }
                Stmt::Item(_) => { last_val = None; }
                Stmt::Semi => { last_val = None; }
            }
        }

        Ok(last_val)
    }

    fn compile_expr(
        &mut self,
        expr: &Expr,
        builder: &mut FunctionBuilder,
    ) -> Result<Value, String> {
        match &expr.kind {
            ExprKind::IntLit(n) => {
                Ok(builder.ins().iconst(types::I64, *n as i64))
            }

            ExprKind::FloatLit(f) => {
                Ok(builder.ins().f64const(*f))
            }

            ExprKind::BoolLit(b) => {
                Ok(builder.ins().iconst(types::I8, if *b { 1 } else { 0 }))
            }

            ExprKind::CharLit(c) => {
                Ok(builder.ins().iconst(types::I32, *c as i64))
            }

            ExprKind::Path(path) => {
                let name = path.name();
                if let Some(var) = self.locals.get(name) {
                    Ok(builder.use_var(*var))
                } else {
                    Ok(builder.ins().iconst(types::I64, 0))
                }
            }

            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.compile_expr(lhs, builder)?;
                let r = self.compile_expr(rhs, builder)?;
                let val = match op {
                    BinOp::Add => builder.ins().iadd(l, r),
                    BinOp::Sub => builder.ins().isub(l, r),
                    BinOp::Mul => builder.ins().imul(l, r),
                    BinOp::Div => builder.ins().sdiv(l, r),
                    BinOp::Rem => builder.ins().srem(l, r),
                    BinOp::BitAnd => builder.ins().band(l, r),
                    BinOp::BitOr => builder.ins().bor(l, r),
                    BinOp::BitXor => builder.ins().bxor(l, r),
                    BinOp::Shl => builder.ins().ishl(l, r),
                    BinOp::Shr => builder.ins().sshr(l, r),
                    BinOp::Eq => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::Ne => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::Lt => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::Gt => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThan, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::Le => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::Ge => {
                        let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, l, r);
                        builder.ins().uextend(types::I64, cmp)
                    }
                    BinOp::And => builder.ins().band(l, r),
                    BinOp::Or => builder.ins().bor(l, r),
                };
                Ok(val)
            }

            ExprKind::Unary { op, expr } => {
                let v = self.compile_expr(expr, builder)?;
                match op {
                    UnaryOp::Neg => Ok(builder.ins().ineg(v)),
                    UnaryOp::Not => Ok(builder.ins().bnot(v)),
                    UnaryOp::Deref => Ok(builder.ins().load(types::I64, MemFlags::new(), v, 0)),
                }
            }

            ExprKind::Assign { lhs, rhs } => {
                let val = self.compile_expr(rhs, builder)?;
                if let ExprKind::Path(path) = &lhs.kind {
                    if let Some(var) = self.locals.get(path.name()) {
                        builder.def_var(*var, val);
                    }
                }
                Ok(val)
            }

            ExprKind::AssignOp { op, lhs, rhs } => {
                let lval = self.compile_expr(lhs, builder)?;
                let rval = self.compile_expr(rhs, builder)?;
                let result = match op {
                    BinOp::Add => builder.ins().iadd(lval, rval),
                    BinOp::Sub => builder.ins().isub(lval, rval),
                    BinOp::Mul => builder.ins().imul(lval, rval),
                    BinOp::Div => builder.ins().sdiv(lval, rval),
                    BinOp::Rem => builder.ins().srem(lval, rval),
                    BinOp::BitAnd => builder.ins().band(lval, rval),
                    BinOp::BitOr => builder.ins().bor(lval, rval),
                    BinOp::BitXor => builder.ins().bxor(lval, rval),
                    BinOp::Shl => builder.ins().ishl(lval, rval),
                    BinOp::Shr => builder.ins().sshr(lval, rval),
                    _ => return Err(format!("invalid assign op: {:?}", op)),
                };
                if let ExprKind::Path(path) = &lhs.kind {
                    if let Some(var) = self.locals.get(path.name()) {
                        builder.def_var(*var, result);
                    }
                }
                Ok(result)
            }

            ExprKind::If { cond, then_block, else_expr } => {
                let cond_val = self.compile_expr(cond, builder)?;
                let then_bb = builder.create_block();
                let else_bb = builder.create_block();
                let merge_bb = builder.create_block();
                builder.append_block_param(merge_bb, types::I64);

                let cond_i8 = builder.ins().ireduce(types::I8, cond_val);
                builder.ins().brif(cond_i8, then_bb, &[], else_bb, &[]);

                builder.switch_to_block(then_bb);
                builder.seal_block(then_bb);
                let then_val = self.compile_block(then_block, builder)?
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                builder.ins().jump(merge_bb, &[then_val]);

                builder.switch_to_block(else_bb);
                builder.seal_block(else_bb);
                let else_val = if let Some(else_e) = else_expr {
                    self.compile_expr(else_e, builder)?
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                builder.ins().jump(merge_bb, &[else_val]);

                builder.switch_to_block(merge_bb);
                builder.seal_block(merge_bb);
                Ok(builder.block_params(merge_bb)[0])
            }

            ExprKind::Loop { body, label } => {
                // loop_bb = unconditional loop body, exit_bb = target for `break`
                let loop_bb = builder.create_block();
                let exit_bb = builder.create_block();
                // exit_bb carries the break value as a block parameter
                builder.append_block_param(exit_bb, types::I64);

                builder.ins().jump(loop_bb, &[]);
                builder.switch_to_block(loop_bb);

                // Push context so nested break/continue can find our blocks
                self.loop_stack.push(LoopContext { label: label.clone(), header_bb: loop_bb, exit_bb });
                self.compile_block(body, builder)?;
                self.loop_stack.pop();

                // Unconditional back-edge: loop forever until break
                builder.ins().jump(loop_bb, &[]);
                builder.seal_block(loop_bb);
                builder.switch_to_block(exit_bb);
                builder.seal_block(exit_bb);
                Ok(builder.block_params(exit_bb)[0])
            }

            ExprKind::While { cond, body, label } => {
                // header_bb: re-evaluate condition each iteration
                // body_bb: loop body, exit_bb: post-loop (receives break value)
                let header_bb = builder.create_block();
                let body_bb = builder.create_block();
                let exit_bb = builder.create_block();
                builder.append_block_param(exit_bb, types::I64);

                builder.ins().jump(header_bb, &[]);
                builder.switch_to_block(header_bb);

                // Branch: true -> body, false -> exit with zero (while has no break value)
                let cond_val = self.compile_expr(cond, builder)?;
                let cond_i8 = builder.ins().ireduce(types::I8, cond_val);
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().brif(cond_i8, body_bb, &[], exit_bb, &[zero]);

                builder.switch_to_block(body_bb);
                builder.seal_block(body_bb);
                self.loop_stack.push(LoopContext { label: label.clone(), header_bb, exit_bb });
                self.compile_block(body, builder)?;
                self.loop_stack.pop();
                // Back-edge to header for next condition check
                builder.ins().jump(header_bb, &[]);

                // header_bb sealed after both predecessors (entry + back-edge) are known
                builder.seal_block(header_bb);
                builder.switch_to_block(exit_bb);
                builder.seal_block(exit_bb);
                Ok(builder.block_params(exit_bb)[0])
            }

            // Desugars `for pat in range` into a counted loop with an explicit
            // iterator variable, header block for bounds check, and increment.
            ExprKind::For { pat, iter, body, label } => {
                // Extract range bounds; non-range iterators desugar to 0..count
                let (start_val, end_val) = match &iter.kind {
                    ExprKind::Range { start: Some(s), end: Some(e), .. } => {
                        (self.compile_expr(s, builder)?, self.compile_expr(e, builder)?)
                    }
                    _ => {
                        let count = self.compile_expr(iter, builder)?;
                        (builder.ins().iconst(types::I64, 0), count)
                    }
                };

                // Create the loop variable and bind it to the pattern name
                let iter_var = self.new_variable(types::I64, builder);
                builder.def_var(iter_var, start_val);
                if let Pattern::Ident { name, .. } = pat {
                    self.locals.insert(name.clone(), iter_var);
                }

                let header_bb = builder.create_block();
                let body_bb = builder.create_block();
                let exit_bb = builder.create_block();
                builder.append_block_param(exit_bb, types::I64);

                builder.ins().jump(header_bb, &[]);
                builder.switch_to_block(header_bb);

                // Bounds check: continue if iter_var < end, else exit
                let current = builder.use_var(iter_var);
                let cmp = builder.ins().icmp(
                    cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, current, end_val,
                );
                let zero_exit = builder.ins().iconst(types::I64, 0);
                builder.ins().brif(cmp, body_bb, &[], exit_bb, &[zero_exit]);

                builder.switch_to_block(body_bb);
                builder.seal_block(body_bb);
                self.loop_stack.push(LoopContext { label: label.clone(), header_bb, exit_bb });
                self.compile_block(body, builder)?;
                self.loop_stack.pop();

                // Increment the iterator and jump back to the bounds check
                let current = builder.use_var(iter_var);
                let one = builder.ins().iconst(types::I64, 1);
                let next = builder.ins().iadd(current, one);
                builder.def_var(iter_var, next);
                builder.ins().jump(header_bb, &[]);

                builder.seal_block(header_bb);
                builder.switch_to_block(exit_bb);
                builder.seal_block(exit_bb);
                Ok(builder.block_params(exit_bb)[0])
            }

            ExprKind::Break { label, value } => {
                let break_val = if let Some(v) = value {
                    self.compile_expr(v, builder)?
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                // Find the target loop: labeled break searches by name, unlabeled uses innermost
                let exit_bb = if let Some(lbl) = label {
                    self.loop_stack.iter().rev()
                        .find(|ctx| ctx.label.as_deref() == Some(lbl.as_str()))
                        .map(|ctx| ctx.exit_bb)
                } else {
                    self.loop_stack.last().map(|ctx| ctx.exit_bb)
                }.ok_or_else(|| "break outside of loop".to_string())?;

                // Jump to the loop's exit block, passing the break value
                builder.ins().jump(exit_bb, &[break_val]);
                // Cranelift requires all code to live in a block; create an unreachable
                // block so subsequent (dead) code has somewhere to go
                let after = builder.create_block();
                builder.switch_to_block(after);
                builder.seal_block(after);
                Ok(builder.ins().iconst(types::I64, 0))
            }

            ExprKind::Continue { label } => {
                // Jump back to the loop's header (condition re-check for while/for)
                let header_bb = if let Some(lbl) = label {
                    self.loop_stack.iter().rev()
                        .find(|ctx| ctx.label.as_deref() == Some(lbl.as_str()))
                        .map(|ctx| ctx.header_bb)
                } else {
                    self.loop_stack.last().map(|ctx| ctx.header_bb)
                }.ok_or_else(|| "continue outside of loop".to_string())?;

                builder.ins().jump(header_bb, &[]);
                // Unreachable block for any dead code after continue
                let after = builder.create_block();
                builder.switch_to_block(after);
                builder.seal_block(after);
                Ok(builder.ins().iconst(types::I64, 0))
            }

            ExprKind::Block(block) => {
                let val = self.compile_block(block, builder)?;
                Ok(val.unwrap_or_else(|| builder.ins().iconst(types::I64, 0)))
            }

            ExprKind::Return(val) => {
                if let Some(v) = val {
                    let ret = self.compile_expr(v, builder)?;
                    builder.ins().return_(&[ret]);
                } else {
                    builder.ins().return_(&[]);
                }
                // Unreachable block after return (same pattern as break/continue)
                let after = builder.create_block();
                builder.switch_to_block(after);
                builder.seal_block(after);
                Ok(builder.ins().iconst(types::I64, 0))
            }

            ExprKind::Call { func, args } => {
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.compile_expr(a, builder))
                    .collect::<Result<_, _>>()?;

                // Try to resolve callee as a named path for signature lookup
                let callee_name = match &func.kind {
                    ExprKind::Path(path) => Some(path.name().to_string()),
                    _ => None,
                };

                if let Some(ref name) = callee_name {
                    if let Some(callee_sig) = self.fn_sigs.get(name).cloned() {
                        // Known function: use its real signature from the pre-populated fn_sigs map
                        let sig_ref = builder.import_signature(callee_sig);
                        // Generate a deterministic placeholder address (0xDEAD_0000 + hash).
                        // The linker patches this to the real address at load time.
                        let name_hash = {
                            let mut h: i64 = 0;
                            for b in name.bytes() { h = h.wrapping_mul(31).wrapping_add(b as i64); }
                            h & 0x0FFF_FFFF
                        };
                        let fn_addr = builder.ins().iconst(types::I64, 0xDEAD_0000_i64.wrapping_add(name_hash));
                        let call_inst = builder.ins().call_indirect(sig_ref, fn_addr, &arg_vals);
                        let results = builder.inst_results(call_inst);
                        if results.is_empty() { Ok(builder.ins().iconst(types::I64, 0)) } else { Ok(results[0]) }
                    } else {
                        // Unknown function: treat callee as a function pointer with a
                        // fallback signature (all params i64, returns i64)
                        let fn_val = self.compile_expr(func, builder)?;
                        let mut sig = Signature::new(CallConv::SystemV);
                        for _ in &arg_vals { sig.params.push(AbiParam::new(types::I64)); }
                        sig.returns.push(AbiParam::new(types::I64));
                        let sig_ref = builder.import_signature(sig);
                        let call_inst = builder.ins().call_indirect(sig_ref, fn_val, &arg_vals);
                        let results = builder.inst_results(call_inst);
                        if results.is_empty() { Ok(builder.ins().iconst(types::I64, 0)) } else { Ok(results[0]) }
                    }
                } else {
                    // Non-path callee (e.g. closure call): use fallback i64 signature
                    let fn_val = self.compile_expr(func, builder)?;
                    let mut sig = Signature::new(CallConv::SystemV);
                    for _ in &arg_vals { sig.params.push(AbiParam::new(types::I64)); }
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let call_inst = builder.ins().call_indirect(sig_ref, fn_val, &arg_vals);
                    let results = builder.inst_results(call_inst);
                    if results.is_empty() { Ok(builder.ins().iconst(types::I64, 0)) } else { Ok(results[0]) }
                }
            }

            // Method calls desugar to fn(self, args...) — receiver becomes first arg
            ExprKind::MethodCall { receiver, method, generics: _, args } => {
                let recv_val = self.compile_expr(receiver, builder)?;
                // Prepend receiver as implicit `self` parameter
                let mut all_args = Vec::new();
                all_args.push(recv_val);
                for a in args { all_args.push(self.compile_expr(a, builder)?); }

                // Placeholder address for link-time patching (same scheme as Call)
                let name_hash = {
                    let mut h: i64 = 0;
                    for b in method.bytes() { h = h.wrapping_mul(31).wrapping_add(b as i64); }
                    h & 0x0FFF_FFFF
                };
                let fn_addr = builder.ins().iconst(types::I64, 0xDEAD_0000_i64.wrapping_add(name_hash));

                if let Some(callee_sig) = self.fn_sigs.get(method.as_str()).cloned() {
                    let sig_ref = builder.import_signature(callee_sig);
                    let call_inst = builder.ins().call_indirect(sig_ref, fn_addr, &all_args);
                    let results = builder.inst_results(call_inst);
                    if results.is_empty() { Ok(builder.ins().iconst(types::I64, 0)) } else { Ok(results[0]) }
                } else {
                    // Fallback: assume all-i64 signature when method isn't in fn_sigs
                    let mut sig = Signature::new(CallConv::SystemV);
                    for _ in &all_args { sig.params.push(AbiParam::new(types::I64)); }
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let call_inst = builder.ins().call_indirect(sig_ref, fn_addr, &all_args);
                    let results = builder.inst_results(call_inst);
                    if results.is_empty() { Ok(builder.ins().iconst(types::I64, 0)) } else { Ok(results[0]) }
                }
            }

            ExprKind::Closure { params, body, .. } => {
                let closure_id = self.closures.len() as i64;
                self.closures.push(ClosureDef { params: params.clone(), body: (**body).clone() });
                Ok(builder.ins().iconst(types::I64, closure_id))
            }

            ExprKind::Cast { expr, .. } => self.compile_expr(expr, builder),
            ExprKind::Ref { expr, .. } => self.compile_expr(expr, builder),

            ExprKind::Deref(expr) => {
                let ptr = self.compile_expr(expr, builder)?;
                Ok(builder.ins().load(types::I64, MemFlags::new(), ptr, 0))
            }

            ExprKind::Tuple(exprs) => {
                if exprs.is_empty() {
                    return Ok(builder.ins().iconst(types::I64, 0));
                }
                let mut field_info: Vec<(CraneliftType, i32)> = Vec::new();
                let mut offset: i32 = 0;
                let mut compiled_vals: Vec<Value> = Vec::new();
                for e in exprs {
                    let val = self.compile_expr(e, builder)?;
                    let cl_ty = builder.func.dfg.value_type(val);
                    let size = cranelift_type_size(cl_ty) as i32;
                    if size > 0 { offset = (offset + size - 1) & !(size - 1); }
                    field_info.push((cl_ty, offset));
                    compiled_vals.push(val);
                    offset += size;
                }
                let total = ((offset + 7) & !7) as u32;
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, total, 3));
                for (i, val) in compiled_vals.into_iter().enumerate() {
                    builder.ins().stack_store(val, slot, field_info[i].1);
                }
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            // Allocate struct on the stack and store each field at its computed offset
            ExprKind::StructLit { path, fields, rest: _ } => {
                let name = path.name();
                // Look up the pre-registered layout (field name, type, byte offset)
                let layout = self.struct_layouts.get(name)
                    .ok_or_else(|| format!("unknown struct: {}", name))?.clone();
                // Total size rounded up to 8-byte alignment
                let struct_size = if layout.is_empty() { 8u32 } else {
                    let last = &layout[layout.len() - 1];
                    ((last.2 + cranelift_type_size(last.1) as i32 + 7) & !7) as u32
                };
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, struct_size, 3));
                for field_init in fields {
                    // Find this field's byte offset in the layout registry
                    let (_, _, field_offset) = layout.iter()
                        .find(|(n, _, _)| n == &field_init.name)
                        .ok_or_else(|| format!("unknown field: {}.{}", name, field_init.name))?;
                    let field_offset = *field_offset;
                    let val = self.compile_expr(&field_init.value, builder)?;
                    builder.ins().stack_store(val, slot, field_offset);
                }
                // Return pointer to the stack-allocated struct
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            // Field access: search all struct layouts for the field name to find its offset.
            // This is a linear scan because we lack type info to know which struct it is.
            ExprKind::Field { expr: base_expr, name } => {
                let base_ptr = self.compile_expr(base_expr, builder)?;
                for (_sname, layout) in self.struct_layouts.iter() {
                    if let Some((_n, cl_ty, offset)) = layout.iter().find(|(n, _, _)| n == name) {
                        return Ok(builder.ins().load(*cl_ty, MemFlags::new(), base_ptr, *offset));
                    }
                }
                Ok(builder.ins().iconst(types::I64, 0))
            }

            // Tuple index (e.g. `t.0`): tuple structs store fields as "0", "1", etc.
            // Falls back to stride-based offset if no layout is registered.
            ExprKind::TupleIndex { expr: base_expr, index } => {
                let base_ptr = self.compile_expr(base_expr, builder)?;
                let field_name = format!("{}", index);
                for (_sname, layout) in self.struct_layouts.iter() {
                    if let Some((_n, cl_ty, offset)) = layout.iter().find(|(n, _, _)| *n == field_name) {
                        return Ok(builder.ins().load(*cl_ty, MemFlags::new(), base_ptr, *offset));
                    }
                }
                // Fallback: assume 8-byte stride for unknown tuple types
                Ok(builder.ins().load(types::I64, MemFlags::new(), base_ptr, (*index as i32) * 8))
            }

            // Match compiles as a chain of test blocks: each tests one arm's pattern,
            // falling through to the next on mismatch. The last arm is unconditional
            // (acts as the exhaustiveness catch-all).
            ExprKind::Match { expr, arms } => {
                if arms.is_empty() { return Ok(builder.ins().iconst(types::I64, 0)); }
                let scrutinee_val = self.compile_expr(expr, builder)?;
                // merge_bb collects the result from whichever arm matched
                let merge_bb = builder.create_block();
                builder.append_block_param(merge_bb, types::I64);
                let mut test_blocks = Vec::new();
                for _ in 0..arms.len() { test_blocks.push(builder.create_block()); }
                builder.ins().jump(test_blocks[0], &[]);

                for (i, arm) in arms.iter().enumerate() {
                    let body_bb = builder.create_block();
                    builder.switch_to_block(test_blocks[i]);
                    builder.seal_block(test_blocks[i]);
                    let matches_val = self.compile_pattern_test(&arm.pat, scrutinee_val, builder)?;
                    // If there's a guard, AND it with the pattern test result
                    let final_test = if let Some(guard) = &arm.guard {
                        let guard_val = self.compile_expr(guard, builder)?;
                        builder.ins().band(matches_val, guard_val)
                    } else { matches_val };

                    if i + 1 < arms.len() {
                        // Not the last arm: branch on test, fall through to next arm on failure
                        let test_i8 = builder.ins().ireduce(types::I8, final_test);
                        builder.ins().brif(test_i8, body_bb, &[], test_blocks[i + 1], &[]);
                    } else {
                        // Last arm: unconditional jump (serves as the catch-all/wildcard)
                        builder.ins().jump(body_bb, &[]);
                    }

                    builder.switch_to_block(body_bb);
                    builder.seal_block(body_bb);
                    // Bind pattern variables (e.g. `x` in `Some(x)`) before evaluating body
                    self.bind_pattern_vars(&arm.pat, scrutinee_val, builder)?;
                    let body_val = self.compile_expr(&arm.body, builder)?;
                    builder.ins().jump(merge_bb, &[body_val]);
                }

                builder.switch_to_block(merge_bb);
                builder.seal_block(merge_bb);
                Ok(builder.block_params(merge_bb)[0])
            }

            // Array literal: allocate contiguous stack slot, store each element at 8-byte stride
            ExprKind::Array(exprs) => {
                let elem_size = 8i32;
                let total_size = (exprs.len() as u32) * (elem_size as u32);
                if total_size == 0 { return Ok(builder.ins().iconst(types::I64, 0)); }
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, total_size, 0));
                for (i, expr) in exprs.iter().enumerate() {
                    let val = self.compile_expr(expr, builder)?;
                    builder.ins().stack_store(val, slot, (i as i32) * elem_size);
                }
                // Return pointer to first element
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            // [value; count] — only stores the first element (TODO: fill loop)
            ExprKind::ArrayRepeat { value, count } => {
                let val = self.compile_expr(value, builder)?;
                let _count_val = self.compile_expr(count, builder)?;
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 256 * 8, 0));
                builder.ins().stack_store(val, slot, 0);
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            ExprKind::Index { expr, index } => {
                let base = self.compile_expr(expr, builder)?;
                let idx = self.compile_expr(index, builder)?;
                let elem_size = builder.ins().iconst(types::I64, 8);
                let offset = builder.ins().imul(idx, elem_size);
                let addr = builder.ins().iadd(base, offset);
                Ok(builder.ins().load(types::I64, MemFlags::new(), addr, 0))
            }

            // String literal: pack bytes into a stack slot, 8 bytes at a time for efficiency.
            // Remaining bytes (< 8) are stored individually.
            ExprKind::StringLit(s) => {
                let bytes = s.as_bytes();
                if bytes.is_empty() { return Ok(builder.ins().iconst(types::I64, 0)); }
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes.len() as u32, 0));
                let mut offset = 0i32;
                for chunk in bytes.chunks(8) {
                    if chunk.len() == 8 {
                        // Pack 8 bytes into a single i64 (little-endian) for one store
                        let mut val: i64 = 0;
                        for (j, &b) in chunk.iter().enumerate() { val |= (b as i64) << (j * 8); }
                        let const_val = builder.ins().iconst(types::I64, val);
                        builder.ins().stack_store(const_val, slot, offset);
                    } else {
                        for (j, &b) in chunk.iter().enumerate() {
                            let byte_val = builder.ins().iconst(types::I8, b as i64);
                            builder.ins().stack_store(byte_val, slot, offset + j as i32);
                        }
                    }
                    offset += chunk.len() as i32;
                }
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            // Range expression: store as a (start, end) pair in a 16-byte stack slot
            ExprKind::Range { start, end, inclusive: _ } => {
                let start_val = start.as_ref().map(|s| self.compile_expr(s, builder)).transpose()?.unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                let end_val = end.as_ref().map(|e| self.compile_expr(e, builder)).transpose()?.unwrap_or_else(|| builder.ins().iconst(types::I64, i64::MAX));
                let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 0));
                builder.ins().stack_store(start_val, slot, 0);
                builder.ins().stack_store(end_val, slot, 8);
                Ok(builder.ins().stack_addr(types::I64, slot, 0))
            }

            // Fallback for expressions we can't compile yet
            _ => Ok(builder.ins().iconst(types::I64, 0)),
        }
    }

    /// Returns i64 1 if the pattern matches the scrutinee, 0 otherwise.
    fn compile_pattern_test(&mut self, pat: &Pattern, scrutinee: Value, builder: &mut FunctionBuilder) -> Result<Value, String> {
        match pat {
            // Wildcard `_` always matches
            Pattern::Wildcard => Ok(builder.ins().iconst(types::I64, 1)),
            // Ident binds the value; delegates to sub-pattern if `name @ pat`
            Pattern::Ident { binding, .. } => {
                if let Some(sub_pat) = binding { self.compile_pattern_test(sub_pat, scrutinee, builder) }
                else { Ok(builder.ins().iconst(types::I64, 1)) }
            }
            // Literal pattern: exact equality test against scrutinee
            Pattern::Lit(lit_expr) => {
                let lit_val = self.compile_expr(lit_expr, builder)?;
                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, scrutinee, lit_val);
                Ok(builder.ins().uextend(types::I64, cmp))
            }
            // Path pattern (e.g. `Enum::Variant`): compare discriminant hashes
            Pattern::Path(path) => {
                if path.segments.len() >= 2 {
                    let disc = variant_name_to_discriminant(path.name());
                    let disc_val = builder.ins().iconst(types::I64, disc);
                    let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, scrutinee, disc_val);
                    Ok(builder.ins().uextend(types::I64, cmp))
                } else { Ok(builder.ins().iconst(types::I64, 1)) }
            }
            // Or pattern `a | b | c`: short-circuit OR of each sub-pattern test
            Pattern::Or(pats) => {
                if pats.is_empty() { return Ok(builder.ins().iconst(types::I64, 0)); }
                let mut result = self.compile_pattern_test(&pats[0], scrutinee, builder)?;
                for p in &pats[1..] {
                    let next = self.compile_pattern_test(p, scrutinee, builder)?;
                    result = builder.ins().bor(result, next);
                }
                Ok(result)
            }
            // Range pattern `a..=b`: AND of (scrutinee >= start) and (scrutinee <= end)
            Pattern::Range { start, end, inclusive } => {
                let mut result = builder.ins().iconst(types::I64, 1);
                if let Some(start_expr) = start {
                    let start_val = self.compile_expr(start_expr, builder)?;
                    let ge = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, scrutinee, start_val);
                    let ge_ext = builder.ins().uextend(types::I64, ge);
                    result = builder.ins().band(result, ge_ext);
                }
                if let Some(end_expr) = end {
                    let end_val = self.compile_expr(end_expr, builder)?;
                    let cc = if *inclusive { cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual } else { cranelift_codegen::ir::condcodes::IntCC::SignedLessThan };
                    let le = builder.ins().icmp(cc, scrutinee, end_val);
                    let le_ext = builder.ins().uextend(types::I64, le);
                    result = builder.ins().band(result, le_ext);
                }
                Ok(result)
            }
            // Ref pattern `&pat`: test the inner pattern directly
            Pattern::Ref { pat, .. } => self.compile_pattern_test(pat, scrutinee, builder),
            // Unhandled patterns: optimistically match (relies on exhaustiveness)
            _ => Ok(builder.ins().iconst(types::I64, 1)),
        }
    }

    fn bind_pattern_vars(&mut self, pat: &Pattern, scrutinee: Value, builder: &mut FunctionBuilder) -> Result<(), String> {
        match pat {
            Pattern::Ident { name, binding, .. } => {
                let var = self.new_variable(types::I64, builder);
                builder.def_var(var, scrutinee);
                self.locals.insert(name.clone(), var);
                if let Some(sub_pat) = binding { self.bind_pattern_vars(sub_pat, scrutinee, builder)?; }
            }
            Pattern::Or(pats) => {
                if let Some(first) = pats.first() { self.bind_pattern_vars(first, scrutinee, builder)?; }
            }
            Pattern::Ref { pat, .. } => { self.bind_pattern_vars(pat, scrutinee, builder)?; }
            _ => {}
        }
        Ok(())
    }
}

fn variant_name_to_discriminant(name: &str) -> i64 {
    let mut hash: i64 = 0;
    for b in name.bytes() { hash = hash.wrapping_mul(31).wrapping_add(b as i64); }
    hash
}

// ─── Type mapping ────────────────────────────────────────────────────────

fn ast_type_to_cranelift(ty: &crate::ast::Ty) -> CraneliftType {
    match ty {
        Ty::Path(path) => match path.name() {
            "bool" => types::I8,
            "i8" | "u8" => types::I8,
            "i16" | "u16" => types::I16,
            "i32" | "u32" | "char" => types::I32,
            "i64" | "u64" | "isize" | "usize" => types::I64,
            "i128" | "u128" => types::I128,
            "f32" => types::F32,
            "f64" => types::F64,
            _ => types::I64,
        },
        Ty::Reference { .. } | Ty::RawPtr { .. } => types::I64,
        Ty::Tuple(ts) if ts.is_empty() => types::I64,
        Ty::Never => types::I64,
        _ => types::I64,
    }
}

fn cranelift_type_size(ty: CraneliftType) -> u32 {
    (ty.bits() + 7) / 8
}

fn internal_type_to_cranelift(ty: &Type) -> CraneliftType {
    match ty {
        Type::Bool => types::I8,
        Type::I8 | Type::U8 => types::I8,
        Type::I16 | Type::U16 => types::I16,
        Type::I32 | Type::U32 | Type::Char => types::I32,
        Type::I64 | Type::U64 | Type::Isize | Type::Usize => types::I64,
        Type::I128 | Type::U128 => types::I128,
        Type::F32 => types::F32,
        Type::F64 => types::F64,
        _ => types::I64,
    }
}

// ─── Executable memory helpers ───────────────────────────────────────────

pub unsafe fn make_executable(code: &[u8]) -> *const u8 {
    let mut mem = vec![0u8; code.len()];
    core::ptr::copy_nonoverlapping(code.as_ptr(), mem.as_mut_ptr(), code.len());
    let ptr = mem.as_ptr();
    core::mem::forget(mem);
    ptr
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::typeck::TypeEnv;

    fn compile_src(src: &str) -> CompileResult {
        let tokens = Lexer::tokenize(src).unwrap();
        let file = Parser::parse_file(tokens).unwrap();
        let mut env = TypeEnv::new();
        env.check_file(&file);
        let mut codegen = CodeGen::new().unwrap();
        codegen.compile_file(&file, &env)
    }

    #[test]
    fn compile_simple_add() {
        let result = compile_src("fn add(a: i64, b: i64) -> i64 { a + b }");
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.functions.len(), 1);
        assert!(!result.functions[0].code.is_empty());
    }

    #[test]
    fn compile_if_else() {
        let result = compile_src("fn max(a: i64, b: i64) -> i64 { if a > b { a } else { b } }");
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.functions.len(), 1);
    }

    #[test]
    fn compile_while_loop() {
        let result = compile_src(r#"
            fn sum_to(n: i64) -> i64 {
                let mut total = 0;
                let mut i = 0;
                while i < n {
                    total += i;
                    i += 1;
                }
                total
            }
        "#);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    #[test]
    fn compile_and_execute_add() {
        let result = compile_src("fn add(a: i64, b: i64) -> i64 { a + b }");
        assert!(result.errors.is_empty());
        let code = &result.functions[0].code;
        assert!(!code.is_empty());
        unsafe {
            let ptr = make_executable(code);
            let func: fn(i64, i64) -> i64 = core::mem::transmute(ptr);
            assert_eq!(func(3, 4), 7);
            assert_eq!(func(100, -50), 50);
        }
    }

    #[test]
    fn compile_and_execute_factorial() {
        let result = compile_src(r#"
            fn factorial(n: i64) -> i64 {
                let mut result = 1;
                let mut i = 1;
                while i <= n {
                    result = result * i;
                    i += 1;
                }
                result
            }
        "#);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let code = &result.functions[0].code;
        unsafe {
            let ptr = make_executable(code);
            let func: fn(i64) -> i64 = core::mem::transmute(ptr);
            assert_eq!(func(0), 1);
            assert_eq!(func(1), 1);
            assert_eq!(func(5), 120);
            assert_eq!(func(10), 3628800);
        }
    }
}
