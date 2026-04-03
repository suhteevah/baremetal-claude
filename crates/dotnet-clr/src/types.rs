//! .NET type system (ECMA-335 I.8).
//!
//! Implements the fundamental .NET type hierarchy: System.Object at the root,
//! value types vs reference types, boxing/unboxing, and the built-in primitive
//! types (Int32, Int64, Boolean, Char, Single, Double, String, etc.).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// .NET type categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCategory {
    /// Value types live on the stack/inline. Includes primitives and structs.
    ValueType,
    /// Reference types live on the managed heap. Includes classes, arrays, strings.
    ReferenceType,
}

/// Built-in .NET element types (ECMA-335 II.23.1.16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ElementType {
    Void = 0x01,
    Boolean = 0x02,
    Char = 0x03,
    I1 = 0x04,
    U1 = 0x05,
    I2 = 0x06,
    U2 = 0x07,
    I4 = 0x08,
    U4 = 0x09,
    I8 = 0x0A,
    U8 = 0x0B,
    R4 = 0x0C,
    R8 = 0x0D,
    String = 0x0E,
    Ptr = 0x0F,
    ByRef = 0x10,
    ValueType = 0x11,
    Class = 0x12,
    Var = 0x13,
    Array = 0x14,
    GenericInst = 0x15,
    TypedByRef = 0x16,
    IntPtr = 0x18,
    UIntPtr = 0x19,
    FnPtr = 0x1B,
    Object = 0x1C,
    SzArray = 0x1D,
    MVar = 0x1E,
}

/// A .NET type descriptor.
#[derive(Debug, Clone)]
pub struct TypeDescriptor {
    /// Fully qualified type name (e.g., "System.Int32").
    pub full_name: String,
    /// Namespace (e.g., "System").
    pub namespace: String,
    /// Short name (e.g., "Int32").
    pub name: String,
    /// Category (value or reference type).
    pub category: TypeCategory,
    /// Element type for primitives, or None for user-defined types.
    pub element_type: Option<ElementType>,
    /// Base type name (e.g., "System.Object" or "System.ValueType").
    pub base_type: Option<String>,
    /// Fields defined on this type.
    pub fields: Vec<FieldDescriptor>,
    /// Methods defined on this type.
    pub methods: Vec<MethodDescriptor>,
    /// Interfaces implemented by this type.
    pub interfaces: Vec<String>,
    /// Size in bytes for value types (0 for reference types, managed by GC).
    pub instance_size: u32,
    /// Whether this type is sealed (cannot be inherited).
    pub is_sealed: bool,
    /// Whether this type is abstract.
    pub is_abstract: bool,
    /// Whether this type is an interface.
    pub is_interface: bool,
    /// Whether this type is an enum.
    pub is_enum: bool,
    /// Generic parameters (if any).
    pub generic_params: Vec<String>,
}

/// A field descriptor.
#[derive(Debug, Clone)]
pub struct FieldDescriptor {
    /// Field name.
    pub name: String,
    /// Field type name.
    pub field_type: String,
    /// Whether this is a static field.
    pub is_static: bool,
    /// Offset within the instance (for instance fields).
    pub offset: u32,
}

/// A method descriptor.
#[derive(Debug, Clone)]
pub struct MethodDescriptor {
    /// Method name.
    pub name: String,
    /// Return type name.
    pub return_type: String,
    /// Parameter types.
    pub param_types: Vec<String>,
    /// Parameter names.
    pub param_names: Vec<String>,
    /// Whether this is a static method.
    pub is_static: bool,
    /// Whether this is a virtual method.
    pub is_virtual: bool,
    /// Whether this is an abstract method.
    pub is_abstract: bool,
    /// RVA of the method body (CIL bytecode), 0 if abstract/extern.
    pub body_rva: u32,
}

