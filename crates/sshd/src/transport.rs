//! SSH Transport Layer (RFC 4253).
//!
//! Implements the SSH binary packet protocol, which is the lowest layer of
//! the SSH protocol stack. This layer handles:
//!
//! - **Version string exchange** (`SSH-2.0-ClaudioOS_0.1`): The very first
//!   bytes sent on an SSH connection, identifying the protocol version and
//!   implementation. Per RFC 4253 section 4.2.
//!
//! - **Binary packet framing**: Every SSH message is wrapped in a binary
//!   packet with structure `packet_length(4) + padding_length(1) + payload +
//!   random_padding + MAC`. The padding ensures alignment to cipher block
//!   boundaries and frustrates traffic analysis.
//!
//! - **SSH_MSG_KEXINIT construction and parsing**: Algorithm negotiation
//!   messages that both sides exchange to agree on cryptographic algorithms.
//!
//! - **SSH_MSG_NEWKEYS handling**: Signals the transition from plaintext
//!   to encrypted communication after key exchange completes.
//!
//! - **Packet encryption/decryption**: After keys are established via KEX,
//!   all packets are encrypted using ChaCha20-Poly1305 AEAD. This provides
//!   both confidentiality (ChaCha20 stream cipher) and integrity (Poly1305
//!   MAC) in a single authenticated encryption operation.
//!
//! - **Sequence number tracking**: Each direction (send/receive) maintains
//!   an independent 32-bit counter that wraps at u32::MAX. The sequence
//!   number is used as the nonce for ChaCha20-Poly1305 encryption, ensuring
//!   each packet uses a unique nonce.
//!
//! - **Maximum packet size enforcement**: Prevents memory exhaustion attacks
//!   by rejecting packets exceeding 35,000 bytes (RFC 4253 section 6.1).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ChaCha20-Poly1305 AEAD cipher — the only cipher we support.
// This is the same `chacha20-poly1305@openssh.com` construction used by OpenSSH,
// which combines the ChaCha20 stream cipher with Poly1305 MAC for authenticated
// encryption. It avoids the need for a separate MAC algorithm.
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use chacha20poly1305::aead::generic_array::GenericArray;

use crate::wire::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Our SSH version string (no trailing CR LF -- caller appends that).
/// Format per RFC 4253 section 4.2: `SSH-protoversion-softwareversion [SP comments]`
/// We identify as `ClaudioOS_0.1` so clients can fingerprint our server.
pub const SSH_VERSION_STRING: &str = "SSH-2.0-ClaudioOS_0.1";

/// The required prefix for any valid SSH-2.0 version string from a peer.
/// If a peer sends a version string not starting with this, we reject it.
pub const SSH_VERSION_PREFIX: &str = "SSH-2.0-";

/// KEXINIT cookie size: 16 random bytes included in every SSH_MSG_KEXINIT
/// message. These cookies are used to prevent replay attacks during key
/// exchange -- each side generates a fresh random cookie per KEXINIT.
pub const COOKIE_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Algorithm lists — our server advertises these in SSH_MSG_KEXINIT
// ---------------------------------------------------------------------------

/// Key exchange algorithms, ordered by preference (best first).
/// Per RFC 4253 section 7.1, the server's preference takes priority.
///
/// - `mlkem768x25519-sha256@openssh.com`: Hybrid post-quantum KEX combining
///   ML-KEM-768 (NIST FIPS 203, formerly CRYSTALS-Kyber) with X25519.
///   Provides quantum resistance even if one primitive is broken.
/// - `curve25519-sha256`: Classical Elliptic Curve Diffie-Hellman on Curve25519.
/// - `curve25519-sha256@libssh.org`: Same algorithm, older name used by libssh.
pub const KEX_ALGORITHMS: &[&str] = &[
    "mlkem768x25519-sha256@openssh.com",
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
];

/// Host key algorithms for server identity verification.
///
/// - `mlkem768-ed25519@openssh.com`: Hybrid post-quantum host key combining
///   ML-DSA-65 (FIPS 204 lattice signatures) with Ed25519. Both signatures
///   are verified; security holds if either algorithm remains unbroken.
/// - `ssh-ed25519`: Classical Ed25519 (RFC 8709), the modern standard.
pub const HOST_KEY_ALGORITHMS: &[&str] = &[
    "mlkem768-ed25519@openssh.com", // hybrid PQ host key
    "ssh-ed25519",
];

