//! Tool permission enforcement for multi-agent safety.
//!
//! Ports permission patterns from claw-code into ClaudioOS's tool system.
//! Each agent can be assigned a [`PermissionMode`] that restricts which tools
//! it may invoke and what file paths it may access.
//!
//! Usage from the kernel / agent session manager:
//! ```ignore
//! let enforcer = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/workspace".into());
//! if let PermissionResult::Denied { reason, .. } = enforcer.check_tool("file_write", &input) {
//!     log::warn!("tool denied: {}", reason);
//!     // return error to agent
//! }
//! ```

use alloc::format;
use alloc::string::String;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Permission levels
// ---------------------------------------------------------------------------

/// Permission level governing what an agent session is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Only read operations allowed.
    ReadOnly,
    /// File modifications within workspace only.
    WorkspaceWrite,
    /// Unrestricted access.
    FullAccess,
}

// ---------------------------------------------------------------------------
// Permission result
// ---------------------------------------------------------------------------

/// Outcome of a permission check.
#[derive(Debug, Clone)]
pub enum PermissionResult {
    /// The operation is permitted.
    Allowed,
    /// The operation was denied.
    Denied {
        /// Name of the tool that was denied.
        tool: String,
        /// Human-readable explanation of why the check failed.
        reason: String,
        /// The minimum permission level required for this operation.
        required: PermissionMode,
    },
}

impl PermissionResult {
    /// Returns `true` if the result is [`PermissionResult::Allowed`].
    pub fn is_allowed(&self) -> bool {
        matches!(self, PermissionResult::Allowed)
    }

    /// Returns `true` if the result is [`PermissionResult::Denied`].
    pub fn is_denied(&self) -> bool {
        matches!(self, PermissionResult::Denied { .. })
    }
}

// ---------------------------------------------------------------------------
// Tool → permission mapping
// ---------------------------------------------------------------------------

/// Return the minimum [`PermissionMode`] required to invoke a given tool.
pub fn required_permission(tool_name: &str) -> PermissionMode {
    match tool_name {
        "file_read" | "list_directory" => PermissionMode::ReadOnly,
        "file_write" | "execute_command" | "compile_rust" => PermissionMode::WorkspaceWrite,
        "execute_python" | "execute_javascript" => PermissionMode::WorkspaceWrite,
        _ => PermissionMode::FullAccess,
    }
}

// ---------------------------------------------------------------------------
// Read-only bash heuristic
// ---------------------------------------------------------------------------

/// Commands considered safe in [`PermissionMode::ReadOnly`].
const READ_ONLY_COMMANDS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "sort",
    "uniq",
    "grep",
    "rg",
    "find",
    "fd",
    "ls",
    "dir",
    "tree",
    "file",
    "stat",
    "du",
    "df",
    "which",
    "where",
    "type",
    "echo",
    "printf",
    "date",
    "whoami",
    "hostname",
    "uname",
    "env",
    "printenv",
    "git",
    "cargo",
    "rustc",
    "python",
    "node",
    "npm",
    "yarn",
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "xxd",
    "hexdump",
    "curl",
    "wget", // read-only network ops
    "jq",
    "yq", // JSON/YAML query tools
    "awk",
    "sed", // only if no -i flag (checked below)
];

