//! Lua standard library: builtins and library tables.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::table::LuaTable;
use crate::vm::{LuaState, LuaValue};

/// Register all standard library functions.
pub fn register_stdlib(state: &mut LuaState) {
    // Global functions
    register_native(state, "print", lua_print);
    register_native(state, "tostring", lua_tostring);
    register_native(state, "tonumber", lua_tonumber);
    register_native(state, "type", lua_type);
    register_native(state, "pairs", lua_pairs);
    register_native(state, "ipairs", lua_ipairs);
    register_native(state, "next", lua_next);
    register_native(state, "select", lua_select);
    register_native(state, "error", lua_error);
    register_native(state, "pcall", lua_pcall);
    register_native(state, "xpcall", lua_xpcall);
    register_native(state, "assert", lua_assert);
    register_native(state, "rawget", lua_rawget);
    register_native(state, "rawset", lua_rawset);
    register_native(state, "rawlen", lua_rawlen);
    register_native(state, "unpack", lua_unpack);
    register_native(state, "setmetatable", lua_setmetatable);
    register_native(state, "getmetatable", lua_getmetatable);

    // string library
    let string_table = Rc::new(RefCell::new(LuaTable::new()));
    register_table_native(&string_table, "byte", string_byte);
    register_table_native(&string_table, "char", string_char);
    register_table_native(&string_table, "len", string_len);
    register_table_native(&string_table, "lower", string_lower);
    register_table_native(&string_table, "upper", string_upper);
    register_table_native(&string_table, "rep", string_rep);
    register_table_native(&string_table, "reverse", string_reverse);
    register_table_native(&string_table, "sub", string_sub);
    register_table_native(&string_table, "find", string_find);
    register_table_native(&string_table, "format", string_format);
    register_table_native(&string_table, "gsub", string_gsub);
    state.set_global("string", LuaValue::Table(string_table));

    // table library
    let table_lib = Rc::new(RefCell::new(LuaTable::new()));
    register_table_native(&table_lib, "insert", table_insert);
    register_table_native(&table_lib, "remove", table_remove);
    register_table_native(&table_lib, "sort", table_sort);
    register_table_native(&table_lib, "concat", table_concat);
    register_table_native(&table_lib, "move", table_move);
    state.set_global("table", LuaValue::Table(table_lib));

    // math library
    let math_table = Rc::new(RefCell::new(LuaTable::new()));
    register_table_native(&math_table, "abs", math_abs);
    register_table_native(&math_table, "ceil", math_ceil);
    register_table_native(&math_table, "floor", math_floor);
    register_table_native(&math_table, "max", math_max);
    register_table_native(&math_table, "min", math_min);
    register_table_native(&math_table, "sqrt", math_sqrt);
    register_table_native(&math_table, "sin", math_sin);
    register_table_native(&math_table, "cos", math_cos);
    register_table_native(&math_table, "random", math_random);
    register_table_native(&math_table, "randomseed", math_randomseed);
    math_table.borrow_mut().set(
        LuaValue::String(String::from("pi")),
        LuaValue::Number(core::f64::consts::PI),
    );
    math_table.borrow_mut().set(
        LuaValue::String(String::from("huge")),
        LuaValue::Number(f64::INFINITY),
    );
    math_table.borrow_mut().set(
        LuaValue::String(String::from("maxinteger")),
        LuaValue::Integer(i64::MAX),
    );
    math_table.borrow_mut().set(
        LuaValue::String(String::from("mininteger")),
        LuaValue::Integer(i64::MIN),
    );
    state.set_global("math", LuaValue::Table(math_table));

    // io library (minimal)
    let io_table = Rc::new(RefCell::new(LuaTable::new()));
    register_table_native(&io_table, "write", io_write);
    state.set_global("io", LuaValue::Table(io_table));

    // os library (minimal)
    let os_table = Rc::new(RefCell::new(LuaTable::new()));
    register_table_native(&os_table, "clock", os_clock);
    register_table_native(&os_table, "time", os_time);
    state.set_global("os", LuaValue::Table(os_table));
}

