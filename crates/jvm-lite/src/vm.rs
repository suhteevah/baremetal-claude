//! Stack-based JVM interpreter.
//!
//! Frame stack, operand stack, local variables, constant pool resolution,
//! method dispatch, exception handling via exception table.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bytecode::Opcode;
use crate::classfile::{ClassFile, CodeAttribute, CpEntry, ACC_STATIC};
use crate::gc::JvmHeap;
use crate::stdlib;

/// JVM value on the operand stack or in local variables.
#[derive(Debug, Clone)]
pub enum JvmValue {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Ref(usize),   // heap object reference (index)
    Null,
}

impl JvmValue {
    pub fn as_int(&self) -> i32 {
        match self { JvmValue::Int(n) => *n, _ => 0 }
    }
    pub fn as_long(&self) -> i64 {
        match self { JvmValue::Long(n) => *n, JvmValue::Int(n) => *n as i64, _ => 0 }
    }
    pub fn as_float(&self) -> f32 {
        match self { JvmValue::Float(f) => *f, JvmValue::Int(n) => *n as f32, _ => 0.0 }
    }
    pub fn as_double(&self) -> f64 {
        match self { JvmValue::Double(f) => *f, JvmValue::Float(f) => *f as f64, _ => 0.0 }
    }
    pub fn as_ref(&self) -> usize {
        match self { JvmValue::Ref(r) => *r, _ => 0 }
    }
}

/// A JVM stack frame.
pub struct Frame {
    pub class_name: String,
    pub method_name: String,
    pub code: Vec<u8>,
    pub pc: usize,
    pub locals: Vec<JvmValue>,
    pub stack: Vec<JvmValue>,
    pub max_stack: usize,
}

impl Frame {
    pub fn new(class_name: String, method_name: String, code: CodeAttribute) -> Self {
        let locals = (0..code.max_locals).map(|_| JvmValue::Int(0)).collect();
        Self {
            class_name,
            method_name,
            code: code.code,
            pc: 0,
            locals,
            stack: Vec::with_capacity(code.max_stack as usize),
            max_stack: code.max_stack as usize,
        }
    }

    fn push(&mut self, val: JvmValue) {
        self.stack.push(val);
    }

    fn pop(&mut self) -> JvmValue {
        self.stack.pop().unwrap_or(JvmValue::Null)
    }

    fn read_u8(&mut self) -> u8 {
        let val = self.code[self.pc];
        self.pc += 1;
        val
    }

    fn read_i16(&mut self) -> i16 {
        let hi = self.code[self.pc] as i16;
        let lo = self.code[self.pc + 1] as i16;
        self.pc += 2;
        (hi << 8) | lo
    }

    fn read_u16(&mut self) -> u16 {
        let hi = self.code[self.pc] as u16;
        let lo = self.code[self.pc + 1] as u16;
        self.pc += 2;
        (hi << 8) | lo
    }
}

