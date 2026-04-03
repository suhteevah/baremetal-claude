//! SSH wire format helpers (RFC 4253 §4-6).
//!
//! Provides read/write primitives for the SSH binary packet protocol:
//! byte, boolean, uint32, uint64, string, mpint, and name-list.
//! Also defines all SSH_MSG_* constants used across the protocol.

use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// SSH Message Type Constants (RFC 4253 §12, RFC 4252 §6, RFC 4254 §9)
// ---------------------------------------------------------------------------

// -- Transport layer generic messages (1-19) --------------------------------
pub const SSH_MSG_DISCONNECT: u8 = 1;          // Graceful connection teardown
pub const SSH_MSG_IGNORE: u8 = 2;              // Padding/keep-alive (ignored by receiver)
pub const SSH_MSG_UNIMPLEMENTED: u8 = 3;       // Response to unrecognized message types
pub const SSH_MSG_DEBUG: u8 = 4;               // Debug messages (ignored in production)
pub const SSH_MSG_SERVICE_REQUEST: u8 = 5;     // Client requests a service (e.g., "ssh-userauth")
pub const SSH_MSG_SERVICE_ACCEPT: u8 = 6;      // Server accepts the requested service
pub const SSH_MSG_EXT_INFO: u8 = 7;            // Extension negotiation (RFC 8308)

// -- Key exchange (20-29: generic, 30-49: method-specific) ------------------
pub const SSH_MSG_KEXINIT: u8 = 20;            // Algorithm negotiation (both sides send this)
pub const SSH_MSG_NEWKEYS: u8 = 21;            // Signals transition to new encryption keys

// KEX method-specific messages (30-49). The meaning of 30/31 depends on
// the negotiated key exchange algorithm, but for both curve25519-sha256
// and the hybrid PQ KEX, they are ECDH_INIT and ECDH_REPLY.
pub const SSH_MSG_KEX_ECDH_INIT: u8 = 30;     // Client's ephemeral public value(s)
pub const SSH_MSG_KEX_ECDH_REPLY: u8 = 31;    // Server's host key + ephemeral + signature

// -- User authentication (50-79) --------------------------------------------
pub const SSH_MSG_USERAUTH_REQUEST: u8 = 50;
pub const SSH_MSG_USERAUTH_FAILURE: u8 = 51;
pub const SSH_MSG_USERAUTH_SUCCESS: u8 = 52;
pub const SSH_MSG_USERAUTH_BANNER: u8 = 53;
pub const SSH_MSG_USERAUTH_PK_OK: u8 = 60;

// -- Connection / channels (80-127) -----------------------------------------
pub const SSH_MSG_GLOBAL_REQUEST: u8 = 80;
pub const SSH_MSG_REQUEST_SUCCESS: u8 = 81;
pub const SSH_MSG_REQUEST_FAILURE: u8 = 82;
pub const SSH_MSG_CHANNEL_OPEN: u8 = 90;
pub const SSH_MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
pub const SSH_MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
pub const SSH_MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
pub const SSH_MSG_CHANNEL_DATA: u8 = 94;
pub const SSH_MSG_CHANNEL_EXTENDED_DATA: u8 = 95;
pub const SSH_MSG_CHANNEL_EOF: u8 = 96;
pub const SSH_MSG_CHANNEL_CLOSE: u8 = 97;
pub const SSH_MSG_CHANNEL_REQUEST: u8 = 98;
pub const SSH_MSG_CHANNEL_SUCCESS: u8 = 99;
pub const SSH_MSG_CHANNEL_FAILURE: u8 = 100;

// -- Disconnect reason codes (RFC 4253 §11.1) --------------------------------
pub const SSH_DISCONNECT_HOST_NOT_ALLOWED: u32 = 1;
pub const SSH_DISCONNECT_PROTOCOL_ERROR: u32 = 2;
pub const SSH_DISCONNECT_KEY_EXCHANGE_FAILED: u32 = 3;
pub const SSH_DISCONNECT_RESERVED: u32 = 4;
pub const SSH_DISCONNECT_MAC_ERROR: u32 = 5;
pub const SSH_DISCONNECT_COMPRESSION_ERROR: u32 = 6;
pub const SSH_DISCONNECT_SERVICE_NOT_AVAILABLE: u32 = 7;
pub const SSH_DISCONNECT_PROTOCOL_VERSION_NOT_SUPPORTED: u32 = 8;
pub const SSH_DISCONNECT_HOST_KEY_NOT_VERIFIABLE: u32 = 9;
pub const SSH_DISCONNECT_CONNECTION_LOST: u32 = 10;
pub const SSH_DISCONNECT_BY_APPLICATION: u32 = 11;
pub const SSH_DISCONNECT_TOO_MANY_CONNECTIONS: u32 = 12;
pub const SSH_DISCONNECT_AUTH_CANCELLED_BY_USER: u32 = 13;
pub const SSH_DISCONNECT_NO_MORE_AUTH_METHODS: u32 = 14;
pub const SSH_DISCONNECT_ILLEGAL_USER_NAME: u32 = 15;

