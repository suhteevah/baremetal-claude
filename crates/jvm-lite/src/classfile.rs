//! Java .class file parser (JVM spec, Ch. 4).
//!
//! Parses the binary `.class` file format into structured Rust types.
//! The class file layout is:
//!
//! ```text
//! ClassFile {
//!     magic: 0xCAFEBABE          // 4 bytes — identifies this as a Java class
//!     minor_version, major_version  // 2+2 bytes (e.g., major=52 for Java 8)
//!     constant_pool_count        // 2 bytes
//!     constant_pool[]            // variable — see CpEntry variants
//!     access_flags               // 2 bytes (public, final, abstract, etc.)
//!     this_class, super_class    // 2+2 bytes — indices into constant pool
//!     interfaces[]               // count + indices
//!     fields[]                   // count + FieldInfo structs
//!     methods[]                  // count + MethodInfo structs
//!     attributes[]               // count + generic AttributeInfo
//! }
//! ```
//!
//! ## Constant Pool
//!
//! The constant pool is a 1-indexed array of tagged entries. Key entry types:
//! - **Utf8** (tag 1): Raw string data (method names, descriptors, etc.)
//! - **Class** (tag 7): Points to a Utf8 entry with the class name
//! - **Methodref** (tag 10): Points to a Class and a NameAndType
//! - **NameAndType** (tag 12): Points to name (Utf8) and descriptor (Utf8)
//! - **Long/Double** (tags 5/6): Occupy TWO constant pool slots (JVM spec quirk)
//!
//! ## Code Attribute
//!
//! The Code attribute (found on methods) contains the actual bytecode plus:
//! - `max_stack`: Maximum operand stack depth
//! - `max_locals`: Number of local variable slots
//! - `exception_table[]`: Try/catch ranges with handler offsets

use alloc::string::String;
use alloc::vec::Vec;

/// A parsed Java .class file.
#[derive(Debug, Clone)]
pub struct ClassFile {
    pub minor_version: u16,
    pub major_version: u16,
    pub constant_pool: Vec<CpEntry>,
    pub access_flags: u16,
    pub this_class: u16,
    pub super_class: u16,
    pub interfaces: Vec<u16>,
    pub fields: Vec<FieldInfo>,
    pub methods: Vec<MethodInfo>,
    pub attributes: Vec<AttributeInfo>,
}

/// Constant pool entry.
#[derive(Debug, Clone)]
pub enum CpEntry {
    Empty, // index 0 is unused
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(u16),          // name_index
    String(u16),         // string_index
    Fieldref(u16, u16),  // class_index, name_and_type_index
    Methodref(u16, u16), // class_index, name_and_type_index
    InterfaceMethodref(u16, u16),
    NameAndType(u16, u16), // name_index, descriptor_index
    MethodHandle(u8, u16),
    MethodType(u16),
    InvokeDynamic(u16, u16),
    /// Placeholder for long/double second slot
    LongDouble2ndSlot,
}

/// Field info.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub access_flags: u16,
    pub name_index: u16,
    pub descriptor_index: u16,
    pub attributes: Vec<AttributeInfo>,
}

/// Method info.
#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub access_flags: u16,
    pub name_index: u16,
    pub descriptor_index: u16,
    pub attributes: Vec<AttributeInfo>,
}

/// Attribute info (generic).
#[derive(Debug, Clone)]
pub struct AttributeInfo {
    pub name_index: u16,
    pub data: Vec<u8>,
}

/// Code attribute (parsed from AttributeInfo).
#[derive(Debug, Clone)]
pub struct CodeAttribute {
    pub max_stack: u16,
    pub max_locals: u16,
    pub code: Vec<u8>,
    pub exception_table: Vec<ExceptionEntry>,
    pub attributes: Vec<AttributeInfo>,
}

/// Exception table entry.
#[derive(Debug, Clone)]
pub struct ExceptionEntry {
    pub start_pc: u16,
    pub end_pc: u16,
    pub handler_pc: u16,
    pub catch_type: u16, // 0 = finally
}

/// Access flag constants.
pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_PRIVATE: u16 = 0x0002;
pub const ACC_PROTECTED: u16 = 0x0004;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_FINAL: u16 = 0x0010;
pub const ACC_SUPER: u16 = 0x0020;
pub const ACC_SYNCHRONIZED: u16 = 0x0020;
pub const ACC_VOLATILE: u16 = 0x0040;
pub const ACC_TRANSIENT: u16 = 0x0080;
pub const ACC_NATIVE: u16 = 0x0100;
pub const ACC_INTERFACE: u16 = 0x0200;
pub const ACC_ABSTRACT: u16 = 0x0400;

