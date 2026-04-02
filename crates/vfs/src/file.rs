//! File operations: open, read, write, seek, close, stat.
//!
//! Provides `FileDescriptor` as an opaque handle, `OpenFlags` for controlling
//! file access modes, `SeekWhence` for seek positioning, and `FileInfo` for
//! stat results.

use alloc::string::String;

/// Opaque file descriptor index into the VFS open-file table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileDescriptor(pub usize);

/// Flags controlling how a file is opened.
#[derive(Debug, Clone, Copy)]
pub struct OpenFlags {
    bits: u32,
}

impl OpenFlags {
    /// Open for reading.
    pub const READ: u32 = 1 << 0;
    /// Open for writing.
    pub const WRITE: u32 = 1 << 1;
    /// Create the file if it does not exist.
    pub const CREATE: u32 = 1 << 2;
    /// Truncate the file to zero length on open.
    pub const TRUNCATE: u32 = 1 << 3;
    /// Writes append to the end of the file.
    pub const APPEND: u32 = 1 << 4;

    /// Create flags from a raw bitmask.
    pub fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    /// Return the raw bitmask.
    pub fn bits(&self) -> u32 {
        self.bits
    }

    /// Check if a specific flag is set.
    pub fn has(&self, flag: u32) -> bool {
        self.bits & flag != 0
    }

    /// Convenience: read-only flags.
    pub fn read_only() -> Self {
        Self::from_bits(Self::READ)
    }

    /// Convenience: write-only flags.
    pub fn write_only() -> Self {
        Self::from_bits(Self::WRITE)
    }

    /// Convenience: read + write flags.
    pub fn read_write() -> Self {
        Self::from_bits(Self::READ | Self::WRITE)
    }

    /// Convenience: create + write + truncate.
    pub fn create_truncate() -> Self {
        Self::from_bits(Self::WRITE | Self::CREATE | Self::TRUNCATE)
    }
}

/// Reference point for seek operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekWhence {
    /// Seek from the beginning of the file.
    Start,
    /// Seek from the current position.
    Current,
    /// Seek from the end of the file (offset is typically negative).
    End,
}

/// The type of a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Block device.
    BlockDevice,
    /// Character device.
    CharDevice,
    /// Unknown or unsupported type.
    Unknown,
}

/// Metadata about a file or directory, returned by `stat`.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Size in bytes (0 for directories on some filesystems).
    pub size: u64,
    /// Entry type.
    pub file_type: FileType,
    /// POSIX-style permissions (e.g. 0o755). 0 if the FS doesn't support them.
    pub permissions: u32,
    /// Creation timestamp (seconds since epoch), or 0 if unavailable.
    pub created: u64,
    /// Last modification timestamp (seconds since epoch), or 0 if unavailable.
    pub modified: u64,
    /// Last access timestamp (seconds since epoch), or 0 if unavailable.
    pub accessed: u64,
    /// Inode number or equivalent handle (filesystem-specific).
    pub inode: u64,
    /// Number of hard links.
    pub nlinks: u32,
    /// Owner user ID (0 if not applicable).
    pub uid: u32,
    /// Owner group ID (0 if not applicable).
    pub gid: u32,
}

impl FileInfo {
    /// Create a minimal `FileInfo` for a regular file with only size populated.
    pub fn simple_file(size: u64) -> Self {
        Self {
            size,
            file_type: FileType::File,
            permissions: 0o644,
            created: 0,
            modified: 0,
            accessed: 0,
            inode: 0,
            nlinks: 1,
            uid: 0,
            gid: 0,
        }
    }

    /// Create a minimal `FileInfo` for a directory.
    pub fn simple_dir() -> Self {
        Self {
            size: 0,
            file_type: FileType::Directory,
            permissions: 0o755,
            created: 0,
            modified: 0,
            accessed: 0,
            inode: 0,
            nlinks: 2,
            uid: 0,
            gid: 0,
        }
    }
}

/// Internal state of an open file, tracked in the VFS open-file table.
#[derive(Debug)]
pub struct OpenFile {
    /// The absolute path this file was opened with.
    pub path: String,
    /// Index into the mount table identifying which filesystem owns this file.
    pub mount_index: usize,
    /// The path relative to the mount point's root.
    pub relative_path: String,
    /// Current read/write position within the file.
    pub position: u64,
    /// The flags used to open this file.
    pub flags: OpenFlags,
    /// Cached file size (updated on stat/write).
    pub size: u64,
}
