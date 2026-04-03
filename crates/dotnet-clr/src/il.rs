//! CIL (Common Intermediate Language) opcode definitions and method body parser.
//!
//! Implements the bytecode layer of the .NET CLR as defined in ECMA-335 Part III.
//! CIL is a stack-based bytecode similar to JVM bytecodes but with key differences:
//!
//! ## Opcode Encoding
//!
//! Most CIL opcodes are single-byte (0x00..0xFE). The byte 0xFE is a prefix
//! indicating a two-byte opcode (0xFE followed by a second byte). This module
//! represents all opcodes as `u16` values: single-byte opcodes use their byte
//! value directly, and two-byte opcodes use `0xFE00 | second_byte`.
//!
//! ## Opcode Categories
//!
//! | Category | Examples | Description |
//! |----------|---------|-------------|
//! | Constants | `ldc.i4.0`..`ldc.i4.8`, `ldc.i4.s`, `ldc.i8`, `ldc.r4/r8` | Push constants |
//! | Locals | `ldloc.0`..`ldloc.3`, `stloc.0`..`stloc.3`, `ldloc.s` | Load/store locals |
//! | Arguments | `ldarg.0`..`ldarg.3`, `starg.s` | Load/store method args |
//! | Arithmetic | `add`, `sub`, `mul`, `div`, `rem`, `neg`, `not` | Stack arithmetic |
//! | Bitwise | `and`, `or`, `xor`, `shl`, `shr` | Bitwise operations |
//! | Comparison | `ceq` (0xFE01), `cgt` (0xFE02), `clt` (0xFE04) | Two-byte comparison |
//! | Branching | `br.s`, `brfalse.s`, `brtrue.s`, `beq.s`..`blt.un.s` | Short (i8) branches |
//! | Branching | `br`, `brfalse`, `brtrue`, `beq`..`blt.un` | Long (i32) branches |
//! | Objects | `newobj`, `callvirt`, `ldfld`, `stfld`, `box`, `unbox.any` | Object model |
//! | Arrays | `newarr`, `ldlen`, `ldelem.*`, `stelem.*` | Array operations |
//! | Conversion | `conv.i1`..`conv.u8`, `conv.r4`, `conv.r8` | Type conversions |
//! | Control | `call`, `ret`, `throw`, `leave`, `endfinally` | Flow control |
//!
//! ## Method Body Format (ECMA-335 II.25.4)
//!
//! Method bodies are either **tiny** (1-byte header, max 64 bytes of code, no locals,
//! no exception handling) or **fat** (12-byte header with max_stack, local var sig,
//! code size, and optional exception handling sections).

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::ClrValue;

/// CIL opcode definitions (ECMA-335 III.1).
///
/// Single-byte opcodes (0x00..0xFE) and two-byte opcodes (0xFE prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CilOpcode {
    // -- Basic operations --
    Nop = 0x00,
    Break = 0x01,

    // -- Load argument --
    Ldarg0 = 0x02,
    Ldarg1 = 0x03,
    Ldarg2 = 0x04,
    Ldarg3 = 0x05,

    // -- Load local --
    Ldloc0 = 0x06,
    Ldloc1 = 0x07,
    Ldloc2 = 0x08,
    Ldloc3 = 0x09,

    // -- Store local --
    Stloc0 = 0x0A,
    Stloc1 = 0x0B,
    Stloc2 = 0x0C,
    Stloc3 = 0x0D,

    // -- Load/store argument (short) --
    LdargS = 0x0E,
    LdargaS = 0x0F,
    StargS = 0x10,

    // -- Load/store local (short) --
    LdlocS = 0x11,
    LdlocaS = 0x12,
    StlocS = 0x13,

    // -- Constants --
    Ldnull = 0x14,
    LdcI4M1 = 0x15,
    LdcI4_0 = 0x16,
    LdcI4_1 = 0x17,
    LdcI4_2 = 0x18,
    LdcI4_3 = 0x19,
    LdcI4_4 = 0x1A,
    LdcI4_5 = 0x1B,
    LdcI4_6 = 0x1C,
    LdcI4_7 = 0x1D,
    LdcI4_8 = 0x1E,
    LdcI4S = 0x1F,
    LdcI4 = 0x20,
    LdcI8 = 0x21,
    LdcR4 = 0x22,
    LdcR8 = 0x23,

    // -- Duplicate / Pop --
    Dup = 0x25,
    Pop = 0x26,

    // -- Jump / Call / Return --
    Jmp = 0x27,
    Call = 0x28,
    Calli = 0x29,
    Ret = 0x2A,

    // -- Branch --
    BrS = 0x2B,
    BrfalseS = 0x2C,
    BrtrueS = 0x2D,
    BeqS = 0x2E,
    BgeS = 0x2F,
    BgtS = 0x30,
    BleS = 0x31,
    BltS = 0x32,
    BneUnS = 0x33,
    BgeUnS = 0x34,
    BgtUnS = 0x35,
    BleUnS = 0x36,
    BltUnS = 0x37,
    Br = 0x38,
    Brfalse = 0x39,
    Brtrue = 0x3A,
    Beq = 0x3B,
    Bge = 0x3C,
    Bgt = 0x3D,
    Ble = 0x3E,
    Blt = 0x3F,
    BneUn = 0x40,
    BgeUn = 0x41,
    BgtUn = 0x42,
    BleUn = 0x43,
    BltUn = 0x44,

    // -- Switch --
    Switch = 0x45,

    // -- Indirect load --
    LdindI1 = 0x46,
    LdindU1 = 0x47,
    LdindI2 = 0x48,
    LdindU2 = 0x49,
    LdindI4 = 0x4A,
    LdindU4 = 0x4B,
    LdindI8 = 0x4C,
    LdindI = 0x4D,
    LdindR4 = 0x4E,
    LdindR8 = 0x4F,
    LdindRef = 0x50,

    // -- Indirect store --
    StindRef = 0x51,
    StindI1 = 0x52,
    StindI2 = 0x53,
    StindI4 = 0x54,
    StindI8 = 0x55,
    StindR4 = 0x56,
    StindR8 = 0x57,

    // -- Arithmetic --
    Add = 0x58,
    Sub = 0x59,
    Mul = 0x5A,
    Div = 0x5B,
    DivUn = 0x5C,
    Rem = 0x5D,
    RemUn = 0x5E,
    And = 0x5F,
    Or = 0x60,
    Xor = 0x61,
    Shl = 0x62,
    Shr = 0x63,
    ShrUn = 0x64,
    Neg = 0x65,
    Not = 0x66,

    // -- Conversion --
    ConvI1 = 0x67,
    ConvI2 = 0x68,
    ConvI4 = 0x69,
    ConvI8 = 0x6A,
    ConvR4 = 0x6B,
    ConvR8 = 0x6C,
    ConvU4 = 0x6D,
    ConvU8 = 0x6E,

    // -- Object model --
    Callvirt = 0x6F,
    Cpobj = 0x70,
    Ldobj = 0x71,
    Ldstr = 0x72,
    Newobj = 0x73,
    Castclass = 0x74,
    Isinst = 0x75,
    ConvRUn = 0x76,

    Unbox = 0x79,
    Throw = 0x7A,
    Ldfld = 0x7B,
    Ldflda = 0x7C,
    Stfld = 0x7D,
    Ldsfld = 0x7E,
    Ldsflda = 0x7F,
    Stsfld = 0x80,
    Stobj = 0x81,

    ConvOvfI1Un = 0x82,
    ConvOvfI2Un = 0x83,
    ConvOvfI4Un = 0x84,
    ConvOvfI8Un = 0x85,
    ConvOvfU1Un = 0x86,
    ConvOvfU2Un = 0x87,
    ConvOvfU4Un = 0x88,
    ConvOvfU8Un = 0x89,
    ConvOvfIUn = 0x8A,
    ConvOvfUUn = 0x8B,

    Box = 0x8C,
    Newarr = 0x8D,
    Ldlen = 0x8E,
    Ldelema = 0x8F,
    LdelemI1 = 0x90,
    LdelemU1 = 0x91,
    LdelemI2 = 0x92,
    LdelemU2 = 0x93,
    LdelemI4 = 0x94,
    LdelemU4 = 0x95,
    LdelemI8 = 0x96,
    LdelemI = 0x97,
    LdelemR4 = 0x98,
    LdelemR8 = 0x99,
    LdelemRef = 0x9A,
    StelemI = 0x9B,
    StelemI1 = 0x9C,
    StelemI2 = 0x9D,
    StelemI4 = 0x9E,
    StelemI8 = 0x9F,
    StelemR4 = 0xA0,
    StelemR8 = 0xA1,
    StelemRef = 0xA2,
    Ldelem = 0xA3,
    Stelem = 0xA4,
    UnboxAny = 0xA5,

    ConvOvfI1 = 0xB3,
    ConvOvfU1 = 0xB4,
    ConvOvfI2 = 0xB5,
    ConvOvfU2 = 0xB6,
    ConvOvfI4 = 0xB7,
    ConvOvfU4 = 0xB8,
    ConvOvfI8 = 0xB9,
    ConvOvfU8 = 0xBA,

    ConvU2 = 0xD1,
    ConvU1 = 0xD2,
    ConvI = 0xD3,
    ConvOvfI = 0xD4,
    ConvOvfU = 0xD5,
    AddOvf = 0xD6,
    AddOvfUn = 0xD7,
    MulOvf = 0xD8,
    MulOvfUn = 0xD9,
    SubOvf = 0xDA,
    SubOvfUn = 0xDB,
    Endfinally = 0xDC,
    Leave = 0xDD,
    LeaveS = 0xDE,
    StindI = 0xDF,
    ConvU = 0xE0,

    // -- Prefix for two-byte opcodes --
    Prefix = 0xFE,

    // -- Two-byte opcodes (0xFE xx) --
    /// ceq (0xFE 0x01)
    Ceq = 0xFE01,
    /// cgt (0xFE 0x02)
    Cgt = 0xFE02,
    /// cgt.un (0xFE 0x03)
    CgtUn = 0xFE03,
    /// clt (0xFE 0x04)
    Clt = 0xFE04,
    /// clt.un (0xFE 0x05)
    CltUn = 0xFE05,
    /// ldftn (0xFE 0x06)
    Ldftn = 0xFE06,
    /// ldvirtftn (0xFE 0x07)
    Ldvirtftn = 0xFE07,
    /// ldarg (0xFE 0x09)
    Ldarg = 0xFE09,
    /// ldarga (0xFE 0x0A)
    Ldarga = 0xFE0A,
    /// starg (0xFE 0x0B)
    Starg = 0xFE0B,
    /// ldloc (0xFE 0x0C)
    Ldloc = 0xFE0C,
    /// ldloca (0xFE 0x0D)
    Ldloca = 0xFE0D,
    /// stloc (0xFE 0x0E)
    Stloc = 0xFE0E,
    /// localloc (0xFE 0x0F)
    Localloc = 0xFE0F,
    /// endfilter (0xFE 0x11)
    Endfilter = 0xFE11,
    /// initobj (0xFE 0x15)
    Initobj = 0xFE15,
    /// cpblk (0xFE 0x17)
    Cpblk = 0xFE17,
    /// initblk (0xFE 0x18)
    Initblk = 0xFE18,
    /// rethrow (0xFE 0x1A)
    Rethrow = 0xFE1A,
    /// sizeof (0xFE 0x1C)
    Sizeof = 0xFE1C,
}