/// Determine whether a bash command is safe to run under [`PermissionMode::ReadOnly`].
///
/// Uses a whitelist of known read-only commands and blocks write-indicating
/// flags and shell redirection operators.
fn is_read_only_bash(command: &str) -> bool {
    let first_token = match command.split_whitespace().next() {
        Some(t) => t,
        None => return false,
    };

    // Check if the base command is in the whitelist.
    if !READ_ONLY_COMMANDS.contains(&first_token) {
        return false;
    }

    // Block write-indicating flags.
    if command.contains(" -i ") || command.contains("--in-place") {
        return false;
    }

    // Block shell redirection operators.
    if command.contains(" > ") || command.contains(" >> ") {
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// Workspace boundary check
// ---------------------------------------------------------------------------

/// Check whether `path` is inside `workspace_root`.
///
/// Normalises path separators so this works on both Unix and Windows-style
/// paths (ClaudioOS may receive either from agent tool calls).
fn is_within_workspace(path: &str, workspace_root: &str) -> bool {
    // Normalise separators.
    let path = path.replace('\\', "/");
    let mut root = workspace_root.replace('\\', "/");
    // Ensure the root ends with '/' so "/workspace" doesn't match "/workspace2".
    if !root.ends_with('/') {
        root.push('/');
    }

    path.replace('\\', "/").starts_with(&root)
        || path.replace('\\', "/") == root.trim_end_matches('/')
}

// ---------------------------------------------------------------------------
// Permission enforcer
// ---------------------------------------------------------------------------

/// Stateful permission enforcer scoped to one agent session.
///
/// Constructed with a [`PermissionMode`] and workspace root path. All tool
/// calls for that agent should be checked via [`check_tool`] before execution.
pub struct PermissionEnforcer {
    mode: PermissionMode,
    workspace_root: String,
}

impl PermissionEnforcer {
    /// Create a new enforcer.
    pub fn new(mode: PermissionMode, workspace_root: String) -> Self {
        Self {
            mode,
            workspace_root,
        }
    }

    /// Return the current permission mode.
    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// Return the configured workspace root.
    pub fn workspace_root(&self) -> &str {
        &self.workspace_root
    }

    /// Check if a tool call is allowed under current permissions.
    ///
    /// Inspects the tool name and its JSON input to decide. For
    /// `execute_command` in [`PermissionMode::ReadOnly`] the bash command is
    /// additionally analysed via [`is_read_only_bash`].
    pub fn check_tool(&self, tool_name: &str, input: &Value) -> PermissionResult {
        let required = required_permission(tool_name);

        // FullAccess always passes.
        if self.mode == PermissionMode::FullAccess {
            return PermissionResult::Allowed;
        }

        // Check permission level hierarchy: ReadOnly < WorkspaceWrite < FullAccess.
        let mode_level = permission_level(self.mode);
        let required_level = permission_level(required);

        if required_level > mode_level {
            // Special case: execute_command may be allowed in ReadOnly if the
            // command itself is read-only.
            if tool_name == "execute_command" && self.mode == PermissionMode::ReadOnly {
                let cmd = input.get("command").and_then(Value::as_str).unwrap_or("");
                if is_read_only_bash(cmd) {
                    return PermissionResult::Allowed;
                }
            }

            return PermissionResult::Denied {
                tool: String::from(tool_name),
                reason: format!(
                    "tool '{}' requires {:?} but agent has {:?}",
                    tool_name, required, self.mode
                ),
                required,
            };
        }

        // WorkspaceWrite: verify file paths are within workspace.
        if self.mode == PermissionMode::WorkspaceWrite {
            if let Some(path) = extract_path_from_input(tool_name, input) {
                if !is_within_workspace(&path, &self.workspace_root) {
                    return PermissionResult::Denied {
                        tool: String::from(tool_name),
                        reason: format!(
                            "path '{}' is outside workspace '{}'",
                            path, self.workspace_root
                        ),
                        required: PermissionMode::FullAccess,
                    };
                }
            }
        }

        PermissionResult::Allowed
    }

    /// Check if a file write to `path` is allowed.
    pub fn check_file_write(&self, path: &str) -> PermissionResult {
        match self.mode {
            PermissionMode::FullAccess => PermissionResult::Allowed,
            PermissionMode::WorkspaceWrite => {
                if is_within_workspace(path, &self.workspace_root) {
                    PermissionResult::Allowed
                } else {
                    PermissionResult::Denied {
                        tool: String::from("file_write"),
                        reason: format!(
                            "path '{}' is outside workspace '{}'",
                            path, self.workspace_root
                        ),
                        required: PermissionMode::FullAccess,
                    }
                }
            }
            PermissionMode::ReadOnly => PermissionResult::Denied {
                tool: String::from("file_write"),
                reason: String::from("file writes are not allowed in ReadOnly mode"),
                required: PermissionMode::WorkspaceWrite,
            },
        }
    }

    /// Check if a bash command is allowed in [`PermissionMode::ReadOnly`].
    ///
    /// In other modes this always returns [`PermissionResult::Allowed`].
    pub fn check_bash(&self, command: &str) -> PermissionResult {
        match self.mode {
            PermissionMode::FullAccess | PermissionMode::WorkspaceWrite => {
                PermissionResult::Allowed
            }
            PermissionMode::ReadOnly => {
                if is_read_only_bash(command) {
                    PermissionResult::Allowed
                } else {
                    PermissionResult::Denied {
                        tool: String::from("execute_command"),
                        reason: format!("command '{}' is not in the read-only whitelist", command),
                        required: PermissionMode::WorkspaceWrite,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map permission modes to numeric levels for ordering.
fn permission_level(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::ReadOnly => 0,
        PermissionMode::WorkspaceWrite => 1,
        PermissionMode::FullAccess => 2,
    }
}

/// Try to extract the file path from a tool's JSON input.
fn extract_path_from_input(tool_name: &str, input: &Value) -> Option<String> {
    match tool_name {
        "file_read" | "file_write" | "list_directory" => {
            input.get("path").and_then(Value::as_str).map(String::from)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_access_allows_everything() {
        let e = PermissionEnforcer::new(PermissionMode::FullAccess, "/ws".into());
        assert!(e
            .check_tool("file_write", &json!({"path": "/etc/passwd"}))
            .is_allowed());
        assert!(e.check_tool("unknown_tool", &json!({})).is_allowed());
    }

    #[test]
    fn read_only_allows_reads() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        assert!(e
            .check_tool("file_read", &json!({"path": "/ws/foo.rs"}))
            .is_allowed());
        assert!(e
            .check_tool("list_directory", &json!({"path": "/ws"}))
            .is_allowed());
    }

    #[test]
    fn read_only_denies_writes() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        let r = e.check_tool("file_write", &json!({"path": "/ws/foo.rs", "content": "x"}));
        assert!(r.is_denied());
    }

    #[test]
    fn read_only_allows_safe_bash() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        let r = e.check_tool("execute_command", &json!({"command": "cat /ws/foo.rs"}));
        assert!(r.is_allowed());
    }

    #[test]
    fn read_only_denies_unsafe_bash() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        let r = e.check_tool("execute_command", &json!({"command": "rm -rf /"}));
        assert!(r.is_denied());
    }

    #[test]
    fn read_only_denies_bash_with_redirect() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        let r = e.check_bash("cat foo > bar");
        assert!(r.is_denied());
    }

    #[test]
    fn read_only_denies_sed_in_place() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        let r = e.check_bash("sed -i 's/a/b/' file.txt");
        assert!(r.is_denied());
    }

    #[test]
    fn workspace_write_allows_within_workspace() {
        let e = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/workspace".into());
        let r = e.check_tool(
            "file_write",
            &json!({"path": "/workspace/src/main.rs", "content": "x"}),
        );
        assert!(r.is_allowed());
    }

    #[test]
    fn workspace_write_denies_outside_workspace() {
        let e = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/workspace".into());
        let r = e.check_tool(
            "file_write",
            &json!({"path": "/etc/passwd", "content": "x"}),
        );
        assert!(r.is_denied());
    }

    #[test]
    fn workspace_boundary_no_prefix_attack() {
        // "/workspace2/foo" should NOT match workspace "/workspace"
        assert!(!is_within_workspace("/workspace2/foo", "/workspace"));
    }

    #[test]
    fn workspace_boundary_windows_paths() {
        assert!(is_within_workspace(
            "C:\\workspace\\src\\main.rs",
            "C:\\workspace"
        ));
    }

    #[test]
    fn check_file_write_read_only() {
        let e = PermissionEnforcer::new(PermissionMode::ReadOnly, "/ws".into());
        assert!(e.check_file_write("/ws/foo").is_denied());
    }

    #[test]
    fn check_file_write_workspace_inside() {
        let e = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/ws".into());
        assert!(e.check_file_write("/ws/foo").is_allowed());
    }

    #[test]
    fn check_file_write_workspace_outside() {
        let e = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/ws".into());
        assert!(e.check_file_write("/other/foo").is_denied());
    }

    #[test]
    fn unknown_tool_requires_full_access() {
        let e = PermissionEnforcer::new(PermissionMode::WorkspaceWrite, "/ws".into());
        let r = e.check_tool("some_new_dangerous_tool", &json!({}));
        assert!(r.is_denied());
    }
}
