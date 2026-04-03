//! Go standard library stubs: fmt, strings, strconv, os, math.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::runtime::GoValue;

/// Dispatch a stdlib function call.
pub fn call_stdlib(pkg: &str, func: &str, args: &[GoValue], output: &mut String) -> Option<GoValue> {
    match (pkg, func) {
        // === fmt ===
        ("fmt", "Println") => {
            let parts: Vec<String> = args.iter().map(|a| a.to_string_repr()).collect();
            let line = parts.join(" ");
            output.push_str(&line);
            output.push('\n');
            None
        }
        ("fmt", "Printf") => {
            if let Some(GoValue::String(ref fmt_str)) = args.first() {
                let formatted = go_sprintf(fmt_str, &args[1..]);
                output.push_str(&formatted);
            }
            None
        }
        ("fmt", "Sprintf") => {
            if let Some(GoValue::String(ref fmt_str)) = args.first() {
                let formatted = go_sprintf(fmt_str, &args[1..]);
                Some(GoValue::String(formatted))
            } else {
                Some(GoValue::String(String::new()))
            }
        }
        ("fmt", "Sprint") => {
            let parts: Vec<String> = args.iter().map(|a| a.to_string_repr()).collect();
            Some(GoValue::String(parts.join(" ")))
        }

        // === strings ===
        ("strings", "Contains") => {
            if let (Some(GoValue::String(s)), Some(GoValue::String(sub))) = (args.get(0), args.get(1)) {
                Some(GoValue::Bool(s.contains(sub.as_str())))
            } else {
                Some(GoValue::Bool(false))
            }
        }
        ("strings", "HasPrefix") => {
            if let (Some(GoValue::String(s)), Some(GoValue::String(pre))) = (args.get(0), args.get(1)) {
                Some(GoValue::Bool(s.starts_with(pre.as_str())))
            } else {
                Some(GoValue::Bool(false))
            }
        }
        ("strings", "HasSuffix") => {
            if let (Some(GoValue::String(s)), Some(GoValue::String(suf))) = (args.get(0), args.get(1)) {
                Some(GoValue::Bool(s.ends_with(suf.as_str())))
            } else {
                Some(GoValue::Bool(false))
            }
        }
        ("strings", "Join") => {
            if let (Some(GoValue::Slice(sl)), Some(GoValue::String(sep))) = (args.get(0), args.get(1)) {
                let parts: Vec<String> = sl.data.iter().map(|v| v.to_string_repr()).collect();
                Some(GoValue::String(parts.join(sep.as_str())))
            } else {
                Some(GoValue::String(String::new()))
            }
        }
        ("strings", "Split") => {
            if let (Some(GoValue::String(s)), Some(GoValue::String(sep))) = (args.get(0), args.get(1)) {
                let parts: Vec<GoValue> = s.split(sep.as_str())
                    .map(|p| GoValue::String(String::from(p)))
                    .collect();
                Some(GoValue::Slice(crate::runtime::GoSlice {
                    len: parts.len(),
                    cap: parts.len(),
                    data: parts,
                }))
            } else {
                Some(GoValue::Nil)
            }
        }
        ("strings", "Replace") => {
            if let (Some(GoValue::String(s)), Some(GoValue::String(old)), Some(GoValue::String(new_s)), Some(GoValue::Int(n))) =
                (args.get(0), args.get(1), args.get(2), args.get(3))
            {
                let result = if *n < 0 {
                    s.replace(old.as_str(), new_s.as_str())
                } else {
                    s.replacen(old.as_str(), new_s.as_str(), *n as usize)
                };
                Some(GoValue::String(result))
            } else {
                Some(GoValue::String(String::new()))
            }
        }
        ("strings", "ToLower") => {
            if let Some(GoValue::String(s)) = args.get(0) {
                Some(GoValue::String(s.to_ascii_lowercase()))
            } else {
                Some(GoValue::String(String::new()))
            }
        }
        ("strings", "ToUpper") => {
            if let Some(GoValue::String(s)) = args.get(0) {
                Some(GoValue::String(s.to_ascii_uppercase()))
            } else {
                Some(GoValue::String(String::new()))
            }
        }

        // === strconv ===
        ("strconv", "Itoa") => {
            if let Some(GoValue::Int(n)) = args.get(0) {
                Some(GoValue::String(format!("{}", n)))
            } else {
                Some(GoValue::String(String::from("0")))
            }
        }
        ("strconv", "Atoi") => {
            if let Some(GoValue::String(s)) = args.get(0) {
                match s.parse::<i64>() {
                    Ok(n) => Some(GoValue::Int(n)),
                    Err(_) => Some(GoValue::Int(0)),
                }
            } else {
                Some(GoValue::Int(0))
            }
        }

        // === os ===
        ("os", "Exit") => {
            // In bare-metal context, just return the exit code
            if let Some(GoValue::Int(code)) = args.get(0) {
                log::info!("[go] os.Exit({})", code);
            }
            None
        }

        // === math ===
        ("math", "Abs") => {
            if let Some(GoValue::Float(f)) = args.get(0) {
                Some(GoValue::Float(if *f < 0.0 { -f } else { *f }))
            } else {
                Some(GoValue::Float(0.0))
            }
        }
        ("math", "Sqrt") => {
            if let Some(GoValue::Float(f)) = args.get(0) {
                // no_std: use Newton's method
                let mut x = *f;
                if x > 0.0 {
                    let mut guess = x / 2.0;
                    for _ in 0..50 {
                        guess = (guess + x / guess) / 2.0;
                    }
                    x = guess;
                }
                Some(GoValue::Float(x))
            } else {
                Some(GoValue::Float(0.0))
            }
        }
        ("math", "Max") => {
            if let (Some(GoValue::Float(a)), Some(GoValue::Float(b))) = (args.get(0), args.get(1)) {
                Some(GoValue::Float(if *a > *b { *a } else { *b }))
            } else {
                Some(GoValue::Float(0.0))
            }
        }
        ("math", "Min") => {
            if let (Some(GoValue::Float(a)), Some(GoValue::Float(b))) = (args.get(0), args.get(1)) {
                Some(GoValue::Float(if *a < *b { *a } else { *b }))
            } else {
                Some(GoValue::Float(0.0))
            }
        }

        _ => {
            log::warn!("[go] unknown stdlib call: {}.{}", pkg, func);
            None
        }
    }
}

/// Simple Go-style printf formatting.
fn go_sprintf(fmt_str: &str, args: &[GoValue]) -> String {
    let mut result = String::new();
    let mut chars = fmt_str.chars().peekable();
    let mut arg_idx = 0;

    while let Some(ch) = chars.next() {
        if ch == '%' {
            if let Some(&next) = chars.peek() {
                chars.next();
                let val = args.get(arg_idx).cloned().unwrap_or(GoValue::Nil);
                arg_idx += 1;
                match next {
                    'd' => result.push_str(&format!("{}", match val {
                        GoValue::Int(n) => n,
                        _ => 0,
                    })),
                    's' => result.push_str(&val.to_string_repr()),
                    'f' => result.push_str(&format!("{}", match val {
                        GoValue::Float(f) => f,
                        _ => 0.0,
                    })),
                    'v' => result.push_str(&val.to_string_repr()),
                    '%' => { result.push('%'); arg_idx -= 1; }
                    _ => {
                        result.push('%');
                        result.push(next);
                        arg_idx -= 1;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}