/// Decode a single CIL opcode from the bytecode stream.
///
/// Returns (opcode, bytes_consumed) or None if the stream is exhausted/invalid.
pub fn decode_opcode(bytecode: &[u8], offset: usize) -> Option<(CilOpcode, usize)> {
    if offset >= bytecode.len() {
        return None;
    }
    let first = bytecode[offset];
    if first == 0xFE {
        // Two-byte opcode
        if offset + 1 >= bytecode.len() {
            return None;
        }
        let second = bytecode[offset + 1];
        let combined = 0xFE00u16 | second as u16;
        let opcode = match combined {
            0xFE01 => CilOpcode::Ceq,
            0xFE02 => CilOpcode::Cgt,
            0xFE03 => CilOpcode::CgtUn,
            0xFE04 => CilOpcode::Clt,
            0xFE05 => CilOpcode::CltUn,
            0xFE06 => CilOpcode::Ldftn,
            0xFE07 => CilOpcode::Ldvirtftn,
            0xFE09 => CilOpcode::Ldarg,
            0xFE0A => CilOpcode::Ldarga,
            0xFE0B => CilOpcode::Starg,
            0xFE0C => CilOpcode::Ldloc,
            0xFE0D => CilOpcode::Ldloca,
            0xFE0E => CilOpcode::Stloc,
            0xFE0F => CilOpcode::Localloc,
            0xFE11 => CilOpcode::Endfilter,
            0xFE15 => CilOpcode::Initobj,
            0xFE17 => CilOpcode::Cpblk,
            0xFE18 => CilOpcode::Initblk,
            0xFE1A => CilOpcode::Rethrow,
            0xFE1C => CilOpcode::Sizeof,
            _ => return None,
        };
        Some((opcode, 2))
    } else {
        let opcode = match first {
            0x00 => CilOpcode::Nop,
            0x01 => CilOpcode::Break,
            0x02 => CilOpcode::Ldarg0,
            0x03 => CilOpcode::Ldarg1,
            0x04 => CilOpcode::Ldarg2,
            0x05 => CilOpcode::Ldarg3,
            0x06 => CilOpcode::Ldloc0,
            0x07 => CilOpcode::Ldloc1,
            0x08 => CilOpcode::Ldloc2,
            0x09 => CilOpcode::Ldloc3,
            0x0A => CilOpcode::Stloc0,
            0x0B => CilOpcode::Stloc1,
            0x0C => CilOpcode::Stloc2,
            0x0D => CilOpcode::Stloc3,
            0x0E => CilOpcode::LdargS,
            0x0F => CilOpcode::LdargaS,
            0x10 => CilOpcode::StargS,
            0x11 => CilOpcode::LdlocS,
            0x12 => CilOpcode::LdlocaS,
            0x13 => CilOpcode::StlocS,
            0x14 => CilOpcode::Ldnull,
            0x15 => CilOpcode::LdcI4M1,
            0x16 => CilOpcode::LdcI4_0,
            0x17 => CilOpcode::LdcI4_1,
            0x18 => CilOpcode::LdcI4_2,
            0x19 => CilOpcode::LdcI4_3,
            0x1A => CilOpcode::LdcI4_4,
            0x1B => CilOpcode::LdcI4_5,
            0x1C => CilOpcode::LdcI4_6,
            0x1D => CilOpcode::LdcI4_7,
            0x1E => CilOpcode::LdcI4_8,
            0x1F => CilOpcode::LdcI4S,
            0x20 => CilOpcode::LdcI4,
            0x21 => CilOpcode::LdcI8,
            0x22 => CilOpcode::LdcR4,
            0x23 => CilOpcode::LdcR8,
            0x25 => CilOpcode::Dup,
            0x26 => CilOpcode::Pop,
            0x27 => CilOpcode::Jmp,
            0x28 => CilOpcode::Call,
            0x29 => CilOpcode::Calli,
            0x2A => CilOpcode::Ret,
            0x2B => CilOpcode::BrS,
            0x2C => CilOpcode::BrfalseS,
            0x2D => CilOpcode::BrtrueS,
            0x2E => CilOpcode::BeqS,
            0x2F => CilOpcode::BgeS,
            0x30 => CilOpcode::BgtS,
            0x31 => CilOpcode::BleS,
            0x32 => CilOpcode::BltS,
            0x33 => CilOpcode::BneUnS,
            0x34 => CilOpcode::BgeUnS,
            0x35 => CilOpcode::BgtUnS,
            0x36 => CilOpcode::BleUnS,
            0x37 => CilOpcode::BltUnS,
            0x38 => CilOpcode::Br,
            0x39 => CilOpcode::Brfalse,
            0x3A => CilOpcode::Brtrue,
            0x3B => CilOpcode::Beq,
            0x3C => CilOpcode::Bge,
            0x3D => CilOpcode::Bgt,
            0x3E => CilOpcode::Ble,
            0x3F => CilOpcode::Blt,
            0x40 => CilOpcode::BneUn,
            0x41 => CilOpcode::BgeUn,
            0x42 => CilOpcode::BgtUn,
            0x43 => CilOpcode::BleUn,
            0x44 => CilOpcode::BltUn,
            0x45 => CilOpcode::Switch,
            0x46 => CilOpcode::LdindI1,
            0x47 => CilOpcode::LdindU1,
            0x48 => CilOpcode::LdindI2,
            0x49 => CilOpcode::LdindU2,
            0x4A => CilOpcode::LdindI4,
            0x4B => CilOpcode::LdindU4,
            0x4C => CilOpcode::LdindI8,
            0x4D => CilOpcode::LdindI,
            0x4E => CilOpcode::LdindR4,
            0x4F => CilOpcode::LdindR8,
            0x50 => CilOpcode::LdindRef,
            0x51 => CilOpcode::StindRef,
            0x52 => CilOpcode::StindI1,
            0x53 => CilOpcode::StindI2,
            0x54 => CilOpcode::StindI4,
            0x55 => CilOpcode::StindI8,
            0x56 => CilOpcode::StindR4,
            0x57 => CilOpcode::StindR8,
            0x58 => CilOpcode::Add,
            0x59 => CilOpcode::Sub,
            0x5A => CilOpcode::Mul,
            0x5B => CilOpcode::Div,
            0x5C => CilOpcode::DivUn,
            0x5D => CilOpcode::Rem,
            0x5E => CilOpcode::RemUn,
            0x5F => CilOpcode::And,
            0x60 => CilOpcode::Or,
            0x61 => CilOpcode::Xor,
            0x62 => CilOpcode::Shl,
            0x63 => CilOpcode::Shr,
            0x64 => CilOpcode::ShrUn,
            0x65 => CilOpcode::Neg,
            0x66 => CilOpcode::Not,
            0x67 => CilOpcode::ConvI1,
            0x68 => CilOpcode::ConvI2,
            0x69 => CilOpcode::ConvI4,
            0x6A => CilOpcode::ConvI8,
            0x6B => CilOpcode::ConvR4,
            0x6C => CilOpcode::ConvR8,
            0x6D => CilOpcode::ConvU4,
            0x6E => CilOpcode::ConvU8,
            0x6F => CilOpcode::Callvirt,
            0x70 => CilOpcode::Cpobj,
            0x71 => CilOpcode::Ldobj,
            0x72 => CilOpcode::Ldstr,
            0x73 => CilOpcode::Newobj,
            0x74 => CilOpcode::Castclass,
            0x75 => CilOpcode::Isinst,
            0x76 => CilOpcode::ConvRUn,
            0x79 => CilOpcode::Unbox,
            0x7A => CilOpcode::Throw,
            0x7B => CilOpcode::Ldfld,
            0x7C => CilOpcode::Ldflda,
            0x7D => CilOpcode::Stfld,
            0x7E => CilOpcode::Ldsfld,
            0x7F => CilOpcode::Ldsflda,
            0x80 => CilOpcode::Stsfld,
            0x81 => CilOpcode::Stobj,
            0x8C => CilOpcode::Box,
            0x8D => CilOpcode::Newarr,
            0x8E => CilOpcode::Ldlen,
            0x8F => CilOpcode::Ldelema,
            0x90 => CilOpcode::LdelemI1,
            0x91 => CilOpcode::LdelemU1,
            0x92 => CilOpcode::LdelemI2,
            0x93 => CilOpcode::LdelemU2,
            0x94 => CilOpcode::LdelemI4,
            0x95 => CilOpcode::LdelemU4,
            0x96 => CilOpcode::LdelemI8,
            0x97 => CilOpcode::LdelemI,
            0x98 => CilOpcode::LdelemR4,
            0x99 => CilOpcode::LdelemR8,
            0x9A => CilOpcode::LdelemRef,
            0x9B => CilOpcode::StelemI,
            0x9C => CilOpcode::StelemI1,
            0x9D => CilOpcode::StelemI2,
            0x9E => CilOpcode::StelemI4,
            0x9F => CilOpcode::StelemI8,
            0xA0 => CilOpcode::StelemR4,
            0xA1 => CilOpcode::StelemR8,
            0xA2 => CilOpcode::StelemRef,
            0xA3 => CilOpcode::Ldelem,
            0xA4 => CilOpcode::Stelem,
            0xA5 => CilOpcode::UnboxAny,
            0xB3 => CilOpcode::ConvOvfI1,
            0xB4 => CilOpcode::ConvOvfU1,
            0xB5 => CilOpcode::ConvOvfI2,
            0xB6 => CilOpcode::ConvOvfU2,
            0xB7 => CilOpcode::ConvOvfI4,
            0xB8 => CilOpcode::ConvOvfU4,
            0xB9 => CilOpcode::ConvOvfI8,
            0xBA => CilOpcode::ConvOvfU8,
            0xD1 => CilOpcode::ConvU2,
            0xD2 => CilOpcode::ConvU1,
            0xD3 => CilOpcode::ConvI,
            0xD4 => CilOpcode::ConvOvfI,
            0xD5 => CilOpcode::ConvOvfU,
            0xD6 => CilOpcode::AddOvf,
            0xD7 => CilOpcode::AddOvfUn,
            0xD8 => CilOpcode::MulOvf,
            0xD9 => CilOpcode::MulOvfUn,
            0xDA => CilOpcode::SubOvf,
            0xDB => CilOpcode::SubOvfUn,
            0xDC => CilOpcode::Endfinally,
            0xDD => CilOpcode::Leave,
            0xDE => CilOpcode::LeaveS,
            0xDF => CilOpcode::StindI,
            0xE0 => CilOpcode::ConvU,
            _ => return None,
        };
        Some((opcode, 1))
    }
}