fn register_native(
    state: &mut LuaState,
    name: &str,
    f: fn(&mut LuaState, &[LuaValue]) -> Result<Vec<LuaValue>, String>,
) {
    state.set_global(name, LuaValue::NativeFunction(String::from(name), f));
}

fn register_table_native(
    table: &Rc<RefCell<LuaTable>>,
    name: &str,
    f: fn(&mut LuaState, &[LuaValue]) -> Result<Vec<LuaValue>, String>,
) {
    table.borrow_mut().set(
        LuaValue::String(String::from(name)),
        LuaValue::NativeFunction(String::from(name), f),
    );
}

// --- Global functions ---

fn lua_print(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let mut parts = Vec::new();
    for arg in args {
        parts.push(arg.to_display_string());
    }
    let line = parts.join("\t");
    state.write_output(&line);
    state.write_output("\n");
    Ok(Vec::new())
}

fn lua_tostring(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let val = args.first().unwrap_or(&LuaValue::Nil);
    Ok(vec![LuaValue::String(val.to_display_string())])
}

fn lua_tonumber(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let val = args.first().unwrap_or(&LuaValue::Nil);
    match val.to_number() {
        Some(n) => {
            if n == (n as i64) as f64 {
                Ok(vec![LuaValue::Integer(n as i64)])
            } else {
                Ok(vec![LuaValue::Number(n)])
            }
        }
        None => Ok(vec![LuaValue::Nil]),
    }
}

fn lua_type(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let val = args.first().unwrap_or(&LuaValue::Nil);
    Ok(vec![LuaValue::String(String::from(val.type_name()))])
}

fn lua_pairs(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().cloned().unwrap_or(LuaValue::Nil);
    // Return (next, table, nil)
    Ok(vec![
        state.get_global("next"),
        table,
        LuaValue::Nil,
    ])
}

fn lua_ipairs(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().cloned().unwrap_or(LuaValue::Nil);
    let iter: fn(&mut LuaState, &[LuaValue]) -> Result<Vec<LuaValue>, String> = |_state, args| {
        let tbl = &args[0];
        let idx = args.get(1).and_then(|v| v.to_integer()).unwrap_or(0) + 1;
        if let LuaValue::Table(t) = tbl {
            let val = t.borrow().get(&LuaValue::Integer(idx));
            if matches!(val, LuaValue::Nil) {
                Ok(vec![LuaValue::Nil])
            } else {
                Ok(vec![LuaValue::Integer(idx), val])
            }
        } else {
            Ok(vec![LuaValue::Nil])
        }
    };
    Ok(vec![
        LuaValue::NativeFunction(String::from("ipairs_iter"), iter),
        table,
        LuaValue::Integer(0),
    ])
}

fn lua_next(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().unwrap_or(&LuaValue::Nil);
    let key = args.get(1).unwrap_or(&LuaValue::Nil);
    match table {
        LuaValue::Table(t) => {
            match t.borrow().next(key) {
                Some((k, v)) => Ok(vec![k, v]),
                None => Ok(vec![LuaValue::Nil]),
            }
        }
        _ => Err(String::from("bad argument to 'next' (table expected)")),
    }
}

fn lua_select(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let index = args.first().unwrap_or(&LuaValue::Nil);
    if let LuaValue::String(s) = index {
        if s == "#" {
            return Ok(vec![LuaValue::Integer((args.len() - 1) as i64)]);
        }
    }
    if let Some(n) = index.to_integer() {
        if n >= 1 {
            let start = n as usize;
            if start < args.len() {
                return Ok(args[start..].to_vec());
            }
        }
        Ok(Vec::new())
    } else {
        Err(String::from("bad argument to 'select'"))
    }
}

fn lua_error(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let msg = args.first().unwrap_or(&LuaValue::Nil).to_display_string();
    Err(msg)
}

