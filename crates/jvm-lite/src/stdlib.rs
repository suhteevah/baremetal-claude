//! Java standard library stubs.
//!
//! java.lang.Object, java.lang.String, java.lang.System, java.lang.Math,
//! java.lang.Integer, java.util.ArrayList, java.util.HashMap, java.io basics.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::gc::{JvmHeap, JvmObject};
use crate::vm::JvmValue;

/// Handle a stdlib method call. Returns Some(value) if it was handled.
pub fn handle_stdlib_call(
    class: &str,
    method: &str,
    stack: &mut Vec<JvmValue>,
    output: &mut String,
    heap: &mut JvmHeap,
) -> Option<JvmValue> {
    match (class, method) {
        // === java.io.PrintStream (System.out) ===
        ("java.io.PrintStream", "println") | ("java.lang.System", "println") => {
            let val = stack.pop().unwrap_or(JvmValue::Null);
            let s = jvm_value_to_string(&val, heap);
            output.push_str(&s);
            output.push('\n');
            None
        }
        ("java.io.PrintStream", "print") | ("java.lang.System", "print") => {
            let val = stack.pop().unwrap_or(JvmValue::Null);
            let s = jvm_value_to_string(&val, heap);
            output.push_str(&s);
            None
        }

        // === java.lang.System ===
        ("java.lang.System", "currentTimeMillis") => {
            Some(JvmValue::Long(0)) // no real clock in bare metal stub
        }
        ("java.lang.System", "exit") => {
            let code = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            log::info!("[jvm] System.exit({})", code);
            None
        }
        ("java.lang.System", "arraycopy") => {
            // arraycopy(src, srcPos, dest, destPos, length) — simplified stub
            for _ in 0..5 { stack.pop(); }
            None
        }

        // === java.lang.String ===
        ("java.lang.String", "length") => {
            let str_ref = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(idx) = str_ref {
                if let Some(obj) = heap.get(idx) {
                    if let Some(ref s) = obj.string_value {
                        return Some(JvmValue::Int(s.len() as i32));
                    }
                }
            }
            Some(JvmValue::Int(0))
        }
        ("java.lang.String", "charAt") => {
            let idx = stack.pop().unwrap_or(JvmValue::Int(0)).as_int() as usize;
            let str_ref = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = str_ref {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        if let Some(ch) = s.chars().nth(idx) {
                            return Some(JvmValue::Int(ch as i32));
                        }
                    }
                }
            }
            Some(JvmValue::Int(0))
        }
        ("java.lang.String", "substring") => {
            // Two-arg version: substring(beginIndex, endIndex)
            let end = stack.pop().unwrap_or(JvmValue::Int(0)).as_int() as usize;
            let begin = stack.pop().unwrap_or(JvmValue::Int(0)).as_int() as usize;
            let str_ref = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = str_ref {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        let sub: String = s.chars().skip(begin).take(end - begin).collect();
                        let new_obj = JvmObject::new_string(sub);
                        let new_ref = heap.allocate_object(new_obj);
                        return Some(JvmValue::Ref(new_ref));
                    }
                }
            }
            Some(JvmValue::Null)
        }
        ("java.lang.String", "equals") => {
            let other = stack.pop().unwrap_or(JvmValue::Null);
            let this = stack.pop().unwrap_or(JvmValue::Null);
            let result = match (&this, &other) {
                (JvmValue::Ref(a), JvmValue::Ref(b)) => {
                    let sa = heap.get(*a).and_then(|o| o.string_value.as_ref());
                    let sb = heap.get(*b).and_then(|o| o.string_value.as_ref());
                    sa == sb
                }
                _ => false,
            };
            Some(JvmValue::Int(if result { 1 } else { 0 }))
        }
        ("java.lang.String", "concat") => {
            let other = stack.pop().unwrap_or(JvmValue::Null);
            let this = stack.pop().unwrap_or(JvmValue::Null);
            let mut result = String::new();
            if let JvmValue::Ref(r) = this {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        result.push_str(s);
                    }
                }
            }
            if let JvmValue::Ref(r) = other {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        result.push_str(s);
                    }
                }
            }
            let new_obj = JvmObject::new_string(result);
            let new_ref = heap.allocate_object(new_obj);
            Some(JvmValue::Ref(new_ref))
        }
        ("java.lang.String", "toUpperCase") => {
            let this = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = this {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        let upper = s.to_ascii_uppercase();
                        let new_obj = JvmObject::new_string(upper);
                        let new_ref = heap.allocate_object(new_obj);
                        return Some(JvmValue::Ref(new_ref));
                    }
                }
            }
            Some(JvmValue::Null)
        }
        ("java.lang.String", "toLowerCase") => {
            let this = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = this {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        let lower = s.to_ascii_lowercase();
                        let new_obj = JvmObject::new_string(lower);
                        let new_ref = heap.allocate_object(new_obj);
                        return Some(JvmValue::Ref(new_ref));
                    }
                }
            }
            Some(JvmValue::Null)
        }
        ("java.lang.String", "trim") => {
            let this = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = this {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        let trimmed = String::from(s.trim());
                        let new_obj = JvmObject::new_string(trimmed);
                        let new_ref = heap.allocate_object(new_obj);
                        return Some(JvmValue::Ref(new_ref));
                    }
                }
            }
            Some(JvmValue::Null)
        }
        ("java.lang.String", "contains") => {
            let sub = stack.pop().unwrap_or(JvmValue::Null);
            let this = stack.pop().unwrap_or(JvmValue::Null);
            let result = match (&this, &sub) {
                (JvmValue::Ref(a), JvmValue::Ref(b)) => {
                    let sa = heap.get(*a).and_then(|o| o.string_value.as_ref());
                    let sb = heap.get(*b).and_then(|o| o.string_value.as_ref());
                    match (sa, sb) {
                        (Some(s), Some(sub)) => s.contains(sub.as_str()),
                        _ => false,
                    }
                }
                _ => false,
            };
            Some(JvmValue::Int(if result { 1 } else { 0 }))
        }
        ("java.lang.String", "isEmpty") => {
            let this = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = this {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        return Some(JvmValue::Int(if s.is_empty() { 1 } else { 0 }));
                    }
                }
            }
            Some(JvmValue::Int(1))
        }

        // === java.lang.Integer ===
        ("java.lang.Integer", "parseInt") => {
            let str_ref = stack.pop().unwrap_or(JvmValue::Null);
            if let JvmValue::Ref(r) = str_ref {
                if let Some(obj) = heap.get(r) {
                    if let Some(ref s) = obj.string_value {
                        if let Ok(n) = s.parse::<i32>() {
                            return Some(JvmValue::Int(n));
                        }
                    }
                }
            }
            Some(JvmValue::Int(0))
        }
        ("java.lang.Integer", "toString") => {
            let val = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            let s = format!("{}", val);
            let obj = JvmObject::new_string(s);
            let r = heap.allocate_object(obj);
            Some(JvmValue::Ref(r))
        }

        // === java.lang.Math ===
        ("java.lang.Math", "abs") => {
            let val = stack.pop().unwrap_or(JvmValue::Int(0));
            match val {
                JvmValue::Int(n) => Some(JvmValue::Int(n.abs())),
                JvmValue::Long(n) => Some(JvmValue::Long(n.abs())),
                JvmValue::Float(n) => Some(JvmValue::Float(if n < 0.0 { -n } else { n })),
                JvmValue::Double(n) => Some(JvmValue::Double(if n < 0.0 { -n } else { n })),
                _ => Some(JvmValue::Int(0)),
            }
        }
        ("java.lang.Math", "max") => {
            let b = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            let a = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            Some(JvmValue::Int(if a > b { a } else { b }))
        }
        ("java.lang.Math", "min") => {
            let b = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            let a = stack.pop().unwrap_or(JvmValue::Int(0)).as_int();
            Some(JvmValue::Int(if a < b { a } else { b }))
        }

        _ => {
            log::warn!("[jvm] unknown stdlib call: {}.{}", class, method);
            None
        }
    }
}

/// Convert a JVM value to a display string.
fn jvm_value_to_string(val: &JvmValue, heap: &JvmHeap) -> String {
    match val {
        JvmValue::Int(n) => format!("{}", n),
        JvmValue::Long(n) => format!("{}", n),
        JvmValue::Float(f) => format!("{}", f),
        JvmValue::Double(f) => format!("{}", f),
        JvmValue::Ref(r) => {
            if let Some(obj) = heap.get(*r) {
                if let Some(ref s) = obj.string_value {
                    return s.clone();
                }
                format!("{}@{:x}", obj.class_name, r)
            } else {
                String::from("null")
            }
        }
        JvmValue::Null => String::from("null"),
    }
}