/// Parse a .class file from bytes.
pub fn parse_class(data: &[u8]) -> Result<ClassFile, String> {
    let mut reader = ClassReader::new(data);
    reader.parse()
}

struct ClassReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ClassReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err(String::from("unexpected end of class file"));
        }
        let val = self.data[self.pos];
        self.pos += 1;
        Ok(val)
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let hi = self.read_u8()? as u16;
        let lo = self.read_u8()? as u16;
        Ok((hi << 8) | lo)
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let a = self.read_u8()? as u32;
        let b = self.read_u8()? as u32;
        let c = self.read_u8()? as u32;
        let d = self.read_u8()? as u32;
        Ok((a << 24) | (b << 16) | (c << 8) | d)
    }

    fn read_i32(&mut self) -> Result<i32, String> {
        Ok(self.read_u32()? as i32)
    }

    fn read_i64(&mut self) -> Result<i64, String> {
        let hi = self.read_u32()? as i64;
        let lo = self.read_u32()? as i64;
        Ok((hi << 32) | lo)
    }

    fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, String> {
        if self.pos + n > self.data.len() {
            return Err(String::from("unexpected end of class file"));
        }
        let bytes = self.data[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(bytes)
    }

    fn parse(&mut self) -> Result<ClassFile, String> {
        // Magic
        let magic = self.read_u32()?;
        if magic != 0xCAFEBABE {
            return Err(alloc::format!("invalid magic: 0x{:08X}", magic));
        }

        let minor_version = self.read_u16()?;
        let major_version = self.read_u16()?;

        // Constant pool
        let cp_count = self.read_u16()?;
        let mut constant_pool = Vec::with_capacity(cp_count as usize);
        constant_pool.push(CpEntry::Empty); // index 0

        let mut i = 1;
        while i < cp_count {
            let tag = self.read_u8()?;
            let entry = match tag {
                1 => { // Utf8
                    let len = self.read_u16()? as usize;
                    let bytes = self.read_bytes(len)?;
                    let s = String::from_utf8(bytes).map_err(|_| String::from("invalid utf8"))?;
                    CpEntry::Utf8(s)
                }
                3 => CpEntry::Integer(self.read_i32()?),
                4 => CpEntry::Float(f32::from_bits(self.read_u32()?)),
                5 => {
                    let val = self.read_i64()?;
                    constant_pool.push(CpEntry::Long(val));
                    i += 1;
                    constant_pool.push(CpEntry::LongDouble2ndSlot);
                    i += 1;
                    continue;
                }
                6 => {
                    let val = f64::from_bits(self.read_i64()? as u64);
                    constant_pool.push(CpEntry::Double(val));
                    i += 1;
                    constant_pool.push(CpEntry::LongDouble2ndSlot);
                    i += 1;
                    continue;
                }
                7 => CpEntry::Class(self.read_u16()?),
                8 => CpEntry::String(self.read_u16()?),
                9 => CpEntry::Fieldref(self.read_u16()?, self.read_u16()?),
                10 => CpEntry::Methodref(self.read_u16()?, self.read_u16()?),
                11 => CpEntry::InterfaceMethodref(self.read_u16()?, self.read_u16()?),
                12 => CpEntry::NameAndType(self.read_u16()?, self.read_u16()?),
                15 => CpEntry::MethodHandle(self.read_u8()?, self.read_u16()?),
                16 => CpEntry::MethodType(self.read_u16()?),
                18 => CpEntry::InvokeDynamic(self.read_u16()?, self.read_u16()?),
                _ => return Err(alloc::format!("unknown constant pool tag: {}", tag)),
            };
            constant_pool.push(entry);
            i += 1;
        }

        let access_flags = self.read_u16()?;
        let this_class = self.read_u16()?;
        let super_class = self.read_u16()?;

        // Interfaces
        let iface_count = self.read_u16()?;
        let mut interfaces = Vec::with_capacity(iface_count as usize);
        for _ in 0..iface_count {
            interfaces.push(self.read_u16()?);
        }

        // Fields
        let field_count = self.read_u16()?;
        let mut fields = Vec::with_capacity(field_count as usize);
        for _ in 0..field_count {
            fields.push(self.parse_field()?);
        }

        // Methods
        let method_count = self.read_u16()?;
        let mut methods = Vec::with_capacity(method_count as usize);
        for _ in 0..method_count {
            methods.push(self.parse_method()?);
        }

        // Attributes
        let attr_count = self.read_u16()?;
        let mut attributes = Vec::with_capacity(attr_count as usize);
        for _ in 0..attr_count {
            attributes.push(self.parse_attribute()?);
        }

        Ok(ClassFile {
            minor_version,
            major_version,
            constant_pool,
            access_flags,
            this_class,
            super_class,
            interfaces,
            fields,
            methods,
            attributes,
        })
    }

    fn parse_field(&mut self) -> Result<FieldInfo, String> {
        let access_flags = self.read_u16()?;
        let name_index = self.read_u16()?;
        let descriptor_index = self.read_u16()?;
        let attr_count = self.read_u16()?;
        let mut attributes = Vec::new();
        for _ in 0..attr_count {
            attributes.push(self.parse_attribute()?);
        }
        Ok(FieldInfo { access_flags, name_index, descriptor_index, attributes })
    }

    fn parse_method(&mut self) -> Result<MethodInfo, String> {
        let access_flags = self.read_u16()?;
        let name_index = self.read_u16()?;
        let descriptor_index = self.read_u16()?;
        let attr_count = self.read_u16()?;
        let mut attributes = Vec::new();
        for _ in 0..attr_count {
            attributes.push(self.parse_attribute()?);
        }
        Ok(MethodInfo { access_flags, name_index, descriptor_index, attributes })
    }

    fn parse_attribute(&mut self) -> Result<AttributeInfo, String> {
        let name_index = self.read_u16()?;
        let length = self.read_u32()? as usize;
        let data = self.read_bytes(length)?;
        Ok(AttributeInfo { name_index, data })
    }
}

