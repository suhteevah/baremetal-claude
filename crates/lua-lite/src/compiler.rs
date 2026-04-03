//! Compile AST to an optimized form (constant folding, basic checks).
//!
//! For this tree-walking interpreter, the "compiler" phase performs
//! AST-level optimizations rather than generating bytecode.

use alloc::string::String;

use crate::ast::*;

/// Perform constant folding on an expression.
pub fn fold_constants(exp: &mut Exp) {
    match exp {
        Exp::BinOp { op, left, right } => {
            fold_constants(left);
            fold_constants(right);

            // Fold constant integer arithmetic
            if let (Exp::Integer(a), Exp::Integer(b)) = (left.as_ref(), right.as_ref()) {
                let (a, b) = (*a, *b);
                let result = match op {
                    BinaryOp::Add => Some(a.wrapping_add(b)),
                    BinaryOp::Sub => Some(a.wrapping_sub(b)),
                    BinaryOp::Mul => Some(a.wrapping_mul(b)),
                    BinaryOp::Mod => if b != 0 { Some(a % b) } else { None },
                    BinaryOp::IDiv => if b != 0 { Some(a / b) } else { None },
                    BinaryOp::BAnd => Some(a & b),
                    BinaryOp::BOr => Some(a | b),
                    BinaryOp::BXor => Some(a ^ b),
                    _ => None,
                };
                if let Some(val) = result {
                    *exp = Exp::Integer(val);
                    return;
                }
            }

            // Fold constant float arithmetic
            if let (Some(a), Some(b)) = (as_number(left), as_number(right)) {
                let result = match op {
                    BinaryOp::Add => Some(a + b),
                    BinaryOp::Sub => Some(a - b),
                    BinaryOp::Mul => Some(a * b),
                    BinaryOp::Div => if b != 0.0 { Some(a / b) } else { None },
                    _ => None,
                };
                if let Some(val) = result {
                    *exp = Exp::Number(val);
                    return;
                }
            }

            // Fold string concatenation
            if *op == BinaryOp::Concat {
                if let (Exp::Str(a), Exp::Str(b)) = (left.as_ref(), right.as_ref()) {
                    let mut s = a.clone();
                    s.push_str(&b);
                    *exp = Exp::Str(s);
                }
            }
        }

        Exp::UnOp { op, operand } => {
            fold_constants(operand);
            match (op, operand.as_ref()) {
                (UnaryOp::Neg, Exp::Integer(n)) => {
                    *exp = Exp::Integer(-n);
                }
                (UnaryOp::Neg, Exp::Number(n)) => {
                    *exp = Exp::Number(-n);
                }
                (UnaryOp::Not, Exp::True) => {
                    *exp = Exp::False;
                }
                (UnaryOp::Not, Exp::False) | (UnaryOp::Not, Exp::Nil) => {
                    *exp = Exp::True;
                }
                (UnaryOp::Len, Exp::Str(s)) => {
                    *exp = Exp::Integer(s.len() as i64);
                }
                _ => {}
            }
        }

        _ => {}
    }
}

/// Optimize a block by folding constants in all expressions.
pub fn optimize_block(block: &mut Block) {
    for stat in &mut block.stats {
        optimize_stat(stat);
    }
    if let Some(ret) = &mut block.ret {
        for exp in ret {
            fold_constants(exp);
        }
    }
}

fn optimize_stat(stat: &mut Stat) {
    match stat {
        Stat::Assign { values, .. } => {
            for v in values { fold_constants(v); }
        }
        Stat::Local { values, .. } => {
            for v in values { fold_constants(v); }
        }
        Stat::While { condition, body, .. } => {
            fold_constants(condition);
            optimize_block(body);
        }
        Stat::Repeat { body, condition } => {
            optimize_block(body);
            fold_constants(condition);
        }
        Stat::If { conditions, else_block } => {
            for (cond, body) in conditions {
                fold_constants(cond);
                optimize_block(body);
            }
            if let Some(eb) = else_block {
                optimize_block(eb);
            }
        }
        Stat::ForNumeric { start, stop, step, body, .. } => {
            fold_constants(start);
            fold_constants(stop);
            if let Some(s) = step { fold_constants(s); }
            optimize_block(body);
        }
        Stat::ForGeneric { iterators, body, .. } => {
            for i in iterators { fold_constants(i); }
            optimize_block(body);
        }
        Stat::FunctionDef { body, .. } | Stat::LocalFunction { body, .. } => {
            optimize_block(body);
        }
        Stat::Do(block) => {
            optimize_block(block);
        }
        Stat::Return(exps) => {
            for e in exps { fold_constants(e); }
        }
        Stat::ExprStat(exp) => {
            fold_constants(exp);
        }
        _ => {}
    }
}

fn as_number(exp: &Exp) -> Option<f64> {
    match exp {
        Exp::Integer(i) => Some(*i as f64),
        Exp::Number(f) => Some(*f),
        _ => None,
    }
}

/// Validate an AST for common errors before execution.
pub fn validate_chunk(chunk: &Chunk) -> Result<(), String> {
    validate_block(chunk, false)
}

fn validate_block(block: &Block, in_loop: bool) -> Result<(), String> {
    for stat in &block.stats {
        validate_stat(stat, in_loop)?;
    }
    Ok(())
}

fn validate_stat(stat: &Stat, in_loop: bool) -> Result<(), String> {
    match stat {
        Stat::Break => {
            if !in_loop {
                return Err(String::from("<break> at line 0 not inside a loop"));
            }
        }
        Stat::While { body, .. } | Stat::Repeat { body, .. } => {
            validate_block(body, true)?;
        }
        Stat::ForNumeric { body, .. } | Stat::ForGeneric { body, .. } => {
            validate_block(body, true)?;
        }
        Stat::If { conditions, else_block } => {
            for (_, body) in conditions {
                validate_block(body, in_loop)?;
            }
            if let Some(eb) = else_block {
                validate_block(eb, in_loop)?;
            }
        }
        Stat::Do(block) => validate_block(block, in_loop)?,
        Stat::FunctionDef { body, .. } | Stat::LocalFunction { body, .. } => {
            validate_block(body, false)?;
        }
        _ => {}
    }
    Ok(())
}
