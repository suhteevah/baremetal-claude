//! ext4 encryption awareness.
//!
//! ext4 supports per-file encryption (fscrypt). Encrypted inodes have the
//! `EXT4_ENCRYPT_FL` (0x800) flag set. The encryption context is stored
//! in an extended attribute named "encryption.c" (or "c" in the "encryption"
//! namespace).
//!
//! This module detects encrypted inodes and returns clear errors instead of
//! serving garbage ciphertext. Full decryption (AES-256-XTS) is left for
//! future implementation.
//!
//! Reference: <https://www.kernel.org/doc/html/latest/filesystems/fscrypt.html>

/// EXT4_ENCRYPT_FL: inode flag indicating the file is encrypted.
pub const EXT4_ENCRYPT_FL: u32 = 0x00000800;

/// INCOMPAT_ENCRYPT feature flag value.
pub const INCOMPAT_ENCRYPT: u32 = 0x10000;

/// fscrypt encryption context version 1.
pub const FSCRYPT_CONTEXT_V1: u8 = 1;
/// fscrypt encryption context version 2.
pub const FSCRYPT_CONTEXT_V2: u8 = 2;

/// Encryption mode: AES-256-XTS (for file contents).
pub const FSCRYPT_MODE_AES_256_XTS: u8 = 1;
/// Encryption mode: AES-256-CTS-CBC (for filenames).
pub const FSCRYPT_MODE_AES_256_CTS: u8 = 4;
/// Encryption mode: Adiantum (for low-power devices).
pub const FSCRYPT_MODE_ADIANTUM: u8 = 9;

/// Parsed fscrypt context (version-independent).
#[derive(Clone, Debug)]
pub struct FscryptContext {
    /// Context version (1 or 2).
    pub version: u8,
    /// Contents encryption mode.
    pub contents_encryption_mode: u8,
    /// Filenames encryption mode.
    pub filenames_encryption_mode: u8,
    /// Flags.
    pub flags: u8,
    /// Master key identifier (16 bytes for v1, 16 bytes for v2).
    pub master_key_identifier: [u8; 16],
    /// Nonce (16 bytes).
    pub nonce: [u8; 16],
}

impl FscryptContext {
    /// Parse an fscrypt context from raw xattr bytes.
    ///
    /// Returns `None` if the data is too small or has an unknown version.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            log::error!("[ext4::encrypt] empty fscrypt context");
            return None;
        }

        let version = data[0];
        match version {
            FSCRYPT_CONTEXT_V1 => {
                // v1: 1 + 1 + 1 + 1 + 8 + 16 = 28 bytes
                if data.len() < 28 {
                    log::error!(
                        "[ext4::encrypt] fscrypt v1 context too small: {} bytes (need 28)",
                        data.len()
                    );
                    return None;
                }
                let mut master_key = [0u8; 16];
                master_key[..8].copy_from_slice(&data[4..12]);
                let mut nonce = [0u8; 16];
                nonce.copy_from_slice(&data[12..28]);
                Some(FscryptContext {
                    version,
                    contents_encryption_mode: data[1],
                    filenames_encryption_mode: data[2],
                    flags: data[3],
                    master_key_identifier: master_key,
                    nonce,
                })
            }
            FSCRYPT_CONTEXT_V2 => {
                // v2: 1 + 1 + 1 + 1 + 4(padding) + 16 + 16 = 40 bytes
                if data.len() < 40 {
                    log::error!(
                        "[ext4::encrypt] fscrypt v2 context too small: {} bytes (need 40)",
                        data.len()
                    );
                    return None;
                }
                let mut master_key = [0u8; 16];
                master_key.copy_from_slice(&data[8..24]);
                let mut nonce = [0u8; 16];
                nonce.copy_from_slice(&data[24..40]);
                Some(FscryptContext {
                    version,
                    contents_encryption_mode: data[1],
                    filenames_encryption_mode: data[2],
                    flags: data[3],
                    master_key_identifier: master_key,
                    nonce,
                })
            }
            _ => {
                log::error!(
                    "[ext4::encrypt] unknown fscrypt context version: {}",
                    version
                );
                None
            }
        }
    }

    /// Describe the encryption mode for logging.
    pub fn mode_name(mode: u8) -> &'static str {
        match mode {
            FSCRYPT_MODE_AES_256_XTS => "AES-256-XTS",
            FSCRYPT_MODE_AES_256_CTS => "AES-256-CTS-CBC",
            FSCRYPT_MODE_ADIANTUM => "Adiantum",
            _ => "unknown",
        }
    }
}

/// Check if an inode is encrypted and return an error if so.
///
/// Call this before reading inode data. If the inode is encrypted,
/// this returns `Err(Ext4Error::UnsupportedFeature)` with a clear message.
///
/// Returns `Ok(false)` if the inode is not encrypted.
/// Returns `Ok(true)` if encrypted (but for now, we always return an error).
pub fn check_encryption(inode_flags: u32) -> Result<(), crate::readwrite::Ext4Error> {
    if inode_flags & EXT4_ENCRYPT_FL != 0 {
        log::error!(
            "[ext4::encrypt] inode has EXT4_ENCRYPT_FL (0x{:08X}): encrypted inode, decryption not supported",
            inode_flags
        );
        return Err(crate::readwrite::Ext4Error::UnsupportedFeature(
            "encrypted inode, decryption not supported",
        ));
    }
    Ok(())
}

/// Check if the filesystem uses encryption at the feature level.
pub fn fs_has_encryption(feature_incompat: u32) -> bool {
    feature_incompat & INCOMPAT_ENCRYPT != 0
}
