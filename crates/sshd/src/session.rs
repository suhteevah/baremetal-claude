//! SSH Session Handler.
//!
//! Per-connection state machine managing the full SSH session lifecycle:
//! `VersionExchange -> KeyExchange -> Authentication -> Interactive`
//!
//! Reads/writes SSH packets, dispatches messages by type, handles
//! session timeouts and clean shutdown (SSH_MSG_DISCONNECT).

use alloc::string::String;
use alloc::vec::Vec;

use crate::auth::{self, AuthState, AuthorizedUser};
use crate::channel::{ChannelAction, ChannelManager};
use crate::hostkey::HybridHostKey;
use crate::kex::{self, HybridKexServerState, ClassicalKexServerState, KexError, NegotiatedAlgorithms};
use crate::transport::{self, CipherState, KexInit, SequenceCounter};
use crate::wire::*;

// ---------------------------------------------------------------------------
// Session state machine
// ---------------------------------------------------------------------------

/// High-level session states forming a linear state machine.
///
/// The SSH protocol progresses through these states in order:
/// ```text
/// VersionExchange -> KeyExchange -> Authenticating -> Interactive -> Disconnected
/// ```
/// Any state can transition directly to `Disconnected` if an error occurs
/// or either side sends SSH_MSG_DISCONNECT.
///
/// Re-keying (repeat key exchange during an active session) would cycle
/// back from Interactive to KeyExchange, but is not yet implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Initial state: exchanging SSH-2.0 version strings over raw TCP.
    /// No encryption, no framing -- just line-oriented text.
    VersionExchange,
    /// KEXINIT sent, performing key exchange (algorithm negotiation, DH, NEWKEYS).
    /// Packets are still unencrypted until NEWKEYS is received.
    KeyExchange,
    /// Key exchange complete, encryption active. Waiting for the client to
    /// request and complete user authentication (SSH_MSG_USERAUTH_REQUEST).
    Authenticating,
    /// Authenticated. The client can open channels, request shells/exec,
    /// and send/receive data. This is the normal operating state.
    Interactive,
    /// Session has been disconnected (cleanly or due to error).
    /// No further packets will be sent or processed.
    Disconnected,
}

impl core::fmt::Display for SessionState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::VersionExchange => write!(f, "VersionExchange"),
            Self::KeyExchange => write!(f, "KeyExchange"),
            Self::Authenticating => write!(f, "Authenticating"),
            Self::Interactive => write!(f, "Interactive"),
            Self::Disconnected => write!(f, "Disconnected"),
        }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A single SSH session (one per TCP connection).
///
/// Encapsulates all state for an SSH connection: protocol state machine,
/// encryption keys, authentication progress, and open channels. The session
/// processes raw TCP data and produces framed, encrypted packets for
/// transmission.
///
/// ## Ownership
/// Each TCP connection spawns one `SshSession`. The caller (kernel network
/// stack) feeds raw bytes in via `on_version_received` and `on_data_received`,
/// and drains outgoing packets via `drain_outgoing`.
pub struct SshSession {
    /// Current protocol state (version exchange, KEX, auth, interactive, etc.).
    pub state: SessionState,
    /// Session ID: the exchange hash H from the *first* key exchange.
    /// Per RFC 4253 section 7.2, the session ID never changes, even after
    /// re-keying. It is used in key derivation and authentication signatures.
    session_id: Option<Vec<u8>>,
    /// Peer's version string.
    peer_version: Option<String>,
    /// Our server KEXINIT payload (needed for exchange hash).
    server_kexinit_payload: Option<Vec<u8>>,
    /// Client's parsed KEXINIT.
    client_kexinit: Option<KexInit>,
    /// Negotiated algorithms.
    negotiated: Option<NegotiatedAlgorithms>,
    /// Host key reference (shared across sessions).
    host_key: HybridHostKey,
    /// Authorized users.
    authorized_users: Vec<AuthorizedUser>,
    /// Authentication state.
    auth_state: AuthState,
    /// Channel manager.
    channels: ChannelManager,
    /// Send sequence counter.
    send_seq: SequenceCounter,
    /// Receive sequence counter.
    recv_seq: SequenceCounter,
    /// Send cipher.
    send_cipher: CipherState,
    /// Receive cipher.
    recv_cipher: CipherState,
    /// Output buffer: packets to be sent to the client.
    outgoing: Vec<Vec<u8>>,
    /// RNG function.
    rng_fill: fn(&mut [u8]),
    /// Session banner to send before auth.
    banner: Option<String>,
    /// Inactivity timeout in seconds (0 = disabled).
    timeout_secs: u64,
    /// Timestamp of last activity.
    last_activity: u64,
}

