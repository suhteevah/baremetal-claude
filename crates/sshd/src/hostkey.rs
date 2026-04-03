//! SSH Host Key Management.
//!
//! Manages the server's long-term identity keys used to prove the server's
//! identity to clients during key exchange. The host key is the SSH equivalent
//! of a TLS certificate -- it lets clients verify they are talking to the
//! expected server and not a man-in-the-middle.
//!
//! We support three host key types:
//! - **Ed25519** (classical, RFC 8709): A 32-byte public key with 64-byte
//!   signatures. Fast, small, and the modern default for SSH.
//! - **ML-DSA-65** (post-quantum, FIPS 204 / CRYSTALS-Dilithium): Lattice-based
//!   signatures believed to be quantum-resistant. Larger keys (1952 bytes) and
//!   signatures (3309 bytes), but provides NIST security level 3.
//! - **Hybrid**: ML-DSA-65 + Ed25519 dual signature. Both algorithms sign the
//!   same data independently. Security holds if either algorithm remains unbroken.
//!
//! Provides SSH wire-format serialization for public keys and signatures,
//! and signing of the exchange hash during key exchange.

use alloc::vec::Vec;

use ed25519_dalek::{SigningKey as Ed25519SigningKey, VerifyingKey as Ed25519VerifyingKey};
use ed25519_dalek::{Signer as Ed25519Signer, Verifier as Ed25519Verifier};

use ml_dsa::{MlDsa65, VerifyingKey as MlDsaVerifyingKey};
use ml_dsa::{EncodedSignature as MlDsaEncodedSignature, EncodedVerifyingKey as MlDsaEncodedVerifyingKey};
use ml_dsa::signature::Signer as _;
use ml_dsa::signature::Verifier as _;
use ml_dsa::KeyGen;
use crate::wire::SshWriter;

// ---------------------------------------------------------------------------
// Host key type identifiers (SSH name strings)
// ---------------------------------------------------------------------------

/// SSH algorithm name string for Ed25519 host keys (RFC 8709).
/// This exact string is used in KEXINIT negotiation and in the wire-format
/// encoding of public key blobs and signature blobs.
pub const SSH_ED25519: &str = "ssh-ed25519";

/// SSH algorithm name string for hybrid ML-DSA-65 + Ed25519 host keys.
/// Uses the OpenSSH naming convention with `@openssh.com` suffix for
/// vendor-specific extensions. Note: the "mlkem768" prefix is a naming
/// artifact from the KEX algorithm -- the host key uses ML-DSA, not ML-KEM.
pub const SSH_MLDSA65_ED25519: &str = "mlkem768-ed25519@openssh.com";

// ---------------------------------------------------------------------------
// Ed25519 host key
// ---------------------------------------------------------------------------

/// An Ed25519 host keypair (RFC 8032).
///
/// Ed25519 uses a 32-byte seed (the "secret key") from which the actual
/// 64-byte expanded secret key and the 32-byte public key are derived
/// deterministically. We store only the 32-byte seed for compactness.
///
/// Ed25519 signatures are 64 bytes and are deterministic (no randomness
/// needed at signing time), which eliminates a class of implementation
/// bugs related to nonce generation.
pub struct Ed25519HostKey {
    /// Ed25519 secret key seed (32 bytes). The actual signing key is derived
    /// from this seed via SHA-512 hashing (per RFC 8032).
    secret: [u8; 32],
    /// Ed25519 public key (32 bytes). A compressed Edwards curve point.
    public: [u8; 32],
}

impl Ed25519HostKey {
    /// Generate a new Ed25519 host keypair from random bytes.
    ///
    /// # Parameters
    /// - `rng`: A cryptographically secure random byte source. Must provide
    ///   32 bytes of entropy for the secret key seed.
    pub fn generate(rng: &mut dyn FnMut(&mut [u8])) -> Self {
        log::info!("hostkey: generating Ed25519 host keypair");

        let mut secret = [0u8; 32];
        rng(&mut secret);

        // Real Ed25519 key derivation via ed25519-dalek
        let signing_key = Ed25519SigningKey::from_bytes(&secret);
        let public = signing_key.verifying_key().to_bytes();

        log::debug!(
            "hostkey: Ed25519 public key = {:02x}{:02x}{:02x}{:02x}...",
            public[0], public[1], public[2], public[3],
        );

        Self { secret, public }
    }

    /// Serialize the public key in SSH wire format.
    ///
    /// ```text
    /// string    "ssh-ed25519"
    /// string    public_key (32 bytes)
    /// ```
    pub fn public_key_blob(&self) -> Vec<u8> {
        let mut w = SshWriter::new();
        w.write_string_utf8(SSH_ED25519);
        w.write_string(&self.public);
        w.into_bytes()
    }

