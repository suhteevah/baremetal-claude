//! Go runtime basics: goroutine scheduler (simplified cooperative), channel
//! send/recv with select, defer/panic/recover, make(), new(), append(),
//! len(), cap(), copy(), delete(), close().

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use spin::Mutex;

/// Runtime value representation for the interpreter path.
#[derive(Debug, Clone)]
pub enum GoValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Rune(char),
    Slice(GoSlice),
    Array(Vec<GoValue>),
    Map(BTreeMap<String, GoValue>),
    Chan(usize), // index into channel registry
    Struct(BTreeMap<String, GoValue>),
    Pointer(Box<GoValue>),
    Func(String), // function name
    Interface { type_id: u64, data: Box<GoValue> },
}

impl GoValue {
    pub fn to_string_repr(&self) -> String {
        match self {
            GoValue::Nil => String::from("<nil>"),
            GoValue::Bool(b) => alloc::format!("{}", b),
            GoValue::Int(n) => alloc::format!("{}", n),
            GoValue::Float(f) => alloc::format!("{}", f),
            GoValue::String(s) => s.clone(),
            GoValue::Rune(c) => alloc::format!("{}", c),
            GoValue::Slice(s) => {
                let parts: Vec<String> = s.data.iter().map(|v| v.to_string_repr()).collect();
                alloc::format!("[{}]", parts.join(" "))
            }
            GoValue::Array(a) => {
                let parts: Vec<String> = a.iter().map(|v| v.to_string_repr()).collect();
                alloc::format!("[{}]", parts.join(" "))
            }
            GoValue::Map(m) => {
                let parts: Vec<String> = m.iter()
                    .map(|(k, v)| alloc::format!("{}:{}", k, v.to_string_repr()))
                    .collect();
                alloc::format!("map[{}]", parts.join(" "))
            }
            GoValue::Struct(fields) => {
                let parts: Vec<String> = fields.iter()
                    .map(|(k, v)| alloc::format!("{}:{}", k, v.to_string_repr()))
                    .collect();
                alloc::format!("{{{}}}", parts.join(" "))
            }
            GoValue::Pointer(inner) => alloc::format!("&{}", inner.to_string_repr()),
            GoValue::Func(name) => alloc::format!("func {}", name),
            GoValue::Interface { type_id, data } => {
                alloc::format!("<interface type={} data={}>", type_id, data.to_string_repr())
            }
            GoValue::Chan(id) => alloc::format!("chan({})", id),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            GoValue::Nil => false,
            GoValue::Bool(b) => *b,
            GoValue::Int(n) => *n != 0,
            GoValue::Float(f) => *f != 0.0,
            GoValue::String(s) => !s.is_empty(),
            _ => true,
        }
    }
}

/// Go slice: (data, len, cap).
#[derive(Debug, Clone)]
pub struct GoSlice {
    pub data: Vec<GoValue>,
    pub len: usize,
    pub cap: usize,
}

impl GoSlice {
    pub fn new() -> Self {
        Self { data: Vec::new(), len: 0, cap: 0 }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            data: Vec::with_capacity(cap),
            len: 0,
            cap,
        }
    }

    pub fn append(&mut self, val: GoValue) {
        if self.len >= self.cap {
            let new_cap = if self.cap == 0 { 1 } else { self.cap * 2 };
            self.cap = new_cap;
            self.data.reserve(new_cap - self.data.len());
        }
        self.data.push(val);
        self.len += 1;
    }
}

/// Simplified channel: buffered queue with mutex.
pub struct Channel {
    buffer: Mutex<VecDeque<GoValue>>,
    capacity: usize,
    closed: Mutex<bool>,
}