impl ClassFile {
    /// Look up a UTF-8 string from the constant pool.
    pub fn get_utf8(&self, index: u16) -> Option<&str> {
        match self.constant_pool.get(index as usize) {
            Some(CpEntry::Utf8(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get the class name.
    pub fn class_name(&self) -> Option<&str> {
        if let Some(CpEntry::Class(name_idx)) = self.constant_pool.get(self.this_class as usize) {
            self.get_utf8(*name_idx)
        } else {
            None
        }
    }

    /// Get the super class name.
    pub fn super_class_name(&self) -> Option<&str> {
        if self.super_class == 0 {
            return None;
        }
        if let Some(CpEntry::Class(name_idx)) = self.constant_pool.get(self.super_class as usize) {
            self.get_utf8(*name_idx)
        } else {
            None
        }
    }

    /// Parse a Code attribute from raw attribute data.
    pub fn parse_code_attribute(data: &[u8]) -> Result<CodeAttribute, String> {
        if data.len() < 8 {
            return Err(String::from("Code attribute too short"));
        }
        let max_stack = ((data[0] as u16) << 8) | data[1] as u16;
        let max_locals = ((data[2] as u16) << 8) | data[3] as u16;
        let code_len = ((data[4] as u32) << 24)
            | ((data[5] as u32) << 16)
            | ((data[6] as u32) << 8)
            | data[7] as u32;

        let code_start = 8;
        let code_end = code_start + code_len as usize;
        if code_end > data.len() {
            return Err(String::from("Code attribute truncated"));
        }
        let code = data[code_start..code_end].to_vec();

        let mut pos = code_end;
        let exc_count = if pos + 2 <= data.len() {
            let c = ((data[pos] as u16) << 8) | data[pos + 1] as u16;
            pos += 2;
            c
        } else {
            0
        };

        let mut exception_table = Vec::new();
        for _ in 0..exc_count {
            if pos + 8 > data.len() { break; }
            exception_table.push(ExceptionEntry {
                start_pc: ((data[pos] as u16) << 8) | data[pos + 1] as u16,
                end_pc: ((data[pos + 2] as u16) << 8) | data[pos + 3] as u16,
                handler_pc: ((data[pos + 4] as u16) << 8) | data[pos + 5] as u16,
                catch_type: ((data[pos + 6] as u16) << 8) | data[pos + 7] as u16,
            });
            pos += 8;
        }

        Ok(CodeAttribute {
            max_stack,
            max_locals,
            code,
            exception_table,
            attributes: Vec::new(),
        })
    }
}