/// Encryption algorithms (same list for client-to-server and server-to-client).
///
/// We only support `chacha20-poly1305@openssh.com`, which is an AEAD cipher
/// that combines ChaCha20 encryption with Poly1305 authentication. This is
/// the preferred cipher in modern OpenSSH and avoids known issues with
/// AES-CBC and the need for separate MAC negotiation.
pub const ENCRYPTION_ALGORITHMS: &[&str] = &[
    "chacha20-poly1305@openssh.com",
];

/// MAC algorithms. With chacha20-poly1305, the MAC is integrated into the AEAD
/// cipher (Poly1305 provides authentication), so a separate MAC is not used.
/// We still advertise hmac-sha2-256 for protocol compliance in case a non-AEAD
/// cipher were ever negotiated.
pub const MAC_ALGORITHMS: &[&str] = &[
    "hmac-sha2-256",
];

/// Compression algorithms. We only support "none" -- no compression.
/// SSH compression (zlib) is rarely used in practice and adds attack surface
/// (e.g., CRIME-style compression oracles).
pub const COMPRESSION_ALGORITHMS: &[&str] = &["none"];

// ---------------------------------------------------------------------------
// Packet sequencing
// ---------------------------------------------------------------------------

/// Tracks sequence numbers for a single direction (send or receive).
///
/// Per RFC 4253 section 6.4, each side maintains two sequence counters:
/// one for packets sent and one for packets received. The counter starts
/// at 0 for the first packet and increments by 1 for each packet. It
/// wraps around at u32::MAX (2^32 - 1) back to 0 -- this is expected
/// behavior, not an error, though re-keying should happen well before
/// wrap-around to avoid nonce reuse in AEAD ciphers.
///
/// The sequence number serves as the nonce for ChaCha20-Poly1305 AEAD,
/// which means each packet uses a unique nonce. Nonce reuse would be
/// catastrophic for security (it would leak XOR of plaintexts).
#[derive(Debug)]
pub struct SequenceCounter {
    /// Current sequence number value, starting at 0.
    seq: u32,
}

impl SequenceCounter {
    /// Create a new sequence counter starting at 0.
    pub fn new() -> Self {
        Self { seq: 0 }
    }

    /// Get the current sequence number and advance the counter by 1.
    ///
    /// Returns the sequence number to use for the *current* packet,
    /// then increments for the next packet. Uses wrapping arithmetic
    /// per RFC 4253 section 6.4.
    pub fn next(&mut self) -> u32 {
        let current = self.seq;
        self.seq = self.seq.wrapping_add(1);
        current
    }

    /// Peek at the current sequence number without advancing.
    /// Used for SSH_MSG_UNIMPLEMENTED responses which reference the
    /// sequence number of the unrecognized packet.
    pub fn current(&self) -> u32 {
        self.seq
    }
}

// ---------------------------------------------------------------------------
// Encryption state
// ---------------------------------------------------------------------------

/// Represents the encryption state for one direction (send or receive).
///
/// The SSH transport starts in Plaintext mode. After key exchange completes
/// and both sides send SSH_MSG_NEWKEYS, the cipher transitions to an
/// authenticated encryption mode. We only support ChaCha20-Poly1305.
#[derive(Debug)]
pub enum CipherState {
    /// No encryption active. Used during the initial version exchange and
    /// key exchange phases, before SSH_MSG_NEWKEYS is processed.
    Plaintext,
    /// ChaCha20-Poly1305@openssh.com AEAD mode.
    ///
    /// The OpenSSH variant of ChaCha20-Poly1305 uses **two separate 256-bit
    /// keys** per direction:
    /// - `key`: Main ChaCha20 key used to encrypt the packet body (everything
    ///   after the 4-byte packet length). The AEAD construction also produces
    ///   a 16-byte Poly1305 authentication tag appended to the packet.
    /// - `header_key`: A separate ChaCha20 key used *only* to encrypt the
    ///   4-byte packet length field. This prevents an attacker from learning
    ///   packet sizes without decrypting, which would leak information about
    ///   the session (e.g., keystroke timing in interactive sessions).
    ///
    /// Both keys use the packet sequence number as the nonce (zero-padded
    /// to 12 bytes, big-endian).
    ChaCha20Poly1305 {
        /// 256-bit key for main packet body encryption + Poly1305 MAC.
        key: [u8; 32],
        /// 256-bit key for encrypting the 4-byte packet length field.
        header_key: [u8; 32],
    },
}

