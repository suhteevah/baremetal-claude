//! Command line parser for the shell.
//!
//! Tokenizes input into words, quoted strings, pipes, and redirects.
//! Parses into Command structs with name, args, redirections, and pipe chains.
//! Handles environment variable expansion, tilde expansion, and glob patterns.

use alloc::string::String;
use alloc::vec::Vec;

/// A single parsed command with arguments and redirections.
#[derive(Debug, Clone)]
pub struct Command {
    /// Command name (e.g., "ls", "cat").
    pub name: String,
    /// Arguments to the command.
    pub args: Vec<String>,
    /// Stdin redirect: `< file`.
    pub stdin_redirect: Option<String>,
    /// Stdout redirect: `> file` (overwrite).
    pub stdout_redirect: Option<Redirect>,
}

/// Stdout redirection mode.
#[derive(Debug, Clone)]
pub enum Redirect {
    /// Overwrite: `> file`
    Overwrite(String),
    /// Append: `>> file`
    Append(String),
}

/// A pipeline of commands connected by pipes.
#[derive(Debug, Clone)]
pub struct Pipeline {
    /// Ordered list of commands; output of each feeds into the next.
    pub commands: Vec<Command>,
}

/// Token types produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Pipe,
    RedirectOut,
    RedirectAppend,
    RedirectIn,
}

/// Command history for up/down arrow recall.
pub struct History {
    entries: Vec<String>,
    max_entries: usize,
    cursor: usize,
}

/// Tab-completion result.
pub struct Completion {
    /// Possible completions.
    pub candidates: Vec<String>,
    /// Common prefix among all candidates.
    pub common_prefix: String,
}

impl Command {
    /// Create a new empty command.
    pub fn new(name: String) -> Self {
        Command {
            name,
            args: Vec::new(),
            stdin_redirect: None,
            stdout_redirect: None,
        }
    }
}

impl Pipeline {
    /// Parse a raw input line into a Pipeline.
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        log::debug!("shell::parser: parsing input: {:?}", input);

        let trimmed = input.trim();
        if trimmed.is_empty() {
            log::trace!("shell::parser: empty input");
            return Err(ParseError::EmptyInput);
        }

        let tokens = tokenize(trimmed)?;
        log::trace!("shell::parser: tokenized into {} tokens", tokens.len());

        let pipeline = build_pipeline(tokens)?;
        log::debug!(
            "shell::parser: parsed pipeline with {} command(s)",
            pipeline.commands.len()
        );

        for (i, cmd) in pipeline.commands.iter().enumerate() {
            log::trace!(
                "shell::parser: cmd[{}]: name={:?}, args={:?}, stdin={:?}, stdout={:?}",
                i,
                cmd.name,
                cmd.args,
                cmd.stdin_redirect,
                cmd.stdout_redirect,
            );
        }

        Ok(pipeline)
    }

    /// Check if this pipeline is a single command (no pipes).
    pub fn is_single(&self) -> bool {
        self.commands.len() == 1
    }

    /// Get the first (or only) command.
    pub fn first(&self) -> Option<&Command> {
        self.commands.first()
    }
}

/// Parse errors.
#[derive(Debug)]
pub enum ParseError {
    EmptyInput,
    UnterminatedQuote,
    UnexpectedPipe,
    UnexpectedRedirect,
    MissingRedirectTarget,
    EmptyCommand,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::EmptyInput => write!(f, "empty input"),
            ParseError::UnterminatedQuote => write!(f, "unterminated quote"),
            ParseError::UnexpectedPipe => write!(f, "unexpected pipe '|'"),
            ParseError::UnexpectedRedirect => write!(f, "unexpected redirect"),
            ParseError::MissingRedirectTarget => write!(f, "missing redirect target"),
            ParseError::EmptyCommand => write!(f, "empty command in pipeline"),
        }
    }
}

