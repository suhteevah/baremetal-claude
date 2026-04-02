//! Pipes and I/O redirection for the shell.
//!
//! Implements pipe buffers (Vec<u8> between commands), stdout redirection
//! (> overwrite, >> append), and stdin redirection (< file).

use alloc::string::String;
use alloc::vec::Vec;

use crate::parser::{Pipeline, Redirect};

/// Result of executing a single command in a pipeline.
pub struct CommandOutput {
    /// Stdout content.
    pub stdout: Vec<u8>,
    /// Stderr content.
    pub stderr: Vec<u8>,
    /// Exit code (0 = success).
    pub exit_code: i32,
}

impl CommandOutput {
    /// Create a successful output with the given stdout.
    pub fn success(stdout: Vec<u8>) -> Self {
        CommandOutput {
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        }
    }

    /// Create an error output.
    pub fn error(message: &str, code: i32) -> Self {
        CommandOutput {
            stdout: Vec::new(),
            stderr: message.as_bytes().to_vec(),
            exit_code: code,
        }
    }

    /// Get stdout as a string (lossy).
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Get stderr as a string (lossy).
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Trait for executing a single command, provided by the shell.
/// The pipeline executor calls this for each command in the chain.
pub trait CommandExecutor {
    /// Execute a command with the given name, args, and optional stdin.
    /// Returns the command's output.
    fn execute(
        &mut self,
        name: &str,
        args: &[String],
        stdin: Option<&[u8]>,
    ) -> CommandOutput;

    /// Read a file's contents (for `< file` redirection).
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String>;

    /// Write data to a file (for `> file` redirection), overwriting.
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), String>;

    /// Append data to a file (for `>> file` redirection).
    fn append_file(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
}

/// Executes a full pipeline, wiring pipes and redirections between commands.
pub struct PipelineExecutor;

impl PipelineExecutor {
    /// Execute a pipeline using the provided command executor.
    /// Returns the final output of the last command in the chain.
    pub fn execute(
        pipeline: &Pipeline,
        executor: &mut dyn CommandExecutor,
    ) -> CommandOutput {
        log::info!(
            "shell::pipe: executing pipeline with {} command(s)",
            pipeline.commands.len()
        );

        if pipeline.commands.is_empty() {
            log::warn!("shell::pipe: empty pipeline");
            return CommandOutput::error("empty pipeline", 1);
        }

        let mut pipe_input: Option<Vec<u8>> = None;

        for (i, cmd) in pipeline.commands.iter().enumerate() {
            let is_last = i == pipeline.commands.len() - 1;

            log::debug!(
                "shell::pipe: executing command {}/{}: {:?}",
                i + 1,
                pipeline.commands.len(),
                cmd.name
            );

            // Determine stdin: either from pipe or from redirect
            let stdin_data = if let Some(ref path) = cmd.stdin_redirect {
                log::debug!("shell::pipe: reading stdin from file {:?}", path);
                match executor.read_file(path) {
                    Ok(data) => {
                        log::trace!(
                            "shell::pipe: stdin redirect read {} bytes",
                            data.len()
                        );
                        Some(data)
                    }
                    Err(e) => {
                        log::error!("shell::pipe: stdin redirect failed: {}", e);
                        return CommandOutput::error(
                            &alloc::format!("{}: {}: {}", cmd.name, path, e),
                            1,
                        );
                    }
                }
            } else {
                pipe_input.take()
            };

            // Execute the command
            let output = executor.execute(
                &cmd.name,
                &cmd.args,
                stdin_data.as_deref(),
            );

            log::trace!(
                "shell::pipe: cmd {:?} exited with code {}, stdout={} bytes, stderr={} bytes",
                cmd.name,
                output.exit_code,
                output.stdout.len(),
                output.stderr.len(),
            );

            // Handle stdout redirect (only for the last command, or per-command)
            if let Some(ref redirect) = cmd.stdout_redirect {
                let result = match redirect {
                    Redirect::Overwrite(path) => {
                        log::debug!("shell::pipe: redirecting stdout to {:?} (overwrite)", path);
                        executor.write_file(path, &output.stdout)
                    }
                    Redirect::Append(path) => {
                        log::debug!("shell::pipe: redirecting stdout to {:?} (append)", path);
                        executor.append_file(path, &output.stdout)
                    }
                };

                if let Err(e) = result {
                    log::error!("shell::pipe: stdout redirect failed: {}", e);
                    return CommandOutput::error(
                        &alloc::format!("{}: redirect: {}", cmd.name, e),
                        1,
                    );
                }

                // If not the last command, the redirect consumed stdout,
                // so the next command gets nothing from pipe.
                if !is_last {
                    pipe_input = None;
                }

                // For last command with redirect, return empty stdout (it went to file).
                if is_last {
                    return CommandOutput {
                        stdout: Vec::new(),
                        stderr: output.stderr,
                        exit_code: output.exit_code,
                    };
                }
            } else if !is_last {
                // Feed stdout into the next command's stdin via pipe buffer
                log::trace!(
                    "shell::pipe: piping {} bytes to next command",
                    output.stdout.len()
                );
                pipe_input = Some(output.stdout);
            } else {
                // Last command, no redirect — return output as-is
                return output;
            }

            // If a command in the middle of the pipeline fails, we still continue
            // (like real Unix shells) but log the error.
            if output.exit_code != 0 {
                log::warn!(
                    "shell::pipe: command {:?} exited with non-zero code {}",
                    cmd.name,
                    output.exit_code
                );
            }
        }

        // Should not reach here, but just in case
        CommandOutput::error("pipeline execution error", 1)
    }
}
