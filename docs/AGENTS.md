# ClaudioOS Multi-Agent System

## Overview

ClaudioOS runs multiple Claude AI coding agents simultaneously, each in its own
terminal pane with independent conversation state. The system supports two
authentication modes: direct claude.ai Max subscription access, and standard
Anthropic API key.

---

## Authentication Modes

### ClaudeAi Mode (Max Subscription)

Connects directly to `claude.ai` using the web chat API. This uses the same
protocol as the Claude web interface.

**Flow:**
1. On first boot, ClaudioOS sends an email login request to `claude.ai/api/auth`
2. User enters the verification code received via email
3. ClaudioOS receives a `sessionKey` cookie (valid for 28 days)
4. Session is persisted to `target/session.txt` via QEMU `fw_cfg`
5. On subsequent boots, the session is loaded automatically

**Headers:**
- `anthropic-client-platform: web`
- `anthropic-client-sha: <build hash>`
- Custom `source: "claude"` in request bodies
- Standard browser-like headers for compatibility

**Endpoints:**
- `claude.ai/api/auth/send_email_code` -- initiate login
- `claude.ai/api/auth/verify_email_code` -- complete login
- `claude.ai/api/organizations/<org_id>/chat_conversations` -- create/list conversations
- `claude.ai/api/organizations/<org_id>/chat_conversations/<id>/completion` -- send messages (SSE)

### ApiKey Mode

Standard Anthropic Messages API with an API key.

**Configuration:**
- Compile-time: `CLAUDIO_API_KEY=sk-ant-api03-... cargo build`
- Runtime: auth relay server (`tools/auth-relay.py`) on host port 8444

**Endpoint:** `api.anthropic.com/v1/messages`

**Headers:**
- `x-api-key: <key>`
- `anthropic-version: 2023-06-01`
- `Content-Type: application/json`

---

## Session Persistence

Sessions survive reboots via QEMU's `fw_cfg` mechanism:

1. **Save**: When ClaudioOS obtains a session, it writes credentials to serial
2. **Host capture**: `run.ps1` saves serial output to `target/session.txt`
3. **Load**: On next boot, QEMU passes `-fw_cfg name=opt/claudio/session,file=target/session.txt`
4. **Kernel reads**: The kernel reads `fw_cfg` at boot and restores the session

This enables conversation reuse across reboots without re-authentication.

---

## Dashboard (tmux-style Panes)

The dashboard provides a split-pane terminal interface, similar to tmux.

### Layout

```
+---------------------------+---------------------------+
|                           |                           |
|   Agent 1 (focused)       |   Agent 2                 |
|   claude session          |   claude session          |
|   [typing/streaming]      |   [idle]                  |
|                           |                           |
+---------------------------+---------------------------+
|                                                       |
|   Agent 3                                             |
|   claude session                                      |
|   [tool execution]                                    |
|                                                       |
+-------------------------------------------------------+
```

### Keyboard Shortcuts

All shortcuts use a **Ctrl+B prefix** (press Ctrl+B, release, then press the
action key), matching tmux conventions.

| Shortcut | Action |
|----------|--------|
| `Ctrl+B "` | Split pane horizontally |
| `Ctrl+B %` | Split pane vertically |
| `Ctrl+B o` | Switch focus to next pane |
| `Ctrl+B c` | Create new agent session in current pane |
| `Ctrl+B x` | Close current pane / kill agent |
| `Ctrl+B Up/Down/Left/Right` | Move focus directionally |

### Layout Engine

The layout uses a binary tree of viewports:

```rust
pub enum LayoutNode {
    Leaf { pane: Pane },
    Split {
        direction: SplitDirection, // Horizontal or Vertical
        ratio: f32,                // 0.0 to 1.0
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}
```

Each `Pane` contains:
- A `Terminal` instance (VTE parser + character grid)
- Viewport coordinates (x, y, width, height) in pixels
- Scroll position and history buffer
- Reference to the agent session (if any)

---

## Agent Sessions

Each agent session is an async task with its own:
- Conversation history (message list)
- Authentication credentials (shared via reference)
- Terminal pane (for rendering output)
- Tool execution context

### Session Lifecycle

```
1. User creates agent (Ctrl+B c)
2. Agent session spawns as async task
3. Welcome banner displayed
4. User types a prompt
5. Prompt sent to Claude (API key or claude.ai)
6. SSE stream received, tokens rendered to pane
7. If tool_use: execute tool, send result, repeat (up to 20 rounds)
8. Final response displayed
9. Wait for next user input
```

### Tool Loop

The agent tool loop handles multi-turn tool use:

```
User prompt
  |
  v
Send to Claude API (with tools declaration)
  |
  v
Parse response:
  +-- text content -> render to pane
  +-- tool_use content -> execute tool
        |
        v
      Build tool_result message
        |
        v
      Send back to Claude (with tool_result)
        |
        v
      Parse response (repeat up to 20 rounds)
  |
  v
Final text response -> render to pane
```

---

## Available Tools

Tools are declared in the API request and executed locally when Claude requests them.

| Tool | Description | Implementation |
|------|-------------|----------------|
| `file_read` | Read a file's contents | VFS `read_file()` |
| `file_write` | Write content to a file | VFS `write_file()` |
| `edit_file` | Edit a file (nano-like operations) | `claudio-editor` crate |
| `execute_python` | Run Python code | `python-lite` interpreter |
| `execute_javascript` | Run JavaScript code | `js-lite` evaluator |
| `compile_rust` | Compile Rust code | `rustc-lite` + Cranelift, or host build server |
| `list_files` | List directory contents | VFS `list_dir()` |
| `search_files` | Search for files by pattern | VFS traversal |

### Tool Declaration (JSON)

```json
{
  "name": "execute_python",
  "description": "Execute Python code and return stdout/stderr",
  "input_schema": {
    "type": "object",
    "properties": {
      "code": {
        "type": "string",
        "description": "Python source code to execute"
      }
    },
    "required": ["code"]
  }
}
```

### Tool Result Format

```json
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc123",
  "content": "Output of the tool execution..."
}
```

---

## Conversation State

Each agent maintains a message list:

```rust
pub struct Conversation {
    pub id: String,              // conversation UUID
    pub messages: Vec<Message>,  // alternating user/assistant messages
    pub model: String,           // e.g., "claude-sonnet-4-20250514"
    pub system_prompt: Option<String>,
}

pub struct Message {
    pub role: Role,              // User or Assistant
    pub content: Vec<ContentBlock>,
}

pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String },
}
```

In ClaudeAi mode, conversations persist on claude.ai servers and can be
resumed across reboots using the saved session.
