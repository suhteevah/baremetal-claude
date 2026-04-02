//! Filesystem trait — the interface that concrete filesystem crates implement.
//!
//! Each filesystem (ext4, btrfs, NTFS, FAT32) provides a struct that implements
//! `Filesystem`. The VFS dispatches operations through this trait after resolving
//! mount points and converting absolute paths to filesystem-relative paths.

use alloc::vec::Vec;

use crate::dir::DirEntry;
use crate::file::FileInfo;

/// Identifies which filesystem type is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    /// Fourth extended filesystem (Linux).
    Ext4,
    /// B-tree filesystem (Linux, copy-on-write).
    Btrfs,
    /// New Technology File System (Windows).
    Ntfs,
    /// File Allocation Table, 32-bit variant.
    Fat32,
    /// Unknown or unrecognized filesystem.
    Unknown,
}

impl core::fmt::Display for FsType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FsType::Ext4 => write!(f, "ext4"),
            FsType::Btrfs => write!(f, "btrfs"),
            FsType::Ntfs => write!(f, "NTFS"),
            FsType::Fat32 => write!(f, "FAT32"),
            FsType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Error type for filesystem operations.
#[derive(Debug)]
pub enum FsError {
    /// The requested file or directory was not found.
    NotFound,
    /// The path already exists (e.g. trying to create an existing file/dir).
    AlreadyExists,
    /// Permission denied (filesystem-level, not VFS mount-level).
    PermissionDenied,
    /// The operation targets a file but the path is a directory, or vice versa.
    WrongType,
    /// The directory is not empty (for rmdir).
    NotEmpty,
    /// A storage I/O error occurred.
    IoError,
    /// The filesystem is full — no free blocks or inodes.
    NoSpace,
    /// The filesystem's internal structures are corrupt.
    Corrupt,
    /// The operation is not supported by this filesystem.
    Unsupported,
    /// An offset or size is out of range.
    OutOfRange,
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FsError::NotFound => write!(f, "not found"),
            FsError::AlreadyExists => write!(f, "already exists"),
            FsError::PermissionDenied => write!(f, "permission denied"),
            FsError::WrongType => write!(f, "wrong file type"),
            FsError::NotEmpty => write!(f, "directory not empty"),
            FsError::IoError => write!(f, "I/O error"),
            FsError::NoSpace => write!(f, "no space left on device"),
            FsError::Corrupt => write!(f, "filesystem corrupt"),
            FsError::Unsupported => write!(f, "operation not supported"),
            FsError::OutOfRange => write!(f, "offset out of range"),
        }
    }
}

/// The trait that every concrete filesystem implementation must provide.
///
/// All paths passed to these methods are **relative to the filesystem root**
/// (the VFS handles mount-point prefix stripping). Paths always start with `/`.
pub trait Filesystem: Send + Sync {
    /// Return the filesystem type.
    fn fs_type(&self) -> FsType;

    /// Return a human-readable label (e.g. volume name), if available.
    fn label(&self) -> Option<&str> {
        None
    }

    /// Read data from a file at the given path and offset into `buf`.
    ///
    /// Returns the number of bytes actually read (may be less than `buf.len()`
    /// at end-of-file).
    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Write data to a file at the given path and offset.
    ///
    /// Returns the number of bytes written. The file is extended if writing past
    /// the current end.
    fn write_file(&self, path: &str, offset: u64, data: &[u8]) -> Result<usize, FsError>;

    /// Create a new empty file at the given path.
    ///
    /// Returns `AlreadyExists` if the file already exists.
    fn create_file(&self, path: &str) -> Result<(), FsError>;

    /// Delete a file at the given path.
    fn delete_file(&self, path: &str) -> Result<(), FsError>;

    /// Create a directory at the given path.
    fn mkdir(&self, path: &str) -> Result<(), FsError>;

    /// Remove an empty directory at the given path.
    fn rmdir(&self, path: &str) -> Result<(), FsError>;

    /// Retrieve metadata for the file or directory at the given path.
    fn stat(&self, path: &str) -> Result<FileInfo, FsError>;

    /// List entries in the directory at the given path.
    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;

    /// Rename or move a file/directory from `old` to `new`.
    fn rename(&self, old: &str, new: &str) -> Result<(), FsError>;

    /// Truncate a file to the specified size.
    fn truncate(&self, path: &str, size: u64) -> Result<(), FsError>;

    /// Flush all pending writes to the underlying block device.
    fn sync(&self) -> Result<(), FsError>;
}

impl core::fmt::Debug for dyn Filesystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Filesystem({})", self.fs_type())
    }
}