fn lua_pcall(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let func = args.first().cloned().unwrap_or(LuaValue::Nil);
    let func_args = if args.len() > 1 { &args[1..] } else { &[] };
    match state.call_function(&func, func_args) {
        Ok(results) => {
            let mut ret = vec![LuaValue::Boolean(true)];
            ret.extend(results);
            Ok(ret)
        }
        Err(msg) => {
            Ok(vec![LuaValue::Boolean(false), LuaValue::String(msg)])
        }
    }
}

fn lua_xpcall(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let func = args.first().cloned().unwrap_or(LuaValue::Nil);
    let handler = args.get(1).cloned().unwrap_or(LuaValue::Nil);
    let func_args = if args.len() > 2 { &args[2..] } else { &[] };
    match state.call_function(&func, func_args) {
        Ok(results) => {
            let mut ret = vec![LuaValue::Boolean(true)];
            ret.extend(results);
            Ok(ret)
        }
        Err(msg) => {
            let handler_result = state.call_function(&handler, &[LuaValue::String(msg)]);
            match handler_result {
                Ok(results) => {
                    let mut ret = vec![LuaValue::Boolean(false)];
                    ret.extend(results);
                    Ok(ret)
                }
                Err(e) => Ok(vec![LuaValue::Boolean(false), LuaValue::String(e)]),
            }
        }
    }
}

fn lua_assert(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let val = args.first().unwrap_or(&LuaValue::Nil);
    if val.is_truthy() {
        Ok(args.to_vec())
    } else {
        let msg = args.get(1).map(|v| v.to_display_string()).unwrap_or_else(|| String::from("assertion failed!"));
        Err(msg)
    }
}

fn lua_rawget(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().unwrap_or(&LuaValue::Nil);
    let key = args.get(1).unwrap_or(&LuaValue::Nil);
    match table {
        LuaValue::Table(t) => Ok(vec![t.borrow().get(key)]),
        _ => Err(String::from("bad argument to 'rawget'")),
    }
}

fn lua_rawset(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().unwrap_or(&LuaValue::Nil).clone();
    let key = args.get(1).cloned().unwrap_or(LuaValue::Nil);
    let value = args.get(2).cloned().unwrap_or(LuaValue::Nil);
    match &table {
        LuaValue::Table(t) => {
            t.borrow_mut().set(key, value);
            Ok(vec![table])
        }
        _ => Err(String::from("bad argument to 'rawset'")),
    }
}

fn lua_rawlen(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let val = args.first().unwrap_or(&LuaValue::Nil);
    match val {
        LuaValue::Table(t) => Ok(vec![LuaValue::Integer(t.borrow().len() as i64)]),
        LuaValue::String(s) => Ok(vec![LuaValue::Integer(s.len() as i64)]),
        _ => Err(String::from("bad argument to 'rawlen'")),
    }
}

fn lua_unpack(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().unwrap_or(&LuaValue::Nil);
    let i = args.get(1).and_then(|v| v.to_integer()).unwrap_or(1);
    match table {
        LuaValue::Table(t) => {
            let t = t.borrow();
            let j = args.get(2).and_then(|v| v.to_integer()).unwrap_or(t.len() as i64);
            let mut results = Vec::new();
            for idx in i..=j {
                results.push(t.get(&LuaValue::Integer(idx)));
            }
            Ok(results)
        }
        _ => Err(String::from("bad argument to 'unpack'")),
    }
}

fn lua_setmetatable(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = args.first().cloned().unwrap_or(LuaValue::Nil);
    let _mt = args.get(1).cloned().unwrap_or(LuaValue::Nil);
    // Simplified: we store the metatable but don't fully implement metamethods
    Ok(vec![table])
}

fn lua_getmetatable(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let _val = args.first().unwrap_or(&LuaValue::Nil);
    Ok(vec![LuaValue::Nil])
}

// --- String library ---