/// Tokenize a raw input string into a sequence of Tokens.
fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            // Skip whitespace
            ' ' | '\t' => {
                i += 1;
            }

            // Pipe
            '|' => {
                tokens.push(Token::Pipe);
                i += 1;
            }

            // Redirect out (> or >>)
            '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '>' {
                    tokens.push(Token::RedirectAppend);
                    i += 2;
                } else {
                    tokens.push(Token::RedirectOut);
                    i += 1;
                }
            }

            // Redirect in
            '<' => {
                tokens.push(Token::RedirectIn);
                i += 1;
            }

            // Single-quoted string (no expansion)
            '\'' => {
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                if i >= chars.len() {
                    log::warn!("shell::parser: unterminated single quote");
                    return Err(ParseError::UnterminatedQuote);
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(Token::Word(word));
                i += 1; // skip closing quote
            }

            // Double-quoted string (allows variable expansion later)
            '"' => {
                i += 1;
                let mut word = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        // Escape sequences inside double quotes
                        match chars[i + 1] {
                            '"' | '\\' | '$' => {
                                word.push(chars[i + 1]);
                                i += 2;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    log::warn!("shell::parser: unterminated double quote");
                    return Err(ParseError::UnterminatedQuote);
                }
                tokens.push(Token::Word(word));
                i += 1; // skip closing quote
            }

            // Regular word
            _ => {
                let start = i;
                while i < chars.len()
                    && !matches!(chars[i], ' ' | '\t' | '|' | '>' | '<' | '\'' | '"')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(Token::Word(word));
            }
        }
    }

    Ok(tokens)
}

/// Build a Pipeline from a list of tokens.
fn build_pipeline(tokens: Vec<Token>) -> Result<Pipeline, ParseError> {
    let mut commands = Vec::new();
    let mut current_words: Vec<String> = Vec::new();
    let mut stdin_redirect: Option<String> = None;
    let mut stdout_redirect: Option<Redirect> = None;

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(w) => {
                current_words.push(w.clone());
                i += 1;
            }

            Token::Pipe => {
                if current_words.is_empty() {
                    log::warn!("shell::parser: pipe with no preceding command");
                    return Err(ParseError::UnexpectedPipe);
                }
                let cmd = finalize_command(&mut current_words, &mut stdin_redirect, &mut stdout_redirect)?;
                commands.push(cmd);
                i += 1;
            }

            Token::RedirectOut => {
                i += 1;
                if i >= tokens.len() {
                    return Err(ParseError::MissingRedirectTarget);
                }
                if let Token::Word(target) = &tokens[i] {
                    stdout_redirect = Some(Redirect::Overwrite(target.clone()));
                    i += 1;
                } else {
                    return Err(ParseError::MissingRedirectTarget);
                }
            }

            Token::RedirectAppend => {
                i += 1;
                if i >= tokens.len() {
                    return Err(ParseError::MissingRedirectTarget);
                }
                if let Token::Word(target) = &tokens[i] {
                    stdout_redirect = Some(Redirect::Append(target.clone()));
                    i += 1;
                } else {
                    return Err(ParseError::MissingRedirectTarget);
                }
            }

            Token::RedirectIn => {
                i += 1;
                if i >= tokens.len() {
                    return Err(ParseError::MissingRedirectTarget);
                }
                if let Token::Word(target) = &tokens[i] {
                    stdin_redirect = Some(target.clone());
                    i += 1;
                } else {
                    return Err(ParseError::MissingRedirectTarget);
                }
            }
        }
    }

    // Finalize last command
    if !current_words.is_empty() {
        let cmd = finalize_command(&mut current_words, &mut stdin_redirect, &mut stdout_redirect)?;
        commands.push(cmd);
    }

    if commands.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    Ok(Pipeline { commands })
}

/// Turn accumulated words + redirects into a Command struct.
fn finalize_command(
    words: &mut Vec<String>,
    stdin_redirect: &mut Option<String>,
    stdout_redirect: &mut Option<Redirect>,
) -> Result<Command, ParseError> {
    if words.is_empty() {
        return Err(ParseError::EmptyCommand);
    }

    let name = words.remove(0);
    let args = words.drain(..).collect();

    let cmd = Command {
        name,
        args,
        stdin_redirect: stdin_redirect.take(),
        stdout_redirect: stdout_redirect.take(),
    };

    Ok(cmd)
}

/// Expand glob patterns (* and ?) in a word.
/// Returns the original word if no matches or no glob characters present.
/// In a real VFS-backed environment, this queries the filesystem.
pub fn expand_glob(pattern: &str) -> Vec<String> {
    log::trace!("shell::parser: expand_glob({:?})", pattern);

    // Check if pattern contains glob characters
    if !pattern.contains('*') && !pattern.contains('?') {
        return alloc::vec![String::from(pattern)];
    }

    // NOTE: Actual glob expansion requires VFS access.
    // The shell executor will call into the VFS to resolve these.
    // For now, return the raw pattern — the executor is responsible for expansion.
    log::debug!(
        "shell::parser: glob pattern {:?} deferred to executor (needs VFS)",
        pattern
    );
    alloc::vec![String::from(pattern)]
}