impl Channel {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
            capacity,
            closed: Mutex::new(false),
        }
    }

    pub fn send(&self, val: GoValue) -> Result<(), String> {
        if *self.closed.lock() {
            return Err(String::from("send on closed channel"));
        }
        let mut buf = self.buffer.lock();
        if self.capacity > 0 && buf.len() >= self.capacity {
            return Err(String::from("channel buffer full"));
        }
        buf.push_back(val);
        Ok(())
    }

    pub fn recv(&self) -> Result<(GoValue, bool), String> {
        let mut buf = self.buffer.lock();
        if let Some(val) = buf.pop_front() {
            Ok((val, true))
        } else if *self.closed.lock() {
            Ok((GoValue::Nil, false))
        } else {
            Err(String::from("channel empty"))
        }
    }

    pub fn close(&self) {
        *self.closed.lock() = true;
    }
}

/// Defer stack for a goroutine.
pub struct DeferStack {
    deferred: Vec<Box<dyn FnOnce()>>,
}

impl DeferStack {
    pub fn new() -> Self {
        Self { deferred: Vec::new() }
    }

    pub fn push(&mut self, f: Box<dyn FnOnce()>) {
        self.deferred.push(f);
    }

    pub fn run_all(&mut self) {
        while let Some(f) = self.deferred.pop() {
            f();
        }
    }
}

/// Go runtime builtins.
pub struct Runtime {
    channels: Vec<Channel>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// make(chan T, capacity)
    pub fn make_chan(&mut self, capacity: usize) -> usize {
        let id = self.channels.len();
        self.channels.push(Channel::new(capacity));
        id
    }

    /// make([]T, len, cap)
    pub fn make_slice(len: usize, cap: usize) -> GoSlice {
        let mut s = GoSlice::with_capacity(cap);
        for _ in 0..len {
            s.data.push(GoValue::Nil);
        }
        s.len = len;
        s
    }

    /// new(T) -> *T
    pub fn go_new(zero: GoValue) -> GoValue {
        GoValue::Pointer(Box::new(zero))
    }

    /// len() builtin
    pub fn go_len(val: &GoValue) -> i64 {
        match val {
            GoValue::String(s) => s.len() as i64,
            GoValue::Slice(s) => s.len as i64,
            GoValue::Array(a) => a.len() as i64,
            GoValue::Map(m) => m.len() as i64,
            _ => 0,
        }
    }

    /// cap() builtin
    pub fn go_cap(val: &GoValue) -> i64 {
        match val {
            GoValue::Slice(s) => s.cap as i64,
            GoValue::Array(a) => a.len() as i64,
            _ => 0,
        }
    }

    /// append(slice, elems...) -> slice
    pub fn go_append(slice: &GoValue, elems: &[GoValue]) -> GoValue {
        if let GoValue::Slice(s) = slice {
            let mut new_slice = s.clone();
            for e in elems {
                new_slice.append(e.clone());
            }
            GoValue::Slice(new_slice)
        } else {
            GoValue::Nil
        }
    }

    /// copy(dst, src) -> int
    pub fn go_copy(dst: &mut GoValue, src: &GoValue) -> i64 {
        if let (GoValue::Slice(d), GoValue::Slice(s)) = (dst, src) {
            let n = d.len.min(s.len);
            for i in 0..n {
                if i < s.data.len() {
                    if i < d.data.len() {
                        d.data[i] = s.data[i].clone();
                    }
                }
            }
            n as i64
        } else {
            0
        }
    }

    /// delete(map, key)
    pub fn go_delete(m: &mut GoValue, key: &str) {
        if let GoValue::Map(map) = m {
            map.remove(key);
        }
    }

    /// close(chan)
    pub fn go_close(&self, chan_id: usize) {
        if let Some(ch) = self.channels.get(chan_id) {
            ch.close();
        }
    }

    /// Channel send
    pub fn chan_send(&self, chan_id: usize, val: GoValue) -> Result<(), String> {
        self.channels.get(chan_id)
            .ok_or_else(|| String::from("invalid channel"))?
            .send(val)
    }

    /// Channel receive
    pub fn chan_recv(&self, chan_id: usize) -> Result<(GoValue, bool), String> {
        self.channels.get(chan_id)
            .ok_or_else(|| String::from("invalid channel"))?
            .recv()
    }
}