impl SshSession {
    /// Create a new SSH session.
    pub fn new(
        host_key: HybridHostKey,
        authorized_users: Vec<AuthorizedUser>,
        rng_fill: fn(&mut [u8]),
        banner: Option<String>,
        timeout_secs: u64,
    ) -> Self {
        log::info!("session: new SSH session created");
        Self {
            state: SessionState::VersionExchange,
            session_id: None,
            peer_version: None,
            server_kexinit_payload: None,
            client_kexinit: None,
            negotiated: None,
            host_key,
            authorized_users,
            auth_state: AuthState::new(),
            channels: ChannelManager::new(),
            send_seq: SequenceCounter::new(),
            recv_seq: SequenceCounter::new(),
            send_cipher: CipherState::Plaintext,
            recv_cipher: CipherState::Plaintext,
            outgoing: Vec::new(),
            rng_fill,
            banner,
            timeout_secs,
            last_activity: 0,
        }
    }

    /// Get the current session state.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Check if the session is still alive.
    pub fn is_alive(&self) -> bool {
        self.state != SessionState::Disconnected
    }

    /// Get pending outgoing packets (framed, ready to send over TCP).
    pub fn drain_outgoing(&mut self) -> Vec<Vec<u8>> {
        core::mem::take(&mut self.outgoing)
    }

    /// Get the channel manager for external I/O wiring.
    pub fn channels(&mut self) -> &mut ChannelManager {
        &mut self.channels
    }

    /// Encrypt, frame, and queue a payload for sending to the client.
    /// Handles sequence number advancement and cipher state automatically.
    fn queue_packet(&mut self, payload: &[u8]) {
        let seq = self.send_seq.next();
        let rng = self.rng_fill;
        let packet = match transport::frame_packet_encrypted(
            payload,
            seq,
            &self.send_cipher,
            &|buf| rng(buf),
        ) {
            Ok(p) => p,
            Err(e) => {
                log::error!("session: failed to frame packet: {}", e);
                return;
            }
        };
        self.outgoing.push(packet);
    }

    // -----------------------------------------------------------------------
    // Phase 1: Version exchange
    // -----------------------------------------------------------------------

    /// Get the server version string bytes (with CR LF) to send first.
    pub fn version_bytes(&self) -> Vec<u8> {
        transport::version_string()
    }

    /// Feed the client's version string. Transitions to KeyExchange.
    pub fn on_version_received(&mut self, data: &[u8]) -> Result<(), SessionError> {
        log::info!("session: processing peer version string ({} bytes)", data.len());

        let version = transport::parse_version_string(data)
            .map_err(|e| SessionError::Transport(e))?;

        log::info!("session: peer version: {}", version);
        self.peer_version = Some(version);

        // Transition to key exchange: send our KEXINIT
        self.state = SessionState::KeyExchange;
        self.send_kexinit();

        Ok(())
    }

    /// Build and queue our server KEXINIT.
    fn send_kexinit(&mut self) {
        let mut cookie = [0u8; 16];
        (self.rng_fill)(&mut cookie);

        let payload = KexInit::build_server(cookie);
        self.server_kexinit_payload = Some(payload.clone());
        self.queue_packet(&payload);

        log::info!("session: server KEXINIT queued");
    }

    // -----------------------------------------------------------------------
    // Phase 2+: Packet dispatch
    // -----------------------------------------------------------------------