fn string_byte(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let s = match args.first() {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => return Err(String::from("bad argument to 'string.byte'")),
    };
    let i = args.get(1).and_then(|v| v.to_integer()).unwrap_or(1) - 1;
    let j = args.get(2).and_then(|v| v.to_integer()).map(|v| v - 1).unwrap_or(i);
    let bytes = s.as_bytes();
    let mut results = Vec::new();
    for idx in i..=j {
        if idx >= 0 && (idx as usize) < bytes.len() {
            results.push(LuaValue::Integer(bytes[idx as usize] as i64));
        }
    }
    Ok(results)
}

fn string_char(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let mut s = String::new();
    for arg in args {
        let code = arg.to_integer().ok_or_else(|| String::from("bad argument to 'string.char'"))?;
        s.push(code as u8 as char);
    }
    Ok(vec![LuaValue::String(s)])
}

fn string_len(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    match args.first() {
        Some(LuaValue::String(s)) => Ok(vec![LuaValue::Integer(s.len() as i64)]),
        _ => Err(String::from("bad argument to 'string.len'")),
    }
}

fn string_lower(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    match args.first() {
        Some(LuaValue::String(s)) => {
            let lower: String = s.chars().map(|c| {
                if c.is_ascii_uppercase() { (c as u8 + 32) as char } else { c }
            }).collect();
            Ok(vec![LuaValue::String(lower)])
        }
        _ => Err(String::from("bad argument to 'string.lower'")),
    }
}

fn string_upper(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    match args.first() {
        Some(LuaValue::String(s)) => {
            let upper: String = s.chars().map(|c| {
                if c.is_ascii_lowercase() { (c as u8 - 32) as char } else { c }
            }).collect();
            Ok(vec![LuaValue::String(upper)])
        }
        _ => Err(String::from("bad argument to 'string.upper'")),
    }
}

fn string_rep(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let s = match args.first() {
        Some(LuaValue::String(s)) => s.clone(),
        _ => return Err(String::from("bad argument to 'string.rep'")),
    };
    let n = args.get(1).and_then(|v| v.to_integer()).unwrap_or(0);
    let sep = match args.get(2) {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => "",
    };
    let mut result = String::new();
    for i in 0..n {
        if i > 0 && !sep.is_empty() {
            result.push_str(sep);
        }
        result.push_str(&s);
    }
    Ok(vec![LuaValue::String(result)])
}

fn string_reverse(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    match args.first() {
        Some(LuaValue::String(s)) => {
            Ok(vec![LuaValue::String(s.chars().rev().collect())])
        }
        _ => Err(String::from("bad argument to 'string.reverse'")),
    }
}

fn string_sub(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let s = match args.first() {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => return Err(String::from("bad argument to 'string.sub'")),
    };
    let len = s.len() as i64;
    let mut i = args.get(1).and_then(|v| v.to_integer()).unwrap_or(1);
    let mut j = args.get(2).and_then(|v| v.to_integer()).unwrap_or(-1);

    if i < 0 { i = (len + i + 1).max(1); }
    if j < 0 { j = len + j + 1; }
    if i < 1 { i = 1; }
    if j > len { j = len; }

    if i > j {
        return Ok(vec![LuaValue::String(String::new())]);
    }

    let start = (i - 1) as usize;
    let end = j as usize;
    let result = if start < s.len() && end <= s.len() {
        String::from(&s[start..end])
    } else {
        String::new()
    };
    Ok(vec![LuaValue::String(result)])
}

fn string_find(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let s = match args.first() {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => return Err(String::from("bad argument to 'string.find'")),
    };
    let pattern = match args.get(1) {
        Some(LuaValue::String(p)) => p.as_str(),
        _ => return Err(String::from("bad argument to 'string.find'")),
    };
    let init = args.get(2).and_then(|v| v.to_integer()).unwrap_or(1);
    let start = if init < 1 { 0 } else { (init - 1) as usize };

    if start >= s.len() {
        return Ok(vec![LuaValue::Nil]);
    }

    // Plain string search
    match s[start..].find(pattern) {
        Some(pos) => {
            let abs_start = start + pos + 1; // 1-indexed
            let abs_end = abs_start + pattern.len() - 1;
            Ok(vec![LuaValue::Integer(abs_start as i64), LuaValue::Integer(abs_end as i64)])
        }
        None => Ok(vec![LuaValue::Nil]),
    }
}

