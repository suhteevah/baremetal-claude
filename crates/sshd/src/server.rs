//! SSH Server.
//!
//! Top-level `SshServer` struct that manages host keys, authorized users,
//! configuration, and connection acceptance. Integrates with ClaudioOS by
//! providing a `PaneCallback` trait — when a client requests a shell, the
//! callback creates a terminal pane and wires I/O.

use alloc::string::String;
use alloc::vec::Vec;

use crate::auth::{AuthorizedKey, AuthorizedUser};
use crate::hostkey::HybridHostKey;
use crate::session::{SessionState, SshSession};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// SSH server configuration.
#[derive(Debug, Clone)]
pub struct SshConfig {
    /// TCP port to listen on (default 22).
    pub port: u16,
    /// Maximum simultaneous connections.
    pub max_connections: usize,
    /// Authentication methods to offer.
    pub auth_methods: Vec<String>,
    /// Banner message displayed before auth prompt.
    pub banner: Option<String>,
    /// Session inactivity timeout in seconds (0 = disabled).
    pub timeout_secs: u64,
    /// Whether password authentication is enabled.
    pub allow_password_auth: bool,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            port: 22,
            max_connections: 8,
            auth_methods: {
                let mut v = Vec::new();
                v.push(String::from("publickey"));
                v
            },
            banner: Some(String::from(
                "Welcome to ClaudioOS SSH\r\nPost-quantum hybrid key exchange active.\r\n",
            )),
            timeout_secs: 300,
            allow_password_auth: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Pane callback trait
// ---------------------------------------------------------------------------

/// Callback trait for integrating SSH sessions with the ClaudioOS terminal.
///
/// When a connected SSH client requests a shell, the server calls
/// `on_shell_request` to create a terminal pane. Data from the pane
/// flows back to the SSH channel, and vice versa.
pub trait PaneCallback {
    /// Called when a shell is requested on a channel.
    ///
    /// The implementation should:
    /// 1. Create a new terminal pane with the given dimensions
    /// 2. Return a pane ID that can be used to route I/O
    ///
    /// Returns `Some(pane_id)` on success, `None` if pane creation failed.
    fn on_shell_request(
        &mut self,
        channel_id: u32,
        term: &str,
        width: u32,
        height: u32,
    ) -> Option<u32>;

    /// Called when an exec command is requested.
    ///
    /// Returns `Some(pane_id)` on success, `None` if the command is rejected.
    fn on_exec_request(
        &mut self,
        channel_id: u32,
        command: &str,
    ) -> Option<u32>;

    /// Called when data arrives from the SSH client for a channel.
    fn on_channel_data(&mut self, channel_id: u32, data: &[u8]);

    /// Called when the terminal window size changes.
    fn on_window_change(&mut self, channel_id: u32, width: u32, height: u32);

    /// Called when a channel is closed.
    fn on_channel_close(&mut self, channel_id: u32);
}

// ---------------------------------------------------------------------------
// SSH Server
// ---------------------------------------------------------------------------

/// The main SSH server.
///
/// Manages host keys, authorized users, and active sessions. The kernel
/// integrates this by:
/// 1. Creating an `SshServer` at boot with generated host keys
/// 2. Calling `accept_connection()` when a TCP connection arrives
/// 3. Feeding TCP data into the returned `SshSession`
/// 4. Sending outgoing packets from the session back over TCP
pub struct SshServer {
    /// Server configuration.
    config: SshConfig,
    /// Host key (hybrid ML-DSA-65 + Ed25519).
    host_key: HybridHostKey,
    /// Authorized users and their keys.
    authorized_users: Vec<AuthorizedUser>,
    /// Active session count.
    active_sessions: usize,
    /// RNG function (kernel-provided).
    rng_fill: fn(&mut [u8]),
}

impl SshServer {
    /// Create a new SSH server.
    ///
    /// Generates host keys using the provided RNG. In production, load
    /// persisted keys from FAT32 via `fs-persist`.
    pub fn new(config: SshConfig, rng_fill: fn(&mut [u8])) -> Self {
        log::info!(
            "server: initializing SSH server on port {} (max {} connections)",
            config.port,
            config.max_connections,
        );

        let host_key = HybridHostKey::generate(&mut |buf| rng_fill(buf));

        log::info!("server: host keys generated (Ed25519 + ML-DSA-65 hybrid)");
        log::info!(
            "server: auth methods: {:?}, password_auth={}",
            config.auth_methods,
            config.allow_password_auth,
        );

        Self {
            config,
            host_key,
            authorized_users: Vec::new(),
            active_sessions: 0,
            rng_fill,
        }
    }

