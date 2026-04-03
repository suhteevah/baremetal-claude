//! Stack-based WASM interpreter: ~200 opcodes.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use crate::binary::BinaryReader;
use crate::instance::WasmInstance;
use crate::types::*;
use crate::module::ConstExpr;

/// Maximum call depth to prevent stack overflow.
const MAX_CALL_DEPTH: usize = 1024;

/// Maximum instruction count per invocation (prevents infinite loops).
const MAX_INSTRUCTIONS: u64 = 100_000_000;

/// A call frame on the call stack.
#[derive(Debug)]
pub struct CallFrame {
    /// Function index being executed.
    pub func_idx: u32,
    /// Local variables (params + declared locals).
    pub locals: Vec<Value>,
    /// Return address: position in the calling function's code.
    pub return_pc: usize,
    /// Base of the value stack for this frame.
    pub stack_base: usize,
    /// Label stack base for this frame.
    pub label_base: usize,
}

/// A label for structured control flow (block/loop/if).
#[derive(Debug, Clone)]
pub struct Label {
    /// Number of result values.
    pub arity: usize,
    /// For blocks: continuation is after end. For loops: continuation is at start.
    pub continuation: usize,
    /// Is this a loop label? (br targets loop header vs block end).
    pub is_loop: bool,
    /// Stack height when this label was pushed.
    pub stack_height: usize,
}

