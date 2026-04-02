//! User management — authentication, user database, and SSH auth integration.
//!
//! Provides a simple user database loaded from init config. Each user has a
//! username, optional password hash (SHA-256), optional authorized SSH keys,
//! a home directory, and a default shell.
//!
//! ## Default user
//!
//! If no users are configured, a default user "matt" is created with no
//! password (auto-login enabled).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// SHA-256 (minimal no_std implementation)
// ---------------------------------------------------------------------------

/// Minimal SHA-256 for password hashing. We don't have ring or sha2 crate
/// in-kernel, so this is a compact implementation for auth purposes.
mod sha256 {
    /// SHA-256 constants: first 32 bits of the fractional parts of the cube
    /// roots of the first 64 primes.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    /// Initial hash values: first 32 bits of the fractional parts of the
    /// square roots of the first 8 primes.
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (!x & z) }
    fn maj(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (x & z) ^ (y & z) }
    fn sigma0(x: u32) -> u32 { x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22) }
    fn sigma1(x: u32) -> u32 { x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25) }
    fn lsigma0(x: u32) -> u32 { x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3) }
    fn lsigma1(x: u32) -> u32 { x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10) }

    /// Compute SHA-256 hash of the input bytes. Returns 32-byte digest.
    pub fn hash(data: &[u8]) -> [u8; 32] {
        let mut h = H0;

        // Pre-processing: pad message
        let bit_len = (data.len() as u64) * 8;
        let mut msg = alloc::vec::Vec::with_capacity(data.len() + 72);
        msg.extend_from_slice(data);
        msg.push(0x80);
        // Pad with zeros until length is 56 mod 64
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        // Append original length as 64-bit big-endian
        msg.extend_from_slice(&bit_len.to_be_bytes());

        // Process each 512-bit (64-byte) block
        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                w[i] = lsigma1(w[i - 2])
                    .wrapping_add(w[i - 7])
                    .wrapping_add(lsigma0(w[i - 15]))
                    .wrapping_add(w[i - 16]);
            }

            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

            for i in 0..64 {
                let t1 = hh
                    .wrapping_add(sigma1(e))
                    .wrapping_add(ch(e, f, g))
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let t2 = sigma0(a).wrapping_add(maj(a, b, c));
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }

            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }

        let mut digest = [0u8; 32];
        for (i, val) in h.iter().enumerate() {
            digest[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
        }
        digest
    }

    /// Convert a 32-byte digest to a 64-character hex string.
    pub fn to_hex(digest: &[u8; 32]) -> alloc::string::String {
        let mut s = alloc::string::String::with_capacity(64);
        for byte in digest {
            s.push(HEX_CHARS[(*byte >> 4) as usize]);
            s.push(HEX_CHARS[(*byte & 0xf) as usize]);
        }
        s
    }

    const HEX_CHARS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7',
        '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
}

// ---------------------------------------------------------------------------
// User struct
// ---------------------------------------------------------------------------

/// A system user.
#[derive(Debug, Clone)]
pub struct User {
    /// Login name.
    pub username: String,
    /// SHA-256 hex digest of the password. Empty string means no password
    /// (auto-login / password-less SSH).
    pub password_hash: String,
    /// SSH authorized public keys (one per entry, OpenSSH format).
    pub authorized_keys: Vec<String>,
    /// Home directory path.
    pub home_dir: String,
    /// Default shell (e.g. "/bin/claudio-shell").
    pub shell: String,
}

impl User {
    /// Create a new user with no password.
    pub fn new(username: &str) -> Self {
        User {
            username: String::from(username),
            password_hash: String::new(),
            authorized_keys: Vec::new(),
            home_dir: alloc::format!("/home/{}", username),
            shell: String::from("/bin/claudio-shell"),
        }
    }

    /// Create a user with a password (hashed immediately).
    pub fn with_password(username: &str, password: &str) -> Self {
        let mut user = Self::new(username);
        user.set_password(password);
        user
    }

    /// Set the user's password (stores SHA-256 hash, never plaintext).
    pub fn set_password(&mut self, password: &str) {
        let digest = sha256::hash(password.as_bytes());
        self.password_hash = sha256::to_hex(&digest);
    }

    /// Check if a password matches. Returns `true` if the user has no
    /// password (empty hash) or if the hash matches.
    pub fn check_password(&self, password: &str) -> bool {
        if self.password_hash.is_empty() {
            // No password set — always matches (auto-login).
            return true;
        }
        let digest = sha256::hash(password.as_bytes());
        let hex = sha256::to_hex(&digest);
        hex == self.password_hash
    }