    /// Create a server with pre-existing host keys (loaded from persistence).
    pub fn with_host_key(
        config: SshConfig,
        host_key: HybridHostKey,
        rng_fill: fn(&mut [u8]),
    ) -> Self {
        log::info!("server: initializing SSH server with persisted host keys");

        Self {
            config,
            host_key,
            authorized_users: Vec::new(),
            active_sessions: 0,
            rng_fill,
        }
    }

    /// Add an authorized user.
    pub fn add_user(&mut self, user: AuthorizedUser) {
        log::info!(
            "server: added authorized user '{}' ({} keys, password={})",
            user.username,
            user.authorized_keys.len(),
            user.allow_password,
        );
        self.authorized_users.push(user);
    }

    /// Add an authorized Ed25519 public key for a user.
    pub fn add_ed25519_key(&mut self, username: &str, public_key: &[u8; 32], comment: &str) {
        // Build the SSH public key blob
        let mut w = crate::wire::SshWriter::new();
        w.write_string_utf8("ssh-ed25519");
        w.write_string(public_key);
        let blob = w.into_bytes();

        let key = AuthorizedKey {
            key_type: String::from("ssh-ed25519"),
            public_key: blob,
            comment: String::from(comment),
        };

        // Find or create user
        if let Some(user) = self.authorized_users.iter_mut().find(|u| u.username == username) {
            user.authorized_keys.push(key);
            log::info!(
                "server: added Ed25519 key for existing user '{}' (comment: '{}')",
                username,
                comment,
            );
        } else {
            self.authorized_users.push(AuthorizedUser {
                username: String::from(username),
                authorized_keys: {
                    let mut v = Vec::new();
                    v.push(key);
                    v
                },
                allow_password: false,
                password_hash: None,
            });
            log::info!(
                "server: created user '{}' with Ed25519 key (comment: '{}')",
                username,
                comment,
            );
        }
    }

    /// Accept a new TCP connection and create a session.
    ///
    /// Returns `None` if the server is at capacity.
    pub fn accept_connection(&mut self) -> Option<SshSession> {
        if self.active_sessions >= self.config.max_connections {
            log::warn!(
                "server: rejecting connection — at capacity ({}/{})",
                self.active_sessions,
                self.config.max_connections,
            );
            return None;
        }

        self.active_sessions += 1;
        log::info!(
            "server: accepted connection ({}/{} active)",
            self.active_sessions,
            self.config.max_connections,
        );

        // Generate a fresh host key set for this session
        // In practice, we'd share the long-term host key across sessions.
        // For now, clone the key data.
        let host_key = HybridHostKey::generate(&mut |buf| (self.rng_fill)(buf));

        let session = SshSession::new(
            host_key,
            self.authorized_users.clone(),
            self.rng_fill,
            self.config.banner.clone(),
            self.config.timeout_secs,
        );

        Some(session)
    }

    /// Notify the server that a session has ended.
    pub fn session_ended(&mut self) {
        self.active_sessions = self.active_sessions.saturating_sub(1);
        log::info!(
            "server: session ended ({}/{} active)",
            self.active_sessions,
            self.config.max_connections,
        );
    }

    /// Get the configured port.
    pub fn port(&self) -> u16 {
        self.config.port
    }

    /// Get the server's host key fingerprint (SHA-256 of Ed25519 public key blob).
    pub fn host_key_fingerprint(&self) -> Vec<u8> {
        use sha2::{Sha256, Digest};
        let blob = self.host_key.ed25519.public_key_blob();
        let mut hasher = Sha256::new();
        hasher.update(&blob);
        Vec::from(hasher.finalize().as_slice())
    }

    /// Get the number of active sessions.
    pub fn active_sessions(&self) -> usize {
        self.active_sessions
    }

    /// Serialize host keys for persistence.
    pub fn host_key_bytes(&self) -> Vec<u8> {
        self.host_key.to_bytes()
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &SshConfig {
        &self.config
    }
}
