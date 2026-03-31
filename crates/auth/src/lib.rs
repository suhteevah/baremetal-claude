//! OAuth 2.0 Device Authorization Grant (RFC 8628) + API key fallback.
//!
//! Boot-time auth gate → check persist → refresh or device flow → persist.

#![no_std]
extern crate alloc;

use alloc::string::String;

#[derive(Debug, Clone)]
pub enum Credentials {
    ApiKey(String),
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at: u64,
    },
}

impl Credentials {
    pub fn is_expired(&self, now_unix: u64) -> bool {
        match self {
            Credentials::ApiKey(_) => false,
            Credentials::OAuth { expires_at, .. } => now_unix >= *expires_at,
        }
    }
}

#[derive(Debug)]
pub struct DeviceFlowPrompt {
    pub verification_uri: String,
    pub user_code: String,
    pub expires_in: u32,
    pub interval: u32,
}

pub async fn authenticate() -> Credentials {
    log::info!("[auth] checking for saved credentials...");
    todo!("auth flow")
}

pub async fn token_refresh_loop(_creds: Credentials) {
    loop {
        log::trace!("[auth] refresh check");
        todo!("async sleep + refresh")
    }
}