/// The interpreter engine.
pub struct Interpreter {
    /// Value stack.
    pub value_stack: Vec<Value>,
    /// Call stack.
    pub call_stack: Vec<CallFrame>,
    /// Label stack for structured control flow.
    pub label_stack: Vec<Label>,
    /// Instruction counter for fuel-based limiting.
    pub instruction_count: u64,
    /// Captured stdout output from WASI fd_write.
    pub stdout_buffer: Vec<u8>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            value_stack: Vec::with_capacity(1024),
            call_stack: Vec::new(),
            label_stack: Vec::new(),
            instruction_count: 0,
            stdout_buffer: Vec::new(),
        }
    }

    fn push(&mut self, val: Value) {
        self.value_stack.push(val);
    }

    fn pop(&mut self) -> Result<Value, String> {
        self.value_stack.pop().ok_or_else(|| String::from("value stack underflow"))
    }

    fn pop_i32(&mut self) -> Result<i32, String> {
        Ok(self.pop()?.as_i32()?)
    }

    fn pop_i64(&mut self) -> Result<i64, String> {
        Ok(self.pop()?.as_i64()?)
    }

    fn pop_f32(&mut self) -> Result<f32, String> {
        Ok(self.pop()?.as_f32()?)
    }

    fn pop_f64(&mut self) -> Result<f64, String> {
        Ok(self.pop()?.as_f64()?)
    }

    /// Invoke a function by index.
    pub fn invoke(
        &mut self,
        instance: &mut WasmInstance,
        func_idx: u32,
        args: &[Value],
    ) -> Result<Vec<Value>, String> {
        let import_count = instance.module.import_func_count() as u32;

        if func_idx < import_count {
            // Call imported function
            return instance.call_import(func_idx, args, self);
        }

        let func_type = instance.module.func_type(func_idx)
            .ok_or_else(|| format!("function {} not found", func_idx))?
            .clone();

        let code_idx = (func_idx - import_count) as usize;
        let code_body = instance.module.code.get(code_idx)
            .ok_or_else(|| format!("code body {} not found", code_idx))?;

        // Set up locals: params first, then declared locals with default values
        let mut locals = Vec::with_capacity(func_type.params.len() + code_body.locals.len());
        for (i, _) in func_type.params.iter().enumerate() {
            if i < args.len() {
                locals.push(args[i]);
            } else {
                locals.push(Value::default_for(func_type.params[i]));
            }
        }
        for lt in &code_body.locals {
            locals.push(Value::default_for(*lt));
        }

        let frame = CallFrame {
            func_idx,
            locals,
            return_pc: 0,
            stack_base: self.value_stack.len(),
            label_base: self.label_stack.len(),
        };

        if self.call_stack.len() >= MAX_CALL_DEPTH {
            return Err(String::from("call stack overflow"));
        }

        self.call_stack.push(frame);

        // Push implicit block label for the function body
        let result_arity = func_type.results.len();
        self.label_stack.push(Label {
            arity: result_arity,
            continuation: code_body.code.len(),
            is_loop: false,
            stack_height: self.value_stack.len(),
        });

        let code = code_body.code.clone();
        let result = self.execute_code(instance, &code, result_arity);

        // Pop the frame
        self.call_stack.pop();

        result
    }

    fn execute_code(
        &mut self,
        instance: &mut WasmInstance,
        code: &[u8],
        result_arity: usize,
    ) -> Result<Vec<Value>, String> {
        let mut reader = BinaryReader::new(code);

        while !reader.is_empty() {
            self.instruction_count += 1;
            if self.instruction_count > MAX_INSTRUCTIONS {
                return Err(String::from("instruction limit exceeded"));
            }

            let opcode = reader.read_byte()?;

            match opcode {
                // unreachable
                0x00 => return Err(String::from("unreachable executed")),

                // nop
                0x01 => {}

                // block
                0x02 => {
                    let bt = self.read_block_type(&mut reader)?;
                    let arity = self.block_type_arity(&bt, instance);
                    // Find matching end
                    let cont = self.find_end(code, reader.position())?;
                    self.label_stack.push(Label {
                        arity,
                        continuation: cont,
                        is_loop: false,
                        stack_height: self.value_stack.len(),
                    });
                }

                // loop
                0x03 => {
                    let bt = self.read_block_type(&mut reader)?;
                    let _arity = self.block_type_arity(&bt, instance);
                    let loop_start = reader.position();
                    let cont = self.find_end(code, reader.position())?;
                    let _ = cont; // loops branch back to start
                    self.label_stack.push(Label {
                        arity: 0, // loop br arity is 0 for the loop label
                        continuation: loop_start,
                        is_loop: true,
                        stack_height: self.value_stack.len(),
                    });
                }

                // if
                0x04 => {
                    let bt = self.read_block_type(&mut reader)?;
                    let arity = self.block_type_arity(&bt, instance);
                    let cond = self.pop_i32()?;
                    let cont = self.find_end(code, reader.position())?;

                    if cond != 0 {
                        // Take the if branch
                        self.label_stack.push(Label {
                            arity,
                            continuation: cont,
                            is_loop: false,
                            stack_height: self.value_stack.len(),
                        });
                    } else {
                        // Find else or end
                        match self.find_else(code, reader.position())? {
                            Some(else_pos) => {
                                reader = BinaryReader::new(code);
                                // Skip to after the else opcode
                                reader.read_bytes(else_pos)?;
                                self.label_stack.push(Label {
                                    arity,
                                    continuation: cont,
                                    is_loop: false,
                                    stack_height: self.value_stack.len(),
                                });
                            }
                            None => {
                                // No else branch, skip to end
                                reader = BinaryReader::new(code);
                                reader.read_bytes(cont)?;
                            }
                        }
                    }
                }

                // else
                0x05 => {
                    // We're in the if-true branch hitting else; jump to end
                    if let Some(label) = self.label_stack.last() {
                        let cont = label.continuation;
                        reader = BinaryReader::new(code);
                        reader.read_bytes(cont)?;
                    }
                }

                // end
                0x0B => {
                    let frame_label_base = self.call_stack.last()
                        .map(|f| f.label_base)
                        .unwrap_or(0);

                    if self.label_stack.len() > frame_label_base {
                        self.label_stack.pop();
                    }

                    if self.label_stack.len() <= frame_label_base {
                        // Function end
                        break;
                    }
                }

                // br
                0x0C => {
                    let depth = reader.read_u32_leb128()?;
                    self.do_branch(depth, code, &mut reader)?;
                }

                // br_if
                0x0D => {
                    let depth = reader.read_u32_leb128()?;
                    let cond = self.pop_i32()?;
                    if cond != 0 {
                        self.do_branch(depth, code, &mut reader)?;
                    }
                }

                // br_table
                0x0E => {
                    let count = reader.read_u32_leb128()? as usize;
                    let mut targets = Vec::with_capacity(count);
                    for _ in 0..count {
                        targets.push(reader.read_u32_leb128()?);
                    }
                    let default = reader.read_u32_leb128()?;
                    let idx = self.pop_i32()? as u32 as usize;
                    let depth = if idx < targets.len() {
                        targets[idx]
                    } else {
                        default
                    };
                    self.do_branch(depth, code, &mut reader)?;
                }

                // return
                0x0F => {
                    break;
                }

                // call
                0x10 => {
                    let func_idx = reader.read_u32_leb128()?;
                    self.do_call(instance, func_idx)?;
                }

                // call_indirect
                0x11 => {
                    let type_idx = reader.read_u32_leb128()?;
                    let table_idx = reader.read_u32_leb128()?;
                    let elem_idx = self.pop_i32()? as u32;

                    let table = instance.tables.get(table_idx as usize)
                        .ok_or_else(|| format!("table {} not found", table_idx))?;
                    let func_idx = table.get(elem_idx)?
                        .ok_or_else(|| String::from("null function reference in table"))?;

                    // Type check
                    let expected = instance.module.types.get(type_idx as usize)
                        .ok_or_else(|| format!("type {} not found", type_idx))?;
                    let actual = instance.module.func_type(func_idx)
                        .ok_or_else(|| format!("function {} type not found", func_idx))?;
                    if expected != actual {
                        return Err(String::from("indirect call type mismatch"));
                    }

                    self.do_call(instance, func_idx)?;
                }

                // drop
                0x1A => { self.pop()?; }

                // select
                0x1B => {
                    let cond = self.pop_i32()?;
                    let val2 = self.pop()?;
                    let val1 = self.pop()?;
                    self.push(if cond != 0 { val1 } else { val2 });
                }

                // local.get
                0x20 => {
                    let idx = reader.read_u32_leb128()? as usize;
                    let val = self.get_local(idx)?;
                    self.push(val);
                }

                // local.set
                0x21 => {
                    let idx = reader.read_u32_leb128()? as usize;
                    let val = self.pop()?;
                    self.set_local(idx, val)?;
                }

                // local.tee
                0x22 => {
                    let idx = reader.read_u32_leb128()? as usize;
                    let val = *self.value_stack.last()
                        .ok_or_else(|| String::from("stack underflow"))?;
                    self.set_local(idx, val)?;
                }

                // global.get
                0x23 => {
                    let idx = reader.read_u32_leb128()? as usize;
                    let val = instance.globals.get(idx)
                        .ok_or_else(|| format!("global {} not found", idx))?
                        .clone();
                    self.push(val);
                }

                // global.set
                0x24 => {
                    let idx = reader.read_u32_leb128()? as usize;
                    let val = self.pop()?;
                    let g = instance.globals.get_mut(idx)
                        .ok_or_else(|| format!("global {} not found", idx))?;
                    *g = val;
                }

                // i32.load
                0x28 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u32_le(addr)?;
                    self.push(Value::I32(val as i32));
                }

                // i64.load
                0x29 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u64_le(addr)?;
                    self.push(Value::I64(val as i64));
                }

                // f32.load
                0x2A => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_f32_le(addr)?;
                    self.push(Value::F32(val));
                }

                // f64.load
                0x2B => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_f64_le(addr)?;
                    self.push(Value::F64(val));
                }

                // i32.load8_s
                0x2C => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u8(addr)? as i8 as i32;
                    self.push(Value::I32(val));
                }

                // i32.load8_u
                0x2D => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u8(addr)? as u32;
                    self.push(Value::I32(val as i32));
                }

                // i32.load16_s
                0x2E => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u16_le(addr)? as i16 as i32;
                    self.push(Value::I32(val));
                }

                // i32.load16_u
                0x2F => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u16_le(addr)? as u32;
                    self.push(Value::I32(val as i32));
                }

                // i64.load8_s
                0x30 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u8(addr)? as i8 as i64;
                    self.push(Value::I64(val));
                }

                // i64.load8_u
                0x31 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u8(addr)? as u64;
                    self.push(Value::I64(val as i64));
                }

                // i64.load16_s
                0x32 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u16_le(addr)? as i16 as i64;
                    self.push(Value::I64(val));
                }

                // i64.load16_u
                0x33 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u16_le(addr)? as u64;
                    self.push(Value::I64(val as i64));
                }

                // i64.load32_s
                0x34 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u32_le(addr)? as i32 as i64;
                    self.push(Value::I64(val));
                }

                // i64.load32_u
                0x35 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory()?;
                    let val = mem.read_u32_le(addr)? as u64;
                    self.push(Value::I64(val as i64));
                }

                // i32.store
                0x36 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i32()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u32_le(addr, val as u32)?;
                }

                // i64.store
                0x37 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i64()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u64_le(addr, val as u64)?;
                }

                // f32.store
                0x38 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_f32()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_f32_le(addr, val)?;
                }

                // f64.store
                0x39 => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_f64()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_f64_le(addr, val)?;
                }

                // i32.store8
                0x3A => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i32()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u8(addr, val as u8)?;
                }

                // i32.store16
                0x3B => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i32()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u16_le(addr, val as u16)?;
                }

                // i64.store8
                0x3C => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i64()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u8(addr, val as u8)?;
                }

                // i64.store16
                0x3D => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i64()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u16_le(addr, val as u16)?;
                }

                // i64.store32
                0x3E => {
                    let _align = reader.read_u32_leb128()?;
                    let offset = reader.read_u32_leb128()?;
                    let val = self.pop_i64()?;
                    let base = self.pop_i32()? as u32;
                    let addr = base.wrapping_add(offset);
                    let mem = instance.memory_mut()?;
                    mem.write_u32_le(addr, val as u32)?;
                }

                // memory.size
                0x3F => {
                    let _mem_idx = reader.read_u32_leb128()?;
                    let mem = instance.memory()?;
                    self.push(Value::I32(mem.size_pages() as i32));
                }

                // memory.grow
                0x40 => {
                    let _mem_idx = reader.read_u32_leb128()?;
                    let delta = self.pop_i32()? as u32;
                    let mem = instance.memory_mut()?;
                    let result = mem.grow(delta);
                    self.push(Value::I32(result));
                }

                // i32.const
                0x41 => {
                    let val = reader.read_i32_leb128()?;
                    self.push(Value::I32(val));
                }

                // i64.const
                0x42 => {
                    let val = reader.read_i64_leb128()?;
                    self.push(Value::I64(val));
                }

                // f32.const
                0x43 => {
                    let val = reader.read_f32()?;
                    self.push(Value::F32(val));
                }

                // f64.const
                0x44 => {
                    let val = reader.read_f64()?;
                    self.push(Value::F64(val));
                }

                // i32.eqz
                0x45 => {
                    let v = self.pop_i32()?;
                    self.push(Value::I32(if v == 0 { 1 } else { 0 }));
                }

                // i32.eq
                0x46 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a == b { 1 } else { 0 })); }
                // i32.ne
                0x47 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a != b { 1 } else { 0 })); }
                // i32.lt_s
                0x48 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a < b { 1 } else { 0 })); }
                // i32.lt_u
                0x49 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if (a as u32) < (b as u32) { 1 } else { 0 })); }
                // i32.gt_s
                0x4A => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a > b { 1 } else { 0 })); }
                // i32.gt_u
                0x4B => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if (a as u32) > (b as u32) { 1 } else { 0 })); }
                // i32.le_s
                0x4C => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a <= b { 1 } else { 0 })); }
                // i32.le_u
                0x4D => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if (a as u32) <= (b as u32) { 1 } else { 0 })); }
                // i32.ge_s
                0x4E => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if a >= b { 1 } else { 0 })); }
                // i32.ge_u
                0x4F => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(if (a as u32) >= (b as u32) { 1 } else { 0 })); }

                // i64.eqz
                0x50 => { let v = self.pop_i64()?; self.push(Value::I32(if v == 0 { 1 } else { 0 })); }
                // i64.eq
                0x51 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a == b { 1 } else { 0 })); }
                // i64.ne
                0x52 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a != b { 1 } else { 0 })); }
                // i64.lt_s
                0x53 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a < b { 1 } else { 0 })); }
                // i64.lt_u
                0x54 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if (a as u64) < (b as u64) { 1 } else { 0 })); }
                // i64.gt_s
                0x55 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a > b { 1 } else { 0 })); }
                // i64.gt_u
                0x56 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if (a as u64) > (b as u64) { 1 } else { 0 })); }
                // i64.le_s
                0x57 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a <= b { 1 } else { 0 })); }
                // i64.le_u
                0x58 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if (a as u64) <= (b as u64) { 1 } else { 0 })); }
                // i64.ge_s
                0x59 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if a >= b { 1 } else { 0 })); }
                // i64.ge_u
                0x5A => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I32(if (a as u64) >= (b as u64) { 1 } else { 0 })); }

                // f32.eq
                0x5B => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a == b { 1 } else { 0 })); }
                // f32.ne
                0x5C => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a != b { 1 } else { 0 })); }
                // f32.lt
                0x5D => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a < b { 1 } else { 0 })); }
                // f32.gt
                0x5E => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a > b { 1 } else { 0 })); }
                // f32.le
                0x5F => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a <= b { 1 } else { 0 })); }
                // f32.ge
                0x60 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::I32(if a >= b { 1 } else { 0 })); }

                // f64.eq
                0x61 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a == b { 1 } else { 0 })); }
                // f64.ne
                0x62 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a != b { 1 } else { 0 })); }
                // f64.lt
                0x63 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a < b { 1 } else { 0 })); }
                // f64.gt
                0x64 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a > b { 1 } else { 0 })); }
                // f64.le
                0x65 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a <= b { 1 } else { 0 })); }
                // f64.ge
                0x66 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::I32(if a >= b { 1 } else { 0 })); }

                // i32 arithmetic
                // i32.clz
                0x67 => { let v = self.pop_i32()?; self.push(Value::I32((v as u32).leading_zeros() as i32)); }
                // i32.ctz
                0x68 => { let v = self.pop_i32()?; self.push(Value::I32((v as u32).trailing_zeros() as i32)); }
                // i32.popcnt
                0x69 => { let v = self.pop_i32()?; self.push(Value::I32((v as u32).count_ones() as i32)); }
                // i32.add
                0x6A => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a.wrapping_add(b))); }
                // i32.sub
                0x6B => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a.wrapping_sub(b))); }
                // i32.mul
                0x6C => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a.wrapping_mul(b))); }
                // i32.div_s
                0x6D => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    if a == i32::MIN && b == -1 { return Err(String::from("integer overflow")); }
                    self.push(Value::I32(a.wrapping_div(b)));
                }
                // i32.div_u
                0x6E => {
                    let b = self.pop_i32()? as u32;
                    let a = self.pop_i32()? as u32;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I32((a / b) as i32));
                }
                // i32.rem_s
                0x6F => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I32(if a == i32::MIN && b == -1 { 0 } else { a.wrapping_rem(b) }));
                }
                // i32.rem_u
                0x70 => {
                    let b = self.pop_i32()? as u32;
                    let a = self.pop_i32()? as u32;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I32((a % b) as i32));
                }
                // i32.and
                0x71 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a & b)); }
                // i32.or
                0x72 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a | b)); }
                // i32.xor
                0x73 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a ^ b)); }
                // i32.shl
                0x74 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a.wrapping_shl(b as u32))); }
                // i32.shr_s
                0x75 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(a.wrapping_shr(b as u32))); }
                // i32.shr_u
                0x76 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32(((a as u32).wrapping_shr(b as u32)) as i32)); }
                // i32.rotl
                0x77 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32((a as u32).rotate_left(b as u32) as i32)); }
                // i32.rotr
                0x78 => { let b = self.pop_i32()?; let a = self.pop_i32()?; self.push(Value::I32((a as u32).rotate_right(b as u32) as i32)); }

                // i64 arithmetic
                // i64.clz
                0x79 => { let v = self.pop_i64()?; self.push(Value::I64((v as u64).leading_zeros() as i64)); }
                // i64.ctz
                0x7A => { let v = self.pop_i64()?; self.push(Value::I64((v as u64).trailing_zeros() as i64)); }
                // i64.popcnt
                0x7B => { let v = self.pop_i64()?; self.push(Value::I64((v as u64).count_ones() as i64)); }
                // i64.add
                0x7C => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a.wrapping_add(b))); }
                // i64.sub
                0x7D => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a.wrapping_sub(b))); }
                // i64.mul
                0x7E => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a.wrapping_mul(b))); }
                // i64.div_s
                0x7F => {
                    let b = self.pop_i64()?;
                    let a = self.pop_i64()?;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    if a == i64::MIN && b == -1 { return Err(String::from("integer overflow")); }
                    self.push(Value::I64(a.wrapping_div(b)));
                }
                // i64.div_u
                0x80 => {
                    let b = self.pop_i64()? as u64;
                    let a = self.pop_i64()? as u64;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I64((a / b) as i64));
                }
                // i64.rem_s
                0x81 => {
                    let b = self.pop_i64()?;
                    let a = self.pop_i64()?;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I64(if a == i64::MIN && b == -1 { 0 } else { a.wrapping_rem(b) }));
                }
                // i64.rem_u
                0x82 => {
                    let b = self.pop_i64()? as u64;
                    let a = self.pop_i64()? as u64;
                    if b == 0 { return Err(String::from("integer divide by zero")); }
                    self.push(Value::I64((a % b) as i64));
                }
                // i64.and
                0x83 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a & b)); }
                // i64.or
                0x84 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a | b)); }
                // i64.xor
                0x85 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a ^ b)); }
                // i64.shl
                0x86 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a.wrapping_shl(b as u32))); }
                // i64.shr_s
                0x87 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(a.wrapping_shr(b as u32))); }
                // i64.shr_u
                0x88 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64(((a as u64).wrapping_shr(b as u32)) as i64)); }
                // i64.rotl
                0x89 => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64((a as u64).rotate_left(b as u32) as i64)); }
                // i64.rotr
                0x8A => { let b = self.pop_i64()?; let a = self.pop_i64()?; self.push(Value::I64((a as u64).rotate_right(b as u32) as i64)); }

                // f32 arithmetic
                // f32.abs
                0x8B => { let v = self.pop_f32()?; self.push(Value::F32(f32_abs(v))); }
                // f32.neg
                0x8C => { let v = self.pop_f32()?; self.push(Value::F32(-v)); }
                // f32.ceil
                0x8D => { let v = self.pop_f32()?; self.push(Value::F32(f32_ceil(v))); }
                // f32.floor
                0x8E => { let v = self.pop_f32()?; self.push(Value::F32(f32_floor(v))); }
                // f32.trunc
                0x8F => { let v = self.pop_f32()?; self.push(Value::F32(f32_trunc(v))); }
                // f32.nearest
                0x90 => { let v = self.pop_f32()?; self.push(Value::F32(f32_nearest(v))); }
                // f32.sqrt
                0x91 => { let v = self.pop_f32()?; self.push(Value::F32(f32_sqrt(v))); }
                // f32.add
                0x92 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(a + b)); }
                // f32.sub
                0x93 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(a - b)); }
                // f32.mul
                0x94 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(a * b)); }
                // f32.div
                0x95 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(a / b)); }
                // f32.min
                0x96 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(f32_min(a, b))); }
                // f32.max
                0x97 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(f32_max(a, b))); }
                // f32.copysign
                0x98 => { let b = self.pop_f32()?; let a = self.pop_f32()?; self.push(Value::F32(f32_copysign(a, b))); }

                // f64 arithmetic
                // f64.abs
                0x99 => { let v = self.pop_f64()?; self.push(Value::F64(f64_abs(v))); }
                // f64.neg
                0x9A => { let v = self.pop_f64()?; self.push(Value::F64(-v)); }
                // f64.ceil
                0x9B => { let v = self.pop_f64()?; self.push(Value::F64(f64_ceil(v))); }
                // f64.floor
                0x9C => { let v = self.pop_f64()?; self.push(Value::F64(f64_floor(v))); }
                // f64.trunc
                0x9D => { let v = self.pop_f64()?; self.push(Value::F64(f64_trunc(v))); }
                // f64.nearest
                0x9E => { let v = self.pop_f64()?; self.push(Value::F64(f64_nearest(v))); }
                // f64.sqrt
                0x9F => { let v = self.pop_f64()?; self.push(Value::F64(f64_sqrt(v))); }
                // f64.add
                0xA0 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(a + b)); }
                // f64.sub
                0xA1 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(a - b)); }
                // f64.mul
                0xA2 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(a * b)); }
                // f64.div
                0xA3 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(a / b)); }
                // f64.min
                0xA4 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(f64_min(a, b))); }
                // f64.max
                0xA5 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(f64_max(a, b))); }
                // f64.copysign
                0xA6 => { let b = self.pop_f64()?; let a = self.pop_f64()?; self.push(Value::F64(f64_copysign(a, b))); }

                // Conversions
                // i32.wrap_i64
                0xA7 => { let v = self.pop_i64()?; self.push(Value::I32(v as i32)); }
                // i32.trunc_f32_s
                0xA8 => {
                    let v = self.pop_f32()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    if v >= 2147483648.0 || v < -2147483648.0 { return Err(String::from("integer overflow")); }
                    self.push(Value::I32(v as i32));
                }
                // i32.trunc_f32_u
                0xA9 => {
                    let v = self.pop_f32()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    if v >= 4294967296.0 || v < 0.0 { return Err(String::from("integer overflow")); }
                    self.push(Value::I32(v as u32 as i32));
                }
                // i32.trunc_f64_s
                0xAA => {
                    let v = self.pop_f64()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    if v >= 2147483648.0 || v < -2147483649.0 { return Err(String::from("integer overflow")); }
                    self.push(Value::I32(v as i32));
                }
                // i32.trunc_f64_u
                0xAB => {
                    let v = self.pop_f64()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    if v >= 4294967296.0 || v < -1.0 { return Err(String::from("integer overflow")); }
                    self.push(Value::I32(v as u32 as i32));
                }
                // i64.extend_i32_s
                0xAC => { let v = self.pop_i32()?; self.push(Value::I64(v as i64)); }
                // i64.extend_i32_u
                0xAD => { let v = self.pop_i32()?; self.push(Value::I64(v as u32 as i64)); }
                // i64.trunc_f32_s
                0xAE => {
                    let v = self.pop_f32()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    self.push(Value::I64(v as i64));
                }
                // i64.trunc_f32_u
                0xAF => {
                    let v = self.pop_f32()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    self.push(Value::I64(v as u64 as i64));
                }
                // i64.trunc_f64_s
                0xB0 => {
                    let v = self.pop_f64()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    self.push(Value::I64(v as i64));
                }
                // i64.trunc_f64_u
                0xB1 => {
                    let v = self.pop_f64()?;
                    if v.is_nan() { return Err(String::from("invalid conversion to integer")); }
                    self.push(Value::I64(v as u64 as i64));
                }
                // f32.convert_i32_s
                0xB2 => { let v = self.pop_i32()?; self.push(Value::F32(v as f32)); }
                // f32.convert_i32_u
                0xB3 => { let v = self.pop_i32()?; self.push(Value::F32(v as u32 as f32)); }
                // f32.convert_i64_s
                0xB4 => { let v = self.pop_i64()?; self.push(Value::F32(v as f32)); }
                // f32.convert_i64_u
                0xB5 => { let v = self.pop_i64()?; self.push(Value::F32(v as u64 as f32)); }
                // f32.demote_f64
                0xB6 => { let v = self.pop_f64()?; self.push(Value::F32(v as f32)); }
                // f64.convert_i32_s
                0xB7 => { let v = self.pop_i32()?; self.push(Value::F64(v as f64)); }
                // f64.convert_i32_u
                0xB8 => { let v = self.pop_i32()?; self.push(Value::F64(v as u32 as f64)); }
                // f64.convert_i64_s
                0xB9 => { let v = self.pop_i64()?; self.push(Value::F64(v as f64)); }
                // f64.convert_i64_u
                0xBA => { let v = self.pop_i64()?; self.push(Value::F64(v as u64 as f64)); }
                // f64.promote_f32
                0xBB => { let v = self.pop_f32()?; self.push(Value::F64(v as f64)); }
                // i32.reinterpret_f32
                0xBC => { let v = self.pop_f32()?; self.push(Value::I32(v.to_bits() as i32)); }
                // i64.reinterpret_f64
                0xBD => { let v = self.pop_f64()?; self.push(Value::I64(v.to_bits() as i64)); }
                // f32.reinterpret_i32
                0xBE => { let v = self.pop_i32()?; self.push(Value::F32(f32::from_bits(v as u32))); }
                // f64.reinterpret_i64
                0xBF => { let v = self.pop_i64()?; self.push(Value::F64(f64::from_bits(v as u64))); }

                // i32.extend8_s
                0xC0 => { let v = self.pop_i32()?; self.push(Value::I32(v as i8 as i32)); }
                // i32.extend16_s
                0xC1 => { let v = self.pop_i32()?; self.push(Value::I32(v as i16 as i32)); }
                // i64.extend8_s
                0xC2 => { let v = self.pop_i64()?; self.push(Value::I64(v as i8 as i64)); }
                // i64.extend16_s
                0xC3 => { let v = self.pop_i64()?; self.push(Value::I64(v as i16 as i64)); }
                // i64.extend32_s
                0xC4 => { let v = self.pop_i64()?; self.push(Value::I64(v as i32 as i64)); }

                _ => {
                    return Err(format!("unimplemented opcode 0x{:02X}", opcode));
                }
            }
        }

        // Collect results
        let mut results = Vec::new();
        for _ in 0..result_arity {
            results.push(self.pop()?);
        }
        results.reverse();
        Ok(results)
    }

    fn read_block_type(&self, reader: &mut BinaryReader) -> Result<BlockType, String> {
        let byte = reader.read_byte()?;
        if byte == 0x40 {
            return Ok(BlockType::Empty);
        }
        if let Ok(vt) = ValType::from_byte(byte) {
            return Ok(BlockType::Value(vt));
        }
        // Treat as signed LEB128 type index (put byte back by decoding)
        Ok(BlockType::TypeIndex(byte as u32))
    }

    fn block_type_arity(&self, bt: &BlockType, instance: &WasmInstance) -> usize {
        match bt {
            BlockType::Empty => 0,
            BlockType::Value(_) => 1,
            BlockType::TypeIndex(idx) => {
                instance.module.types.get(*idx as usize)
                    .map(|ft| ft.results.len())
                    .unwrap_or(0)
            }
        }
    }

    /// Find the matching `end` opcode for a block/loop/if, handling nesting.
    fn find_end(&self, code: &[u8], start: usize) -> Result<usize, String> {
        let mut depth = 1usize;
        let mut pos = start;
        while pos < code.len() {
            let op = code[pos];
            pos += 1;
            match op {
                0x02 | 0x03 | 0x04 => {
                    // block, loop, if — skip block type byte
                    depth += 1;
                    if pos < code.len() {
                        let bt = code[pos];
                        if bt == 0x40 || ValType::from_byte(bt).is_ok() {
                            pos += 1;
                        } else {
                            // LEB128 type index, skip
                            while pos < code.len() && code[pos] & 0x80 != 0 {
                                pos += 1;
                            }
                            if pos < code.len() { pos += 1; }
                        }
                    }
                }
                0x0B => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(pos);
                    }
                }
                // Skip immediate operands for opcodes that have them
                0x0C | 0x0D => { pos = skip_leb128(code, pos); }
                0x0E => {
                    // br_table
                    let (count, new_pos) = read_leb128_u32(code, pos);
                    pos = new_pos;
                    for _ in 0..count + 1 {
                        pos = skip_leb128(code, pos);
                    }
                }
                0x10 => { pos = skip_leb128(code, pos); } // call
                0x11 => { pos = skip_leb128(code, pos); pos = skip_leb128(code, pos); } // call_indirect
                0x20..=0x24 => { pos = skip_leb128(code, pos); } // local/global ops
                0x28..=0x3E => { pos = skip_leb128(code, pos); pos = skip_leb128(code, pos); } // memory ops
                0x3F | 0x40 => { pos = skip_leb128(code, pos); } // memory.size/grow
                0x41 => { pos = skip_leb128(code, pos); } // i32.const
                0x42 => { pos = skip_leb128_64(code, pos); } // i64.const
                0x43 => { pos += 4; } // f32.const
                0x44 => { pos += 8; } // f64.const
                0x05 => {} // else
                _ => {} // no immediates
            }
        }
        Err(String::from("unmatched block: no end found"))
    }

    /// Find the `else` opcode for an `if` block (at the same nesting level).
    fn find_else(&self, code: &[u8], start: usize) -> Result<Option<usize>, String> {
        let mut depth = 1usize;
        let mut pos = start;
        while pos < code.len() {
            let op = code[pos];
            pos += 1;
            match op {
                0x02 | 0x03 | 0x04 => {
                    depth += 1;
                    if pos < code.len() {
                        let bt = code[pos];
                        if bt == 0x40 || ValType::from_byte(bt).is_ok() {
                            pos += 1;
                        } else {
                            while pos < code.len() && code[pos] & 0x80 != 0 {
                                pos += 1;
                            }
                            if pos < code.len() { pos += 1; }
                        }
                    }
                }
                0x05 if depth == 1 => {
                    return Ok(Some(pos));
                }
                0x0B => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(None);
                    }
                }
                0x0C | 0x0D => { pos = skip_leb128(code, pos); }
                0x0E => {
                    let (count, new_pos) = read_leb128_u32(code, pos);
                    pos = new_pos;
                    for _ in 0..count + 1 {
                        pos = skip_leb128(code, pos);
                    }
                }
                0x10 => { pos = skip_leb128(code, pos); }
                0x11 => { pos = skip_leb128(code, pos); pos = skip_leb128(code, pos); }
                0x20..=0x24 => { pos = skip_leb128(code, pos); }
                0x28..=0x3E => { pos = skip_leb128(code, pos); pos = skip_leb128(code, pos); }
                0x3F | 0x40 => { pos = skip_leb128(code, pos); }
                0x41 => { pos = skip_leb128(code, pos); }
                0x42 => { pos = skip_leb128_64(code, pos); }
                0x43 => { pos += 4; }
                0x44 => { pos += 8; }
                _ => {}
            }
        }
        Err(String::from("unmatched if: no end found"))
    }

    fn do_branch<'a>(&mut self, depth: u32, code: &'a [u8], reader: &mut BinaryReader<'a>) -> Result<(), String> {
        let frame_label_base = self.call_stack.last()
            .map(|f| f.label_base)
            .unwrap_or(0);

        let label_idx = self.label_stack.len().checked_sub(1)
            .and_then(|top| top.checked_sub(depth as usize))
            .ok_or_else(|| String::from("branch depth exceeds label stack"))?;

        if label_idx < frame_label_base {
            return Err(String::from("branch depth exceeds frame"));
        }

        let label = self.label_stack[label_idx].clone();

        // Save branch results
        let mut results = Vec::new();
        for _ in 0..label.arity {
            results.push(self.pop()?);
        }
        results.reverse();

        // Unwind value stack to label height
        self.value_stack.truncate(label.stack_height);

        // Push results back
        for v in results {
            self.push(v);
        }

        // Pop labels down to the target
        self.label_stack.truncate(label_idx + if label.is_loop { 1 } else { 0 });

        // Jump to continuation
        *reader = BinaryReader::new(code);
        reader.read_bytes(label.continuation)?;

        Ok(())
    }

    fn do_call(&mut self, instance: &mut WasmInstance, func_idx: u32) -> Result<(), String> {
        let func_type = instance.module.func_type(func_idx)
            .ok_or_else(|| format!("function {} not found", func_idx))?
            .clone();

        // Pop arguments from value stack
        let mut args = Vec::with_capacity(func_type.params.len());
        for _ in 0..func_type.params.len() {
            args.push(self.pop()?);
        }
        args.reverse();

        let results = self.invoke(instance, func_idx, &args)?;

        for v in results {
            self.push(v);
        }

        Ok(())
    }

    fn get_local(&self, idx: usize) -> Result<Value, String> {
        let frame = self.call_stack.last()
            .ok_or_else(|| String::from("no call frame"))?;
        frame.locals.get(idx)
            .copied()
            .ok_or_else(|| format!("local {} out of bounds", idx))
    }

    fn set_local(&mut self, idx: usize, val: Value) -> Result<(), String> {
        let frame = self.call_stack.last_mut()
            .ok_or_else(|| String::from("no call frame"))?;
        let local = frame.locals.get_mut(idx)
            .ok_or_else(|| format!("local {} out of bounds", idx))?;
        *local = val;
        Ok(())
    }
}