/// Read a signed 8-bit immediate from bytecode.
fn read_i8(bytecode: &[u8], offset: usize) -> Option<i8> {
    bytecode.get(offset).map(|&b| b as i8)
}

/// Read a signed 32-bit immediate from bytecode (little-endian).
fn read_i32(bytecode: &[u8], offset: usize) -> Option<i32> {
    if offset + 4 > bytecode.len() {
        return None;
    }
    Some(i32::from_le_bytes([
        bytecode[offset],
        bytecode[offset + 1],
        bytecode[offset + 2],
        bytecode[offset + 3],
    ]))
}

/// Read an unsigned 32-bit immediate from bytecode (little-endian).
fn read_u32(bytecode: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > bytecode.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        bytecode[offset],
        bytecode[offset + 1],
        bytecode[offset + 2],
        bytecode[offset + 3],
    ]))
}

/// Read a signed 64-bit immediate from bytecode.
fn read_i64(bytecode: &[u8], offset: usize) -> Option<i64> {
    if offset + 8 > bytecode.len() {
        return None;
    }
    Some(i64::from_le_bytes([
        bytecode[offset],
        bytecode[offset + 1],
        bytecode[offset + 2],
        bytecode[offset + 3],
        bytecode[offset + 4],
        bytecode[offset + 5],
        bytecode[offset + 6],
        bytecode[offset + 7],
    ]))
}