    /// Sign data and return the signature in SSH wire format.
    ///
    /// ```text
    /// string    "ssh-ed25519"
    /// string    signature (64 bytes)
    /// ```
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        log::debug!("hostkey: signing {} bytes with Ed25519", data.len());

        // Real Ed25519 signing via ed25519-dalek
        let signing_key = Ed25519SigningKey::from_bytes(&self.secret);
        let sig = signing_key.sign(data);
        let sig_bytes = sig.to_bytes();

        let mut w = SshWriter::new();
        w.write_string_utf8(SSH_ED25519);
        w.write_string(&sig_bytes);
        w.into_bytes()
    }

    /// Verify an Ed25519 signature over data using a raw 32-byte public key.
    ///
    /// # Parameters
    /// - `public_key`: The 32-byte Ed25519 public key (compressed Edwards point).
    /// - `data`: The data that was signed.
    /// - `signature`: The 64-byte Ed25519 signature to verify.
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise. Returns `false`
    /// (rather than panicking) for malformed keys or signatures.
    pub fn verify(public_key: &[u8; 32], data: &[u8], signature: &[u8]) -> bool {
        log::debug!(
            "hostkey: verifying Ed25519 signature — data={} bytes, sig={} bytes",
            data.len(),
            signature.len(),
        );

        // Real Ed25519 verification via ed25519-dalek
        let vk = match Ed25519VerifyingKey::from_bytes(public_key) {
            Ok(vk) => vk,
            Err(e) => {
                log::error!("hostkey: invalid Ed25519 public key: {}", e);
                return false;
            }
        };

        if signature.len() != 64 {
            log::error!("hostkey: Ed25519 signature wrong length: {} (expected 64)", signature.len());
            return false;
        }

        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(signature);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);

        match vk.verify(data, &sig) {
            Ok(()) => {
                log::debug!("hostkey: Ed25519 signature verified successfully");
                true
            }
            Err(e) => {
                log::debug!("hostkey: Ed25519 signature verification failed: {}", e);
                false
            }
        }
    }

    /// Get the raw 32-byte public key.
    pub fn public_key_bytes(&self) -> &[u8; 32] {
        &self.public
    }

    /// Serialize the full keypair for persistence as `secret(32) || public(32)`.
    /// The secret key seed must be stored securely (encrypted on disk).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.secret);
        out.extend_from_slice(&self.public);
        out
    }

    /// Deserialize a keypair from persistence, validating that the stored
    /// public key matches what the secret key derives. This catches corruption.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 64 {
            log::error!("hostkey: Ed25519 key data too short: {} bytes", data.len());
            return None;
        }
        let mut secret = [0u8; 32];
        let mut public = [0u8; 32];
        secret.copy_from_slice(&data[..32]);
        public.copy_from_slice(&data[32..64]);

        // Validate that the public key matches the secret
        let signing_key = Ed25519SigningKey::from_bytes(&secret);
        let derived_public = signing_key.verifying_key().to_bytes();
        if derived_public != public {
            log::error!("hostkey: Ed25519 key mismatch — stored public key doesn't match derived");
            return None;
        }

        log::debug!("hostkey: Ed25519 keypair loaded from persistence");
        Some(Self { secret, public })
    }
}

// ---------------------------------------------------------------------------
// ML-DSA-65 host key (post-quantum)
// ---------------------------------------------------------------------------

/// An ML-DSA-65 host keypair (FIPS 204 / CRYSTALS-Dilithium).
///
/// ML-DSA-65 is a post-quantum digital signature scheme based on the
/// hardness of Module-LWE (Module Learning With Errors) over lattices.
/// It is the NIST-standardized successor to CRYSTALS-Dilithium.
///
/// Key sizes are much larger than Ed25519:
/// - Public key: 1,952 bytes (vs 32 for Ed25519)
/// - Signature: 3,309 bytes (vs 64 for Ed25519)
/// - Secret key: 4,032 bytes (but we only store a 32-byte seed)
///
/// We store only the 32-byte seed from which the full signing key is derived
/// deterministically. This keeps persistence compact while the expanded
/// 4,032-byte signing key is reconstructed in memory when needed.
pub struct MlDsa65HostKey {
    /// ML-DSA-65 seed (32 bytes). The full 4,032-byte signing key is derived
    /// from this seed deterministically via the ML-DSA key generation function.
    seed: [u8; 32],
    /// ML-DSA-65 encoded verifying (public) key (1,952 bytes).
    public: Vec<u8>,
}