    /// Feed a received SSH binary packet (raw bytes from TCP, may contain
    /// multiple packets). Returns actions for the session handler.
    ///
    /// The caller should buffer TCP data and call this when at least one
    /// full packet is available.
    pub fn on_data_received(&mut self, data: &[u8]) -> Result<Vec<ChannelAction>, SessionError> {
        let mut actions = Vec::new();
        let mut offset = 0;

        while offset < data.len() {
            let remaining = &data[offset..];

            // Need at least 5 bytes for a packet header
            if remaining.len() < 5 {
                break;
            }

            let (payload, consumed) = transport::parse_packet_encrypted(
                remaining,
                self.recv_seq.next(),
                &self.recv_cipher,
            )
            .map_err(|e| SessionError::Transport(e))?;

            offset += consumed;
            self.last_activity = 0; // TODO: wire to actual timestamp

            if payload.is_empty() {
                log::warn!("session: received empty payload");
                continue;
            }

            let msg_type = payload[0];
            log::trace!("session: received message type {} ({} bytes payload)", msg_type, payload.len());

            match msg_type {
                SSH_MSG_DISCONNECT => {
                    self.handle_disconnect(&payload)?;
                }
                SSH_MSG_IGNORE => {
                    log::trace!("session: received SSH_MSG_IGNORE");
                }
                SSH_MSG_UNIMPLEMENTED => {
                    log::warn!("session: peer sent SSH_MSG_UNIMPLEMENTED");
                }
                SSH_MSG_DEBUG => {
                    log::trace!("session: received SSH_MSG_DEBUG");
                }
                SSH_MSG_KEXINIT => {
                    self.handle_client_kexinit(&payload)?;
                }
                SSH_MSG_NEWKEYS => {
                    self.handle_newkeys()?;
                }
                SSH_MSG_KEX_ECDH_INIT => {
                    self.handle_kex_ecdh_init(&payload)?;
                }
                SSH_MSG_SERVICE_REQUEST => {
                    self.handle_service_request(&payload)?;
                }
                SSH_MSG_USERAUTH_REQUEST => {
                    self.handle_userauth_request(&payload)?;
                }
                SSH_MSG_CHANNEL_OPEN => {
                    self.handle_channel_open(&payload)?;
                }
                SSH_MSG_CHANNEL_REQUEST => {
                    let action = self.handle_channel_request(&payload)?;
                    if !matches!(action, ChannelAction::None) {
                        actions.push(action);
                    }
                }
                SSH_MSG_CHANNEL_DATA => {
                    let action = self.handle_channel_data(&payload)?;
                    actions.push(action);
                }
                SSH_MSG_CHANNEL_EXTENDED_DATA => {
                    log::debug!("session: received CHANNEL_EXTENDED_DATA — ignoring");
                }
                SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                    self.handle_window_adjust(&payload)?;
                }
                SSH_MSG_CHANNEL_EOF => {
                    self.handle_channel_eof(&payload)?;
                }
                SSH_MSG_CHANNEL_CLOSE => {
                    self.handle_channel_close(&payload)?;
                }
                SSH_MSG_GLOBAL_REQUEST => {
                    log::debug!("session: received GLOBAL_REQUEST — rejecting");
                    self.queue_packet(&[SSH_MSG_REQUEST_FAILURE]);
                }
                _ => {
                    log::warn!("session: unhandled message type: {}", msg_type);
                    let mut w = SshWriter::new();
                    w.write_byte(SSH_MSG_UNIMPLEMENTED);
                    w.write_uint32(self.recv_seq.current().wrapping_sub(1));
                    self.queue_packet(&w.into_bytes());
                }
            }
        }

        // Check if any channels need window adjustments
        let needs_adjust = self.channels.channels_needing_window_adjust();
        for ch_id in needs_adjust {
            if let Ok(payload) = self.channels.build_window_adjust(ch_id, crate::channel::DEFAULT_WINDOW_SIZE / 2) {
                self.queue_packet(&payload);
            }
        }