/// Read a 32-bit float from bytecode.
fn read_f32(bytecode: &[u8], offset: usize) -> Option<f32> {
    if offset + 4 > bytecode.len() {
        return None;
    }
    Some(f32::from_le_bytes([
        bytecode[offset],
        bytecode[offset + 1],
        bytecode[offset + 2],
        bytecode[offset + 3],
    ]))
}

/// Read a 64-bit float from bytecode.
fn read_f64(bytecode: &[u8], offset: usize) -> Option<f64> {
    if offset + 8 > bytecode.len() {
        return None;
    }
    Some(f64::from_le_bytes([
        bytecode[offset],
        bytecode[offset + 1],
        bytecode[offset + 2],
        bytecode[offset + 3],
        bytecode[offset + 4],
        bytecode[offset + 5],
        bytecode[offset + 6],
        bytecode[offset + 7],
    ]))
}

/// CIL exception handling clause (ECMA-335 II.25.4.6).
#[derive(Debug, Clone)]
pub struct ExceptionClause {
    /// Clause kind.
    pub kind: ExceptionClauseKind,
    /// IL offset of the try block start.
    pub try_offset: u32,
    /// Length of the try block.
    pub try_length: u32,
    /// IL offset of the handler start.
    pub handler_offset: u32,
    /// Length of the handler.
    pub handler_length: u32,
    /// For catch: metadata token of the exception type.
    /// For filter: IL offset of the filter block.
    pub class_token_or_filter_offset: u32,
}

/// Exception clause kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionClauseKind {
    /// catch clause — catches exceptions of a specific type.
    Catch,
    /// filter clause — evaluates a filter expression.
    Filter,
    /// finally clause — runs regardless of exception.
    Finally,
    /// fault clause — runs only if an exception occurred.
    Fault,
}

/// A CIL method body (ECMA-335 II.25.4).
#[derive(Debug, Clone)]
pub struct MethodBody {
    /// Maximum evaluation stack depth.
    pub max_stack: u16,
    /// Size of the local variable array.
    pub local_var_count: u16,
    /// Whether locals should be zero-initialized.
    pub init_locals: bool,
    /// Raw CIL bytecode.
    pub code: Vec<u8>,
    /// Exception handling clauses.
    pub exception_clauses: Vec<ExceptionClause>,
}

/// Parse a CIL method body from raw bytes at the given RVA offset.
///
/// Method bodies use either a tiny (1-byte header) or fat (12-byte header) format.
pub fn parse_method_body(data: &[u8], offset: usize) -> Option<MethodBody> {
    if offset >= data.len() {
        return None;
    }

    let first = data[offset];

    // Tiny format: bits [1:0] == 0b10, bits [7:2] = code size
    if first & 0x03 == 0x02 {
        let code_size = (first >> 2) as usize;
        let code_start = offset + 1;
        if code_start + code_size > data.len() {
            return None;
        }
        return Some(MethodBody {
            max_stack: 8,
            local_var_count: 0,
            init_locals: false,
            code: data[code_start..code_start + code_size].to_vec(),
            exception_clauses: Vec::new(),
        });
    }

    // Fat format: bits [1:0] == 0b11
    if first & 0x03 == 0x03 {
        if offset + 12 > data.len() {
            return None;
        }
        let flags_and_size = u16::from_le_bytes([data[offset], data[offset + 1]]);
        let _header_size = ((flags_and_size >> 12) & 0x0F) * 4; // in bytes
        let has_more_sections = (flags_and_size & 0x08) != 0;
        let init_locals = (flags_and_size & 0x10) != 0;

        let max_stack = u16::from_le_bytes([data[offset + 2], data[offset + 3]]);
        let code_size = u32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]) as usize;
        let local_var_sig = u32::from_le_bytes([
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
        ]);

        let code_start = offset + 12;
        if code_start + code_size > data.len() {
            return None;
        }
        let code = data[code_start..code_start + code_size].to_vec();

        // Determine local variable count from the signature token
        // (StandAloneSig table index, would need blob parsing for exact count)
        let local_var_count = if local_var_sig != 0 { 16 } else { 0 }; // conservative default

        let mut exception_clauses = Vec::new();

        // Parse extra data sections (exception handling)
        if has_more_sections {
            let section_start = (code_start + code_size + 3) & !3; // 4-byte aligned
            if section_start < data.len() {
                let kind_byte = data[section_start];
                let is_fat_section = (kind_byte & 0x40) != 0;
                let _is_exception = (kind_byte & 0x01) != 0;

                if !is_fat_section {
                    // Small exception section
                    if section_start + 4 <= data.len() {
                        let data_size = data[section_start + 1] as usize;
                        let num_clauses = (data_size - 4) / 12;
                        let mut pos = section_start + 4;
                        for _ in 0..num_clauses {
                            if pos + 12 > data.len() {
                                break;
                            }
                            let flags = u16::from_le_bytes([data[pos], data[pos + 1]]);
                            let clause_kind = match flags {
                                0 => ExceptionClauseKind::Catch,
                                1 => ExceptionClauseKind::Filter,
                                2 => ExceptionClauseKind::Finally,
                                4 => ExceptionClauseKind::Fault,
                                _ => ExceptionClauseKind::Catch,
                            };
                            exception_clauses.push(ExceptionClause {
                                kind: clause_kind,
                                try_offset: u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as u32,
                                try_length: data[pos + 4] as u32,
                                handler_offset: u16::from_le_bytes([data[pos + 5], data[pos + 6]]) as u32,
                                handler_length: data[pos + 7] as u32,
                                class_token_or_filter_offset: u32::from_le_bytes([
                                    data[pos + 8],
                                    data[pos + 9],
                                    data[pos + 10],
                                    data[pos + 11],
                                ]),
                            });
                            pos += 12;
                        }
                    }
                } else {
                    // Fat exception section
                    if section_start + 4 <= data.len() {
                        let data_size = u32::from_le_bytes([
                            data[section_start + 1],
                            data[section_start + 2],
                            data[section_start + 3],
                            0,
                        ]) as usize;
                        let num_clauses = (data_size - 4) / 24;
                        let mut pos = section_start + 4;
                        for _ in 0..num_clauses {
                            if pos + 24 > data.len() {
                                break;
                            }
                            let flags = u32::from_le_bytes([
                                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                            ]);
                            let clause_kind = match flags {
                                0 => ExceptionClauseKind::Catch,
                                1 => ExceptionClauseKind::Filter,
                                2 => ExceptionClauseKind::Finally,
                                4 => ExceptionClauseKind::Fault,
                                _ => ExceptionClauseKind::Catch,
                            };
                            exception_clauses.push(ExceptionClause {
                                kind: clause_kind,
                                try_offset: u32::from_le_bytes([
                                    data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
                                ]),
                                try_length: u32::from_le_bytes([
                                    data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                                ]),
                                handler_offset: u32::from_le_bytes([
                                    data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
                                ]),
                                handler_length: u32::from_le_bytes([
                                    data[pos + 16], data[pos + 17], data[pos + 18], data[pos + 19],
                                ]),
                                class_token_or_filter_offset: u32::from_le_bytes([
                                    data[pos + 20], data[pos + 21], data[pos + 22], data[pos + 23],
                                ]),
                            });
                            pos += 24;
                        }
                    }
                }
            }
        }

        return Some(MethodBody {
            max_stack,
            local_var_count,
            init_locals,
            code,
            exception_clauses,
        });
    }

    None
}