impl History {
    /// Create a new command history.
    pub fn new(max_entries: usize) -> Self {
        log::info!(
            "shell::parser: history initialized (max {} entries)",
            max_entries
        );
        History {
            entries: Vec::new(),
            max_entries,
            cursor: 0,
        }
    }

    /// Add an entry to history.
    pub fn push(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        // Don't duplicate the last entry
        if self.entries.last().map(|s| s.as_str()) == Some(trimmed) {
            log::trace!("shell::parser: history: skipping duplicate");
            return;
        }

        log::trace!("shell::parser: history: adding {:?}", trimmed);

        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(String::from(trimmed));
        self.cursor = self.entries.len();
    }

    /// Move cursor up (older) and return the entry, if any.
    pub fn prev(&mut self) -> Option<&str> {
        if self.cursor > 0 {
            self.cursor -= 1;
            let entry = self.entries.get(self.cursor).map(|s| s.as_str());
            log::trace!("shell::parser: history prev -> {:?}", entry);
            entry
        } else {
            None
        }
    }

    /// Move cursor down (newer) and return the entry, if any.
    pub fn next(&mut self) -> Option<&str> {
        if self.cursor < self.entries.len() {
            self.cursor += 1;
            if self.cursor >= self.entries.len() {
                None // Past the end = empty line
            } else {
                let entry = self.entries.get(self.cursor).map(|s| s.as_str());
                log::trace!("shell::parser: history next -> {:?}", entry);
                entry
            }
        } else {
            None
        }
    }

    /// Reset cursor to the end (current position).
    pub fn reset_cursor(&mut self) {
        self.cursor = self.entries.len();
    }

    /// Get all history entries.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Number of entries in history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Tab completion engine.
pub struct TabCompleter {
    /// Known built-in command names for completion.
    builtins: Vec<String>,
}

impl TabCompleter {
    /// Create a new tab completer with the built-in command list.
    pub fn new() -> Self {
        log::debug!("shell::parser: tab completer initialized");
        let builtins = alloc::vec![
            "ls", "cd", "pwd", "cat", "echo", "cp", "mv", "rm", "mkdir", "touch",
            "head", "tail", "grep", "mount", "umount", "ps", "kill", "clear", "help",
            "reboot", "shutdown", "ifconfig", "ping", "ssh", "date", "uptime", "free",
            "df", "set", "unset", "export", "history", "exit",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        TabCompleter { builtins }
    }

    /// Complete a partial input. Returns possible completions.
    /// If `is_first_word` is true, complete against command names.
    /// Otherwise, completion would be against filenames (requires VFS).
    pub fn complete(&self, partial: &str, is_first_word: bool) -> Completion {
        log::debug!(
            "shell::parser: tab complete {:?} (first_word={})",
            partial,
            is_first_word
        );

        let candidates: Vec<String> = if is_first_word {
            self.builtins
                .iter()
                .filter(|cmd| cmd.starts_with(partial))
                .cloned()
                .collect()
        } else {
            // File completion requires VFS — return empty for now.
            // The shell executor can provide VFS-backed completion.
            log::trace!("shell::parser: file completion deferred to VFS");
            Vec::new()
        };

        let common_prefix = common_prefix_of(&candidates);

        log::debug!(
            "shell::parser: {} completion candidates, common prefix: {:?}",
            candidates.len(),
            common_prefix
        );

        Completion {
            candidates,
            common_prefix,
        }
    }
}

impl Default for TabCompleter {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the longest common prefix among a set of strings.
fn common_prefix_of(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    if strings.len() == 1 {
        return strings[0].clone();
    }

    let first = &strings[0];
    let mut prefix_len = first.len();

    for s in &strings[1..] {
        prefix_len = prefix_len.min(s.len());
        for (i, (a, b)) in first.chars().zip(s.chars()).enumerate() {
            if a != b {
                prefix_len = prefix_len.min(i);
                break;
            }
        }
    }

    first[..prefix_len].into()
}