// -- Channel open failure reason codes (RFC 4254 §5.1) -----------------------
pub const SSH_OPEN_ADMINISTRATIVELY_PROHIBITED: u32 = 1;
pub const SSH_OPEN_CONNECT_FAILED: u32 = 2;
pub const SSH_OPEN_UNKNOWN_CHANNEL_TYPE: u32 = 3;
pub const SSH_OPEN_RESOURCE_SHORTAGE: u32 = 4;

// -- Extended data type codes (RFC 4254 §5.2) --------------------------------
pub const SSH_EXTENDED_DATA_STDERR: u32 = 1;

// ---------------------------------------------------------------------------
// Maximum sizes
// ---------------------------------------------------------------------------

/// Maximum SSH packet size. RFC 4253 section 6.1 requires implementations
/// to support at least 35,000 bytes. We reject anything larger to prevent
/// memory exhaustion from malicious or buggy peers.
pub const MAX_PACKET_SIZE: usize = 35000;

/// Minimum padding length per RFC 4253 section 6: at least 4 bytes of
/// random padding must be included in every packet. This ensures the
/// packet is always at least partially opaque to traffic analysis.
pub const MIN_PADDING: usize = 4;

/// Maximum padding length: 255 bytes (limited by the single-byte
/// padding_length field in the packet header).
pub const MAX_PADDING: usize = 255;

/// Block size for unencrypted packets. RFC 4253 section 6 mandates a
/// minimum block size of 8 bytes for alignment purposes, even when no
/// cipher is active.
pub const UNENCRYPTED_BLOCK_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// SSH Wire Format Reader
// ---------------------------------------------------------------------------

/// A cursor-based reader for deserializing SSH wire-format data.
///
/// Reads SSH-encoded primitives (byte, boolean, uint32, string, mpint,
/// name-list) sequentially from a byte buffer. The cursor advances with
/// each read, and errors are returned if the buffer is exhausted.
///
/// All multi-byte integers are big-endian per RFC 4251 section 5.
pub struct SshReader<'a> {
    /// The underlying byte buffer being read.
    buf: &'a [u8],
    /// Current read position (byte offset into buf).
    pos: usize,
}

impl<'a> SshReader<'a> {
    /// Create a new reader over the given buffer.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Returns the number of bytes remaining.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Returns the current read position.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Read a single byte.
    pub fn read_byte(&mut self) -> Result<u8, WireError> {
        if self.pos >= self.buf.len() {
            return Err(WireError::UnexpectedEof);
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// Read a boolean (RFC 4251 §5: 0 = false, any other = true).
    pub fn read_boolean(&mut self) -> Result<bool, WireError> {
        Ok(self.read_byte()? != 0)
    }

    /// Read a uint32 in big-endian (RFC 4251 §5).
    pub fn read_uint32(&mut self) -> Result<u32, WireError> {
        if self.remaining() < 4 {
            return Err(WireError::UnexpectedEof);
        }
        let val = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(val)
    }

    /// Read a uint64 in big-endian.
    pub fn read_uint64(&mut self) -> Result<u64, WireError> {
        if self.remaining() < 8 {
            return Err(WireError::UnexpectedEof);
        }
        let val = u64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(val)
    }

    /// Read a string (RFC 4251 §5: uint32 length + data).
    /// Returns the raw bytes, not necessarily valid UTF-8.
    pub fn read_string_raw(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.read_uint32()? as usize;
        if len > MAX_PACKET_SIZE {
            return Err(WireError::StringTooLong(len));
        }
        if self.remaining() < len {
            return Err(WireError::UnexpectedEof);
        }
        let data = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(data)
    }

    /// Read a string as UTF-8.
    pub fn read_string_utf8(&mut self) -> Result<&'a str, WireError> {
        let raw = self.read_string_raw()?;
        core::str::from_utf8(raw).map_err(|_| WireError::InvalidUtf8)
    }

    /// Read a name-list (RFC 4251 §5: comma-separated names in a string).
    pub fn read_name_list(&mut self) -> Result<Vec<String>, WireError> {
        let raw = self.read_string_utf8()?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        Ok(raw.split(',').map(|s| String::from(s)).collect())
    }

    /// Read an mpint (RFC 4251 §5: two's complement big-endian, prefixed with uint32 length).
    /// Returns the raw byte representation.
    pub fn read_mpint(&mut self) -> Result<Vec<u8>, WireError> {
        let raw = self.read_string_raw()?;
        Ok(Vec::from(raw))
    }

    /// Read exactly `n` raw bytes.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.remaining() < n {
            return Err(WireError::UnexpectedEof);
        }
        let data = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(data)
    }

    /// Skip `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<(), WireError> {
        if self.remaining() < n {
            return Err(WireError::UnexpectedEof);
        }
        self.pos += n;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SSH Wire Format Writer
