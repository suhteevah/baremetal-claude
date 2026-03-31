//! FAT32 persistence: credentials, config, agent state, logs.
//!
//! Partition layout: /claudio/{config.json, credentials.json, agents/, logs/}

#![no_std]
extern crate alloc;

use alloc::string::String;

#[derive(Debug)]
pub struct Config {
    pub log_level: String,
    pub default_model: String,
    pub max_agents: usize,
    pub auto_start_agents: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: String::from("info"),
            default_model: String::from("claude-sonnet-4-20250514"),
            max_agents: 8,
            auto_start_agents: 1,
        }
    }
}

pub fn read_credentials() -> Option<claudio_auth::Credentials> {
    log::debug!("[fs] reading credentials from persist partition");
    None
}

pub fn write_credentials(_creds: &claudio_auth::Credentials) -> Result<(), FsError> {
    log::debug!("[fs] writing credentials to persist partition");
    Ok(())
}

#[derive(Debug)]
pub enum FsError { NotMounted, NotFound, WriteFailed, CorruptedData }