/// A runtime .NET value on the evaluation stack or in locals/args.
#[derive(Debug, Clone)]
pub enum ClrValue {
    /// Null reference.
    Null,
    /// Boolean value.
    Bool(bool),
    /// 8-bit signed integer.
    I1(i8),
    /// 8-bit unsigned integer.
    U1(u8),
    /// 16-bit signed integer.
    I2(i16),
    /// 16-bit unsigned integer.
    U2(u16),
    /// 32-bit signed integer.
    I4(i32),
    /// 32-bit unsigned integer.
    U4(u32),
    /// 64-bit signed integer.
    I8(i64),
    /// 64-bit unsigned integer.
    U8(u64),
    /// 32-bit float.
    R4(f32),
    /// 64-bit float.
    R8(f64),
    /// Char (UTF-16 code unit).
    Char(char),
    /// Native integer (pointer-sized).
    IntPtr(isize),
    /// Native unsigned integer.
    UIntPtr(usize),
    /// Managed string.
    String(String),
    /// Reference to a managed heap object (GC handle).
    ObjectRef(u64),
    /// Boxed value type (stored on managed heap).
    Boxed(Box<ClrValue>),
    /// Array reference.
    ArrayRef(u64),
    /// Void (no value, used for return types).
    Void,
}

impl ClrValue {
    /// Get the element type of this value.
    pub fn element_type(&self) -> ElementType {
        match self {
            ClrValue::Null => ElementType::Object,
            ClrValue::Bool(_) => ElementType::Boolean,
            ClrValue::I1(_) => ElementType::I1,
            ClrValue::U1(_) => ElementType::U1,
            ClrValue::I2(_) => ElementType::I2,
            ClrValue::U2(_) => ElementType::U2,
            ClrValue::I4(_) => ElementType::I4,
            ClrValue::U4(_) => ElementType::U4,
            ClrValue::I8(_) => ElementType::I8,
            ClrValue::U8(_) => ElementType::U8,
            ClrValue::R4(_) => ElementType::R4,
            ClrValue::R8(_) => ElementType::R8,
            ClrValue::Char(_) => ElementType::Char,
            ClrValue::IntPtr(_) => ElementType::IntPtr,
            ClrValue::UIntPtr(_) => ElementType::UIntPtr,
            ClrValue::String(_) => ElementType::String,
            ClrValue::ObjectRef(_) => ElementType::Object,
            ClrValue::Boxed(_) => ElementType::Object,
            ClrValue::ArrayRef(_) => ElementType::SzArray,
            ClrValue::Void => ElementType::Void,
        }
    }

    /// Convert this value to an i32, if possible.
    pub fn to_i32(&self) -> Option<i32> {
        match self {
            ClrValue::Bool(b) => Some(if *b { 1 } else { 0 }),
            ClrValue::I1(v) => Some(*v as i32),
            ClrValue::U1(v) => Some(*v as i32),
            ClrValue::I2(v) => Some(*v as i32),
            ClrValue::U2(v) => Some(*v as i32),
            ClrValue::I4(v) => Some(*v),
            ClrValue::Char(c) => Some(*c as i32),
            _ => None,
        }
    }

    /// Convert this value to an i64, if possible.
    pub fn to_i64(&self) -> Option<i64> {
        match self {
            ClrValue::I4(v) => Some(*v as i64),
            ClrValue::U4(v) => Some(*v as i64),
            ClrValue::I8(v) => Some(*v),
            ClrValue::IntPtr(v) => Some(*v as i64),
            _ => self.to_i32().map(|v| v as i64),
        }
    }