/// CIL interpreter execution error.
#[derive(Debug, Clone)]
pub enum CilError {
    /// Stack underflow — tried to pop from empty eval stack.
    StackUnderflow,
    /// Stack overflow — exceeded max_stack.
    StackOverflow,
    /// Invalid opcode encountered.
    InvalidOpcode(u8),
    /// Invalid metadata token.
    InvalidToken(u32),
    /// Null reference exception.
    NullReference,
    /// Index out of range (array access).
    IndexOutOfRange { index: i32, length: i32 },
    /// Invalid cast.
    InvalidCast { from: String, to: String },
    /// Division by zero.
    DivideByZero,
    /// Arithmetic overflow.
    Overflow,
    /// Unhandled exception.
    UnhandledException(String),
    /// Method not found.
    MethodNotFound(String),
    /// Type not found.
    TypeNotFound(String),
    /// Maximum execution steps exceeded (infinite loop protection).
    ExecutionLimitExceeded,
    /// Feature not yet implemented.
    NotImplemented(String),
}

/// Callback for resolving method calls and field accesses during CIL interpretation.
pub trait CilCallbacks {
    /// Resolve and call a method by metadata token.
    ///
    /// Returns the method's return value (or Void for void methods).
    fn call_method(
        &mut self,
        token: u32,
        args: &[ClrValue],
    ) -> Result<ClrValue, CilError>;

    /// Resolve and call a virtual method by metadata token.
    fn callvirt(
        &mut self,
        token: u32,
        this: &ClrValue,
        args: &[ClrValue],
    ) -> Result<ClrValue, CilError>;

    /// Create a new object (newobj) — allocate and call constructor.
    fn new_object(
        &mut self,
        ctor_token: u32,
        args: &[ClrValue],
    ) -> Result<ClrValue, CilError>;

    /// Load a string from the #US heap.
    fn load_string(&self, token: u32) -> Result<ClrValue, CilError>;

    /// Load a static field.
    fn load_static_field(&self, token: u32) -> Result<ClrValue, CilError>;

    /// Store a static field.
    fn store_static_field(&mut self, token: u32, value: ClrValue) -> Result<(), CilError>;

    /// Load an instance field.
    fn load_field(&self, obj: &ClrValue, token: u32) -> Result<ClrValue, CilError>;

    /// Store an instance field.
    fn store_field(&mut self, obj: &ClrValue, token: u32, value: ClrValue) -> Result<(), CilError>;

    /// Create a new array.
    fn new_array(&mut self, type_token: u32, length: i32) -> Result<ClrValue, CilError>;

    /// Load an array element.
    fn load_array_element(&self, array: &ClrValue, index: i32) -> Result<ClrValue, CilError>;

    /// Store an array element.
    fn store_array_element(
        &mut self,
        array: &ClrValue,
        index: i32,
        value: ClrValue,
    ) -> Result<(), CilError>;

    /// Get array length.
    fn array_length(&self, array: &ClrValue) -> Result<i32, CilError>;

    /// Box a value type.
    fn box_value(&mut self, type_token: u32, value: ClrValue) -> Result<ClrValue, CilError>;

    /// Unbox a reference to a value type.
    fn unbox_value(&self, type_token: u32, obj: &ClrValue) -> Result<ClrValue, CilError>;

    /// Cast an object reference.
    fn cast_class(&self, type_token: u32, obj: &ClrValue) -> Result<ClrValue, CilError>;

    /// Test if an object is an instance of a type.
    fn is_instance(&self, type_token: u32, obj: &ClrValue) -> Result<ClrValue, CilError>;
}

/// CIL interpreter state for a single method invocation.
pub struct CilInterpreter<'a> {
    /// Raw CIL bytecode.
    bytecode: &'a [u8],
    /// Instruction pointer (offset into bytecode).
    ip: usize,
    /// Evaluation stack.
    eval_stack: Vec<ClrValue>,
    /// Local variables.
    locals: Vec<ClrValue>,
    /// Method arguments.
    args: Vec<ClrValue>,
    /// Maximum stack depth.
    max_stack: u16,
    /// Maximum execution steps before aborting.
    max_steps: u64,
    /// Steps executed so far.
    steps: u64,
}

impl<'a> CilInterpreter<'a> {
    /// Create a new CIL interpreter for a method body.
    pub fn new(body: &'a MethodBody, args: Vec<ClrValue>) -> Self {
        let mut locals = Vec::with_capacity(body.local_var_count as usize);
        for _ in 0..body.local_var_count {
            locals.push(ClrValue::I4(0)); // Default-initialize to zero
        }

        Self {
            bytecode: &body.code,
            ip: 0,
            eval_stack: Vec::with_capacity(body.max_stack as usize),
            locals,
            args,
            max_stack: body.max_stack,
            max_steps: 10_000_000, // 10M instruction limit
            steps: 0,
        }
    }

    /// Push a value onto the evaluation stack.
    fn push(&mut self, val: ClrValue) -> Result<(), CilError> {
        if self.eval_stack.len() >= self.max_stack as usize {
            return Err(CilError::StackOverflow);
        }
        self.eval_stack.push(val);
        Ok(())
    }

    /// Pop a value from the evaluation stack.
    fn pop(&mut self) -> Result<ClrValue, CilError> {
        self.eval_stack.pop().ok_or(CilError::StackUnderflow)
    }

