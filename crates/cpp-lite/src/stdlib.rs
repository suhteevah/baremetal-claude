//! Mini C++ STL: std::string, std::vector<T>, std::map<K,V>,
//! std::cout/cin, std::unique_ptr<T>, std::shared_ptr<T>.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Runtime value for the C++ interpreter/runtime.
#[derive(Debug, Clone)]
pub enum CppValue {
    Void,
    Bool(bool),
    Char(u8),
    Int(i64),
    Float(f64),
    Nullptr,
    String(CppString),
    Vector(CppVector),
    Map(CppMap),
    UniquePtr(Option<Box<CppValue>>),
    SharedPtr(Box<CppValue>, usize), // value, ref_count
    Object { class: String, fields: BTreeMap<String, CppValue> },
    Pointer(Box<CppValue>),
}

/// std::string implementation.
#[derive(Debug, Clone)]
pub struct CppString {
    pub data: String,
}

impl CppString {
    pub fn new() -> Self { Self { data: String::new() } }
    pub fn from(s: &str) -> Self { Self { data: String::from(s) } }
    pub fn length(&self) -> usize { self.data.len() }
    pub fn empty(&self) -> bool { self.data.is_empty() }
    pub fn push_back(&mut self, ch: char) { self.data.push(ch); }
    pub fn substr(&self, pos: usize, len: usize) -> Self {
        let s: String = self.data.chars().skip(pos).take(len).collect();
        Self { data: s }
    }
    pub fn find(&self, substr: &str) -> Option<usize> {
        self.data.find(substr)
    }
    pub fn append(&mut self, other: &str) { self.data.push_str(other); }
    pub fn c_str(&self) -> &str { &self.data }
}

/// std::vector<T> implementation.
#[derive(Debug, Clone)]
pub struct CppVector {
    pub elements: Vec<CppValue>,
}

impl CppVector {
    pub fn new() -> Self { Self { elements: Vec::new() } }
    pub fn with_capacity(cap: usize) -> Self {
        Self { elements: Vec::with_capacity(cap) }
    }
    pub fn push_back(&mut self, val: CppValue) { self.elements.push(val); }
    pub fn pop_back(&mut self) -> Option<CppValue> { self.elements.pop() }
    pub fn size(&self) -> usize { self.elements.len() }
    pub fn empty(&self) -> bool { self.elements.is_empty() }
    pub fn at(&self, idx: usize) -> Option<&CppValue> { self.elements.get(idx) }
    pub fn front(&self) -> Option<&CppValue> { self.elements.first() }
    pub fn back(&self) -> Option<&CppValue> { self.elements.last() }
    pub fn clear(&mut self) { self.elements.clear(); }
    pub fn capacity(&self) -> usize { self.elements.capacity() }
    pub fn reserve(&mut self, cap: usize) { self.elements.reserve(cap); }
}

/// std::map<K,V> implementation (using BTreeMap).
#[derive(Debug, Clone)]
pub struct CppMap {
    pub entries: BTreeMap<String, CppValue>,
}

impl CppMap {
    pub fn new() -> Self { Self { entries: BTreeMap::new() } }
    pub fn insert(&mut self, key: String, val: CppValue) { self.entries.insert(key, val); }
    pub fn find(&self, key: &str) -> Option<&CppValue> { self.entries.get(key) }
    pub fn erase(&mut self, key: &str) -> bool { self.entries.remove(key).is_some() }
    pub fn size(&self) -> usize { self.entries.len() }
    pub fn empty(&self) -> bool { self.entries.is_empty() }
    pub fn contains(&self, key: &str) -> bool { self.entries.contains_key(key) }
    pub fn clear(&mut self) { self.entries.clear(); }
}

/// Handle std::cout << operations.
pub fn cout_print(val: &CppValue, output: &mut String) {
    match val {
        CppValue::Int(n) => output.push_str(&alloc::format!("{}", n)),
        CppValue::Float(f) => output.push_str(&alloc::format!("{}", f)),
        CppValue::Bool(b) => output.push_str(if *b { "true" } else { "false" }),
        CppValue::Char(c) => output.push(*c as char),
        CppValue::String(s) => output.push_str(&s.data),
        CppValue::Nullptr => output.push_str("nullptr"),
        CppValue::Void => {}
        _ => output.push_str("[object]"),
    }
}

/// endl manipulator.
pub fn cout_endl(output: &mut String) {
    output.push('\n');
}

/// Handle std::unique_ptr operations.
impl CppValue {
    pub fn make_unique(val: CppValue) -> CppValue {
        CppValue::UniquePtr(Some(Box::new(val)))
    }

    pub fn make_shared(val: CppValue) -> CppValue {
        CppValue::SharedPtr(Box::new(val), 1)
    }

    pub fn to_display_string(&self) -> String {
        match self {
            CppValue::Void => String::from("void"),
            CppValue::Bool(b) => alloc::format!("{}", b),
            CppValue::Char(c) => alloc::format!("{}", *c as char),
            CppValue::Int(n) => alloc::format!("{}", n),
            CppValue::Float(f) => alloc::format!("{}", f),
            CppValue::Nullptr => String::from("nullptr"),
            CppValue::String(s) => s.data.clone(),
            CppValue::Vector(v) => alloc::format!("vector(size={})", v.size()),
            CppValue::Map(m) => alloc::format!("map(size={})", m.size()),
            CppValue::UniquePtr(Some(v)) => alloc::format!("unique_ptr({})", v.to_display_string()),
            CppValue::UniquePtr(None) => String::from("unique_ptr(null)"),
            CppValue::SharedPtr(v, rc) => alloc::format!("shared_ptr({}, rc={})", v.to_display_string(), rc),
            CppValue::Object { class, .. } => alloc::format!("{}{{...}}", class),
            CppValue::Pointer(v) => alloc::format!("*{}", v.to_display_string()),
        }
    }
}
