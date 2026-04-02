# ClaudioOS Shell Documentation

## Overview

ClaudioOS includes an AI-native shell (`crates/shell/`, 2,884 lines) that combines
traditional Unix-like commands with natural language processing. Type `ls /mnt/nvme0`
or `"show me what's on the NVMe drive"` -- both work.

The shell is `#![no_std]` and runs directly on the bare-metal kernel.

---

## Module Structure

| Module | Purpose |
|--------|---------|
| `shell.rs` | Main shell loop: line reading, command dispatch, history |
| `parser.rs` | Command parsing: tokenization, pipes, redirects, quoting |
| `builtin.rs` | 28 built-in commands + VFS/SystemInfo trait abstractions |
| `pipe.rs` | Pipeline executor: connect commands with byte-stream pipes |
| `env.rs` | Environment variables: get, set, expand `$VAR` in arguments |
| `ai.rs` | AI mode: send natural language to Claude, execute returned commands |
| `prompt.rs` | Prompt rendering: username, cwd, colors |
| `script.rs` | Script runner: if/for/while control flow, variable expansion |

---

## Built-in Commands Reference

### Filesystem Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `ls` | `ls [path]` | List directory contents |
| `cd` | `cd <path>` | Change working directory |
| `pwd` | `pwd` | Print working directory |
| `cat` | `cat <file> [file...]` | Display file contents (accepts stdin via pipe) |
| `cp` | `cp <src> <dst>` | Copy a file |
| `mv` | `mv <src> <dst>` | Move or rename a file |
| `rm` | `rm <path>` | Remove a file or directory |
| `mkdir` | `mkdir <path>` | Create a directory |
| `touch` | `touch <path>` | Create an empty file or update timestamp |
| `head` | `head [-n N] <file>` | Show first N lines (default 10) |
| `tail` | `tail [-n N] <file>` | Show last N lines (default 10) |
| `grep` | `grep <pattern> [file]` | Search for pattern in file or stdin |
| `mount` | `mount <device> <path> <fstype>` | Mount a filesystem |
| `umount` | `umount <path>` | Unmount a filesystem |
| `df` | `df` | Show disk space usage per mount point |

### Process / Agent Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `ps` | `ps` | List active agent sessions (id, name, status) |
| `kill` | `kill <id>` | Kill an agent session by ID |

### System Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `clear` | `clear` | Clear the terminal screen |
| `reboot` | `reboot` | Reboot the system (via ACPI or keyboard controller) |
| `shutdown` | `shutdown` | Shutdown the system (via ACPI) |
| `date` | `date` | Show current date and time |
| `uptime` | `uptime` | Show system uptime |
| `free` | `free` | Show memory usage (total, used, free) |

### Network Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `ifconfig` | `ifconfig` | Show network interface info (name, IP, MAC, status) |
| `ping` | `ping <host>` | Ping a host, show round-trip time |
| `ssh` | `ssh <host>` | SSH client (placeholder) |

### Environment Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `set` | `set VAR=value` | Set an environment variable |
| `unset` | `unset VAR` | Remove an environment variable |
| `export` | `export VAR=value` | Set and export an environment variable |
| `echo` | `echo [args...]` | Print arguments to stdout |

### Meta Commands

| Command | Usage | Description |
|---------|-------|-------------|
| `help` | `help` | Show list of available commands |
| `history` | `history` | Show command history |
| `exit` | `exit` | Exit the shell |

---

## AI-Native Mode

When input does not match any built-in command, the shell sends it to Claude as
natural language. Claude interprets the request and returns executable commands.

### How It Works

1. User types: `show me the largest files on disk`
2. Shell detects this is not a built-in command
3. The `AiShellCallback` trait sends the text to the active Claude agent
4. Claude responds with commands: `ls -la / | sort -k5 -rn | head -20`
5. Shell executes the returned commands
6. Output is displayed to the user

### AiShellCallback Trait

```rust
pub trait AiShellCallback {
    /// Send natural language text to Claude, return suggested commands.
    fn query_ai(&mut self, input: &str) -> Result<String, String>;
}
```

The kernel provides the implementation that bridges to the active agent session.

---

## Pipes and Redirects

The shell supports Unix-style pipes connecting commands:

```
cat /etc/hosts | grep localhost | head -5
```

### Pipeline Execution

The `PipelineExecutor` connects commands by passing the stdout of one command
as the stdin of the next:

```
Command A (stdout) -> bytes -> Command B (stdin) -> bytes -> Command C (stdin)
```

Each built-in command accepts an optional `stdin: Option<&[u8]>` parameter for
receiving piped input.

### Redirects (Planned)

Output redirection (`>`, `>>`) and input redirection (`<`) are parsed but not
yet fully wired to the VFS.

---

## Environment Variables

The `Environment` struct manages shell variables:

```
set HOME=/home/claudio
set PATH=/bin:/usr/bin
echo $HOME         # prints /home/claudio
```

### Variable Expansion

Arguments containing `$VAR` are expanded before command execution. Supports:
- `$VAR` -- expand named variable
- `${VAR}` -- expand with explicit boundaries
- `$?` -- last command exit status

### Built-in Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HOME` | `/` | Home directory |
| `CWD` | `/` | Current working directory (synced with `cd`) |
| `PS1` | `claudio$` | Shell prompt string |

---

## Shell Scripting

The `ScriptRunner` supports basic control flow:

### If/Else

```bash
if [ -f /etc/hostname ]; then
    cat /etc/hostname
else
    echo "no hostname"
fi
```

### For Loops

```bash
for f in /etc/*; do
    echo $f
done
```

### While Loops

```bash
while true; do
    date
done
```

---

## Tab Completion and History

### History

- Commands are stored in an in-memory history buffer
- Up/Down arrow keys navigate through previous commands
- `history` command displays the full history list

### Tab Completion (Planned)

Tab completion for file paths and command names is parsed but not yet wired
to the VFS for live path completion.

---

## Integration

The shell is designed to run as a pane type in the agent dashboard. The wiring
between the shell crate and the kernel dashboard is tracked in `docs/ROADMAP.md`
under "TODO -- Critical: Wire shell to dashboard."

```rust
use claudio_shell::{Shell, LineReader, Environment};

let mut env = Environment::new();
let mut shell = Shell::new(env);
// shell.run(vfs, system_info, ai_callback);
```