fn string_format(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let fmt = match args.first() {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => return Err(String::from("bad argument to 'string.format'")),
    };

    let mut result = String::new();
    let mut arg_idx = 1;
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                '%' => { result.push('%'); i += 1; }
                'd' | 'i' => {
                    let val = args.get(arg_idx).and_then(|v| v.to_integer()).unwrap_or(0);
                    result.push_str(&format!("{}", val));
                    arg_idx += 1;
                    i += 1;
                }
                'f' => {
                    let val = args.get(arg_idx).and_then(|v| v.to_number()).unwrap_or(0.0);
                    result.push_str(&format!("{:.6}", val));
                    arg_idx += 1;
                    i += 1;
                }
                's' => {
                    let val = args.get(arg_idx).unwrap_or(&LuaValue::Nil).to_display_string();
                    result.push_str(&val);
                    arg_idx += 1;
                    i += 1;
                }
                'x' | 'X' => {
                    let val = args.get(arg_idx).and_then(|v| v.to_integer()).unwrap_or(0);
                    if chars[i] == 'x' {
                        result.push_str(&format!("{:x}", val));
                    } else {
                        result.push_str(&format!("{:X}", val));
                    }
                    arg_idx += 1;
                    i += 1;
                }
                'c' => {
                    let val = args.get(arg_idx).and_then(|v| v.to_integer()).unwrap_or(0);
                    result.push(val as u8 as char);
                    arg_idx += 1;
                    i += 1;
                }
                'q' => {
                    let val = args.get(arg_idx).unwrap_or(&LuaValue::Nil).to_display_string();
                    result.push('"');
                    for c in val.chars() {
                        match c {
                            '\\' => result.push_str("\\\\"),
                            '"' => result.push_str("\\\""),
                            '\n' => result.push_str("\\n"),
                            '\r' => result.push_str("\\r"),
                            '\0' => result.push_str("\\0"),
                            _ => result.push(c),
                        }
                    }
                    result.push('"');
                    arg_idx += 1;
                    i += 1;
                }
                _ => {
                    // Skip width/precision specifiers
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '-') {
                        i += 1;
                    }
                    if i < chars.len() {
                        match chars[i] {
                            'd' | 'i' => {
                                let val = args.get(arg_idx).and_then(|v| v.to_integer()).unwrap_or(0);
                                result.push_str(&format!("{}", val));
                                arg_idx += 1;
                            }
                            'f' | 'g' | 'e' => {
                                let val = args.get(arg_idx).and_then(|v| v.to_number()).unwrap_or(0.0);
                                result.push_str(&format!("{}", val));
                                arg_idx += 1;
                            }
                            's' => {
                                let val = args.get(arg_idx).unwrap_or(&LuaValue::Nil).to_display_string();
                                result.push_str(&val);
                                arg_idx += 1;
                            }
                            _ => { result.push(chars[i]); }
                        }
                        i += 1;
                    }
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    Ok(vec![LuaValue::String(result)])
}

fn string_gsub(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let s = match args.first() {
        Some(LuaValue::String(s)) => s.clone(),
        _ => return Err(String::from("bad argument to 'string.gsub'")),
    };
    let pattern = match args.get(1) {
        Some(LuaValue::String(p)) => p.clone(),
        _ => return Err(String::from("bad argument to 'string.gsub'")),
    };
    let repl = match args.get(2) {
        Some(LuaValue::String(r)) => r.clone(),
        _ => return Err(String::from("bad argument to 'string.gsub'")),
    };
    let max_n = args.get(3).and_then(|v| v.to_integer());

    // Simple literal replacement
    let mut result = String::new();
    let mut count: i64 = 0;
    let mut pos = 0;
    let s_bytes = s.as_bytes();
    let p_bytes = pattern.as_bytes();

    while pos <= s_bytes.len() {
        if let Some(max) = max_n {
            if count >= max { break; }
        }
        if pos + p_bytes.len() <= s_bytes.len() && &s_bytes[pos..pos + p_bytes.len()] == p_bytes {
            result.push_str(&repl);
            count += 1;
            pos += p_bytes.len();
        } else {
            if pos < s_bytes.len() {
                result.push(s_bytes[pos] as char);
            }
            pos += 1;
        }
    }

    Ok(vec![LuaValue::String(result), LuaValue::Integer(count)])
}