impl CipherState {
    /// Returns true if encryption is active (i.e., we are past NEWKEYS).
    pub fn is_encrypted(&self) -> bool {
        !matches!(self, Self::Plaintext)
    }
}

// ---------------------------------------------------------------------------
// KEXINIT message
// ---------------------------------------------------------------------------

/// Parsed SSH_MSG_KEXINIT message (RFC 4253 section 7.1).
///
/// Both client and server send a KEXINIT containing their supported
/// algorithms in order of preference. The server then selects the first
/// algorithm from its own list that appears in the client's list.
///
/// The raw payload bytes are preserved because they are needed verbatim
/// when computing the exchange hash H during key exchange.
#[derive(Debug, Clone)]
pub struct KexInit {
    /// 16 random bytes -- anti-replay cookie, unique per KEXINIT message.
    pub cookie: [u8; 16],
    /// Key exchange algorithms.
    pub kex_algorithms: Vec<String>,
    /// Server host key algorithms.
    pub server_host_key_algorithms: Vec<String>,
    /// Encryption algorithms client-to-server.
    pub encryption_algorithms_c2s: Vec<String>,
    /// Encryption algorithms server-to-client.
    pub encryption_algorithms_s2c: Vec<String>,
    /// MAC algorithms client-to-server.
    pub mac_algorithms_c2s: Vec<String>,
    /// MAC algorithms server-to-client.
    pub mac_algorithms_s2c: Vec<String>,
    /// Compression algorithms client-to-server.
    pub compression_algorithms_c2s: Vec<String>,
    /// Compression algorithms server-to-client.
    pub compression_algorithms_s2c: Vec<String>,
    /// First KEX packet follows.
    pub first_kex_packet_follows: bool,
    /// Reserved (always 0).
    pub reserved: u32,
    /// Raw bytes of the entire KEXINIT payload (needed for exchange hash).
    pub raw_payload: Vec<u8>,
}

impl KexInit {
    /// Build our server KEXINIT message payload.
    ///
    /// # Parameters
    /// - `cookie`: 16 cryptographically random bytes. Must be freshly generated
    ///   for each KEXINIT to prevent replay attacks.
    ///
    /// # Returns
    /// The complete KEXINIT payload (starting with message type byte 20),
    /// ready to be framed into a binary packet.
    pub fn build_server(cookie: [u8; 16]) -> Vec<u8> {
        log::debug!("transport: building server SSH_MSG_KEXINIT");

        let mut w = SshWriter::new();
        w.write_byte(SSH_MSG_KEXINIT);
        w.write_raw(&cookie);
        w.write_name_list(KEX_ALGORITHMS);
        w.write_name_list(HOST_KEY_ALGORITHMS);
        w.write_name_list(ENCRYPTION_ALGORITHMS);
        w.write_name_list(ENCRYPTION_ALGORITHMS);
        w.write_name_list(MAC_ALGORITHMS);
        w.write_name_list(MAC_ALGORITHMS);
        w.write_name_list(COMPRESSION_ALGORITHMS);
        w.write_name_list(COMPRESSION_ALGORITHMS);
        // languages client-to-server
        w.write_name_list(&[]);
        // languages server-to-client
        w.write_name_list(&[]);
        // first_kex_packet_follows
        w.write_boolean(false);
        // reserved
        w.write_uint32(0);

        let payload = w.into_bytes();
        log::trace!(
            "transport: KEXINIT payload {} bytes, kex={:?}, hostkey={:?}, enc={:?}",
            payload.len(),
            KEX_ALGORITHMS,
            HOST_KEY_ALGORITHMS,
            ENCRYPTION_ALGORITHMS,
        );
        payload
    }

    /// Parse a client KEXINIT payload.
    ///
    /// # Parameters
    /// - `payload`: Raw payload bytes starting with the SSH_MSG_KEXINIT type byte (20).
    ///
    /// # Returns
    /// A parsed `KexInit` with all algorithm lists and the raw payload preserved
    /// (needed for exchange hash computation).
    ///
    /// # Errors
    /// Returns `TransportError::MalformedKexInit` if the payload is truncated or
    /// contains invalid wire-format data.
    pub fn parse(payload: &[u8]) -> Result<Self, TransportError> {
        log::debug!("transport: parsing client SSH_MSG_KEXINIT ({} bytes)", payload.len());

        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| TransportError::MalformedKexInit)?;
        if msg_type != SSH_MSG_KEXINIT {
            log::error!("transport: expected KEXINIT (20), got {}", msg_type);
            return Err(TransportError::UnexpectedMessage(msg_type));
        }

