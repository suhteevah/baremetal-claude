//! Inter-agent communication (IPC) for ClaudioOS.
//!
//! Provides a global message bus, named channels, and shared memory buffers
//! so that Claude agents running in separate dashboard panes can collaborate.
//!
//! ## Components
//!
//! - **MessageBus**: Global message queue with per-agent inboxes.
//! - **Message**: Envelope with sender, recipient (or broadcast), content, timestamp.
//! - **Channel**: Named pipe between agent pairs (SPSC ring buffer).
//! - **SharedMemory**: A shared byte buffer protected by a spin lock.
//!
//! ## Agent tools
//!
//! Two tools are exposed to Claude via the tool-use protocol:
//! - `send_to_agent` — send a message to another agent (by name or id) or broadcast.
//! - `read_agent_messages` — drain the calling agent's inbox.
//!
//! ## Thread safety
//!
//! All global state is behind `spin::Mutex` since we have no `std::sync`.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A message passed between agents.
#[derive(Debug, Clone)]
pub struct Message {
    /// Agent id of the sender.
    pub from_agent_id: usize,
    /// Agent name of the sender (for display).
    pub from_agent_name: String,
    /// Target agent id, or `None` for broadcast.
    pub to_agent_id: Option<usize>,
    /// Message content (free-form text).
    pub content: String,
    /// Timestamp in milliseconds since boot.
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// MessageBus — global per-agent inboxes
// ---------------------------------------------------------------------------

/// Global message bus: maps agent_id -> inbox (Vec of pending messages).
pub struct MessageBus {
    /// Per-agent inboxes.  Messages accumulate here until `recv_messages` drains them.
    inboxes: BTreeMap<usize, Vec<Message>>,
    /// Agent name -> agent id mapping, for name-based addressing.
    agent_names: BTreeMap<String, usize>,
}

impl MessageBus {
    /// Create an empty message bus.
    pub const fn new() -> Self {
        Self {
            inboxes: BTreeMap::new(),
            agent_names: BTreeMap::new(),
        }
    }

    /// Register an agent so it can send/receive messages.
    pub fn register_agent(&mut self, id: usize, name: String) {
        self.inboxes.entry(id).or_insert_with(Vec::new);
        self.agent_names.insert(name, id);
        log::debug!("[ipc] registered agent {} (id={})", self.agent_name(id), id);
    }

    /// Unregister an agent (clears inbox and name mapping).
    pub fn unregister_agent(&mut self, id: usize) {
        self.inboxes.remove(&id);
        self.agent_names.retain(|_, v| *v != id);
        log::debug!("[ipc] unregistered agent id={}", id);
    }

    /// Update an agent's name (for rename support).
    pub fn rename_agent(&mut self, id: usize, new_name: String) {
        // Remove old name mapping.
        self.agent_names.retain(|_, v| *v != id);
        self.agent_names.insert(new_name.clone(), id);
        log::debug!("[ipc] renamed agent id={} -> \"{}\"", id, new_name);
    }

    /// Look up agent id by name.
    pub fn agent_id_by_name(&self, name: &str) -> Option<usize> {
        self.agent_names.get(name).copied()
    }

    /// Get agent name by id (returns "unknown" if not found).
    fn agent_name(&self, id: usize) -> String {
        self.agent_names
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| format!("agent-{}", id))
    }