        Ok(actions)
    }

    // -----------------------------------------------------------------------
    // Message handlers
    // -----------------------------------------------------------------------

    fn handle_disconnect(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        let mut r = SshReader::new(payload);
        let _ = r.read_byte(); // msg type
        let reason = r.read_uint32().unwrap_or(0);
        let desc = r.read_string_utf8().unwrap_or("(none)");

        log::info!("session: received DISCONNECT — reason={}, desc='{}'", reason, desc);
        self.state = SessionState::Disconnected;
        Ok(())
    }

    fn handle_client_kexinit(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        log::info!("session: received client KEXINIT");

        let client_kexinit = KexInit::parse(payload)
            .map_err(|e| SessionError::Transport(e))?;

        // Negotiate algorithms
        let algorithms = kex::negotiate(
            &client_kexinit,
            transport::KEX_ALGORITHMS,
            transport::HOST_KEY_ALGORITHMS,
            transport::ENCRYPTION_ALGORITHMS,
            transport::MAC_ALGORITHMS,
            transport::COMPRESSION_ALGORITHMS,
        )
        .map_err(|e| SessionError::Kex(e))?;

        log::info!("session: algorithms negotiated — kex={}", algorithms.kex);

        self.client_kexinit = Some(client_kexinit);
        self.negotiated = Some(algorithms);

        Ok(())
    }

    fn handle_kex_ecdh_init(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        log::info!("session: received KEX_ECDH_INIT");

        let algorithms = self.negotiated.as_ref()
            .ok_or(SessionError::InvalidState("no negotiated algorithms"))?;

        let client_kexinit = self.client_kexinit.as_ref()
            .ok_or(SessionError::InvalidState("no client kexinit"))?;
        let server_kexinit = self.server_kexinit_payload.as_ref()
            .ok_or(SessionError::InvalidState("no server kexinit"))?;
        let peer_version = self.peer_version.as_ref()
            .ok_or(SessionError::InvalidState("no peer version"))?;

        let client_ephemeral = kex::parse_kex_ecdh_init(payload)
            .map_err(|e| SessionError::Kex(e))?;

        // Perform key exchange based on negotiated algorithm
        let (shared_secret, server_ephemeral) = if algorithms.kex == "mlkem768x25519-sha256@openssh.com" {
            log::info!("session: performing hybrid PQ KEX (ML-KEM-768 + X25519)");
            let rng = self.rng_fill;
            let state = HybridKexServerState::generate(&mut |buf| rng(buf));
            let server_eph = state.server_ephemeral_public();
            let shared = state.compute_shared_secret(&client_ephemeral)
                .map_err(|e| SessionError::Kex(e))?;
            (shared, server_eph)
        } else {
            log::info!("session: performing classical KEX (curve25519-sha256)");
            let rng = self.rng_fill;
            let state = ClassicalKexServerState::generate(&mut |buf| rng(buf));
            let server_eph = Vec::from(state.server_ephemeral_public().as_slice());
            let shared = state.compute_shared_secret(&client_ephemeral)
                .map_err(|e| SessionError::Kex(e))?;
            (shared, server_eph)
        };

        // Get host key blob
        let host_key_blob = if algorithms.host_key.contains("ed25519") && algorithms.host_key.contains("mlkem") {
            self.host_key.public_key_blob()
        } else {
            self.host_key.ed25519.public_key_blob()
        };

        // Compute exchange hash
        let exchange_hash = kex::compute_exchange_hash(
            peer_version,
            transport::SSH_VERSION_STRING,
            &client_kexinit.raw_payload,
            server_kexinit,
            &host_key_blob,
            &client_ephemeral,
            &server_ephemeral,
            &shared_secret,
        );

        // Session ID is the first exchange hash
        if self.session_id.is_none() {
            log::info!("session: session ID set from first exchange hash");
            self.session_id = Some(exchange_hash.clone());
        }

        // Sign the exchange hash
        let signature = if algorithms.host_key.contains("mlkem") {
            self.host_key.sign(&exchange_hash)
        } else {
            self.host_key.ed25519.sign(&exchange_hash)
        };

        // Send KEX_ECDH_REPLY
        let reply = kex::build_kex_ecdh_reply(&host_key_blob, &server_ephemeral, &signature);
        self.queue_packet(&reply);

        // Send NEWKEYS
        let newkeys = transport::build_newkeys();
        self.queue_packet(&newkeys);

        // Derive encryption keys per RFC 4253 section 7.2.
        // For chacha20-poly1305@openssh.com, we need:
        // - IV length = 0 (ChaCha20 uses the sequence number as nonce, not a derived IV)
        // - Encryption key length = 64 (two 32-byte keys: main key + header key)
        // - Integrity key length = 0 (Poly1305 is integrated into the AEAD, no separate MAC)
        let session_id = self.session_id.as_ref().unwrap();
        let keys = kex::derive_keys(
            &shared_secret,
            &exchange_hash,
            session_id,
            0,  // no IV needed for chacha20-poly1305 (uses seq number as nonce)
            64, // 64 bytes = two 32-byte keys (main + header) per direction
            0,  // no separate integrity key (MAC is built into the AEAD cipher)
        );

        // Store keys for activation after we receive client NEWKEYS
        log::info!("session: KEX complete — keys derived, waiting for client NEWKEYS");

        // Prepare cipher states for both directions.
        // For chacha20-poly1305@openssh.com, the 64-byte derived key is split:
        //   bytes [0..32]  = main encryption key (ChaCha20 for body + Poly1305 MAC)
        //   bytes [32..64] = header encryption key (ChaCha20 for 4-byte packet length)
        let mut send_key = [0u8; 32];
        let mut send_header = [0u8; 32];
        if keys.enc_key_s2c.len() >= 64 {
            send_key.copy_from_slice(&keys.enc_key_s2c[..32]);
            send_header.copy_from_slice(&keys.enc_key_s2c[32..64]);
        }

        let mut recv_key = [0u8; 32];
        let mut recv_header = [0u8; 32];
        if keys.enc_key_c2s.len() >= 64 {
            recv_key.copy_from_slice(&keys.enc_key_c2s[..32]);
            recv_header.copy_from_slice(&keys.enc_key_c2s[32..64]);
        }

        self.send_cipher = CipherState::ChaCha20Poly1305 {
            key: send_key,
            header_key: send_header,
        };
        self.recv_cipher = CipherState::ChaCha20Poly1305 {
            key: recv_key,
            header_key: recv_header,
        };

        Ok(())
    }

    fn handle_newkeys(&mut self) -> Result<(), SessionError> {
        log::info!("session: received NEWKEYS — encryption active");
        // Ciphers were already set in handle_kex_ecdh_init; they're now live.
        self.state = SessionState::Authenticating;

        // Send banner if configured
        if let Some(ref banner) = self.banner {
            let banner_msg = auth::build_userauth_banner(banner, "en");
            self.queue_packet(&banner_msg);
        }

        Ok(())
    }

    fn handle_service_request(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        let mut r = SshReader::new(payload);
        let _ = r.read_byte(); // msg type
        let service = r.read_string_utf8().map_err(|_| SessionError::MalformedMessage)?;

        log::info!("session: SERVICE_REQUEST for '{}'", service);

        if service == auth::SSH_USERAUTH_SERVICE {
            let accept = transport::build_service_accept(service);
            self.queue_packet(&accept);
        } else {
            log::warn!("session: rejecting unknown service: '{}'", service);
            let disconnect = transport::build_disconnect(
                SSH_DISCONNECT_SERVICE_NOT_AVAILABLE,
                "service not available",
            );
            self.queue_packet(&disconnect);
            self.state = SessionState::Disconnected;
        }

        Ok(())
    }

    fn handle_userauth_request(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        let request = auth::parse_userauth_request(payload)
            .map_err(|e| SessionError::Auth(e))?;

        let session_id = self.session_id.as_ref()
            .ok_or(SessionError::InvalidState("no session ID for auth"))?
            .clone();

        match self.auth_state.process_request(&request, &self.authorized_users, &session_id) {
            Ok(response) => {
                self.queue_packet(&response);

                if self.auth_state.is_authenticated() {
                    log::info!(
                        "session: user '{}' authenticated — transitioning to Interactive",
                        self.auth_state.authenticated_user.as_deref().unwrap_or("?"),
                    );
                    self.state = SessionState::Interactive;
                } else if self.auth_state.attempts_exhausted() {
                    log::error!("session: auth attempts exhausted — disconnecting");
                    let disconnect = transport::build_disconnect(
                        SSH_DISCONNECT_NO_MORE_AUTH_METHODS,
                        "too many authentication failures",
                    );
                    self.queue_packet(&disconnect);
                    self.state = SessionState::Disconnected;
                }
            }
            Err(auth::AuthError::TooManyAttempts) => {
                let disconnect = transport::build_disconnect(
                    SSH_DISCONNECT_NO_MORE_AUTH_METHODS,
                    "too many authentication failures",
                );
                self.queue_packet(&disconnect);
                self.state = SessionState::Disconnected;
            }
            Err(e) => {
                log::error!("session: auth error: {}", e);
                return Err(SessionError::Auth(e));
            }
        }

        Ok(())
    }

    fn handle_channel_open(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        if self.state != SessionState::Interactive {
            log::error!("session: CHANNEL_OPEN before authentication");
            return Err(SessionError::InvalidState("not authenticated"));
        }

        let (response, local_id) = self.channels.handle_channel_open(payload)
            .map_err(|e| SessionError::Channel(e))?;

        self.queue_packet(&response);
        log::info!("session: channel {} opened", local_id);

        Ok(())
    }

    fn handle_channel_request(&mut self, payload: &[u8]) -> Result<ChannelAction, SessionError> {
        let (response, action) = self.channels.handle_channel_request(payload)
            .map_err(|e| SessionError::Channel(e))?;

        if let Some(resp) = response {
            self.queue_packet(&resp);
        }

        Ok(action)
    }

    fn handle_channel_data(&mut self, payload: &[u8]) -> Result<ChannelAction, SessionError> {
        let (channel_id, data) = self.channels.handle_channel_data(payload)
            .map_err(|e| SessionError::Channel(e))?;

        log::trace!("session: channel {} received {} bytes data", channel_id, data.len());

        // Return the data as an action so the server can route it to the terminal pane
        Ok(ChannelAction::ExecCommand {
            channel_id,
            command: String::from_utf8_lossy(&data).into_owned(),
        })
    }

    fn handle_window_adjust(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        self.channels.handle_window_adjust(payload)
            .map_err(|e| SessionError::Channel(e))
    }

    fn handle_channel_eof(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        let channel_id = self.channels.handle_channel_eof(payload)
            .map_err(|e| SessionError::Channel(e))?;
        log::info!("session: channel {} received EOF", channel_id);
        Ok(())
    }

    fn handle_channel_close(&mut self, payload: &[u8]) -> Result<(), SessionError> {
        let (channel_id, response) = self.channels.handle_channel_close(payload)
            .map_err(|e| SessionError::Channel(e))?;

        if let Some(resp) = response {
            self.queue_packet(&resp);
        }

        log::info!("session: channel {} closed", channel_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Sending data to client
    // -----------------------------------------------------------------------

    /// Send data on a channel (from the terminal pane to the SSH client).
    pub fn send_channel_data(&mut self, channel_id: u32, data: &[u8]) -> Result<(), SessionError> {
        let payload = self.channels.build_channel_data(channel_id, data)
            .map_err(|e| SessionError::Channel(e))?;
        self.queue_packet(&payload);
        Ok(())
    }

    /// Initiate a clean disconnect.
    pub fn disconnect(&mut self, reason: u32, description: &str) {
        log::info!("session: initiating disconnect — reason={}, desc='{}'", reason, description);
        let payload = transport::build_disconnect(reason, description);
        self.queue_packet(&payload);
        self.state = SessionState::Disconnected;
    }

    /// Check session timeout. Call periodically with the current timestamp.
    pub fn check_timeout(&mut self, now: u64) -> bool {
        if self.timeout_secs == 0 {
            return false;
        }
        if self.last_activity > 0 && now - self.last_activity > self.timeout_secs {
            log::warn!(
                "session: timeout after {} seconds of inactivity",
                now - self.last_activity,
            );
            self.disconnect(SSH_DISCONNECT_BY_APPLICATION, "session timed out");
            return true;
        }
        self.last_activity = now;
        false
    }
}

// ---------------------------------------------------------------------------
// from_utf8_lossy helper for no_std
// ---------------------------------------------------------------------------

trait FromUtf8Lossy {
    fn from_utf8_lossy(bytes: &[u8]) -> alloc::borrow::Cow<'_, str>;
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SessionError {
    /// Transport-layer error.
    Transport(transport::TransportError),
    /// Key exchange error.
    Kex(KexError),
    /// Authentication error.
    Auth(auth::AuthError),
    /// Channel error.
    Channel(crate::channel::ChannelError),
    /// Invalid state transition.
    InvalidState(&'static str),
    /// Malformed message.
    MalformedMessage,
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {}", e),
            Self::Kex(e) => write!(f, "kex: {}", e),
            Self::Auth(e) => write!(f, "auth: {}", e),
            Self::Channel(e) => write!(f, "channel: {}", e),
            Self::InvalidState(s) => write!(f, "invalid state: {}", s),
            Self::MalformedMessage => write!(f, "malformed message"),
        }
    }
}