        let mut cookie = [0u8; 16];
        let cookie_bytes = r.read_bytes(16).map_err(|_| TransportError::MalformedKexInit)?;
        cookie.copy_from_slice(cookie_bytes);

        let kex_algorithms = r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let server_host_key_algorithms =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let encryption_algorithms_c2s =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let encryption_algorithms_s2c =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let mac_algorithms_c2s =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let mac_algorithms_s2c =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let compression_algorithms_c2s =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        let compression_algorithms_s2c =
            r.read_name_list().map_err(|_| TransportError::MalformedKexInit)?;
        // languages (ignored)
        let _ = r.read_name_list();
        let _ = r.read_name_list();
        let first_kex_packet_follows = r.read_boolean().unwrap_or(false);
        let reserved = r.read_uint32().unwrap_or(0);

        log::info!(
            "transport: client KEXINIT — kex={:?}, hostkey={:?}, enc_c2s={:?}",
            kex_algorithms,
            server_host_key_algorithms,
            encryption_algorithms_c2s,
        );

        Ok(Self {
            cookie,
            kex_algorithms,
            server_host_key_algorithms,
            encryption_algorithms_c2s,
            encryption_algorithms_s2c,
            mac_algorithms_c2s,
            mac_algorithms_s2c,
            compression_algorithms_c2s,
            compression_algorithms_s2c,
            first_kex_packet_follows,
            reserved,
            raw_payload: Vec::from(payload),
        })
    }
}

// ---------------------------------------------------------------------------
// Binary packet framing
// ---------------------------------------------------------------------------

/// Frame a payload into an SSH binary packet (unencrypted).
///
/// # SSH Binary Packet Layout (RFC 4253 section 6)
///
/// ```text
/// packet_length(4)     -- uint32, length of (padding_length + payload + padding)
/// padding_length(1)    -- uint8, length of random padding
/// payload(N)           -- the actual SSH message
/// random_padding(P)    -- P random bytes (4 <= P <= 255)
/// ```
///
/// The total size of `padding_length + payload + random_padding` must be a
/// multiple of the cipher block size (8 bytes for unencrypted packets).
/// Random padding frustrates traffic analysis by obscuring exact payload sizes.
///
/// # Parameters
/// - `payload`: The SSH message payload to frame.
/// - `rng_fill`: Callback to fill the padding with cryptographically random bytes.
///
/// # Returns
/// The complete framed packet including the 4-byte packet_length header.
pub fn frame_packet(payload: &[u8], rng_fill: &dyn Fn(&mut [u8])) -> Vec<u8> {
    let block_size = UNENCRYPTED_BLOCK_SIZE; // 8 bytes for unencrypted packets
    let padding_len = compute_padding(payload.len(), block_size);
    // packet_length covers: padding_length(1 byte) + payload + padding
    let packet_length = 1 + payload.len() + padding_len;

    log::trace!(
        "transport: framing packet — payload={}, padding={}, total={}",
        payload.len(),
        padding_len,
        4 + packet_length,
    );

    let mut pkt = Vec::with_capacity(4 + packet_length);
    pkt.extend_from_slice(&(packet_length as u32).to_be_bytes());
    pkt.push(padding_len as u8);
    pkt.extend_from_slice(payload);

    let mut padding = vec![0u8; padding_len];
    rng_fill(&mut padding);
    pkt.extend_from_slice(&padding);

    pkt
}

