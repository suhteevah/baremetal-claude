//! Directory operations: readdir, mkdir, rmdir.
//!
//! Provides `DirEntry` for directory listing results.

use alloc::string::String;

use crate::file::FileType;

/// A single entry in a directory listing.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// The name of this entry (file or subdirectory name, not the full path).
    pub name: String,
    /// The type of this entry.
    pub file_type: FileType,
    /// Size in bytes (0 for directories on some filesystems).
    pub size: u64,
    /// Inode or equivalent handle (filesystem-specific).
    pub inode: u64,
}

impl DirEntry {
    /// Create a new directory entry.
    pub fn new(name: &str, file_type: FileType, size: u64) -> Self {
        Self {
            name: String::from(name),
            file_type,
            size,
            inode: 0,
        }
    }

    /// Returns `true` if this entry is a regular file.
    pub fn is_file(&self) -> bool {
        self.file_type == FileType::File
    }

    /// Returns `true` if this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }

    /// Returns `true` if this entry is a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.file_type == FileType::Symlink
    }
}
