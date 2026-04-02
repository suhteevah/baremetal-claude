//! Basic shell scripting support.
//!
//! Executes a file as a sequence of commands. Supports:
//! - Sequential command execution
//! - if/else/fi conditionals
//! - for loops (for x in list; do ... done)
//! - Variable assignment (VAR=value)
//! - Comments (# ...)
//! - Exit codes ($?)
//!
//! Intentionally minimal — complex logic should use Python or Rust.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

/// A parsed script ready for execution.
#[derive(Debug)]
pub struct Script {
    /// Top-level statements in order.
    pub statements: Vec<Statement>,
}

/// A single statement in a script.
#[derive(Debug, Clone)]
pub enum Statement {
    /// A shell command line to execute.
    Command(String),

    /// Variable assignment: VAR=value
    Assignment { name: String, value: String },

    /// if/else/fi conditional.
    If {
        /// Condition command (exit code 0 = true).
        condition: String,
        /// Commands to run if condition is true.
        then_body: Vec<Statement>,
        /// Commands to run if condition is false.
        else_body: Vec<Statement>,
    },

    /// for loop: for VAR in ITEMS; do BODY; done
    For {
        /// Loop variable name.
        variable: String,
        /// Items to iterate over.
        items: Vec<String>,
        /// Loop body statements.
        body: Vec<Statement>,
    },

    /// A comment (preserved for debugging, not executed).
    Comment(String),

    /// An empty line (ignored).
    Empty,
}

/// Script execution callback — the shell provides this to run individual commands.
pub trait ScriptExecutor {
    /// Execute a single command line and return (output, exit_code).
    fn execute_line(&mut self, line: &str) -> (String, i32);

    /// Set a variable in the shell environment.
    fn set_var(&mut self, name: &str, value: &str);

    /// Get a variable from the shell environment.
    fn get_var(&self, name: &str) -> Option<String>;
}

/// Parse a script from source text.
pub fn parse_script(source: &str) -> Result<Script, String> {
    log::info!(
        "shell::script: parsing script ({} bytes, {} lines)",
        source.len(),
        source.lines().count()
    );

    let lines: Vec<&str> = source.lines().collect();
    let (statements, _) = parse_block(&lines, 0, &[])?;

    log::debug!(
        "shell::script: parsed {} top-level statement(s)",
        statements.len()
    );

    Ok(Script { statements })
}

/// Parse a block of lines into statements, starting at `start` line index.
/// `terminators` are keywords that end the block (e.g., "fi", "done", "else").
/// Returns (statements, next_line_index).
fn parse_block(
    lines: &[&str],
    start: usize,
    terminators: &[&str],
) -> Result<(Vec<Statement>, usize), String> {
    let mut statements = Vec::new();
    let mut i = start;

    while i < lines.len() {
        let line = lines[i].trim();

        // Check if we hit a terminator
        if terminators.iter().any(|t| line == *t || line.starts_with(&format!("{} ", t))) {
            log::trace!("shell::script: hit terminator at line {}: {:?}", i, line);
            return Ok((statements, i));
        }

        if line.is_empty() {
            statements.push(Statement::Empty);
            i += 1;
            continue;
        }

        if line.starts_with('#') {
            log::trace!("shell::script: comment at line {}", i);
            statements.push(Statement::Comment(String::from(line)));
            i += 1;
            continue;
        }

        // Variable assignment: VAR=value (no spaces around =)
        if let Some(eq_pos) = line.find('=') {
            let before = &line[..eq_pos];
            if !before.is_empty()
                && before.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !before.starts_with(char::is_numeric)
            {
                let name = String::from(before);
                let value = String::from(&line[eq_pos + 1..]);
                log::trace!("shell::script: assignment at line {}: {}={}", i, name, value);
                statements.push(Statement::Assignment { name, value });
                i += 1;
                continue;
            }
        }

        // if CONDITION; then ... else ... fi
        if line.starts_with("if ") {
            log::trace!("shell::script: if-block at line {}", i);
            let (stmt, next_i) = parse_if(lines, i)?;
            statements.push(stmt);
            i = next_i;
            continue;
        }

        // for VAR in ITEMS; do ... done
        if line.starts_with("for ") {
            log::trace!("shell::script: for-loop at line {}", i);
            let (stmt, next_i) = parse_for(lines, i)?;
            statements.push(stmt);
            i = next_i;
            continue;
        }

        // Regular command
        log::trace!("shell::script: command at line {}: {:?}", i, line);
        statements.push(Statement::Command(String::from(line)));
        i += 1;
    }

    Ok((statements, i))
}

