//! Shell prompt rendering with ANSI color support.
//!
//! Configurable prompt format, defaults to `[user@claudio path]$ `.
//! Supports showing the active agent name when in an agent context.

use alloc::string::String;
use alloc::format;

use crate::env::Environment;

/// ANSI color codes for prompt rendering.
pub mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";
}

/// Shell prompt generator.
pub struct Prompt {
    /// Whether colors are enabled.
    use_color: bool,
    /// Optional agent context name (shown when shell is inside an agent pane).
    agent_name: Option<String>,
    /// Custom format string. If None, use the default format.
    custom_format: Option<String>,
}

impl Prompt {
    /// Create a new prompt with default settings.
    pub fn new() -> Self {
        log::info!("shell::prompt: initialized with default format");
        Prompt {
            use_color: true,
            agent_name: None,
            custom_format: None,
        }
    }

    /// Enable or disable ANSI color output.
    pub fn set_color(&mut self, enabled: bool) {
        log::debug!("shell::prompt: color={}", enabled);
        self.use_color = enabled;
    }

    /// Set the active agent name (shown in prompt when non-None).
    pub fn set_agent(&mut self, name: Option<String>) {
        log::debug!("shell::prompt: agent={:?}", name);
        self.agent_name = name;
    }

    /// Set a custom prompt format string.
    /// Supported tokens: {user}, {host}, {path}, {agent}, {$}
    pub fn set_format(&mut self, format: String) {
        log::debug!("shell::prompt: custom format={:?}", format);
        self.custom_format = Some(format);
    }

    /// Render the prompt string given the current environment.
    pub fn render(&self, env: &Environment) -> String {
        log::trace!("shell::prompt: rendering prompt");

        let user = env.get("USER").unwrap_or("root");
        let host = env.get("HOSTNAME").unwrap_or("claudio");
        let pwd = env.pwd();

        // Shorten path: replace HOME prefix with ~
        let home = env.home();
        let display_path = if pwd == home {
            String::from("~")
        } else if let Some(rest) = pwd.strip_prefix(home) {
            format!("~{}", rest)
        } else {
            String::from(pwd)
        };

        // Privilege indicator
        let privilege = if user == "root" { "#" } else { "$" };

        if let Some(ref fmt) = self.custom_format {
            return self.render_custom(fmt, user, host, &display_path, privilege);
        }

        // Default format: [user@claudio path]$ or [user@claudio path](agent)$
        if self.use_color {
            let agent_part = match &self.agent_name {
                Some(name) => format!(
                    "{}({}){}",
                    colors::MAGENTA,
                    name,
                    colors::RESET
                ),
                None => String::new(),
            };

            format!(
                "{}{}[{}{}{}@{}{}{}{}{}]{}{} ",
                colors::BOLD,
                colors::GREEN,
                colors::CYAN,
                user,
                colors::GREEN,
                colors::YELLOW,
                host,
                colors::GREEN,
                colors::BLUE,
                display_path,
                agent_part,
                privilege,
            ) + colors::RESET
        } else {
            let agent_part = match &self.agent_name {
                Some(name) => format!("({})", name),
                None => String::new(),
            };
            format!("[{}@{} {}]{}{} ", user, host, display_path, agent_part, privilege)
        }
    }

    /// Render a custom format string with token substitution.
    fn render_custom(
        &self,
        fmt: &str,
        user: &str,
        host: &str,
        path: &str,
        privilege: &str,
    ) -> String {
        log::trace!("shell::prompt: rendering custom format");

        let agent = self
            .agent_name
            .as_deref()
            .unwrap_or("");

        let result = fmt
            .replace("{user}", user)
            .replace("{host}", host)
            .replace("{path}", path)
            .replace("{agent}", agent)
            .replace("{$}", privilege);

        result
    }
}

impl Default for Prompt {
    fn default() -> Self {
        Self::new()
    }
}
