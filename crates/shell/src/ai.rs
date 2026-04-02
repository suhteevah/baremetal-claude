//! AI-native command processing.
//!
//! Detects whether user input is a traditional shell command or a natural language
//! query. Natural language queries are routed to a Claude agent which interprets
//! the request and generates shell commands. The user confirms before execution.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

/// Known built-in command names used for heuristic detection.
const KNOWN_COMMANDS: &[&str] = &[
    "ls", "cd", "pwd", "cat", "echo", "cp", "mv", "rm", "mkdir", "touch",
    "head", "tail", "grep", "mount", "umount", "ps", "kill", "clear", "help",
    "reboot", "shutdown", "ifconfig", "ping", "ssh", "date", "uptime", "free",
    "df", "set", "unset", "export", "history", "exit",
];

/// Classification of user input.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// Traditional shell command (starts with a known command name).
    Command,
    /// Natural language query to be interpreted by AI.
    NaturalLanguage,
    /// Empty input.
    Empty,
}

/// A proposed action from the AI, pending user confirmation.
#[derive(Debug, Clone)]
pub struct AiProposal {
    /// Human-readable explanation of what the AI intends to do.
    pub explanation: String,
    /// Shell commands the AI wants to execute.
    pub commands: Vec<String>,
    /// Whether the user has confirmed execution.
    pub confirmed: bool,
}

/// Callback trait for wiring the AI shell to the Claude agent API.
/// The shell calls these methods; the kernel/agent layer implements them.
pub trait AiShellCallback {
    /// Send a natural language query to the Claude agent and receive a proposal.
    /// The agent interprets the user's intent and suggests shell commands.
    fn interpret(&mut self, query: &str, context: &AiContext) -> Result<AiProposal, String>;

    /// Check if the AI backend is available (agent session active, API reachable).
    fn is_available(&self) -> bool;
}

/// Context passed to the AI for better interpretation.
#[derive(Debug, Clone)]
pub struct AiContext {
    /// Current working directory.
    pub pwd: String,
    /// Recent command history (last N entries).
    pub recent_history: Vec<String>,
    /// Environment variables that might be relevant.
    pub env_vars: Vec<(String, String)>,
}

/// Classify user input as a command or natural language.
pub fn classify_input(input: &str) -> InputKind {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        log::trace!("shell::ai: input classified as Empty");
        return InputKind::Empty;
    }

    // Get the first word
    let first_word = trimmed.split_whitespace().next().unwrap_or("");

    // Check against known command names
    if KNOWN_COMMANDS.contains(&first_word) {
        log::debug!(
            "shell::ai: input classified as Command (matched {:?})",
            first_word
        );
        return InputKind::Command;
    }

    // Heuristic: if it starts with a path-like token or contains '/', treat as command
    if first_word.starts_with('/') || first_word.starts_with("./") {
        log::debug!("shell::ai: input classified as Command (path-like)");
        return InputKind::Command;
    }

    // Heuristic: variable assignment (VAR=value)
    if first_word.contains('=') && !first_word.starts_with('=') {
        log::debug!("shell::ai: input classified as Command (assignment)");
        return InputKind::Command;
    }

    // Everything else is natural language
    log::debug!("shell::ai: input classified as NaturalLanguage");
    InputKind::NaturalLanguage
}

/// Process a natural language input through the AI callback.
/// Returns the AI's proposal for the user to confirm.
pub fn process_natural_language(
    input: &str,
    context: &AiContext,
    callback: &mut dyn AiShellCallback,
) -> Result<AiProposal, String> {
    log::info!("shell::ai: processing natural language query: {:?}", input);

    if !callback.is_available() {
        log::warn!("shell::ai: AI backend not available");
        return Err(String::from(
            "AI assistant not available. Is an agent session active?",
        ));
    }

    log::debug!(
        "shell::ai: sending to AI with context: pwd={:?}, history_len={}",
        context.pwd,
        context.recent_history.len()
    );

    let proposal = callback.interpret(input, context)?;

    log::info!(
        "shell::ai: proposal received — {} command(s): {:?}",
        proposal.commands.len(),
        proposal.commands
    );
    log::debug!("shell::ai: explanation: {}", proposal.explanation);

    Ok(proposal)
}

/// Format an AI proposal for display to the user.
pub fn format_proposal(proposal: &AiProposal) -> String {
    log::trace!("shell::ai: formatting proposal for display");

    let mut output = String::new();

    output.push_str("\x1b[1;36m[AI Assistant]\x1b[0m ");
    output.push_str(&proposal.explanation);
    output.push('\n');
    output.push('\n');

    output.push_str("\x1b[1;33mProposed commands:\x1b[0m\n");
    for (i, cmd) in proposal.commands.iter().enumerate() {
        output.push_str(&format!("  \x1b[32m{}\x1b[0m. {}\n", i + 1, cmd));
    }

    output.push('\n');
    output.push_str("Execute? [y/N] ");

    output
}

/// A no-op AI callback that always returns unavailable.
/// Used when no agent session is wired to the shell.
pub struct NoOpAiCallback;

impl AiShellCallback for NoOpAiCallback {
    fn interpret(&mut self, _query: &str, _context: &AiContext) -> Result<AiProposal, String> {
        log::debug!("shell::ai: NoOpAiCallback::interpret called — returning error");
        Err(String::from("No AI agent connected to this shell session."))
    }

    fn is_available(&self) -> bool {
        false
    }
}