/// Evaluate a constant expression.
pub fn eval_const_expr(expr: &ConstExpr, globals: &[Value]) -> Result<Value, String> {
    match expr {
        ConstExpr::Value(v) => Ok(*v),
        ConstExpr::GlobalGet(idx) => {
            globals.get(*idx as usize)
                .copied()
                .ok_or_else(|| format!("global {} not found in const expr", idx))
        }
    }
}

// Helper: skip a LEB128 encoded value in the bytecode
fn skip_leb128(code: &[u8], mut pos: usize) -> usize {
    while pos < code.len() {
        let b = code[pos];
        pos += 1;
        if b & 0x80 == 0 {
            break;
        }
    }
    pos
}

fn skip_leb128_64(code: &[u8], pos: usize) -> usize {
    skip_leb128(code, pos)
}

fn read_leb128_u32(code: &[u8], mut pos: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0;
    loop {
        if pos >= code.len() { break; }
        let byte = code[pos];
        pos += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 { break; }
        shift += 7;
    }
    (result, pos)
}

// no_std float helpers using bit manipulation
fn f32_abs(v: f32) -> f32 {
    f32::from_bits(v.to_bits() & 0x7FFF_FFFF)
}

fn f64_abs(v: f64) -> f64 {
    f64::from_bits(v.to_bits() & 0x7FFF_FFFF_FFFF_FFFF)
}