/// ML-DSA-65 public (verifying) key size in bytes per FIPS 204.
pub const MLDSA65_PK_SIZE: usize = 1952;

/// ML-DSA-65 secret (signing) key size in bytes per FIPS 204.
/// We do not store this directly -- it is derived from the 32-byte seed.
pub const MLDSA65_SK_SIZE: usize = 4032;

/// ML-DSA-65 signature size in bytes per FIPS 204.
pub const MLDSA65_SIG_SIZE: usize = 3309;

impl MlDsa65HostKey {
    /// Generate a new ML-DSA-65 host keypair.
    pub fn generate(rng: &mut dyn FnMut(&mut [u8])) -> Self {
        log::info!("hostkey: generating ML-DSA-65 host keypair");

        // Generate a random 32-byte seed
        let mut seed = [0u8; 32];
        rng(&mut seed);

        // Derive the keypair deterministically from the seed via ml-dsa
        let seed_array = ml_dsa::B32::from(seed);
        let kp = MlDsa65::from_seed(&seed_array);

        // Encode the verifying (public) key
        use ml_dsa::signature::Keypair;
        let vk = kp.verifying_key();
        let public = Vec::from(vk.encode().as_slice());

        log::debug!(
            "hostkey: ML-DSA-65 keypair generated — pk={} bytes, seed=32 bytes",
            public.len(),
        );

        Self { seed, public }
    }

    /// Reconstruct the full expanded signing key from the compact 32-byte seed.
    /// This performs the deterministic ML-DSA key generation internally, which
    /// expands the seed into the 4,032-byte signing key.
    fn signing_key(&self) -> ml_dsa::SigningKey<MlDsa65> {
        let seed_array = ml_dsa::B32::from(self.seed);
        MlDsa65::from_seed(&seed_array)
    }

    /// Serialize the public key in SSH wire format.
    ///
    /// ```text
    /// string    "ml-dsa-65"
    /// string    public_key (1952 bytes)
    /// ```
    pub fn public_key_blob(&self) -> Vec<u8> {
        let mut w = SshWriter::new();
        w.write_string_utf8("ml-dsa-65");
        w.write_string(&self.public);
        w.into_bytes()
    }

    /// Sign data and return the signature in SSH wire format.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        log::debug!("hostkey: signing {} bytes with ML-DSA-65", data.len());

        // Real ML-DSA-65 signing via ml-dsa crate (deterministic mode)
        let sk = self.signing_key();
        let sig: ml_dsa::Signature<MlDsa65> = sk.sign(data);
        let sig_bytes = sig.encode();

        let mut w = SshWriter::new();
        w.write_string_utf8("ml-dsa-65");
        w.write_string(sig_bytes.as_slice());
        w.into_bytes()
    }

    /// Verify an ML-DSA-65 signature.
    pub fn verify(public_key: &[u8], data: &[u8], signature: &[u8]) -> bool {
        log::debug!(
            "hostkey: verifying ML-DSA-65 signature — pk={} bytes, data={} bytes, sig={} bytes",
            public_key.len(),
            data.len(),
            signature.len(),
        );

        // Decode the verifying key
        if public_key.len() != MLDSA65_PK_SIZE {
            log::error!("hostkey: ML-DSA-65 public key wrong size: {} (expected {})", public_key.len(), MLDSA65_PK_SIZE);
            return false;
        }
        let vk_enc = match MlDsaEncodedVerifyingKey::<MlDsa65>::try_from(public_key) {
            Ok(enc) => enc,
            Err(_) => {
                log::error!("hostkey: ML-DSA-65 public key encoding error");
                return false;
            }
        };
        let vk = MlDsaVerifyingKey::<MlDsa65>::decode(&vk_enc);

        // Decode the signature
        if signature.len() != MLDSA65_SIG_SIZE {
            log::error!("hostkey: ML-DSA-65 signature wrong size: {} (expected {})", signature.len(), MLDSA65_SIG_SIZE);
            return false;
        }
        let sig_enc = match MlDsaEncodedSignature::<MlDsa65>::try_from(signature) {
            Ok(enc) => enc,
            Err(_) => {
                log::error!("hostkey: ML-DSA-65 signature encoding error");
                return false;
            }
        };
        let sig = match ml_dsa::Signature::<MlDsa65>::decode(&sig_enc) {
            Some(s) => s,
            None => {
                log::error!("hostkey: ML-DSA-65 signature decode failed");
                return false;
            }
        };

        // Verify using the real ml-dsa verifier
        match vk.verify(data, &sig) {
            Ok(()) => {
                log::debug!("hostkey: ML-DSA-65 signature verified successfully");
                true
            }
            Err(e) => {
                log::debug!("hostkey: ML-DSA-65 signature verification failed: {}", e);
                false
            }
        }
    }

    /// Get the raw public key bytes.
    pub fn public_key_bytes(&self) -> &[u8] {
        &self.public
    }

    /// Serialize for persistence (seed + public key).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = SshWriter::new();
        w.write_string(&self.seed);
        w.write_string(&self.public);
        w.into_bytes()
    }

    /// Deserialize from persistence.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = crate::wire::SshReader::new(data);
        let seed_bytes = r.read_string_raw().ok()?;
        let public = r.read_string_raw().ok()?.to_vec();

        if seed_bytes.len() != 32 {
            log::error!(
                "hostkey: ML-DSA-65 seed wrong size — got {}, expected 32",
                seed_bytes.len(),
            );
            return None;
        }
        if public.len() != MLDSA65_PK_SIZE {
            log::error!(
                "hostkey: ML-DSA-65 public key wrong size — got {}, expected {}",
                public.len(),
                MLDSA65_PK_SIZE,
            );
            return None;
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(seed_bytes);

        log::debug!("hostkey: ML-DSA-65 keypair loaded from persistence");
        Some(Self { seed, public })
    }
}