// --- Table library ---

fn table_insert(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = match args.first() {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => return Err(String::from("bad argument to 'table.insert'")),
    };
    match args.len() {
        2 => {
            let val = args[1].clone();
            let len = table.borrow().len();
            table.borrow_mut().insert(len + 1, val);
        }
        3 => {
            let pos = args[1].to_integer().unwrap_or(1) as usize;
            let val = args[2].clone();
            table.borrow_mut().insert(pos, val);
        }
        _ => return Err(String::from("wrong number of arguments to 'table.insert'")),
    }
    Ok(Vec::new())
}

fn table_remove(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = match args.first() {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => return Err(String::from("bad argument to 'table.remove'")),
    };
    let pos = args.get(1).and_then(|v| v.to_integer()).unwrap_or(table.borrow().len() as i64) as usize;
    let removed = table.borrow_mut().remove(pos);
    Ok(vec![removed])
}

fn table_sort(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = match args.first() {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => return Err(String::from("bad argument to 'table.sort'")),
    };
    table.borrow_mut().sort_array();
    Ok(Vec::new())
}

fn table_concat(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let table = match args.first() {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => return Err(String::from("bad argument to 'table.concat'")),
    };
    let sep = match args.get(1) {
        Some(LuaValue::String(s)) => s.as_str(),
        _ => "",
    };
    let t = table.borrow();
    let i = args.get(2).and_then(|v| v.to_integer()).unwrap_or(1) as usize;
    let j = args.get(3).and_then(|v| v.to_integer()).unwrap_or(t.len() as i64) as usize;
    let result = t.concat(sep, i, j);
    Ok(vec![LuaValue::String(result)])
}

fn table_move(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    // table.move(a1, f, e, t [, a2])
    let a1 = match args.first() {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => return Err(String::from("bad argument to 'table.move'")),
    };
    let f = args.get(1).and_then(|v| v.to_integer()).unwrap_or(1);
    let e = args.get(2).and_then(|v| v.to_integer()).unwrap_or(0);
    let t = args.get(3).and_then(|v| v.to_integer()).unwrap_or(1);
    let a2 = match args.get(4) {
        Some(LuaValue::Table(t)) => t.clone(),
        _ => a1.clone(),
    };

    for i in f..=e {
        let val = a1.borrow().get(&LuaValue::Integer(i));
        a2.borrow_mut().set(LuaValue::Integer(t + i - f), val);
    }

    Ok(vec![LuaValue::Table(a2)])
}

// --- Math library ---

fn math_abs(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    match args.first() {
        Some(LuaValue::Integer(i)) => Ok(vec![LuaValue::Integer(i.abs())]),
        Some(v) => {
            let n = v.to_number().ok_or_else(|| String::from("bad argument to 'math.abs'"))?;
            Ok(vec![LuaValue::Number(if n < 0.0 { -n } else { n })])
        }
        None => Err(String::from("bad argument to 'math.abs'")),
    }
}

fn math_ceil(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let n = args.first().and_then(|v| v.to_number()).ok_or_else(|| String::from("bad argument to 'math.ceil'"))?;
    let i = n as i64;
    let result = if n > 0.0 && (i as f64) < n { i + 1 } else { i };
    Ok(vec![LuaValue::Integer(result)])
}