/// Frame a payload into an encrypted SSH binary packet using ChaCha20-Poly1305.
///
/// # ChaCha20-Poly1305@openssh.com Encryption (OpenSSH variant)
///
/// Unlike standard AEAD ciphers, the OpenSSH variant uses **two separate
/// ChaCha20 instances** per packet:
///
/// 1. **Header encryption** (`header_key`): The 4-byte packet_length field
///    is encrypted using ChaCha20 with the header key and nonce = sequence
///    number. This hides packet sizes from passive observers.
///
/// 2. **Body encryption** (`main_key`): The remaining packet body
///    (padding_length + payload + padding) is encrypted with ChaCha20 and
///    authenticated with Poly1305. The AEAD produces a 16-byte MAC tag
///    that is appended to the packet.
///
/// Both use the same 12-byte nonce: the 32-bit sequence number placed in
/// the last 4 bytes (big-endian), with the first 8 bytes zero-padded.
///
/// # Parameters
/// - `payload`: The unencrypted SSH message payload.
/// - `seq`: The current send sequence number (used as AEAD nonce).
/// - `cipher`: The current cipher state (Plaintext or ChaCha20Poly1305).
/// - `rng_fill`: Callback for generating random padding bytes.
///
/// # Returns
/// The encrypted packet: `encrypted_length(4) || encrypted_body || mac_tag(16)`.
///
/// # Errors
/// Returns `TransportError::MacVerifyFailed` if encryption fails (should not
/// happen in normal operation).
pub fn frame_packet_encrypted(
    payload: &[u8],
    seq: u32,
    cipher: &CipherState,
    rng_fill: &dyn Fn(&mut [u8]),
) -> Result<Vec<u8>, TransportError> {
    match cipher {
        CipherState::Plaintext => {
            // No encryption — just frame normally
            Ok(frame_packet(payload, rng_fill))
        }
        CipherState::ChaCha20Poly1305 { key, header_key } => {
            // ChaCha20-Poly1305@openssh.com packet encryption (OpenSSH variant)
            //
            // This uses two independent ChaCha20 instances per packet:
            //   1. header_key encrypts ONLY the 4-byte packet_length (nonce = seq, counter=0)
            //   2. main key encrypts the body + provides Poly1305 MAC (nonce = seq)
            //
            // WHY two keys? The packet length must be decrypted *before* the body
            // can be read, because we need to know how many bytes to read from the
            // network. A separate key prevents length-decryption from leaking any
            // information about the body encryption key.

            // Step 1: Build the plaintext packet (framed with padding)
            let unenc = frame_packet(payload, rng_fill);
            let packet_length_bytes = &unenc[..4];     // 4-byte big-endian packet length
            let packet_body = &unenc[4..];              // padding_length + payload + padding

            // Step 2: Build the 12-byte nonce from sequence number.
            // ChaCha20 requires a 96-bit (12-byte) nonce. We place the 32-bit
            // sequence number in the last 4 bytes (big-endian), with the first
            // 8 bytes zeroed. This matches the OpenSSH ChaCha20-Poly1305 spec.
            let mut nonce_bytes = [0u8; 12];
            nonce_bytes[8..12].copy_from_slice(&seq.to_be_bytes());
            let nonce = GenericArray::from(nonce_bytes);

            // Step 3: Encrypt the 4-byte packet_length with the header_key.
            //
            // The OpenSSH spec calls for raw ChaCha20 keystream XOR (not AEAD).
            // Since the chacha20poly1305 crate only exposes AEAD, we extract
            // the keystream by encrypting 4 zero bytes -- the ciphertext of
            // zeros IS the keystream. We then XOR this keystream with the
            // actual packet length bytes.
            let header_cipher = ChaCha20Poly1305::new(GenericArray::from_slice(header_key));
            let mut encrypted_length = [0u8; 4];
            encrypted_length.copy_from_slice(packet_length_bytes);
            // Encrypt zeros to extract ChaCha20 keystream bytes
            let length_pad = [0u8; 4];
            if let Ok(ct) = header_cipher.encrypt(&nonce, length_pad.as_ref()) {
                // XOR the keystream (first 4 bytes of ciphertext) with the length
                for i in 0..4 {
                    encrypted_length[i] ^= ct[i];
                }
            }

            // Step 4: Encrypt packet body with the main key using full AEAD.
            // The ChaCha20 stream cipher encrypts the body for confidentiality,
            // and Poly1305 computes a 16-byte authentication tag over the
            // ciphertext. The tag is automatically appended by the AEAD.
            let main_cipher = ChaCha20Poly1305::new(GenericArray::from_slice(key));
            let ciphertext = main_cipher.encrypt(&nonce, packet_body)
                .map_err(|_| {
                    log::error!("transport: ChaCha20-Poly1305 encryption failed");
                    TransportError::MacVerifyFailed
                })?;

            // Step 5: Assemble the final encrypted packet.
            // Layout: encrypted_length(4) || encrypted_body_with_tag
            // The AEAD ciphertext already has the 16-byte Poly1305 tag appended.
            let mut out = Vec::with_capacity(4 + ciphertext.len());
            out.extend_from_slice(&encrypted_length);
            out.extend_from_slice(&ciphertext);

            log::trace!(
                "transport: encrypted packet — {} bytes (4 + {} body+tag)",
                out.len(),
                ciphertext.len(),
            );

            Ok(out)
        }
    }
}