// ---------------------------------------------------------------------------

/// A buffer-based writer that appends SSH wire-format data.
pub struct SshWriter {
    buf: Vec<u8>,
}

impl SshWriter {
    /// Create a new writer with default capacity.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(256),
        }
    }

    /// Create a new writer with the given capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    /// Consume the writer and return the underlying buffer.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Get a reference to the written bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Current length of written data.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Write a single byte.
    pub fn write_byte(&mut self, b: u8) {
        self.buf.push(b);
    }

    /// Write a boolean.
    pub fn write_boolean(&mut self, b: bool) {
        self.buf.push(if b { 1 } else { 0 });
    }

    /// Write a uint32 in big-endian.
    pub fn write_uint32(&mut self, val: u32) {
        self.buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Write a uint64 in big-endian.
    pub fn write_uint64(&mut self, val: u64) {
        self.buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Write a string (uint32 length prefix + data).
    pub fn write_string(&mut self, data: &[u8]) {
        self.write_uint32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }

    /// Write a UTF-8 string.
    pub fn write_string_utf8(&mut self, s: &str) {
        self.write_string(s.as_bytes());
    }

    /// Write a name-list (comma-separated names as a string).
    pub fn write_name_list(&mut self, names: &[&str]) {
        let joined: String = names
            .iter()
            .enumerate()
            .fold(String::new(), |mut acc, (i, name)| {
                if i > 0 {
                    acc.push(',');
                }
                acc.push_str(name);
                acc
            });
        self.write_string_utf8(&joined);
    }

    /// Write an mpint (RFC 4251 §5).
    /// Input is unsigned big-endian bytes. Adds leading zero if high bit set.
    pub fn write_mpint(&mut self, data: &[u8]) {
        // Strip leading zeros
        let stripped = match data.iter().position(|&b| b != 0) {
            Some(pos) => &data[pos..],
            None => &[0u8; 0], // value is zero
        };

        if stripped.is_empty() {
            // Zero: length 0
            self.write_uint32(0);
        } else if stripped[0] & 0x80 != 0 {
            // High bit set: prepend zero byte for positive number
            self.write_uint32(stripped.len() as u32 + 1);
            self.buf.push(0);
            self.buf.extend_from_slice(stripped);
        } else {
            self.write_uint32(stripped.len() as u32);
            self.buf.extend_from_slice(stripped);
        }
    }

    /// Write raw bytes (no length prefix).
    pub fn write_raw(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
}

// ---------------------------------------------------------------------------
// Padding calculation
// ---------------------------------------------------------------------------

/// Calculate the padding length for an SSH binary packet.
///
/// RFC 4253 §6: `packet_length || padding_length || payload || padding`
/// - `packet_length` = 4 bytes (not included in itself)
/// - Total of (padding_length + payload + padding) must be multiple of block_size
/// - Padding must be 4..=255 bytes
pub fn compute_padding(payload_len: usize, block_size: usize) -> usize {
    let block = if block_size < UNENCRYPTED_BLOCK_SIZE {
        UNENCRYPTED_BLOCK_SIZE
    } else {
        block_size
    };

    // packet_length(4) + padding_length(1) + payload + padding = multiple of block
    // So: 5 + payload_len + padding = N * block
    let unpadded = 5 + payload_len;
    let mut padding = block - (unpadded % block);
    if padding < MIN_PADDING {
        padding += block;
    }
    padding
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur when reading/writing SSH wire format.
#[derive(Debug, Clone)]
pub enum WireError {
    /// Not enough data in the buffer.
    UnexpectedEof,
    /// String length exceeds maximum.
    StringTooLong(usize),
    /// String is not valid UTF-8.
    InvalidUtf8,
    /// Packet exceeds maximum allowed size.
    PacketTooLarge(usize),
    /// Invalid padding.
    InvalidPadding,
    /// Unknown message type.
    UnknownMessageType(u8),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of SSH data"),
            Self::StringTooLong(n) => write!(f, "SSH string too long: {} bytes", n),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in SSH string"),
            Self::PacketTooLarge(n) => write!(f, "SSH packet too large: {} bytes", n),
            Self::InvalidPadding => write!(f, "invalid SSH packet padding"),
            Self::UnknownMessageType(t) => write!(f, "unknown SSH message type: {}", t),
        }
    }
}