    /// Convert this value to an f64, if possible.
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            ClrValue::R4(v) => Some(*v as f64),
            ClrValue::R8(v) => Some(*v),
            ClrValue::I4(v) => Some(*v as f64),
            ClrValue::I8(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// Convert this value to a bool.
    pub fn to_bool(&self) -> bool {
        match self {
            ClrValue::Null => false,
            ClrValue::Bool(b) => *b,
            ClrValue::I4(v) => *v != 0,
            ClrValue::I8(v) => *v != 0,
            ClrValue::ObjectRef(v) => *v != 0,
            ClrValue::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    /// Box a value type (move it to the managed heap representation).
    pub fn box_value(self) -> ClrValue {
        match self {
            ClrValue::ObjectRef(_) | ClrValue::String(_) | ClrValue::ArrayRef(_) | ClrValue::Null => {
                self // Reference types don't need boxing
            }
            other => ClrValue::Boxed(Box::new(other)),
        }
    }

    /// Unbox a boxed value.
    pub fn unbox_value(self) -> ClrValue {
        match self {
            ClrValue::Boxed(inner) => *inner,
            other => other,
        }
    }
}

/// A managed object on the .NET heap.
#[derive(Debug, Clone)]
pub struct ManagedObject {
    /// Type descriptor for this object.
    pub type_name: String,
    /// Field values (keyed by field name).
    pub fields: BTreeMap<String, ClrValue>,
    /// GC generation (0, 1, or 2).
    pub gc_generation: u8,
    /// Whether this object is marked (for mark-and-sweep GC).
    pub gc_marked: bool,
    /// Sync block index (for lock/Monitor support).
    pub sync_block: u32,
}

impl ManagedObject {
    /// Create a new managed object of the given type.
    pub fn new(type_name: String) -> Self {
        Self {
            type_name,
            fields: BTreeMap::new(),
            gc_generation: 0,
            gc_marked: false,
            sync_block: 0,
        }
    }

    /// Get a field value.
    pub fn get_field(&self, name: &str) -> Option<&ClrValue> {
        self.fields.get(name)
    }

    /// Set a field value.
    pub fn set_field(&mut self, name: String, value: ClrValue) {
        self.fields.insert(name, value);
    }
}

/// A managed array on the .NET heap.
#[derive(Debug, Clone)]
pub struct ManagedArray {
    /// Element type name.
    pub element_type: String,
    /// Array elements.
    pub elements: Vec<ClrValue>,
    /// GC generation.
    pub gc_generation: u8,
    /// Whether this array is marked (for GC).
    pub gc_marked: bool,
}

impl ManagedArray {
    /// Create a new managed array.
    pub fn new(element_type: String, length: usize) -> Self {
        let default = match element_type.as_str() {
            "System.Int32" => ClrValue::I4(0),
            "System.Int64" => ClrValue::I8(0),
            "System.Boolean" => ClrValue::Bool(false),
            "System.Single" => ClrValue::R4(0.0),
            "System.Double" => ClrValue::R8(0.0),
            "System.Char" => ClrValue::Char('\0'),
            "System.Byte" => ClrValue::U1(0),
            "System.String" => ClrValue::Null,
            _ => ClrValue::Null,
        };
        Self {
            element_type,
            elements: alloc::vec![default; length],
            gc_generation: 0,
            gc_marked: false,
        }
    }

    /// Get the length of the array.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Check if the array is empty.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Get an element by index.
    pub fn get(&self, index: usize) -> Option<&ClrValue> {
        self.elements.get(index)
    }

    /// Set an element by index.
    pub fn set(&mut self, index: usize, value: ClrValue) -> bool {
        if index < self.elements.len() {
            self.elements[index] = value;
            true
        } else {
            false
        }
    }
}

/// Global type registry for loaded types.
static TYPE_REGISTRY: Mutex<Option<TypeRegistry>> = Mutex::new(None);

/// Registry of all loaded .NET types.
pub struct TypeRegistry {
    types: BTreeMap<String, TypeDescriptor>,
}

impl TypeRegistry {
    /// Create a new type registry with built-in types pre-registered.
    pub fn new() -> Self {
        let mut reg = Self {
            types: BTreeMap::new(),
        };
        reg.register_builtin_types();
        reg
    }

    /// Register all built-in .NET types.
    fn register_builtin_types(&mut self) {
        // System.Object — root of all types
        self.register(TypeDescriptor {
            full_name: String::from("System.Object"),
            namespace: String::from("System"),
            name: String::from("Object"),
            category: TypeCategory::ReferenceType,
            element_type: Some(ElementType::Object),
            base_type: None,
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 16, // Object header (sync block + type handle)
            is_sealed: false,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.ValueType — base of all value types
        self.register(TypeDescriptor {
            full_name: String::from("System.ValueType"),
            namespace: String::from("System"),
            name: String::from("ValueType"),
            category: TypeCategory::ReferenceType, // ValueType itself is a class
            element_type: Some(ElementType::ValueType),
            base_type: Some(String::from("System.Object")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 16,
            is_sealed: false,
            is_abstract: true,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Boolean
        self.register(TypeDescriptor {
            full_name: String::from("System.Boolean"),
            namespace: String::from("System"),
            name: String::from("Boolean"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::Boolean),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 1,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Char
        self.register(TypeDescriptor {
            full_name: String::from("System.Char"),
            namespace: String::from("System"),
            name: String::from("Char"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::Char),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 2,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Int32
        self.register(TypeDescriptor {
            full_name: String::from("System.Int32"),
            namespace: String::from("System"),
            name: String::from("Int32"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::I4),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 4,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Int64
        self.register(TypeDescriptor {
            full_name: String::from("System.Int64"),
            namespace: String::from("System"),
            name: String::from("Int64"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::I8),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 8,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Single (float)
        self.register(TypeDescriptor {
            full_name: String::from("System.Single"),
            namespace: String::from("System"),
            name: String::from("Single"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::R4),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 4,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Double
        self.register(TypeDescriptor {
            full_name: String::from("System.Double"),
            namespace: String::from("System"),
            name: String::from("Double"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::R8),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 8,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Byte
        self.register(TypeDescriptor {
            full_name: String::from("System.Byte"),
            namespace: String::from("System"),
            name: String::from("Byte"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::U1),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 1,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.String — special reference type
        self.register(TypeDescriptor {
            full_name: String::from("System.String"),
            namespace: String::from("System"),
            name: String::from("String"),
            category: TypeCategory::ReferenceType,
            element_type: Some(ElementType::String),
            base_type: Some(String::from("System.Object")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 0, // Variable size
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Array — base of all arrays
        self.register(TypeDescriptor {
            full_name: String::from("System.Array"),
            namespace: String::from("System"),
            name: String::from("Array"),
            category: TypeCategory::ReferenceType,
            element_type: Some(ElementType::Array),
            base_type: Some(String::from("System.Object")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 0,
            is_sealed: false,
            is_abstract: true,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.Enum — base of all enums
        self.register(TypeDescriptor {
            full_name: String::from("System.Enum"),
            namespace: String::from("System"),
            name: String::from("Enum"),
            category: TypeCategory::ReferenceType,
            element_type: None,
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 0,
            is_sealed: false,
            is_abstract: true,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });

        // System.IntPtr
        self.register(TypeDescriptor {
            full_name: String::from("System.IntPtr"),
            namespace: String::from("System"),
            name: String::from("IntPtr"),
            category: TypeCategory::ValueType,
            element_type: Some(ElementType::IntPtr),
            base_type: Some(String::from("System.ValueType")),
            fields: Vec::new(),
            methods: Vec::new(),
            interfaces: Vec::new(),
            instance_size: 8,
            is_sealed: true,
            is_abstract: false,
            is_interface: false,
            is_enum: false,
            generic_params: Vec::new(),
        });
    }

    /// Register a type in the registry.
    pub fn register(&mut self, desc: TypeDescriptor) {
        log::trace!("[dotnet-clr] Registered type: {}", desc.full_name);
        self.types.insert(desc.full_name.clone(), desc);
    }

    /// Look up a type by fully qualified name.
    pub fn lookup(&self, full_name: &str) -> Option<&TypeDescriptor> {
        self.types.get(full_name)
    }

    /// Get the number of registered types.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// Check if a type is a value type.
    pub fn is_value_type(&self, full_name: &str) -> bool {
        self.types
            .get(full_name)
            .is_some_and(|t| t.category == TypeCategory::ValueType)
    }

    /// Check if `derived` is assignable to `base` (including identity).
    pub fn is_assignable_to(&self, derived: &str, base: &str) -> bool {
        if derived == base {
            return true;
        }
        // Walk the inheritance chain
        let mut current = derived;
        loop {
            if let Some(td) = self.types.get(current) {
                if let Some(ref parent) = td.base_type {
                    if parent == base {
                        return true;
                    }
                    current = parent;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }
    }
}

/// Initialize the global type registry.
pub fn init() {
    let mut reg = TYPE_REGISTRY.lock();
    if reg.is_none() {
        *reg = Some(TypeRegistry::new());
        log::info!("[dotnet-clr] Type registry initialized with built-in types");
    }
}

/// Access the global type registry.
pub fn with_registry<F, R>(f: F) -> R
where
    F: FnOnce(&TypeRegistry) -> R,
{
    let reg = TYPE_REGISTRY.lock();
    f(reg.as_ref().expect("Type registry not initialized"))
}

/// Access the global type registry mutably.
pub fn with_registry_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut TypeRegistry) -> R,
{
    let mut reg = TYPE_REGISTRY.lock();
    f(reg.as_mut().expect("Type registry not initialized"))
}