fn f32_copysign(a: f32, b: f32) -> f32 {
    f32::from_bits((a.to_bits() & 0x7FFF_FFFF) | (b.to_bits() & 0x8000_0000))
}

fn f64_copysign(a: f64, b: f64) -> f64 {
    f64::from_bits((a.to_bits() & 0x7FFF_FFFF_FFFF_FFFF) | (b.to_bits() & 0x8000_0000_0000_0000))
}

fn f32_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() { return f32::NAN; }
    if a < b { a } else { b }
}

fn f32_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() { return f32::NAN; }
    if a > b { a } else { b }
}

fn f64_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { return f64::NAN; }
    if a < b { a } else { b }
}

fn f64_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { return f64::NAN; }
    if a > b { a } else { b }
}

// Software float math for no_std (integer-based approximations)
fn f32_ceil(v: f32) -> f32 {
    if v.is_nan() || v.is_infinite() { return v; }
    let i = v as i32;
    if v > 0.0 && (i as f32) < v { (i + 1) as f32 } else { i as f32 }
}

fn f32_floor(v: f32) -> f32 {
    if v.is_nan() || v.is_infinite() { return v; }
    let i = v as i32;
    if v < 0.0 && (i as f32) > v { (i - 1) as f32 } else { i as f32 }
}

