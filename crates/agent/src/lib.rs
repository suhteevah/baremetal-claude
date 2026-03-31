//! Agent session manager — multi-agent dashboard.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub struct AgentSession {
    pub id: usize,
    pub name: String,
    pub state: AgentState,
    pub conversation: Vec<Message>,
    pub pane_id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentState {
    Idle, WaitingForInput, Thinking, ToolExecuting, Streaming, Error,
}

#[derive(Debug, Clone)]
pub struct Message { pub role: Role, pub content: String }

#[derive(Debug, Clone, Copy)]
pub enum Role { User, Assistant }

#[derive(Debug)]
pub enum Tool {
    FileRead { path: String },
    FileWrite { path: String, content: String },
    ListDir { path: String },
}

pub async fn dashboard(_creds: claudio_auth::Credentials) {
    log::info!("[agent] dashboard starting");
    todo!("agent dashboard event loop")
}