    /// List all registered agent names and ids.
    pub fn list_agents(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .agent_names
            .iter()
            .map(|(name, &id)| (id, name.clone()))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// Send a message to a specific agent's inbox.
    pub fn send_message(&mut self, from_id: usize, to_id: usize, content: String, timestamp: u64) {
        let from_name = self.agent_name(from_id);
        let msg = Message {
            from_agent_id: from_id,
            from_agent_name: from_name,
            to_agent_id: Some(to_id),
            content,
            timestamp,
        };
        log::info!(
            "[ipc] message: agent {} -> agent {} ({} bytes)",
            msg.from_agent_id,
            to_id,
            msg.content.len()
        );
        self.inboxes.entry(to_id).or_insert_with(Vec::new).push(msg);
    }

    /// Broadcast a message to all registered agents (except the sender).
    pub fn broadcast(&mut self, from_id: usize, content: String, timestamp: u64) {
        let from_name = self.agent_name(from_id);
        let recipients: Vec<usize> = self
            .inboxes
            .keys()
            .filter(|&&id| id != from_id)
            .copied()
            .collect();

        log::info!(
            "[ipc] broadcast from agent {} to {} recipients ({} bytes)",
            from_id,
            recipients.len(),
            content.len()
        );

        for to_id in recipients {
            let msg = Message {
                from_agent_id: from_id,
                from_agent_name: from_name.clone(),
                to_agent_id: None, // broadcast
                content: content.clone(),
                timestamp,
            };
            self.inboxes.entry(to_id).or_insert_with(Vec::new).push(msg);
        }
    }

    /// Drain all pending messages from an agent's inbox.
    pub fn recv_messages(&mut self, agent_id: usize) -> Vec<Message> {
        self.inboxes
            .get_mut(&agent_id)
            .map(|inbox| core::mem::replace(inbox, Vec::new()))
            .unwrap_or_default()
    }

    /// Peek at inbox count without draining.
    pub fn inbox_count(&self, agent_id: usize) -> usize {
        self.inboxes.get(&agent_id).map(|v| v.len()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Named channels — point-to-point pipes
// ---------------------------------------------------------------------------

/// A named channel (pipe) for streaming data between agents.
pub struct Channel {
    /// Channel name.
    pub name: String,
    /// Ring buffer for data.
    buffer: Vec<u8>,
    /// Write position (wraps around).
    write_pos: usize,
    /// Read position (wraps around).
    read_pos: usize,
    /// Total capacity.
    capacity: usize,
}

/// Handle returned when creating a channel, used for read/write operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelHandle(pub usize);

const DEFAULT_CHANNEL_CAPACITY: usize = 4096;

impl Channel {
    fn new(name: String) -> Self {
        Self {
            name,
            buffer: alloc::vec![0u8; DEFAULT_CHANNEL_CAPACITY],
            write_pos: 0,
            read_pos: 0,
            capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    /// Write data into the channel. Returns number of bytes written.
    fn write(&mut self, data: &[u8]) -> usize {
        let mut written = 0;
        for &byte in data {
            let next_write = (self.write_pos + 1) % self.capacity;
            if next_write == self.read_pos {
                break; // Full
            }
            self.buffer[self.write_pos] = byte;
            self.write_pos = next_write;
            written += 1;
        }
        written
    }

    /// Read data from the channel. Returns the bytes read.
    fn read(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while self.read_pos != self.write_pos {
            out.push(self.buffer[self.read_pos]);
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
        out
    }

    /// Number of bytes available to read.
    fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.capacity - self.read_pos + self.write_pos
        }
    }
}

/// Registry of named channels.
pub struct ChannelRegistry {
    channels: Vec<Channel>,
}

impl ChannelRegistry {
    pub const fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// Create a named channel. Returns handle for read/write.
    pub fn create_channel(&mut self, name: String) -> ChannelHandle {
        // Check if channel with this name already exists.
        if let Some(idx) = self.channels.iter().position(|c| c.name == name) {
            log::debug!("[ipc] channel \"{}\" already exists (handle={})", name, idx);
            return ChannelHandle(idx);
        }
        let handle = ChannelHandle(self.channels.len());
        log::info!("[ipc] created channel \"{}\" (handle={})", name, handle.0);
        self.channels.push(Channel::new(name));
        handle
    }

    /// Write data to a channel by handle.
    pub fn channel_write(&mut self, handle: ChannelHandle, data: &[u8]) -> Result<usize, String> {
        self.channels
            .get_mut(handle.0)
            .ok_or_else(|| format!("invalid channel handle {}", handle.0))
            .map(|ch| ch.write(data))
    }

    /// Read all available data from a channel by handle.
    pub fn channel_read(&mut self, handle: ChannelHandle) -> Result<Vec<u8>, String> {
        self.channels
            .get_mut(handle.0)
            .ok_or_else(|| format!("invalid channel handle {}", handle.0))
            .map(|ch| ch.read())
    }

    /// Get available byte count for a channel.
    pub fn channel_available(&self, handle: ChannelHandle) -> Result<usize, String> {
        self.channels
            .get(handle.0)
            .ok_or_else(|| format!("invalid channel handle {}", handle.0))
            .map(|ch| ch.available())
    }

    /// List all channel names and handles.
    pub fn list_channels(&self) -> Vec<(ChannelHandle, String)> {
        self.channels
            .iter()
            .enumerate()
            .map(|(i, c)| (ChannelHandle(i), c.name.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Shared memory — a named buffer agents can read/write
// ---------------------------------------------------------------------------

/// A named shared memory region.
pub struct SharedMemoryRegion {
    pub name: String,
    pub data: Vec<u8>,
}

/// Registry of shared memory regions.
pub struct SharedMemoryRegistry {
    regions: Vec<SharedMemoryRegion>,
}

impl SharedMemoryRegistry {
    pub const fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// Create or get a shared memory region by name.
    pub fn get_or_create(&mut self, name: &str, initial_size: usize) -> usize {
        if let Some(idx) = self.regions.iter().position(|r| r.name == name) {
            return idx;
        }
        let idx = self.regions.len();
        log::info!("[ipc] created shared memory \"{}\" ({} bytes, idx={})", name, initial_size, idx);
        self.regions.push(SharedMemoryRegion {
            name: String::from(name),
            data: alloc::vec![0u8; initial_size],
        });
        idx
    }

    /// Write data into a shared memory region at offset.
    pub fn write(&mut self, idx: usize, offset: usize, data: &[u8]) -> Result<(), String> {
        let region = self
            .regions
            .get_mut(idx)
            .ok_or_else(|| format!("invalid shared memory index {}", idx))?;
        let end = offset + data.len();
        if end > region.data.len() {
            // Grow the region to fit.
            region.data.resize(end, 0);
        }
        region.data[offset..end].copy_from_slice(data);
        Ok(())
    }

    /// Read data from a shared memory region.
    pub fn read(&self, idx: usize, offset: usize, len: usize) -> Result<Vec<u8>, String> {
        let region = self
            .regions
            .get(idx)
            .ok_or_else(|| format!("invalid shared memory index {}", idx))?;
        let end = (offset + len).min(region.data.len());
        if offset >= region.data.len() {
            return Ok(Vec::new());
        }
        Ok(region.data[offset..end].to_vec())
    }

    /// List all shared memory regions.
    pub fn list_regions(&self) -> Vec<(usize, String, usize)> {
        self.regions
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.name.clone(), r.data.len()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Global IPC state (locked behind spin::Mutex)
// ---------------------------------------------------------------------------

/// The complete IPC subsystem state.
pub struct IpcState {
    pub bus: MessageBus,
    pub channels: ChannelRegistry,
    pub shared_memory: SharedMemoryRegistry,
}

impl IpcState {
    pub const fn new() -> Self {
        Self {
            bus: MessageBus::new(),
            channels: ChannelRegistry::new(),
            shared_memory: SharedMemoryRegistry::new(),
        }
    }
}

/// Global IPC state, accessible from any agent session.
pub static IPC: Mutex<IpcState> = Mutex::new(IpcState::new());

// ---------------------------------------------------------------------------
// Tool execution — called from the agent tool loop
// ---------------------------------------------------------------------------

/// Execute the `send_to_agent` tool.
///
/// Input JSON fields:
/// - `to`: target agent name or id, or "broadcast" / "all"
/// - `message`: the content to send
///
/// Returns a confirmation string.
pub fn execute_send_to_agent(
    from_agent_id: usize,
    input: &serde_json::Value,
    timestamp: u64,
) -> Result<String, String> {
    let to_raw = input
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'to' field (agent name, id, or \"broadcast\")"))?;
    let message = input
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'message' field"))?;

    let mut ipc = IPC.lock();

    if to_raw == "broadcast" || to_raw == "all" {
        ipc.bus.broadcast(from_agent_id, String::from(message), timestamp);
        Ok(String::from("Message broadcast to all agents."))
    } else {
        // Try to resolve as agent name first, then as numeric id.
        let to_id = ipc
            .bus
            .agent_id_by_name(to_raw)
            .or_else(|| to_raw.parse::<usize>().ok())
            .ok_or_else(|| format!("unknown agent: \"{}\"", to_raw))?;

        ipc.bus
            .send_message(from_agent_id, to_id, String::from(message), timestamp);
        Ok(format!("Message sent to agent {}.", to_raw))
    }
}

/// Execute the `read_agent_messages` tool.
///
/// Returns a JSON array of messages from the agent's inbox.
pub fn execute_read_agent_messages(agent_id: usize) -> Result<String, String> {
    let mut ipc = IPC.lock();
    let messages = ipc.bus.recv_messages(agent_id);

    if messages.is_empty() {
        return Ok(String::from("No new messages."));
    }

    // Format as a readable list.
    let mut out = format!("{} message(s):\n", messages.len());
    for msg in &messages {
        let from_label = &msg.from_agent_name;
        let kind = if msg.to_agent_id.is_none() {
            " (broadcast)"
        } else {
            ""
        };
        out.push_str(&format!(
            "\n--- From: {}{} ---\n{}\n",
            from_label, kind, msg.content
        ));
    }
    Ok(out)
}

/// Execute the `list_agents` IPC tool — shows all agents that can be messaged.
pub fn execute_list_agents_ipc() -> Result<String, String> {
    let ipc = IPC.lock();
    let agents = ipc.bus.list_agents();
    if agents.is_empty() {
        return Ok(String::from("No agents registered."));
    }
    let mut out = format!("{} agent(s) registered:\n", agents.len());
    for (id, name) in &agents {
        let inbox_count = ipc.bus.inbox_count(*id);
        out.push_str(&format!(
            "  - {} (id={}, {} pending messages)\n",
            name, id, inbox_count
        ));
    }
    Ok(out)
}

/// Execute `create_channel` tool.
pub fn execute_create_channel(input: &serde_json::Value) -> Result<String, String> {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'name' field"))?;
    let mut ipc = IPC.lock();
    let handle = ipc.channels.create_channel(String::from(name));
    Ok(format!("Channel \"{}\" created (handle={}).", name, handle.0))
}

/// Execute `channel_write` tool.
pub fn execute_channel_write(input: &serde_json::Value) -> Result<String, String> {
    let name = input
        .get("channel")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'channel' field"))?;
    let data = input
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'data' field"))?;

    let mut ipc = IPC.lock();
    // Find channel by name.
    let handle = ipc
        .channels
        .list_channels()
        .iter()
        .find(|(_, n)| n == name)
        .map(|(h, _)| *h)
        .ok_or_else(|| format!("channel \"{}\" not found", name))?;
    let written = ipc.channels.channel_write(handle, data.as_bytes())?;
    Ok(format!("{} bytes written to channel \"{}\".", written, name))
}

/// Execute `channel_read` tool.
pub fn execute_channel_read(input: &serde_json::Value) -> Result<String, String> {
    let name = input
        .get("channel")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'channel' field"))?;

    let mut ipc = IPC.lock();
    let handle = ipc
        .channels
        .list_channels()
        .iter()
        .find(|(_, n)| n == name)
        .map(|(h, _)| *h)
        .ok_or_else(|| format!("channel \"{}\" not found", name))?;
    let data = ipc.channels.channel_read(handle)?;
    if data.is_empty() {
        return Ok(format!("Channel \"{}\" is empty.", name));
    }
    match core::str::from_utf8(&data) {
        Ok(s) => Ok(format!("{} bytes from channel \"{}\":\n{}", data.len(), name, s)),
        Err(_) => Ok(format!("{} bytes (binary) from channel \"{}\".", data.len(), name)),
    }
}

/// Execute `shared_memory_write` tool.
pub fn execute_shared_memory_write(input: &serde_json::Value) -> Result<String, String> {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'name' field"))?;
    let data = input
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'data' field"))?;
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let mut ipc = IPC.lock();
    let idx = ipc.shared_memory.get_or_create(name, data.len() + offset);
    ipc.shared_memory.write(idx, offset, data.as_bytes())?;
    Ok(format!(
        "{} bytes written to shared memory \"{}\" at offset {}.",
        data.len(),
        name,
        offset
    ))
}

/// Execute `shared_memory_read` tool.
pub fn execute_shared_memory_read(input: &serde_json::Value) -> Result<String, String> {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| String::from("missing 'name' field"))?;
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let len = input
        .get("length")
        .and_then(|v| v.as_u64())
        .unwrap_or(4096) as usize;

    let ipc = IPC.lock();
    let idx = ipc
        .shared_memory
        .list_regions()
        .iter()
        .find(|(_, n, _)| n == name)
        .map(|(i, _, _)| *i)
        .ok_or_else(|| format!("shared memory \"{}\" not found", name))?;
    let data = ipc.shared_memory.read(idx, offset, len)?;
    if data.is_empty() {
        return Ok(format!("Shared memory \"{}\" is empty at offset {}.", name, offset));
    }
    match core::str::from_utf8(&data) {
        Ok(s) => Ok(format!(
            "{} bytes from shared memory \"{}\" (offset {}):\n{}",
            data.len(),
            name,
            offset,
            s
        )),
        Err(_) => Ok(format!(
            "{} bytes (binary) from shared memory \"{}\" (offset {}).",
            data.len(),
            name,
            offset
        )),
    }
}

// ---------------------------------------------------------------------------
// Tool definitions for the Anthropic Messages API
// ---------------------------------------------------------------------------

/// Return the IPC tool definitions for inclusion in API requests.
pub fn ipc_tool_definitions() -> Vec<serde_json::Value> {
    alloc::vec![
        serde_json::json!({
            "name": "send_to_agent",
            "description": "Send a message to another agent by name or id. Use \"broadcast\" or \"all\" as the target to send to all agents. Use list_agents_ipc first to see available targets.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Target agent name, numeric id, or \"broadcast\"/\"all\" for all agents."
                    },
                    "message": {
                        "type": "string",
                        "description": "The message content to send."
                    }
                },
                "required": ["to", "message"]
            }
        }),
        serde_json::json!({
            "name": "read_agent_messages",
            "description": "Read and drain all pending messages from your inbox. Returns messages sent to you by other agents.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        serde_json::json!({
            "name": "list_agents_ipc",
            "description": "List all registered agents that you can send messages to, with their names and ids.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        serde_json::json!({
            "name": "create_channel",
            "description": "Create a named data channel (pipe) for streaming data between agents.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the channel to create."
                    }
                },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "channel_write",
            "description": "Write data to a named channel.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "The name of the channel."
                    },
                    "data": {
                        "type": "string",
                        "description": "The data to write."
                    }
                },
                "required": ["channel", "data"]
            }
        }),
        serde_json::json!({
            "name": "channel_read",
            "description": "Read all available data from a named channel.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "The name of the channel."
                    }
                },
                "required": ["channel"]
            }
        }),
        serde_json::json!({
            "name": "shared_memory_write",
            "description": "Write data to a named shared memory region. Creates the region if it doesn't exist.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the shared memory region."
                    },
                    "data": {
                        "type": "string",
                        "description": "The data to write."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to write at (default: 0)."
                    }
                },
                "required": ["name", "data"]
            }
        }),
        serde_json::json!({
            "name": "shared_memory_read",
            "description": "Read data from a named shared memory region.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the shared memory region."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start reading from (default: 0)."
                    },
                    "length": {
                        "type": "integer",
                        "description": "Number of bytes to read (default: 4096)."
                    }
                },
                "required": ["name"]
            }
        }),
    ]
}

/// Check if a tool name is an IPC tool, and if so, execute it.
///
/// Returns `Some(result)` if the tool was an IPC tool, `None` otherwise.
pub fn try_execute_ipc_tool(
    tool_name: &str,
    input: &serde_json::Value,
    agent_id: usize,
    timestamp: u64,
) -> Option<Result<String, String>> {
    match tool_name {
        "send_to_agent" => Some(execute_send_to_agent(agent_id, input, timestamp)),
        "read_agent_messages" => Some(execute_read_agent_messages(agent_id)),
        "list_agents_ipc" => Some(execute_list_agents_ipc()),
        "create_channel" => Some(execute_create_channel(input)),
        "channel_write" => Some(execute_channel_write(input)),
        "channel_read" => Some(execute_channel_read(input)),
        "shared_memory_write" => Some(execute_shared_memory_write(input)),
        "shared_memory_read" => Some(execute_shared_memory_read(input)),
        _ => None,
    }
}