    /// Execute the CIL bytecode until ret or exception.
    ///
    /// Returns the method's return value.
    pub fn execute(&mut self, callbacks: &mut dyn CilCallbacks) -> Result<ClrValue, CilError> {
        loop {
            if self.ip >= self.bytecode.len() {
                return Ok(ClrValue::Void);
            }

            self.steps += 1;
            if self.steps > self.max_steps {
                return Err(CilError::ExecutionLimitExceeded);
            }

            let (opcode, opcode_size) = decode_opcode(self.bytecode, self.ip)
                .ok_or(CilError::InvalidOpcode(self.bytecode[self.ip]))?;

            self.ip += opcode_size;

            match opcode {
                CilOpcode::Nop => {}
                CilOpcode::Break => {}

                // -- Load argument --
                CilOpcode::Ldarg0 => {
                    let v = self.args.get(0).cloned().unwrap_or(ClrValue::Null);
                    self.push(v)?;
                }
                CilOpcode::Ldarg1 => {
                    let v = self.args.get(1).cloned().unwrap_or(ClrValue::Null);
                    self.push(v)?;
                }
                CilOpcode::Ldarg2 => {
                    let v = self.args.get(2).cloned().unwrap_or(ClrValue::Null);
                    self.push(v)?;
                }
                CilOpcode::Ldarg3 => {
                    let v = self.args.get(3).cloned().unwrap_or(ClrValue::Null);
                    self.push(v)?;
                }
                CilOpcode::LdargS => {
                    let idx = self.bytecode.get(self.ip).copied().unwrap_or(0) as usize;
                    self.ip += 1;
                    let v = self.args.get(idx).cloned().unwrap_or(ClrValue::Null);
                    self.push(v)?;
                }

                // -- Store argument --
                CilOpcode::StargS => {
                    let idx = self.bytecode.get(self.ip).copied().unwrap_or(0) as usize;
                    self.ip += 1;
                    let val = self.pop()?;
                    if idx < self.args.len() {
                        self.args[idx] = val;
                    }
                }

                // -- Load local --
                CilOpcode::Ldloc0 => {
                    let v = self.locals.get(0).cloned().unwrap_or(ClrValue::I4(0));
                    self.push(v)?;
                }
                CilOpcode::Ldloc1 => {
                    let v = self.locals.get(1).cloned().unwrap_or(ClrValue::I4(0));
                    self.push(v)?;
                }
                CilOpcode::Ldloc2 => {
                    let v = self.locals.get(2).cloned().unwrap_or(ClrValue::I4(0));
                    self.push(v)?;
                }
                CilOpcode::Ldloc3 => {
                    let v = self.locals.get(3).cloned().unwrap_or(ClrValue::I4(0));
                    self.push(v)?;
                }
                CilOpcode::LdlocS => {
                    let idx = self.bytecode.get(self.ip).copied().unwrap_or(0) as usize;
                    self.ip += 1;
                    let v = self.locals.get(idx).cloned().unwrap_or(ClrValue::I4(0));
                    self.push(v)?;
                }

                // -- Store local --
                CilOpcode::Stloc0 => {
                    let v = self.pop()?;
                    if self.locals.is_empty() {
                        self.locals.push(v);
                    } else {
                        self.locals[0] = v;
                    }
                }
                CilOpcode::Stloc1 => {
                    let v = self.pop()?;
                    while self.locals.len() <= 1 {
                        self.locals.push(ClrValue::I4(0));
                    }
                    self.locals[1] = v;
                }
                CilOpcode::Stloc2 => {
                    let v = self.pop()?;
                    while self.locals.len() <= 2 {
                        self.locals.push(ClrValue::I4(0));
                    }
                    self.locals[2] = v;
                }
                CilOpcode::Stloc3 => {
                    let v = self.pop()?;
                    while self.locals.len() <= 3 {
                        self.locals.push(ClrValue::I4(0));
                    }
                    self.locals[3] = v;
                }
                CilOpcode::StlocS => {
                    let idx = self.bytecode.get(self.ip).copied().unwrap_or(0) as usize;
                    self.ip += 1;
                    let v = self.pop()?;
                    while self.locals.len() <= idx {
                        self.locals.push(ClrValue::I4(0));
                    }
                    self.locals[idx] = v;
                }

                // -- Constants --
                CilOpcode::Ldnull => self.push(ClrValue::Null)?,
                CilOpcode::LdcI4M1 => self.push(ClrValue::I4(-1))?,
                CilOpcode::LdcI4_0 => self.push(ClrValue::I4(0))?,
                CilOpcode::LdcI4_1 => self.push(ClrValue::I4(1))?,
                CilOpcode::LdcI4_2 => self.push(ClrValue::I4(2))?,
                CilOpcode::LdcI4_3 => self.push(ClrValue::I4(3))?,
                CilOpcode::LdcI4_4 => self.push(ClrValue::I4(4))?,
                CilOpcode::LdcI4_5 => self.push(ClrValue::I4(5))?,
                CilOpcode::LdcI4_6 => self.push(ClrValue::I4(6))?,
                CilOpcode::LdcI4_7 => self.push(ClrValue::I4(7))?,
                CilOpcode::LdcI4_8 => self.push(ClrValue::I4(8))?,
                CilOpcode::LdcI4S => {
                    let val = read_i8(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 1;
                    self.push(ClrValue::I4(val as i32))?;
                }
                CilOpcode::LdcI4 => {
                    let val = read_i32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    self.push(ClrValue::I4(val))?;
                }
                CilOpcode::LdcI8 => {
                    let val = read_i64(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 8;
                    self.push(ClrValue::I8(val))?;
                }
                CilOpcode::LdcR4 => {
                    let val = read_f32(self.bytecode, self.ip).unwrap_or(0.0);
                    self.ip += 4;
                    self.push(ClrValue::R4(val))?;
                }
                CilOpcode::LdcR8 => {
                    let val = read_f64(self.bytecode, self.ip).unwrap_or(0.0);
                    self.ip += 8;
                    self.push(ClrValue::R8(val))?;
                }

                // -- Dup / Pop --
                CilOpcode::Dup => {
                    let v = self.eval_stack.last().cloned().ok_or(CilError::StackUnderflow)?;
                    self.push(v)?;
                }
                CilOpcode::Pop => {
                    self.pop()?;
                }

                // -- Return --
                CilOpcode::Ret => {
                    let ret_val = if self.eval_stack.is_empty() {
                        ClrValue::Void
                    } else {
                        self.pop()?
                    };
                    return Ok(ret_val);
                }

                // -- Branches (short form) --
                CilOpcode::BrS => {
                    let offset = read_i8(self.bytecode, self.ip).unwrap_or(0) as i32;
                    self.ip = ((self.ip as i32) + 1 + offset) as usize;
                }
                CilOpcode::BrfalseS => {
                    let offset = read_i8(self.bytecode, self.ip).unwrap_or(0) as i32;
                    self.ip += 1;
                    let val = self.pop()?;
                    if !val.to_bool() {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }
                CilOpcode::BrtrueS => {
                    let offset = read_i8(self.bytecode, self.ip).unwrap_or(0) as i32;
                    self.ip += 1;
                    let val = self.pop()?;
                    if val.to_bool() {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }

                // -- Branches (long form) --
                CilOpcode::Br => {
                    let offset = read_i32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip = ((self.ip as i32) + 4 + offset) as usize;
                }
                CilOpcode::Brfalse => {
                    let offset = read_i32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = self.pop()?;
                    if !val.to_bool() {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }
                CilOpcode::Brtrue => {
                    let offset = read_i32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = self.pop()?;
                    if val.to_bool() {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }

                // -- Conditional branches (short) --
                CilOpcode::BeqS | CilOpcode::BgeS | CilOpcode::BgtS |
                CilOpcode::BleS | CilOpcode::BltS | CilOpcode::BneUnS |
                CilOpcode::BgeUnS | CilOpcode::BgtUnS | CilOpcode::BleUnS | CilOpcode::BltUnS => {
                    let offset = read_i8(self.bytecode, self.ip).unwrap_or(0) as i32;
                    self.ip += 1;
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    let cond = compare_values(&v1, &v2, opcode);
                    if cond {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }

                // -- Conditional branches (long) --
                CilOpcode::Beq | CilOpcode::Bge | CilOpcode::Bgt |
                CilOpcode::Ble | CilOpcode::Blt | CilOpcode::BneUn |
                CilOpcode::BgeUn | CilOpcode::BgtUn | CilOpcode::BleUn | CilOpcode::BltUn => {
                    let offset = read_i32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    let cond = compare_values(&v1, &v2, opcode);
                    if cond {
                        self.ip = ((self.ip as i32) + offset) as usize;
                    }
                }

                // -- Arithmetic --
                CilOpcode::Add | CilOpcode::AddOvf | CilOpcode::AddOvfUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(arith_op(&v1, &v2, ArithOp::Add)?)?;
                }
                CilOpcode::Sub | CilOpcode::SubOvf | CilOpcode::SubOvfUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(arith_op(&v1, &v2, ArithOp::Sub)?)?;
                }
                CilOpcode::Mul | CilOpcode::MulOvf | CilOpcode::MulOvfUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(arith_op(&v1, &v2, ArithOp::Mul)?)?;
                }
                CilOpcode::Div | CilOpcode::DivUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(arith_op(&v1, &v2, ArithOp::Div)?)?;
                }
                CilOpcode::Rem | CilOpcode::RemUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(arith_op(&v1, &v2, ArithOp::Rem)?)?;
                }

                // -- Bitwise --
                CilOpcode::And => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(bitwise_op(&v1, &v2, BitwiseOp::And)?)?;
                }
                CilOpcode::Or => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(bitwise_op(&v1, &v2, BitwiseOp::Or)?)?;
                }
                CilOpcode::Xor => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    self.push(bitwise_op(&v1, &v2, BitwiseOp::Xor)?)?;
                }
                CilOpcode::Shl => {
                    let shift = self.pop()?;
                    let val = self.pop()?;
                    self.push(bitwise_op(&val, &shift, BitwiseOp::Shl)?)?;
                }
                CilOpcode::Shr | CilOpcode::ShrUn => {
                    let shift = self.pop()?;
                    let val = self.pop()?;
                    self.push(bitwise_op(&val, &shift, BitwiseOp::Shr)?)?;
                }
                CilOpcode::Neg => {
                    let v = self.pop()?;
                    let result = match v {
                        ClrValue::I4(x) => ClrValue::I4(-x),
                        ClrValue::I8(x) => ClrValue::I8(-x),
                        ClrValue::R4(x) => ClrValue::R4(-x),
                        ClrValue::R8(x) => ClrValue::R8(-x),
                        _ => ClrValue::I4(0),
                    };
                    self.push(result)?;
                }
                CilOpcode::Not => {
                    let v = self.pop()?;
                    let result = match v {
                        ClrValue::I4(x) => ClrValue::I4(!x),
                        ClrValue::I8(x) => ClrValue::I8(!x),
                        _ => ClrValue::I4(0),
                    };
                    self.push(result)?;
                }

                // -- Comparison --
                CilOpcode::Ceq => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    let eq = compare_values(&v1, &v2, CilOpcode::BeqS);
                    self.push(ClrValue::I4(if eq { 1 } else { 0 }))?;
                }
                CilOpcode::Cgt | CilOpcode::CgtUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    let gt = compare_values(&v1, &v2, CilOpcode::BgtS);
                    self.push(ClrValue::I4(if gt { 1 } else { 0 }))?;
                }
                CilOpcode::Clt | CilOpcode::CltUn => {
                    let v2 = self.pop()?;
                    let v1 = self.pop()?;
                    let lt = compare_values(&v1, &v2, CilOpcode::BltS);
                    self.push(ClrValue::I4(if lt { 1 } else { 0 }))?;
                }