fn f32_trunc(v: f32) -> f32 {
    if v.is_nan() || v.is_infinite() { return v; }
    (v as i32) as f32
}

fn f32_nearest(v: f32) -> f32 {
    if v.is_nan() || v.is_infinite() { return v; }
    let rounded = (v + 0.5) as i32;
    // Banker's rounding for ties
    if (v - (rounded as f32)).abs() == 0.5 && rounded % 2 != 0 {
        (rounded - 1) as f32
    } else {
        rounded as f32
    }
}

fn f32_sqrt(v: f32) -> f32 {
    if v < 0.0 { return f32::NAN; }
    if v == 0.0 || v.is_nan() || v.is_infinite() { return v; }
    // Newton's method
    let mut x = v;
    for _ in 0..20 {
        x = (x + v / x) * 0.5;
    }
    x
}

fn f64_ceil(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { return v; }
    let i = v as i64;
    if v > 0.0 && (i as f64) < v { (i + 1) as f64 } else { i as f64 }
}

fn f64_floor(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { return v; }
    let i = v as i64;
    if v < 0.0 && (i as f64) > v { (i - 1) as f64 } else { i as f64 }
}

fn f64_trunc(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { return v; }
    (v as i64) as f64
}

fn f64_nearest(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { return v; }
    let rounded = (v + 0.5) as i64;
    if (v - (rounded as f64)).abs() == 0.5 && rounded % 2 != 0 {
        (rounded - 1) as f64
    } else {
        rounded as f64
    }
}

fn f64_sqrt(v: f64) -> f64 {
    if v < 0.0 { return f64::NAN; }
    if v == 0.0 || v.is_nan() || v.is_infinite() { return v; }
    let mut x = v;
    for _ in 0..30 {
        x = (x + v / x) * 0.5;
    }
    x
}