/// Parse a received SSH binary packet (unencrypted).
///
/// # Expected Layout
/// ```text
/// packet_length(4)     -- uint32 big-endian, size of remaining fields
/// padding_length(1)    -- uint8
/// payload(N)           -- the SSH message
/// padding(P)           -- random padding bytes (discarded)
/// ```
///
/// # Returns
/// A tuple of `(payload_bytes, total_bytes_consumed)` so the caller knows
/// how much of the input buffer was consumed (there may be additional
/// packets in the same TCP segment).
///
/// # Errors
/// - `PacketTooShort`: Not enough bytes for a complete packet.
/// - `PacketTooLarge`: Packet exceeds 35,000 bytes (DoS protection).
/// - `InvalidPadding`: Padding length is out of valid range.
pub fn parse_packet(data: &[u8]) -> Result<(Vec<u8>, usize), TransportError> {
    if data.len() < 5 {
        return Err(TransportError::PacketTooShort);
    }

    let packet_length = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    log::trace!("transport: packet_length = {}", packet_length);

    if packet_length > MAX_PACKET_SIZE {
        log::error!("transport: packet too large: {} bytes", packet_length);
        return Err(TransportError::PacketTooLarge(packet_length));
    }

    let total_len = 4 + packet_length;
    if data.len() < total_len {
        return Err(TransportError::PacketTooShort);
    }

    let padding_length = data[4] as usize;
    if padding_length < MIN_PADDING || padding_length >= packet_length {
        log::error!(
            "transport: invalid padding length: {} (packet_length={})",
            padding_length,
            packet_length,
        );
        return Err(TransportError::InvalidPadding);
    }

    let payload_length = packet_length - 1 - padding_length;
    let payload = Vec::from(&data[5..5 + payload_length]);

    log::trace!(
        "transport: parsed packet — payload={} bytes, padding={}, consumed={}",
        payload_length,
        padding_length,
        total_len,
    );

    Ok((payload, total_len))
}