                // -- Conversions --
                CilOpcode::ConvI1 | CilOpcode::ConvOvfI1 | CilOpcode::ConvOvfI1Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0) as i8 as i32))?;
                }
                CilOpcode::ConvI2 | CilOpcode::ConvOvfI2 | CilOpcode::ConvOvfI2Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0) as i16 as i32))?;
                }
                CilOpcode::ConvI4 | CilOpcode::ConvOvfI4 | CilOpcode::ConvOvfI4Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0)))?;
                }
                CilOpcode::ConvI8 | CilOpcode::ConvOvfI8 | CilOpcode::ConvOvfI8Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I8(v.to_i64().unwrap_or(0)))?;
                }
                CilOpcode::ConvR4 => {
                    let v = self.pop()?;
                    self.push(ClrValue::R4(v.to_f64().unwrap_or(0.0) as f32))?;
                }
                CilOpcode::ConvR8 | CilOpcode::ConvRUn => {
                    let v = self.pop()?;
                    self.push(ClrValue::R8(v.to_f64().unwrap_or(0.0)))?;
                }
                CilOpcode::ConvU1 | CilOpcode::ConvOvfU1 | CilOpcode::ConvOvfU1Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0) as u8 as i32))?;
                }
                CilOpcode::ConvU2 | CilOpcode::ConvOvfU2 | CilOpcode::ConvOvfU2Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0) as u16 as i32))?;
                }
                CilOpcode::ConvU4 | CilOpcode::ConvOvfU4 | CilOpcode::ConvOvfU4Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I4(v.to_i32().unwrap_or(0)))?;
                }
                CilOpcode::ConvU8 | CilOpcode::ConvOvfU8 | CilOpcode::ConvOvfU8Un => {
                    let v = self.pop()?;
                    self.push(ClrValue::I8(v.to_i64().unwrap_or(0)))?;
                }
                CilOpcode::ConvI | CilOpcode::ConvOvfI | CilOpcode::ConvOvfIUn => {
                    let v = self.pop()?;
                    self.push(ClrValue::IntPtr(v.to_i64().unwrap_or(0) as isize))?;
                }
                CilOpcode::ConvU | CilOpcode::ConvOvfU | CilOpcode::ConvOvfUUn => {
                    let v = self.pop()?;
                    self.push(ClrValue::UIntPtr(v.to_i64().unwrap_or(0) as usize))?;
                }

                // -- Call --
                CilOpcode::Call => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    // Pop arguments (simplified — actual count comes from signature)
                    let result = callbacks.call_method(token, &[])?;
                    if !matches!(result, ClrValue::Void) {
                        self.push(result)?;
                    }
                }
                CilOpcode::Callvirt => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let this = self.pop()?;
                    let result = callbacks.callvirt(token, &this, &[])?;
                    if !matches!(result, ClrValue::Void) {
                        self.push(result)?;
                    }
                }

                // -- String loading --
                CilOpcode::Ldstr => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let s = callbacks.load_string(token)?;
                    self.push(s)?;
                }

                // -- Object creation --
                CilOpcode::Newobj => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let obj = callbacks.new_object(token, &[])?;
                    self.push(obj)?;
                }

                // -- Field access --
                CilOpcode::Ldfld => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let obj = self.pop()?;
                    let val = callbacks.load_field(&obj, token)?;
                    self.push(val)?;
                }
                CilOpcode::Stfld => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = self.pop()?;
                    let obj = self.pop()?;
                    callbacks.store_field(&obj, token, val)?;
                }
                CilOpcode::Ldsfld => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = callbacks.load_static_field(token)?;
                    self.push(val)?;
                }
                CilOpcode::Stsfld => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = self.pop()?;
                    callbacks.store_static_field(token, val)?;
                }

                // -- Boxing / Unboxing --
                CilOpcode::Box => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let val = self.pop()?;
                    let boxed = callbacks.box_value(token, val)?;
                    self.push(boxed)?;
                }
                CilOpcode::Unbox | CilOpcode::UnboxAny => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let obj = self.pop()?;
                    let val = callbacks.unbox_value(token, &obj)?;
                    self.push(val)?;
                }

                // -- Array operations --
                CilOpcode::Newarr => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let length = self.pop()?.to_i32().unwrap_or(0);
                    let arr = callbacks.new_array(token, length)?;
                    self.push(arr)?;
                }
                CilOpcode::Ldlen => {
                    let arr = self.pop()?;
                    let len = callbacks.array_length(&arr)?;
                    self.push(ClrValue::IntPtr(len as isize))?;
                }
                CilOpcode::LdelemI4 | CilOpcode::LdelemI1 | CilOpcode::LdelemU1 |
                CilOpcode::LdelemI2 | CilOpcode::LdelemU2 | CilOpcode::LdelemU4 |
                CilOpcode::LdelemI8 | CilOpcode::LdelemI | CilOpcode::LdelemR4 |
                CilOpcode::LdelemR8 | CilOpcode::LdelemRef | CilOpcode::Ldelem => {
                    if matches!(opcode, CilOpcode::Ldelem) {
                        let _token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                        self.ip += 4;
                    }
                    let index = self.pop()?.to_i32().unwrap_or(0);
                    let arr = self.pop()?;
                    let val = callbacks.load_array_element(&arr, index)?;
                    self.push(val)?;
                }
                CilOpcode::StelemI4 | CilOpcode::StelemI1 | CilOpcode::StelemI2 |
                CilOpcode::StelemI8 | CilOpcode::StelemI | CilOpcode::StelemR4 |
                CilOpcode::StelemR8 | CilOpcode::StelemRef | CilOpcode::Stelem => {
                    if matches!(opcode, CilOpcode::Stelem) {
                        let _token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                        self.ip += 4;
                    }
                    let val = self.pop()?;
                    let index = self.pop()?.to_i32().unwrap_or(0);
                    let arr = self.pop()?;
                    callbacks.store_array_element(&arr, index, val)?;
                }

                // -- Type casting --
                CilOpcode::Castclass => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let obj = self.pop()?;
                    let result = callbacks.cast_class(token, &obj)?;
                    self.push(result)?;
                }
                CilOpcode::Isinst => {
                    let token = read_u32(self.bytecode, self.ip).unwrap_or(0);
                    self.ip += 4;
                    let obj = self.pop()?;
                    let result = callbacks.is_instance(token, &obj)?;
                    self.push(result)?;
                }

                // -- Exception handling --
                CilOpcode::Throw => {
                    let _exc = self.pop()?;
                    return Err(CilError::UnhandledException(String::from("Managed exception thrown")));
                }
                CilOpcode::Endfinally => {
                    // Continue to the next instruction after the finally block
                }
                CilOpcode::Leave | CilOpcode::LeaveS => {
                    let offset = if opcode == CilOpcode::LeaveS {
                        let o = read_i8(self.bytecode, self.ip).unwrap_or(0) as i32;
                        self.ip += 1;
                        o
                    } else {
                        let o = read_i32(self.bytecode, self.ip).unwrap_or(0);
                        self.ip += 4;
                        o
                    };
                    // Clear the eval stack (leave empties it)
                    self.eval_stack.clear();
                    self.ip = ((self.ip as i32) + offset) as usize;
                }
                CilOpcode::Rethrow => {
                    return Err(CilError::UnhandledException(String::from("Rethrown exception")));
                }

                // -- Switch --
                CilOpcode::Switch => {
                    let n = read_u32(self.bytecode, self.ip).unwrap_or(0) as usize;
                    self.ip += 4;
                    let base = self.ip + n * 4;
                    let val = self.pop()?.to_i32().unwrap_or(-1);
                    if val >= 0 && (val as usize) < n {
                        let target_offset = read_i32(self.bytecode, self.ip + (val as usize) * 4).unwrap_or(0);
                        self.ip = ((base as i32) + target_offset) as usize;
                    } else {
                        self.ip = base;
                    }
                }

                // -- Misc (skip token operands for unimplemented ops) --
                CilOpcode::Jmp | CilOpcode::Calli | CilOpcode::Ldftn |
                CilOpcode::Ldvirtftn | CilOpcode::Cpobj | CilOpcode::Ldobj |
                CilOpcode::Stobj | CilOpcode::Ldelema | CilOpcode::Ldflda |
                CilOpcode::Ldsflda | CilOpcode::Initobj | CilOpcode::Sizeof => {
                    self.ip += 4; // Skip 32-bit token
                }

                CilOpcode::LdargaS | CilOpcode::LdlocaS => {
                    let idx = self.bytecode.get(self.ip).copied().unwrap_or(0);
                    self.ip += 1;
                    // Push a placeholder address (pointer to local/arg)
                    self.push(ClrValue::IntPtr(idx as isize))?;
                }

                CilOpcode::Ldarg | CilOpcode::Ldarga | CilOpcode::Starg |
                CilOpcode::Ldloc | CilOpcode::Ldloca | CilOpcode::Stloc => {
                    // Two-byte opcodes with 16-bit index
                    let _idx = if self.ip + 2 <= self.bytecode.len() {
                        u16::from_le_bytes([self.bytecode[self.ip], self.bytecode[self.ip + 1]])
                    } else {
                        0
                    };
                    self.ip += 2;
                    // Simplified: push zero/pop
                    self.push(ClrValue::I4(0))?;
                }

                CilOpcode::Localloc => {
                    let _size = self.pop()?;
                    self.push(ClrValue::IntPtr(0))?; // stub
                }

                CilOpcode::Endfilter => {
                    // Pop the filter result
                    let _result = self.pop()?;
                }

                CilOpcode::Cpblk | CilOpcode::Initblk => {
                    // Pop 3 values (dest, src/value, size)
                    self.pop()?;
                    self.pop()?;
                    self.pop()?;
                }

                // -- Indirect load/store (stubbed) --
                CilOpcode::LdindI1 | CilOpcode::LdindU1 | CilOpcode::LdindI2 |
                CilOpcode::LdindU2 | CilOpcode::LdindI4 | CilOpcode::LdindU4 |
                CilOpcode::LdindI8 | CilOpcode::LdindI | CilOpcode::LdindR4 |
                CilOpcode::LdindR8 | CilOpcode::LdindRef => {
                    let _addr = self.pop()?;
                    self.push(ClrValue::I4(0))?; // stub: deref not implemented
                }
                CilOpcode::StindRef | CilOpcode::StindI1 | CilOpcode::StindI2 |
                CilOpcode::StindI4 | CilOpcode::StindI8 | CilOpcode::StindR4 |
                CilOpcode::StindR8 | CilOpcode::StindI => {
                    let _val = self.pop()?;
                    let _addr = self.pop()?;
                }

                CilOpcode::Prefix => {
                    // Should not reach here — prefix is handled in decode_opcode
                    return Err(CilError::InvalidOpcode(0xFE));
                }
            }
        }
    }
}

