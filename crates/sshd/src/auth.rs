//! SSH User Authentication (RFC 4252).
//!
//! Handles SSH_MSG_USERAUTH_REQUEST and implements:
//! - `"none"` method: always rejects, but learns the username
//! - `"publickey"` method: verifies Ed25519 or ML-DSA-65 signatures
//! - `"password"` method: optional, can be disabled in config
//!
//! Authentication attempt limiting (max 6 attempts per connection).

use alloc::string::String;
use alloc::vec::Vec;

use crate::hostkey;
use crate::wire::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum authentication attempts before the server disconnects the client.
/// This limits brute-force attacks. After 6 failed attempts, we send
/// SSH_DISCONNECT_NO_MORE_AUTH_METHODS and close the connection.
/// The value 6 matches OpenSSH's default `MaxAuthTries`.
pub const MAX_AUTH_ATTEMPTS: u32 = 6;

/// SSH service name for the authentication protocol.
pub const SSH_USERAUTH_SERVICE: &str = "ssh-userauth";

/// SSH service name requested after auth succeeds.
pub const SSH_CONNECTION_SERVICE: &str = "ssh-connection";

// ---------------------------------------------------------------------------
// Authorized key entry
// ---------------------------------------------------------------------------

/// An authorized public key for a user.
#[derive(Debug, Clone)]
pub struct AuthorizedKey {
    /// Key type name (e.g., "ssh-ed25519", "ml-dsa-65").
    pub key_type: String,
    /// Raw public key bytes.
    pub public_key: Vec<u8>,
    /// Optional comment/label.
    pub comment: String,
}

/// An authorized user with their allowed keys.
#[derive(Debug, Clone)]
pub struct AuthorizedUser {
    /// Username.
    pub username: String,
    /// List of authorized public keys.
    pub authorized_keys: Vec<AuthorizedKey>,
    /// Whether password auth is allowed for this user.
    pub allow_password: bool,
    /// Password hash (if password auth is enabled). SHA-256 hash.
    pub password_hash: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Authentication request parsing
// ---------------------------------------------------------------------------

/// Parsed SSH_MSG_USERAUTH_REQUEST (RFC 4252 section 5).
///
/// The SSH authentication protocol is a series of request/response exchanges.
/// The client sends a USERAUTH_REQUEST specifying a method, and the server
/// responds with SUCCESS, FAILURE, or a method-specific continuation message.
///
/// The "none" method is always the client's first request -- it probes for
/// available methods. The server always rejects it but returns the list of
/// methods the client can try (publickey, password).
#[derive(Debug)]
pub enum AuthRequest {
    /// "none" method — client probing for available methods.
    None {
        username: String,
        service: String,
    },
    /// "publickey" method without signature (query: "is this key acceptable?").
    PublicKeyQuery {
        username: String,
        service: String,
        algorithm: String,
        public_key_blob: Vec<u8>,
    },
    /// "publickey" method with signature (actual authentication attempt).
    PublicKeyAuth {
        username: String,
        service: String,
        algorithm: String,
        public_key_blob: Vec<u8>,
        signature: Vec<u8>,
    },
    /// "password" method.
    Password {
        username: String,
        service: String,
        password: String,
    },
}

/// Parse an SSH_MSG_USERAUTH_REQUEST payload.
pub fn parse_userauth_request(payload: &[u8]) -> Result<AuthRequest, AuthError> {
    let mut r = SshReader::new(payload);

    let msg_type = r.read_byte().map_err(|_| AuthError::MalformedRequest)?;
    if msg_type != SSH_MSG_USERAUTH_REQUEST {
        log::error!("auth: expected USERAUTH_REQUEST (50), got {}", msg_type);
        return Err(AuthError::UnexpectedMessage(msg_type));
    }

    let username = String::from(
        r.read_string_utf8().map_err(|_| AuthError::MalformedRequest)?,
    );
    let service = String::from(
        r.read_string_utf8().map_err(|_| AuthError::MalformedRequest)?,
    );
    let method = r.read_string_utf8().map_err(|_| AuthError::MalformedRequest)?;

    log::info!(
        "auth: USERAUTH_REQUEST — user='{}', service='{}', method='{}'",
        username,
        service,
        method,
    );

    match method {
        "none" => Ok(AuthRequest::None { username, service }),

        "publickey" => {
            let has_signature = r.read_boolean().map_err(|_| AuthError::MalformedRequest)?;
            let algorithm = String::from(
                r.read_string_utf8().map_err(|_| AuthError::MalformedRequest)?,
            );
            let public_key_blob = Vec::from(
                r.read_string_raw().map_err(|_| AuthError::MalformedRequest)?,
            );

            if has_signature {
                let signature = Vec::from(
                    r.read_string_raw().map_err(|_| AuthError::MalformedRequest)?,
                );
                log::debug!(
                    "auth: publickey auth — algo='{}', key={} bytes, sig={} bytes",
                    algorithm,
                    public_key_blob.len(),
                    signature.len(),
                );
                Ok(AuthRequest::PublicKeyAuth {
                    username,
                    service,
                    algorithm,
                    public_key_blob,
                    signature,
                })
            } else {
                log::debug!(
                    "auth: publickey query — algo='{}', key={} bytes",
                    algorithm,
                    public_key_blob.len(),
                );
                Ok(AuthRequest::PublicKeyQuery {
                    username,
                    service,
                    algorithm,
                    public_key_blob,
                })
            }
        }

        "password" => {
            let _change_password = r.read_boolean().unwrap_or(false);
            let password = String::from(
                r.read_string_utf8().map_err(|_| AuthError::MalformedRequest)?,
            );
            log::debug!("auth: password auth — user='{}'", username);
            Ok(AuthRequest::Password {
                username,
                service,
                password,
            })
        }

        _ => {
            log::warn!("auth: unsupported auth method: '{}'", method);
            Err(AuthError::UnsupportedMethod(String::from(method)))
        }
    }
}

// ---------------------------------------------------------------------------
// Authentication state
// ---------------------------------------------------------------------------

/// Per-connection authentication state.
///
/// Tracks the progress of authentication for a single SSH connection.
/// Each connection gets its own `AuthState` to prevent cross-connection
/// information leakage.
pub struct AuthState {
    /// Number of authentication attempts so far (including the "none" probe).
    /// Incremented on every request, even failed ones, to enforce MAX_AUTH_ATTEMPTS.
    pub attempts: u32,
    /// The authenticated username, set to `Some(name)` after a successful auth.
    /// Once set, the session transitions to the Interactive state.
    pub authenticated_user: Option<String>,
    /// Username from the most recent request. Learned from the "none" method
    /// probe so we can use it for logging and diagnostics before auth succeeds.
    pub last_username: Option<String>,
}

impl AuthState {
    pub fn new() -> Self {
        log::debug!("auth: new authentication state created");
        Self {
            attempts: 0,
            authenticated_user: None,
            last_username: None,
        }
    }