/// Parse an if/else/fi block starting at line `start`.
fn parse_if(lines: &[&str], start: usize) -> Result<(Statement, usize), String> {
    let first_line = lines[start].trim();

    // Extract condition: "if CONDITION; then" or "if CONDITION" followed by "then" on next line
    let condition = if let Some(rest) = first_line.strip_prefix("if ") {
        let rest = rest.trim();
        if let Some(cond) = rest.strip_suffix("; then") {
            String::from(cond.trim())
        } else if rest == "then" {
            return Err(format!(
                "line {}: if with no condition",
                start + 1
            ));
        } else {
            String::from(rest)
        }
    } else {
        return Err(format!("line {}: expected 'if'", start + 1));
    };

    log::debug!("shell::script: if condition: {:?}", condition);

    // Parse then-body until "else" or "fi"
    let (then_body, then_end) = parse_block(lines, start + 1, &["else", "fi"])?;

    if then_end >= lines.len() {
        return Err(format!(
            "line {}: unterminated if (missing fi)",
            start + 1
        ));
    }

    let terminator_line = lines[then_end].trim();

    let (else_body, end_line) = if terminator_line == "else" {
        // Parse else-body until "fi"
        let (else_stmts, fi_line) = parse_block(lines, then_end + 1, &["fi"])?;
        if fi_line >= lines.len() || lines[fi_line].trim() != "fi" {
            return Err(format!(
                "line {}: unterminated else (missing fi)",
                then_end + 1
            ));
        }
        (else_stmts, fi_line + 1)
    } else {
        // No else, terminator was "fi"
        (Vec::new(), then_end + 1)
    };

    Ok((
        Statement::If {
            condition,
            then_body,
            else_body,
        },
        end_line,
    ))
}

/// Parse a for loop starting at line `start`.
fn parse_for(lines: &[&str], start: usize) -> Result<(Statement, usize), String> {
    let first_line = lines[start].trim();

    // "for VAR in ITEM1 ITEM2 ...; do" or split across lines
    let rest = first_line
        .strip_prefix("for ")
        .ok_or_else(|| format!("line {}: expected 'for'", start + 1))?
        .trim();

    // Find "in" keyword
    let in_pos = rest
        .find(" in ")
        .ok_or_else(|| format!("line {}: missing 'in' in for loop", start + 1))?;

    let variable = String::from(rest[..in_pos].trim());
    let remainder = rest[in_pos + 4..].trim();

    // Strip trailing "; do"
    let items_str = if let Some(items) = remainder.strip_suffix("; do") {
        items.trim()
    } else {
        remainder
    };

    let items: Vec<String> = items_str
        .split_whitespace()
        .map(String::from)
        .collect();

    log::debug!(
        "shell::script: for {}  in {:?}",
        variable,
        items
    );

    // Parse body until "done"
    let (body, done_line) = parse_block(lines, start + 1, &["done"])?;

    if done_line >= lines.len() || lines[done_line].trim() != "done" {
        return Err(format!(
            "line {}: unterminated for loop (missing done)",
            start + 1
        ));
    }

    Ok((
        Statement::For {
            variable,
            items,
            body,
        },
        done_line + 1,
    ))
}

/// Runner that executes a parsed script.
pub struct ScriptRunner;

impl ScriptRunner {
    /// Execute a parsed script using the provided executor.
    /// Returns the output of all commands concatenated, and the last exit code.
    pub fn run(script: &Script, executor: &mut dyn ScriptExecutor) -> (String, i32) {
        log::info!(
            "shell::script: running script with {} statement(s)",
            script.statements.len()
        );

        let mut output = String::new();
        let mut last_exit = 0i32;

        for stmt in &script.statements {
            let (out, code) = Self::execute_statement(stmt, executor);
            if !out.is_empty() {
                output.push_str(&out);
                if !out.ends_with('\n') {
                    output.push('\n');
                }
            }
            last_exit = code;
            executor.set_var("?", &alloc::format!("{}", code));
        }

        log::info!("shell::script: script finished with exit code {}", last_exit);
        (output, last_exit)
    }

    /// Execute a single statement.
    fn execute_statement(
        stmt: &Statement,
        executor: &mut dyn ScriptExecutor,
    ) -> (String, i32) {
        match stmt {
            Statement::Empty | Statement::Comment(_) => (String::new(), 0),

            Statement::Command(line) => {
                log::debug!("shell::script: exec command: {:?}", line);
                executor.execute_line(line)
            }

            Statement::Assignment { name, value } => {
                log::debug!("shell::script: assign {}={}", name, value);
                executor.set_var(name, value);
                (String::new(), 0)
            }

            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                log::debug!("shell::script: evaluating if condition: {:?}", condition);
                let (_, cond_code) = executor.execute_line(condition);

                let body = if cond_code == 0 {
                    log::debug!("shell::script: if condition true, executing then-body");
                    then_body
                } else {
                    log::debug!("shell::script: if condition false, executing else-body");
                    else_body
                };

                let mut output = String::new();
                let mut last_exit = 0;
                for s in body {
                    let (out, code) = Self::execute_statement(s, executor);
                    if !out.is_empty() {
                        output.push_str(&out);
                        if !out.ends_with('\n') {
                            output.push('\n');
                        }
                    }
                    last_exit = code;
                }
                (output, last_exit)
            }

            Statement::For {
                variable,
                items,
                body,
            } => {
                log::debug!(
                    "shell::script: for {} in {} items",
                    variable,
                    items.len()
                );

                let mut output = String::new();
                let mut last_exit = 0;

                for item in items {
                    log::trace!("shell::script: for iteration: {}={}", variable, item);
                    executor.set_var(variable, item);

                    for s in body {
                        let (out, code) = Self::execute_statement(s, executor);
                        if !out.is_empty() {
                            output.push_str(&out);
                            if !out.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                        last_exit = code;
                    }
                }

                (output, last_exit)
            }
        }
    }
}