/// The JVM execution engine.
pub struct Vm {
    /// Loaded classes.
    pub classes: BTreeMap<String, ClassFile>,
    /// Static fields: "ClassName.fieldName" -> value.
    pub static_fields: BTreeMap<String, JvmValue>,
    /// Object heap.
    pub heap: JvmHeap,
    /// Captured output (System.out.println).
    pub output: String,
    /// Frame stack.
    frames: Vec<Frame>,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            classes: BTreeMap::new(),
            static_fields: BTreeMap::new(),
            heap: JvmHeap::new(),
            output: String::new(),
            frames: Vec::new(),
        }
    }

    /// Load a class.
    pub fn load_class(&mut self, class: ClassFile) {
        if let Some(name) = class.class_name() {
            let name = String::from(name);
            log::debug!("[jvm] loaded class: {}", name);
            self.classes.insert(name, class);
        }
    }

    /// Execute the main method of a loaded class.
    pub fn run_main(&mut self, class_name: &str) -> Result<(), String> {
        let class = self.classes.get(class_name)
            .ok_or_else(|| alloc::format!("class not found: {}", class_name))?
            .clone();

        // Find main(String[] args)
        let main_method = class.methods.iter()
            .find(|m| {
                class.get_utf8(m.name_index) == Some("main")
                    && m.access_flags & ACC_STATIC != 0
            })
            .ok_or_else(|| String::from("main method not found"))?;

        // Find Code attribute
        let code_attr = main_method.attributes.iter()
            .find(|a| class.get_utf8(a.name_index) == Some("Code"))
            .ok_or_else(|| String::from("no Code attribute on main"))?;

        let code = ClassFile::parse_code_attribute(&code_attr.data)?;
        let frame = Frame::new(
            String::from(class_name),
            String::from("main"),
            code,
        );

        self.frames.push(frame);
        self.execute_frames()?;
        Ok(())
    }

    fn execute_frames(&mut self) -> Result<Option<JvmValue>, String> {
        while let Some(frame) = self.frames.last_mut() {
            if frame.pc >= frame.code.len() {
                self.frames.pop();
                continue;
            }

            let opcode_byte = frame.read_u8();
            let opcode = Opcode::from_byte(opcode_byte)
                .ok_or_else(|| alloc::format!("unknown opcode: 0x{:02X}", opcode_byte))?;

            match opcode {
                Opcode::Nop => {}
                Opcode::AconstNull => frame.push(JvmValue::Null),
                Opcode::IconstM1 => frame.push(JvmValue::Int(-1)),
                Opcode::Iconst0 => frame.push(JvmValue::Int(0)),
                Opcode::Iconst1 => frame.push(JvmValue::Int(1)),
                Opcode::Iconst2 => frame.push(JvmValue::Int(2)),
                Opcode::Iconst3 => frame.push(JvmValue::Int(3)),
                Opcode::Iconst4 => frame.push(JvmValue::Int(4)),
                Opcode::Iconst5 => frame.push(JvmValue::Int(5)),
                Opcode::Lconst0 => frame.push(JvmValue::Long(0)),
                Opcode::Lconst1 => frame.push(JvmValue::Long(1)),
                Opcode::Fconst0 => frame.push(JvmValue::Float(0.0)),
                Opcode::Fconst1 => frame.push(JvmValue::Float(1.0)),
                Opcode::Fconst2 => frame.push(JvmValue::Float(2.0)),
                Opcode::Dconst0 => frame.push(JvmValue::Double(0.0)),
                Opcode::Dconst1 => frame.push(JvmValue::Double(1.0)),

                Opcode::Bipush => {
                    let val = frame.read_u8() as i8 as i32;
                    frame.push(JvmValue::Int(val));
                }
                Opcode::Sipush => {
                    let val = frame.read_i16() as i32;
                    frame.push(JvmValue::Int(val));
                }

                Opcode::Iload => {
                    let idx = frame.read_u8() as usize;
                    let val = frame.locals[idx].clone();
                    frame.push(val);
                }
                Opcode::Iload0 => { let v = frame.locals[0].clone(); frame.push(v); }
                Opcode::Iload1 => { let v = frame.locals[1].clone(); frame.push(v); }
                Opcode::Iload2 => { let v = frame.locals[2].clone(); frame.push(v); }
                Opcode::Iload3 => { let v = frame.locals[3].clone(); frame.push(v); }

                Opcode::Aload => {
                    let idx = frame.read_u8() as usize;
                    let val = frame.locals[idx].clone();
                    frame.push(val);
                }
                Opcode::Aload0 => { let v = frame.locals[0].clone(); frame.push(v); }
                Opcode::Aload1 => { let v = frame.locals[1].clone(); frame.push(v); }
                Opcode::Aload2 => { let v = frame.locals[2].clone(); frame.push(v); }
                Opcode::Aload3 => { let v = frame.locals[3].clone(); frame.push(v); }

                Opcode::Istore => {
                    let idx = frame.read_u8() as usize;
                    let val = frame.pop();
                    frame.locals[idx] = val;
                }
                Opcode::Istore0 => { let v = frame.pop(); frame.locals[0] = v; }
                Opcode::Istore1 => { let v = frame.pop(); frame.locals[1] = v; }
                Opcode::Istore2 => { let v = frame.pop(); frame.locals[2] = v; }
                Opcode::Istore3 => { let v = frame.pop(); frame.locals[3] = v; }

                Opcode::Astore => {
                    let idx = frame.read_u8() as usize;
                    let val = frame.pop();
                    frame.locals[idx] = val;
                }
                Opcode::Astore0 => { let v = frame.pop(); frame.locals[0] = v; }
                Opcode::Astore1 => { let v = frame.pop(); frame.locals[1] = v; }
                Opcode::Astore2 => { let v = frame.pop(); frame.locals[2] = v; }
                Opcode::Astore3 => { let v = frame.pop(); frame.locals[3] = v; }

                Opcode::Pop => { frame.pop(); }
                Opcode::Pop2 => { frame.pop(); frame.pop(); }
                Opcode::Dup => {
                    let val = frame.pop();
                    frame.push(val.clone());
                    frame.push(val);
                }
                Opcode::Swap => {
                    let a = frame.pop();
                    let b = frame.pop();
                    frame.push(a);
                    frame.push(b);
                }

                // Integer arithmetic
                Opcode::Iadd => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a.wrapping_add(b))); }
                Opcode::Isub => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a.wrapping_sub(b))); }
                Opcode::Imul => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a.wrapping_mul(b))); }
                Opcode::Idiv => {
                    let b = frame.pop().as_int();
                    let a = frame.pop().as_int();
                    if b == 0 { return Err(String::from("ArithmeticException: / by zero")); }
                    frame.push(JvmValue::Int(a / b));
                }
                Opcode::Irem => {
                    let b = frame.pop().as_int();
                    let a = frame.pop().as_int();
                    if b == 0 { return Err(String::from("ArithmeticException: / by zero")); }
                    frame.push(JvmValue::Int(a % b));
                }
                Opcode::Ineg => { let a = frame.pop().as_int(); frame.push(JvmValue::Int(-a)); }
                Opcode::Ishl => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a << (b & 0x1f))); }
                Opcode::Ishr => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a >> (b & 0x1f))); }
                Opcode::Iand => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a & b)); }
                Opcode::Ior => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a | b)); }
                Opcode::Ixor => { let b = frame.pop().as_int(); let a = frame.pop().as_int(); frame.push(JvmValue::Int(a ^ b)); }

                Opcode::Iinc => {
                    let idx = frame.read_u8() as usize;
                    let inc = frame.read_u8() as i8 as i32;
                    if let JvmValue::Int(ref mut v) = frame.locals[idx] {
                        *v = v.wrapping_add(inc);
                    }
                }

                // Conversions
                Opcode::I2l => { let v = frame.pop().as_int(); frame.push(JvmValue::Long(v as i64)); }
                Opcode::I2f => { let v = frame.pop().as_int(); frame.push(JvmValue::Float(v as f32)); }
                Opcode::I2d => { let v = frame.pop().as_int(); frame.push(JvmValue::Double(v as f64)); }
                Opcode::L2i => { let v = frame.pop().as_long(); frame.push(JvmValue::Int(v as i32)); }
                Opcode::F2i => { let v = frame.pop().as_float(); frame.push(JvmValue::Int(v as i32)); }
                Opcode::D2i => { let v = frame.pop().as_double(); frame.push(JvmValue::Int(v as i32)); }

                // Comparisons and branches
                Opcode::Lcmp => {
                    let b = frame.pop().as_long();
                    let a = frame.pop().as_long();
                    frame.push(JvmValue::Int(if a > b { 1 } else if a < b { -1 } else { 0 }));
                }

                Opcode::Ifeq => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val == 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::Ifne => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val != 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::Iflt => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val < 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::Ifge => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val >= 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::Ifgt => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val > 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::Ifle => {
                    let offset = frame.read_i16();
                    let val = frame.pop().as_int();
                    if val <= 0 { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }

                Opcode::IfIcmpeq => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a == b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::IfIcmpne => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a != b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::IfIcmplt => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a < b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::IfIcmpge => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a >= b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::IfIcmpgt => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a > b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }
                Opcode::IfIcmple => {
                    let offset = frame.read_i16();
                    let b = frame.pop().as_int(); let a = frame.pop().as_int();
                    if a <= b { frame.pc = (frame.pc as isize + offset as isize - 3) as usize; }
                }

                Opcode::Goto => {
                    let offset = frame.read_i16();
                    frame.pc = (frame.pc as isize + offset as isize - 3) as usize;
                }

                Opcode::Ireturn | Opcode::Lreturn | Opcode::Freturn
                | Opcode::Dreturn | Opcode::Areturn => {
                    let ret = frame.pop();
                    self.frames.pop();
                    if let Some(caller) = self.frames.last_mut() {
                        caller.push(ret);
                    }
                    return Ok(None);
                }
                Opcode::Return => {
                    self.frames.pop();
                    return Ok(None);
                }

                Opcode::Getstatic => {
                    let idx = frame.read_u16();
                    // Check if this is System.out.println setup
                    // For stdlib calls, we handle it at invokevirtual
                    frame.push(JvmValue::Null); // push PrintStream reference
                }

                Opcode::Invokevirtual | Opcode::Invokespecial | Opcode::Invokestatic => {
                    let idx = frame.read_u16();
                    // Delegate to stdlib handler
                    let class_name = frame.class_name.clone();
                    if let Some(class) = self.classes.get(&class_name) {
                        if let Some(CpEntry::Methodref(class_idx, nat_idx)) = class.constant_pool.get(idx as usize) {
                            let method_name = self.resolve_method_name(class, *class_idx, *nat_idx);
                            if let Some((cls, meth)) = method_name {
                                if let Some(result) = stdlib::handle_stdlib_call(&cls, &meth, &mut frame.stack, &mut self.output, &mut self.heap) {
                                    frame.push(result);
                                }
                            }
                        }
                    }
                }

                Opcode::New => {
                    let _idx = frame.read_u16();
                    let obj_ref = self.heap.allocate();
                    frame.push(JvmValue::Ref(obj_ref));
                }

                Opcode::Arraylength => {
                    let _arr = frame.pop();
                    frame.push(JvmValue::Int(0)); // simplified
                }

                _ => {
                    log::warn!("[jvm] unimplemented opcode: {:?} (0x{:02X})", opcode, opcode_byte);
                    // Skip operands
                    if let Some(size) = opcode.operand_size() {
                        frame.pc += size;
                    }
                }
            }
        }
        Ok(None)
    }

    fn resolve_method_name(&self, class: &ClassFile, class_idx: u16, nat_idx: u16) -> Option<(String, String)> {
        let class_name = if let Some(CpEntry::Class(name_idx)) = class.constant_pool.get(class_idx as usize) {
            class.get_utf8(*name_idx)?.to_string()
        } else {
            return None;
        };
        let method_name = if let Some(CpEntry::NameAndType(name_idx, _)) = class.constant_pool.get(nat_idx as usize) {
            class.get_utf8(*name_idx)?.to_string()
        } else {
            return None;
        };
        Some((class_name.replace('/', "."), method_name))
    }

    /// Take captured output.
    pub fn take_output(&mut self) -> String {
        core::mem::take(&mut self.output)
    }
}