/// Parse and decrypt an encrypted SSH packet.
///
/// This reverses the encryption process from `frame_packet_encrypted`:
/// 1. Decrypt the 4-byte packet length using the header key
/// 2. Decrypt and authenticate the body using the main key (AEAD)
/// 3. Extract the payload from the decrypted body
///
/// # Parameters
/// - `data`: Raw bytes from the network (may contain trailing data).
/// - `seq`: The current receive sequence number (used as AEAD nonce).
/// - `cipher`: The current cipher state.
///
/// # Returns
/// A tuple of `(decrypted_payload, total_bytes_consumed)`.
///
/// # Errors
/// - `MacVerifyFailed`: The Poly1305 authentication tag did not verify.
///   This means the packet was tampered with or the wrong key is being used.
///   The connection MUST be terminated immediately.
pub fn parse_packet_encrypted(
    data: &[u8],
    seq: u32,
    cipher: &CipherState,
) -> Result<(Vec<u8>, usize), TransportError> {
    match cipher {
        CipherState::Plaintext => parse_packet(data),
        CipherState::ChaCha20Poly1305 { key, header_key } => {
            // ChaCha20-Poly1305@openssh.com packet decryption (OpenSSH variant)
            //
            // Input layout: encrypted_length(4) || encrypted_body(N) || mac_tag(16)
            // Total bytes needed: 4 + packet_length + 16

            if data.len() < 4 {
                return Err(TransportError::PacketTooShort);
            }

            // Build the 12-byte nonce from the receive sequence number.
            // Must match the nonce used by the sender for this packet.
            let mut nonce_bytes = [0u8; 12];
            nonce_bytes[8..12].copy_from_slice(&seq.to_be_bytes());
            let nonce = GenericArray::from(nonce_bytes);

            // Step 1: Decrypt the 4-byte packet_length with the header_key.
            // We use the same keystream-extraction trick as encryption:
            // encrypt 4 zero bytes to get the keystream, then XOR with the
            // encrypted length bytes. XOR is its own inverse, so this reverses
            // the encryption.
            let header_cipher = ChaCha20Poly1305::new(GenericArray::from_slice(header_key));
            let mut decrypted_length = [0u8; 4];
            decrypted_length.copy_from_slice(&data[..4]);
            let length_pad = [0u8; 4];
            if let Ok(ct) = header_cipher.encrypt(&nonce, length_pad.as_ref()) {
                for i in 0..4 {
                    decrypted_length[i] ^= ct[i]; // XOR to recover plaintext length
                }
            }

            let packet_length = u32::from_be_bytes(decrypted_length) as usize;
            log::trace!("transport: decrypted packet_length = {}", packet_length);

            if packet_length > MAX_PACKET_SIZE {
                log::error!("transport: encrypted packet too large: {} bytes", packet_length);
                return Err(TransportError::PacketTooLarge(packet_length));
            }

            // Total wire bytes: 4 (encrypted length) + packet_length (encrypted body) + 16 (Poly1305 MAC tag)
            let total_len = 4 + packet_length + 16;
            if data.len() < total_len {
                return Err(TransportError::PacketTooShort);
            }

            // Step 2: Decrypt and authenticate the body with the main key.
            // The AEAD decrypt operation:
            //   (a) Verifies the 16-byte Poly1305 MAC tag -- if this fails,
            //       the packet was tampered with and we MUST abort.
            //   (b) Decrypts the body using ChaCha20.
            // These happen atomically: if the MAC fails, no plaintext is returned.
            let main_cipher = ChaCha20Poly1305::new(GenericArray::from_slice(key));
            let encrypted_body_with_tag = &data[4..4 + packet_length + 16];
            let decrypted_body = main_cipher.decrypt(&nonce, encrypted_body_with_tag)
                .map_err(|_| {
                    log::error!("transport: ChaCha20-Poly1305 MAC verification failed");
                    TransportError::MacVerifyFailed
                })?;

            // Step 3: Parse the decrypted body (padding_length + payload + padding)
            if decrypted_body.is_empty() {
                return Err(TransportError::PacketTooShort);
            }

            let padding_length = decrypted_body[0] as usize;
            if padding_length < MIN_PADDING || padding_length >= packet_length {
                log::error!(
                    "transport: invalid padding in encrypted packet: {} (packet_length={})",
                    padding_length,
                    packet_length,
                );
                return Err(TransportError::InvalidPadding);
            }

            let payload_length = packet_length - 1 - padding_length;
            let payload = Vec::from(&decrypted_body[1..1 + payload_length]);

            log::trace!(
                "transport: decrypted packet — payload={} bytes, padding={}, consumed={}",
                payload_length,
                padding_length,
                total_len,
            );

            Ok((payload, total_len))
        }
    }
}

// ---------------------------------------------------------------------------
// Version string exchange
// ---------------------------------------------------------------------------

/// Build our SSH version string with CR LF terminator for transmission.
/// Per RFC 4253 section 4.2, the version string MUST end with `\r\n`.
pub fn version_string() -> Vec<u8> {
    let mut v = Vec::from(SSH_VERSION_STRING.as_bytes());
    v.push(b'\r');
    v.push(b'\n');
    log::info!("transport: sending version string: {}", SSH_VERSION_STRING);
    v
}

/// Parse a received version string from the peer.
///
/// Per RFC 4253 section 4.2, the server/client may send banner lines before
/// the version string. Any line not starting with `SSH-` is treated as a
/// banner line and silently ignored. The first line starting with `SSH-` is
/// the version string.
///
/// # Parameters
/// - `data`: Raw bytes received from the peer.
///
/// # Returns
/// The version string without the trailing CR LF (e.g., `"SSH-2.0-OpenSSH_9.5"`).
///
/// # Errors
/// - `InvalidVersionString`: No line starting with `SSH-` was found.
/// - `UnsupportedVersion`: Version string found but does not start with `SSH-2.0-`.
pub fn parse_version_string(data: &[u8]) -> Result<String, TransportError> {
    // Find lines separated by \r\n or \n
    let mut start = 0;
    while start < data.len() {
        let end = data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p)
            .unwrap_or(data.len());

        let line_end = if end > 0 && data[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };

        let line = &data[start..line_end];

        if line.starts_with(b"SSH-") {
            let version = core::str::from_utf8(line)
                .map_err(|_| TransportError::InvalidVersionString)?;

            if !version.starts_with(SSH_VERSION_PREFIX) {
                log::error!(
                    "transport: unsupported SSH version: {}",
                    version,
                );
                return Err(TransportError::UnsupportedVersion);
            }

            log::info!("transport: peer version string: {}", version);
            return Ok(String::from(version));
        }

        // Skip banner lines
        log::trace!(
            "transport: skipping banner line ({} bytes)",
            line_end - start,
        );
        start = end + 1;
    }

    Err(TransportError::InvalidVersionString)
}

