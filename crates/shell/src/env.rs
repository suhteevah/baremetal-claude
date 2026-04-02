//! Environment variable management for the shell.
//!
//! Maintains a HashMap of key=value pairs including PATH, HOME, USER, etc.
//! Supports set, unset, export, and config file loading.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Environment variable store.
pub struct Environment {
    /// All environment variables (key -> value).
    vars: BTreeMap<String, String>,
    /// Variables marked as exported (visible to child commands).
    exported: BTreeMap<String, bool>,
}

impl Environment {
    /// Create a new environment with sensible defaults for ClaudioOS.
    pub fn new() -> Self {
        log::info!("shell::env: initializing default environment");

        let mut vars = BTreeMap::new();
        vars.insert(String::from("HOME"), String::from("/"));
        vars.insert(String::from("USER"), String::from("root"));
        vars.insert(String::from("HOSTNAME"), String::from("claudio"));
        vars.insert(String::from("PWD"), String::from("/"));
        vars.insert(String::from("PATH"), String::from("/bin:/usr/bin"));
        vars.insert(String::from("SHELL"), String::from("/bin/claudio-shell"));
        vars.insert(String::from("TERM"), String::from("claudio-term"));
        vars.insert(String::from("LANG"), String::from("en_US.UTF-8"));

        log::debug!(
            "shell::env: defaults set — HOME=/, USER=root, HOSTNAME=claudio, PWD=/"
        );

        Environment {
            vars,
            exported: BTreeMap::new(),
        }
    }

    /// Get the value of an environment variable.
    pub fn get(&self, key: &str) -> Option<&str> {
        let val = self.vars.get(key).map(|s| s.as_str());
        log::trace!("shell::env: get({}) -> {:?}", key, val);
        val
    }

    /// Set an environment variable.
    pub fn set(&mut self, key: &str, value: &str) {
        log::debug!("shell::env: set {}={}", key, value);
        self.vars.insert(String::from(key), String::from(value));
    }

    /// Unset (remove) an environment variable.
    pub fn unset(&mut self, key: &str) {
        log::debug!("shell::env: unset {}", key);
        self.vars.remove(key);
        self.exported.remove(key);
    }

    /// Mark a variable as exported.
    pub fn export(&mut self, key: &str) {
        log::debug!("shell::env: export {}", key);
        self.exported.insert(String::from(key), true);
    }

    /// Set and export in one call (like `export VAR=value`).
    pub fn set_export(&mut self, key: &str, value: &str) {
        self.set(key, value);
        self.export(key);
    }

    /// Check if a variable is exported.
    pub fn is_exported(&self, key: &str) -> bool {
        self.exported.get(key).copied().unwrap_or(false)
    }

    /// Get the current working directory.
    pub fn pwd(&self) -> &str {
        self.get("PWD").unwrap_or("/")
    }

    /// Set the current working directory.
    pub fn set_pwd(&mut self, path: &str) {
        log::info!("shell::env: pwd changed to {}", path);
        self.set("PWD", path);
    }

    /// Get the HOME directory.
    pub fn home(&self) -> &str {
        self.get("HOME").unwrap_or("/")
    }

    /// Get PATH entries as a vector.
    pub fn path_entries(&self) -> Vec<&str> {
        match self.get("PATH") {
            Some(path) => path.split(':').collect(),
            None => Vec::new(),
        }
    }

    /// Return all variables as key=value pairs (for display / debugging).
    pub fn all(&self) -> Vec<(&str, &str)> {
        self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    /// Return only exported variables.
    pub fn exported_vars(&self) -> Vec<(&str, &str)> {
        self.vars
            .iter()
            .filter(|(k, _)| self.is_exported(k))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// Expand environment variable references in a string.
    /// Handles $VAR and ${VAR} syntax.
    pub fn expand_vars(&self, input: &str) -> String {
        log::trace!("shell::env: expanding vars in {:?}", input);

        let mut result = String::new();
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() {
                if chars[i + 1] == '{' {
                    // ${VAR} syntax
                    if let Some(close) = chars[i + 2..].iter().position(|&c| c == '}') {
                        let var_name: String = chars[i + 2..i + 2 + close].iter().collect();
                        if let Some(val) = self.get(&var_name) {
                            result.push_str(val);
                        }
                        i = i + 3 + close;
                        continue;
                    }
                } else if chars[i + 1] == '?' {
                    // $? — last exit code, handled by shell, placeholder here
                    result.push_str("0");
                    i += 2;
                    continue;
                } else {
                    // $VAR syntax
                    let start = i + 1;
                    let mut end = start;
                    while end < chars.len()
                        && (chars[end].is_alphanumeric() || chars[end] == '_')
                    {
                        end += 1;
                    }
                    if end > start {
                        let var_name: String = chars[start..end].iter().collect();
                        if let Some(val) = self.get(&var_name) {
                            result.push_str(val);
                        }
                        i = end;
                        continue;
                    }
                }
            }

            if chars[i] == '~' && (i == 0 || chars[i - 1] == ' ' || chars[i - 1] == '=') {
                // Tilde expansion
                if i + 1 >= chars.len() || chars[i + 1] == '/' || chars[i + 1] == ' ' {
                    result.push_str(self.home());
                    i += 1;
                    continue;
                }
            }

            result.push(chars[i]);
            i += 1;
        }

        log::trace!("shell::env: expanded to {:?}", result);
        result
    }

    /// Parse and apply lines from a config file (key=value per line, # comments).
    pub fn load_config(&mut self, content: &str) {
        log::info!("shell::env: loading config ({} bytes)", content.len());

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Handle "export KEY=VALUE"
            let (is_export, assignment) = if let Some(rest) = trimmed.strip_prefix("export ") {
                (true, rest.trim())
            } else {
                (false, trimmed)
            };

            if let Some(eq_pos) = assignment.find('=') {
                let key = assignment[..eq_pos].trim();
                let value = assignment[eq_pos + 1..].trim();
                // Strip surrounding quotes if present
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value);

                log::debug!("shell::env: config: {}={} (export={})", key, value, is_export);
                self.set(key, value);
                if is_export {
                    self.export(key);
                }
            }
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}