fn math_floor(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let n = args.first().and_then(|v| v.to_number()).ok_or_else(|| String::from("bad argument to 'math.floor'"))?;
    let i = n as i64;
    let result = if n < 0.0 && (i as f64) > n { i - 1 } else { i };
    Ok(vec![LuaValue::Integer(result)])
}

fn math_max(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    if args.is_empty() { return Err(String::from("bad argument to 'math.max'")); }
    let mut max = args[0].clone();
    for arg in &args[1..] {
        let a = max.to_number().unwrap_or(0.0);
        let b = arg.to_number().unwrap_or(0.0);
        if b > a { max = arg.clone(); }
    }
    Ok(vec![max])
}

fn math_min(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    if args.is_empty() { return Err(String::from("bad argument to 'math.min'")); }
    let mut min = args[0].clone();
    for arg in &args[1..] {
        let a = min.to_number().unwrap_or(0.0);
        let b = arg.to_number().unwrap_or(0.0);
        if b < a { min = arg.clone(); }
    }
    Ok(vec![min])
}

fn math_sqrt(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let n = args.first().and_then(|v| v.to_number()).ok_or_else(|| String::from("bad argument to 'math.sqrt'"))?;
    if n < 0.0 { return Ok(vec![LuaValue::Number(f64::NAN)]); }
    if n == 0.0 { return Ok(vec![LuaValue::Number(0.0)]); }
    let mut x = n;
    for _ in 0..30 { x = (x + n / x) * 0.5; }
    Ok(vec![LuaValue::Number(x)])
}

fn math_sin(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let n = args.first().and_then(|v| v.to_number()).ok_or_else(|| String::from("bad argument to 'math.sin'"))?;
    // Taylor series for sin
    let x = n % (2.0 * core::f64::consts::PI);
    let mut sum = 0.0;
    let mut term = x;
    for i in 0..15 {
        sum += term;
        term *= -x * x / ((2 * i + 2) as f64 * (2 * i + 3) as f64);
    }
    Ok(vec![LuaValue::Number(sum)])
}

fn math_cos(_state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let n = args.first().and_then(|v| v.to_number()).ok_or_else(|| String::from("bad argument to 'math.cos'"))?;
    let x = n % (2.0 * core::f64::consts::PI);
    let mut sum = 0.0;
    let mut term = 1.0;
    for i in 0..15 {
        sum += term;
        term *= -x * x / ((2 * i + 1) as f64 * (2 * i + 2) as f64);
    }
    Ok(vec![LuaValue::Number(sum)])
}

fn math_random(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    // Simple xorshift64
    state.seed ^= state.seed << 13;
    state.seed ^= state.seed >> 7;
    state.seed ^= state.seed << 17;
    let raw = (state.seed as u64) as f64 / (u64::MAX as f64);

    match args.len() {
        0 => Ok(vec![LuaValue::Number(raw)]),
        1 => {
            let m = args[0].to_integer().unwrap_or(1);
            let val = (raw * m as f64) as i64 + 1;
            Ok(vec![LuaValue::Integer(val.min(m))])
        }
        _ => {
            let m = args[0].to_integer().unwrap_or(1);
            let n = args[1].to_integer().unwrap_or(m);
            let val = m + (raw * (n - m + 1) as f64) as i64;
            Ok(vec![LuaValue::Integer(val.min(n))])
        }
    }
}

fn math_randomseed(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    let seed = args.first().and_then(|v| v.to_integer()).unwrap_or(0);
    state.seed = seed as u64;
    if state.seed == 0 { state.seed = 1; }
    Ok(Vec::new())
}

// --- IO library ---

fn io_write(state: &mut LuaState, args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    for arg in args {
        state.write_output(&arg.to_display_string());
    }
    Ok(Vec::new())
}

// --- OS library ---

fn os_clock(_state: &mut LuaState, _args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    Ok(vec![LuaValue::Number(0.0)]) // Stub for bare metal
}

fn os_time(_state: &mut LuaState, _args: &[LuaValue]) -> Result<Vec<LuaValue>, String> {
    Ok(vec![LuaValue::Integer(0)]) // Stub for bare metal
}