// ---------------------------------------------------------------------------
// Hybrid host key: ML-DSA-65 + Ed25519
// ---------------------------------------------------------------------------

/// A hybrid host key combining ML-DSA-65 and Ed25519.
///
/// This is the "belt and suspenders" approach to post-quantum security:
/// both keys are presented together in the public key blob, and both
/// algorithms sign the exchange hash independently during key exchange.
/// A client MUST verify both signatures. Security holds as long as at
/// least one of the two signature schemes remains unbroken.
pub struct HybridHostKey {
    /// The Ed25519 component.
    pub ed25519: Ed25519HostKey,
    /// The ML-DSA-65 component.
    pub ml_dsa: MlDsa65HostKey,
}

impl HybridHostKey {
    /// Generate a new hybrid host keypair.
    pub fn generate(rng: &mut dyn FnMut(&mut [u8])) -> Self {
        log::info!("hostkey: generating hybrid ML-DSA-65 + Ed25519 host keypair");
        Self {
            ed25519: Ed25519HostKey::generate(rng),
            ml_dsa: MlDsa65HostKey::generate(rng),
        }
    }

    /// Serialize the hybrid public key in SSH wire format.
    ///
    /// ```text
    /// string    "mlkem768-ed25519@openssh.com"
    /// string    ed25519_public_key (32 bytes)
    /// string    ml_dsa_65_public_key (1952 bytes)
    /// ```
    pub fn public_key_blob(&self) -> Vec<u8> {
        let mut w = SshWriter::new();
        w.write_string_utf8(SSH_MLDSA65_ED25519);
        w.write_string(self.ed25519.public_key_bytes());
        w.write_string(self.ml_dsa.public_key_bytes());
        w.into_bytes()
    }

    /// Dual-sign: produce both Ed25519 and ML-DSA-65 signatures over data.
    ///
    /// ```text
    /// string    "mlkem768-ed25519@openssh.com"
    /// string    ed25519_signature (64 bytes)
    /// string    ml_dsa_65_signature (3309 bytes)
    /// ```
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        log::info!("hostkey: dual-signing {} bytes (Ed25519 + ML-DSA-65)", data.len());

        // Get raw signatures (without their own type prefixes)
        // For the hybrid format, we embed both raw signatures.
        let ed25519_sig = self.ed25519.sign(data);
        let ml_dsa_sig = self.ml_dsa.sign(data);

        let mut w = SshWriter::new();
        w.write_string_utf8(SSH_MLDSA65_ED25519);
        w.write_string(&ed25519_sig);
        w.write_string(&ml_dsa_sig);
        w.into_bytes()
    }

    /// Serialize for persistence.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = SshWriter::new();
        let ed_bytes = self.ed25519.to_bytes();
        let ml_bytes = self.ml_dsa.to_bytes();
        w.write_string(&ed_bytes);
        w.write_string(&ml_bytes);
        w.into_bytes()
    }

    /// Deserialize from persistence.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = crate::wire::SshReader::new(data);
        let ed_bytes = r.read_string_raw().ok()?;
        let ml_bytes = r.read_string_raw().ok()?;
        let ed25519 = Ed25519HostKey::from_bytes(ed_bytes)?;
        let ml_dsa = MlDsa65HostKey::from_bytes(ml_bytes)?;
        log::debug!("hostkey: hybrid keypair loaded from persistence");
        Some(Self { ed25519, ml_dsa })
    }
}