/// Build an SSH_MSG_DISCONNECT packet (message type 1).
///
/// # Parameters
/// - `reason_code`: One of the SSH_DISCONNECT_* constants (RFC 4253 section 11.1).
/// - `description`: Human-readable reason for the disconnect.
///
/// # Returns
/// Payload bytes for the disconnect message, ready to be framed.
pub fn build_disconnect(reason_code: u32, description: &str) -> Vec<u8> {
    log::info!(
        "transport: building DISCONNECT — reason={}, desc={}",
        reason_code,
        description,
    );
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_DISCONNECT);
    w.write_uint32(reason_code);
    w.write_string_utf8(description);
    w.write_string_utf8(""); // language tag
    w.into_bytes()
}

/// Build an SSH_MSG_NEWKEYS packet (message type 21).
///
/// This single-byte message signals that the sender will use the newly
/// derived keys for all subsequent packets. Both sides must send NEWKEYS
/// after key exchange; the transition to encrypted mode happens immediately.
pub fn build_newkeys() -> Vec<u8> {
    log::debug!("transport: building SSH_MSG_NEWKEYS");
    vec![SSH_MSG_NEWKEYS]
}

/// Build an SSH_MSG_SERVICE_ACCEPT packet (message type 6).
///
/// Sent in response to a client's SSH_MSG_SERVICE_REQUEST to confirm that
/// the requested service (typically "ssh-userauth") is available.
///
/// # Parameters
/// - `service_name`: The accepted service name (e.g., "ssh-userauth").
pub fn build_service_accept(service_name: &str) -> Vec<u8> {
    log::debug!("transport: building SERVICE_ACCEPT for '{}'", service_name);
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_SERVICE_ACCEPT);
    w.write_string_utf8(service_name);
    w.into_bytes()
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur at the SSH transport layer.
///
/// These errors generally indicate either a protocol violation by the peer,
/// a corrupted/tampered packet, or a network issue. Most of these should
/// result in connection termination.
#[derive(Debug, Clone)]
pub enum TransportError {
    /// Peer version string is not valid UTF-8 or is otherwise unparseable.
    InvalidVersionString,
    /// Peer uses an SSH version other than 2.0 (the only version we support).
    UnsupportedVersion,
    /// Not enough bytes received to parse a complete packet. The caller should
    /// buffer more data from the network and retry.
    PacketTooShort,
    /// Packet claims a size exceeding MAX_PACKET_SIZE (35,000 bytes).
    /// This is likely a DoS attempt or corrupted data.
    PacketTooLarge(usize),
    /// Padding length is outside the valid range (4..=255) or exceeds packet size.
    InvalidPadding,
    /// Received a message type that is not expected in the current protocol state.
    UnexpectedMessage(u8),
    /// SSH_MSG_KEXINIT message could not be parsed (truncated or invalid fields).
    MalformedKexInit,
    /// ChaCha20-Poly1305 MAC verification failed. This means the packet was
    /// tampered with, corrupted in transit, or encrypted with the wrong key.
    /// The connection MUST be terminated immediately per RFC 4253 section 6.3.
    MacVerifyFailed,
    /// Sequence number has wrapped around without re-keying. Extremely unlikely
    /// (would require sending 2^32 packets without re-keying) but tracked for safety.
    SequenceOverflow,
    /// Lower-level wire format error during serialization/deserialization.
    Wire(WireError),
}

impl From<WireError> for TransportError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidVersionString => write!(f, "invalid SSH version string"),
            Self::UnsupportedVersion => write!(f, "unsupported SSH protocol version"),
            Self::PacketTooShort => write!(f, "SSH packet too short"),
            Self::PacketTooLarge(n) => write!(f, "SSH packet too large: {} bytes", n),
            Self::InvalidPadding => write!(f, "invalid SSH packet padding"),
            Self::UnexpectedMessage(t) => write!(f, "unexpected SSH message type: {}", t),
            Self::MalformedKexInit => write!(f, "malformed KEXINIT message"),
            Self::MacVerifyFailed => write!(f, "MAC verification failed"),
            Self::SequenceOverflow => write!(f, "sequence number overflow"),
            Self::Wire(e) => write!(f, "wire format error: {}", e),
        }
    }
}