    /// Check if a public key is in this user's authorized_keys.
    pub fn check_public_key(&self, key: &str) -> bool {
        // Compare the key data portion (skip key type prefix for flexibility).
        // In practice, compare the full "ssh-rsa AAAA..." or "ssh-ed25519 AAAA..." string.
        for ak in &self.authorized_keys {
            if ak.trim() == key.trim() {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// UserDatabase
// ---------------------------------------------------------------------------

/// In-memory user database.
pub struct UserDatabase {
    users: Vec<User>,
}

impl UserDatabase {
    /// Create a new user database with the default user.
    pub fn new() -> Self {
        let mut db = UserDatabase {
            users: Vec::new(),
        };

        // Default user: "matt" with no password (auto-login)
        let mut matt = User::new("matt");
        matt.home_dir = String::from("/home/matt");
        matt.shell = String::from("/bin/claudio-shell");
        db.users.push(matt);

        // Root user (no login by default — empty password means auto-login,
        // but root should require explicit config to enable).
        let mut root = User::new("root");
        root.home_dir = String::from("/root");
        root.password_hash = String::from("!"); // "!" = locked account
        db.users.push(root);

        db
    }

    /// Load additional users from config text.
    ///
    /// Format: one user per line as `username:password_hash:home_dir:shell:authorized_keys`
    /// where authorized_keys are comma-separated.
    /// A hash of "!" means the account is locked. Empty hash means no password.
    pub fn load_from_config(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.splitn(5, ':').collect();
            if parts.len() < 2 {
                log::warn!("[users] ignoring malformed user line: {}", line);
                continue;
            }

            let username = parts[0];

            // Skip if user already exists (don't override defaults)
            if self.lookup(username).is_some() {
                log::debug!("[users] user '{}' already exists, updating", username);
                // Update existing user
                if let Some(user) = self.users.iter_mut().find(|u| u.username == username) {
                    if parts.len() > 1 && !parts[1].is_empty() {
                        user.password_hash = String::from(parts[1]);
                    }
                    if parts.len() > 2 && !parts[2].is_empty() {
                        user.home_dir = String::from(parts[2]);
                    }
                    if parts.len() > 3 && !parts[3].is_empty() {
                        user.shell = String::from(parts[3]);
                    }
                    if parts.len() > 4 && !parts[4].is_empty() {
                        user.authorized_keys = parts[4]
                            .split(',')
                            .map(|k| String::from(k.trim()))
                            .collect();
                    }
                }
                continue;
            }

            let mut user = User::new(username);
            if parts.len() > 1 && !parts[1].is_empty() {
                user.password_hash = String::from(parts[1]);
            }
            if parts.len() > 2 && !parts[2].is_empty() {
                user.home_dir = String::from(parts[2]);
            }
            if parts.len() > 3 && !parts[3].is_empty() {
                user.shell = String::from(parts[3]);
            }
            if parts.len() > 4 && !parts[4].is_empty() {
                user.authorized_keys = parts[4]
                    .split(',')
                    .map(|k| String::from(k.trim()))
                    .collect();
            }

            log::info!("[users] loaded user '{}' (home={})", user.username, user.home_dir);
            self.users.push(user);
        }
    }

    /// Authenticate a user with username and password.
    pub fn authenticate(&self, username: &str, password: &str) -> bool {
        match self.lookup(username) {
            Some(user) => {
                if user.password_hash == "!" {
                    log::warn!("[users] login denied: account '{}' is locked", username);
                    return false;
                }
                let ok = user.check_password(password);
                if ok {
                    log::info!("[users] authenticated user '{}'", username);
                } else {
                    log::warn!("[users] failed auth for user '{}'", username);
                }
                ok
            }
            None => {
                log::warn!("[users] unknown user '{}'", username);
                false
            }
        }
    }

    /// Authenticate a user with an SSH public key.
    pub fn authenticate_pubkey(&self, username: &str, key: &str) -> bool {
        match self.lookup(username) {
            Some(user) => {
                if user.password_hash == "!" {
                    log::warn!("[users] pubkey denied: account '{}' is locked", username);
                    return false;
                }
                let ok = user.check_public_key(key);
                if ok {
                    log::info!("[users] pubkey authenticated user '{}'", username);
                } else {
                    log::debug!("[users] no matching pubkey for user '{}'", username);
                }
                ok
            }
            None => {
                log::warn!("[users] unknown user '{}' (pubkey auth)", username);
                false
            }
        }
    }

    /// Look up a user by username.
    pub fn lookup(&self, username: &str) -> Option<&User> {
        self.users.iter().find(|u| u.username == username)
    }

    /// Get all users (for listing / admin purposes).
    pub fn all_users(&self) -> &[User] {
        &self.users
    }

    /// Add a new user to the database.
    pub fn add_user(&mut self, user: User) {
        if self.lookup(&user.username).is_some() {
            log::warn!("[users] user '{}' already exists", user.username);
            return;
        }
        log::info!("[users] added user '{}'", user.username);
        self.users.push(user);
    }
}

// ---------------------------------------------------------------------------
// Global user database
// ---------------------------------------------------------------------------

static USER_DB: spin::Mutex<Option<UserDatabase>> = spin::Mutex::new(None);

/// Initialize the global user database.
pub fn init() {
    let mut lock = USER_DB.lock();
    if lock.is_some() {
        log::warn!("[users] user database already initialized");
        return;
    }
    let db = UserDatabase::new();
    log::info!("[users] user database initialized ({} users)", db.users.len());
    *lock = Some(db);
}

/// Initialize the user database and load additional users from config text.
pub fn init_with_config(config_text: &str) {
    let mut lock = USER_DB.lock();
    let mut db = if lock.is_some() {
        lock.take().unwrap()
    } else {
        UserDatabase::new()
    };
    db.load_from_config(config_text);
    log::info!("[users] user database loaded ({} users)", db.users.len());
    *lock = Some(db);
}

/// Authenticate a user (password-based). Thread-safe.
pub fn authenticate(username: &str, password: &str) -> bool {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.authenticate(username, password),
        None => {
            log::warn!("[users] user database not initialized");
            false
        }
    }
}

/// Authenticate a user (public-key-based). Thread-safe.
pub fn authenticate_pubkey(username: &str, key: &str) -> bool {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.authenticate_pubkey(username, key),
        None => {
            log::warn!("[users] user database not initialized");
            false
        }
    }
}

/// Look up a user by username. Returns a clone of the User if found.
pub fn lookup_user(username: &str) -> Option<User> {
    let lock = USER_DB.lock();
    lock.as_ref().and_then(|db| db.lookup(username).cloned())
}