    /// Check if authentication is complete.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated_user.is_some()
    }

    /// Check if attempts are exhausted.
    pub fn attempts_exhausted(&self) -> bool {
        self.attempts >= MAX_AUTH_ATTEMPTS
    }

    /// Process an authentication request against the authorized users list.
    ///
    /// This is the core authentication dispatch. It handles each method:
    /// - **"none"**: Always rejects, but tells the client which methods to try.
    /// - **"publickey" (query)**: Checks if the key is in authorized_keys without
    ///   requiring a signature. Responds with PK_OK if accepted.
    /// - **"publickey" (auth)**: Verifies the cryptographic signature over the
    ///   session-specific signed data (RFC 4252 section 7). This prevents replay.
    /// - **"password"**: Compares SHA-256 hash of the password against stored hash.
    ///
    /// # Parameters
    /// - `request`: The parsed USERAUTH_REQUEST.
    /// - `authorized_users`: The list of users and their authorized keys/passwords.
    /// - `session_id`: The SSH session ID (first exchange hash), used in signature
    ///   verification to bind the auth to this specific session.
    ///
    /// # Returns
    /// The response payload to send to the client (SUCCESS, FAILURE, or PK_OK).
    pub fn process_request(
        &mut self,
        request: &AuthRequest,
        authorized_users: &[AuthorizedUser],
        session_id: &[u8],
    ) -> Result<Vec<u8>, AuthError> {
        if self.attempts_exhausted() {
            log::error!("auth: max attempts ({}) exceeded — disconnecting", MAX_AUTH_ATTEMPTS);
            return Err(AuthError::TooManyAttempts);
        }

        self.attempts += 1;
        log::debug!("auth: processing attempt {}/{}", self.attempts, MAX_AUTH_ATTEMPTS);

        match request {
            AuthRequest::None { username, .. } => {
                self.last_username = Some(username.clone());
                log::info!("auth: 'none' method for user '{}' — rejecting (as expected)", username);
                Ok(build_userauth_failure(&["publickey", "password"], false))
            }

            AuthRequest::PublicKeyQuery {
                username,
                algorithm,
                public_key_blob,
                ..
            } => {
                self.last_username = Some(username.clone());
                if is_key_authorized(username, algorithm, public_key_blob, authorized_users) {
                    log::info!("auth: public key query accepted for user '{}', algo='{}'", username, algorithm);
                    Ok(build_userauth_pk_ok(algorithm, public_key_blob))
                } else {
                    log::info!("auth: public key query rejected for user '{}', algo='{}'", username, algorithm);
                    Ok(build_userauth_failure(&["publickey", "password"], false))
                }
            }

            AuthRequest::PublicKeyAuth {
                username,
                service,
                algorithm,
                public_key_blob,
                signature,
            } => {
                self.last_username = Some(username.clone());

                if !is_key_authorized(username, algorithm, public_key_blob, authorized_users) {
                    log::warn!("auth: public key not authorized for user '{}'", username);
                    return Ok(build_userauth_failure(&["publickey", "password"], false));
                }

                // CRITICAL: Verify the client's signature over session-bound data.
                //
                // Per RFC 4252 section 7, the signature covers:
                //   string    session_id        <-- binds to THIS session (prevents replay)
                //   byte      SSH_MSG_USERAUTH_REQUEST (50)
                //   string    user name
                //   string    service name
                //   string    "publickey"
                //   boolean   TRUE
                //   string    public key algorithm name
                //   string    public key blob
                //
                // The session_id inclusion is essential: without it, an attacker who
                // observes one session could replay the signature in a different session.
                // Since session_id is derived from the key exchange (which uses fresh
                // ephemeral keys), each session has a unique ID.
                let signed_data = build_publickey_signed_data(
                    session_id,
                    username,
                    service,
                    algorithm,
                    public_key_blob,
                );

                let verified = match algorithm.as_str() {
                    "ssh-ed25519" => {
                        if public_key_blob.len() < 32 {
                            false
                        } else {
                            // Parse the public key from the blob
                            // Blob format: string "ssh-ed25519" + string key_data(32)
                            let mut kr = SshReader::new(public_key_blob);
                            let _key_type = kr.read_string_raw().ok();
                            let key_data = kr.read_string_raw().ok();
                            if let Some(kd) = key_data {
                                if kd.len() == 32 {
                                    let mut pk = [0u8; 32];
                                    pk.copy_from_slice(kd);
                                    // Parse signature blob: string "ssh-ed25519" + string sig(64)
                                    let mut sr = SshReader::new(signature);
                                    let _sig_type = sr.read_string_raw().ok();
                                    let sig_data = sr.read_string_raw().ok();
                                    if let Some(sd) = sig_data {
                                        hostkey::Ed25519HostKey::verify(&pk, &signed_data, sd)
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        }
                    }
                    "ml-dsa-65" => {
                        let mut kr = SshReader::new(public_key_blob);
                        let _key_type = kr.read_string_raw().ok();
                        let key_data = kr.read_string_raw().ok();
                        let mut sr = SshReader::new(signature);
                        let _sig_type = sr.read_string_raw().ok();
                        let sig_data = sr.read_string_raw().ok();
                        match (key_data, sig_data) {
                            (Some(kd), Some(sd)) => {
                                hostkey::MlDsa65HostKey::verify(kd, &signed_data, sd)
                            }
                            _ => false,
                        }
                    }
                    _ => {
                        log::warn!("auth: unsupported public key algorithm: '{}'", algorithm);
                        false
                    }
                };

                if verified {
                    log::info!("auth: user '{}' authenticated successfully via publickey ({})", username, algorithm);
                    self.authenticated_user = Some(username.clone());
                    Ok(build_userauth_success())
                } else {
                    log::warn!("auth: signature verification failed for user '{}' ({})", username, algorithm);
                    Ok(build_userauth_failure(&["publickey", "password"], false))
                }
            }

            AuthRequest::Password {
                username,
                password,
                ..
            } => {
                self.last_username = Some(username.clone());

                if let Some(user) = authorized_users.iter().find(|u| u.username == *username) {
                    if !user.allow_password {
                        log::info!("auth: password auth disabled for user '{}'", username);
                        return Ok(build_userauth_failure(&["publickey"], false));
                    }
                    if let Some(ref expected_hash) = user.password_hash {
                        use sha2::{Sha256, Digest};
                        let mut hasher = Sha256::new();
                        hasher.update(password.as_bytes());
                        let hash = hasher.finalize();
                        if hash.as_slice() == expected_hash.as_slice() {
                            log::info!("auth: user '{}' authenticated via password", username);
                            self.authenticated_user = Some(username.clone());
                            return Ok(build_userauth_success());
                        }
                    }
                }

                log::warn!("auth: password auth failed for user '{}'", username);
                Ok(build_userauth_failure(&["publickey", "password"], false))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key authorization check
// ---------------------------------------------------------------------------

/// Check if the given public key is authorized for the given user.
fn is_key_authorized(
    username: &str,
    algorithm: &str,
    public_key_blob: &[u8],
    authorized_users: &[AuthorizedUser],
) -> bool {
    for user in authorized_users {
        if user.username != username {
            continue;
        }
        for key in &user.authorized_keys {
            if key.key_type == algorithm && key.public_key == public_key_blob {
                log::debug!(
                    "auth: found authorized key for user '{}' — type='{}', comment='{}'",
                    username,
                    key.key_type,
                    key.comment,
                );
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Message builders
// ---------------------------------------------------------------------------

/// Build the data blob that the client's signature must cover (RFC 4252 section 7).
///
/// This constructs the exact byte sequence that the client should have signed
/// with their private key. We reconstruct it server-side and verify the
/// signature against this reconstruction.
///
/// The session_id at the beginning binds the signature to this specific
/// SSH session, preventing cross-session replay attacks.
fn build_publickey_signed_data(
    session_id: &[u8],
    username: &str,
    service: &str,
    algorithm: &str,
    public_key_blob: &[u8],
) -> Vec<u8> {
    let mut w = SshWriter::new();
    w.write_string(session_id);
    w.write_byte(SSH_MSG_USERAUTH_REQUEST);
    w.write_string_utf8(username);
    w.write_string_utf8(service);
    w.write_string_utf8("publickey");
    w.write_boolean(true);
    w.write_string_utf8(algorithm);
    w.write_string(public_key_blob);
    w.into_bytes()
}

/// Build SSH_MSG_USERAUTH_SUCCESS.
pub fn build_userauth_success() -> Vec<u8> {
    log::debug!("auth: building USERAUTH_SUCCESS");
    alloc::vec![SSH_MSG_USERAUTH_SUCCESS]
}

/// Build SSH_MSG_USERAUTH_FAILURE.
///
/// `methods` lists the authentication methods that can continue.
/// `partial_success` indicates if at least one auth method succeeded.
pub fn build_userauth_failure(methods: &[&str], partial_success: bool) -> Vec<u8> {
    log::debug!(
        "auth: building USERAUTH_FAILURE — methods={:?}, partial_success={}",
        methods,
        partial_success,
    );
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_USERAUTH_FAILURE);
    w.write_name_list(methods);
    w.write_boolean(partial_success);
    w.into_bytes()
}

/// Build SSH_MSG_USERAUTH_PK_OK (message type 60).
///
/// Sent when the server accepts a public key query (without signature).
pub fn build_userauth_pk_ok(algorithm: &str, public_key_blob: &[u8]) -> Vec<u8> {
    log::debug!("auth: building USERAUTH_PK_OK — algo='{}'", algorithm);
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_USERAUTH_PK_OK);
    w.write_string_utf8(algorithm);
    w.write_string(public_key_blob);
    w.into_bytes()
}

/// Build SSH_MSG_USERAUTH_BANNER.
pub fn build_userauth_banner(message: &str, language: &str) -> Vec<u8> {
    log::debug!("auth: building USERAUTH_BANNER — {} chars", message.len());
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_USERAUTH_BANNER);
    w.write_string_utf8(message);
    w.write_string_utf8(language);
    w.into_bytes()
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AuthError {
    /// USERAUTH_REQUEST is malformed.
    MalformedRequest,
    /// Unexpected message type.
    UnexpectedMessage(u8),
    /// Unsupported authentication method.
    UnsupportedMethod(String),
    /// Too many authentication attempts.
    TooManyAttempts,
    /// Signature verification failed.
    SignatureVerifyFailed,
}

impl core::fmt::Display for AuthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MalformedRequest => write!(f, "malformed USERAUTH_REQUEST"),
            Self::UnexpectedMessage(t) => write!(f, "unexpected message type: {}", t),
            Self::UnsupportedMethod(m) => write!(f, "unsupported auth method: {}", m),
            Self::TooManyAttempts => write!(f, "too many authentication attempts"),
            Self::SignatureVerifyFailed => write!(f, "signature verification failed"),
        }
    }
}