/// Arithmetic operation kind.
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// Perform an arithmetic operation on two CIL values.
fn arith_op(v1: &ClrValue, v2: &ClrValue, op: ArithOp) -> Result<ClrValue, CilError> {
    // Try i32 path first
    if let (Some(a), Some(b)) = (v1.to_i32(), v2.to_i32()) {
        return Ok(ClrValue::I4(match op {
            ArithOp::Add => a.wrapping_add(b),
            ArithOp::Sub => a.wrapping_sub(b),
            ArithOp::Mul => a.wrapping_mul(b),
            ArithOp::Div => {
                if b == 0 {
                    return Err(CilError::DivideByZero);
                }
                a.wrapping_div(b)
            }
            ArithOp::Rem => {
                if b == 0 {
                    return Err(CilError::DivideByZero);
                }
                a.wrapping_rem(b)
            }
        }));
    }

    // Try i64 path
    if let (Some(a), Some(b)) = (v1.to_i64(), v2.to_i64()) {
        return Ok(ClrValue::I8(match op {
            ArithOp::Add => a.wrapping_add(b),
            ArithOp::Sub => a.wrapping_sub(b),
            ArithOp::Mul => a.wrapping_mul(b),
            ArithOp::Div => {
                if b == 0 {
                    return Err(CilError::DivideByZero);
                }
                a.wrapping_div(b)
            }
            ArithOp::Rem => {
                if b == 0 {
                    return Err(CilError::DivideByZero);
                }
                a.wrapping_rem(b)
            }
        }));
    }

    // Try f64 path
    if let (Some(a), Some(b)) = (v1.to_f64(), v2.to_f64()) {
        return Ok(ClrValue::R8(match op {
            ArithOp::Add => a + b,
            ArithOp::Sub => a - b,
            ArithOp::Mul => a * b,
            ArithOp::Div => a / b,
            ArithOp::Rem => a % b,
        }));
    }

    Ok(ClrValue::I4(0))
}

/// Bitwise operation kind.
enum BitwiseOp {
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

/// Perform a bitwise operation on two CIL values.
fn bitwise_op(v1: &ClrValue, v2: &ClrValue, op: BitwiseOp) -> Result<ClrValue, CilError> {
    let a = v1.to_i64().unwrap_or(0);
    let b = v2.to_i64().unwrap_or(0);

    let result = match op {
        BitwiseOp::And => a & b,
        BitwiseOp::Or => a | b,
        BitwiseOp::Xor => a ^ b,
        BitwiseOp::Shl => a.wrapping_shl(b as u32),
        BitwiseOp::Shr => a.wrapping_shr(b as u32),
    };

    // Return i32 if both inputs were i32
    if v1.to_i32().is_some() && v2.to_i32().is_some() {
        Ok(ClrValue::I4(result as i32))
    } else {
        Ok(ClrValue::I8(result))
    }
}

/// Compare two CIL values based on a branch opcode.
fn compare_values(v1: &ClrValue, v2: &ClrValue, opcode: CilOpcode) -> bool {
    let a = v1.to_i64().unwrap_or(0);
    let b = v2.to_i64().unwrap_or(0);

    match opcode {
        CilOpcode::BeqS | CilOpcode::Beq => a == b,
        CilOpcode::BgeS | CilOpcode::Bge | CilOpcode::BgeUnS | CilOpcode::BgeUn => a >= b,
        CilOpcode::BgtS | CilOpcode::Bgt | CilOpcode::BgtUnS | CilOpcode::BgtUn => a > b,
        CilOpcode::BleS | CilOpcode::Ble | CilOpcode::BleUnS | CilOpcode::BleUn => a <= b,
        CilOpcode::BltS | CilOpcode::Blt | CilOpcode::BltUnS | CilOpcode::BltUn => a < b,
        CilOpcode::BneUnS | CilOpcode::BneUn => a != b,
        _ => false,
    }
}
