//! # claudio-ext4
//!
//! A `no_std` ext4 filesystem implementation for ClaudioOS.
//!
//! This crate provides read and write access to ext4 filesystems, including:
//! - Superblock parsing and validation
//! - Block group descriptor table management
//! - Inode reading and writing (with extent tree support)
//! - Directory entry parsing, creation, and lookup
//! - Block and inode bitmap allocation
//! - High-level file read/write/create/mkdir API
//! - HTree indexed directory lookup
//! - Legacy indirect block map (ext2/ext3 compatibility)
//! - JBD2 journal replay
//! - CRC32C metadata checksums
//! - Encryption detection
//!
//! ## Usage
//!
//! ```rust,no_run
//! use claudio_ext4::{Ext4Fs, BlockDevice};
//!
//! // Implement BlockDevice for your storage backend
//! // Then mount the filesystem:
//! let fs = Ext4Fs::mount(device).expect("failed to mount ext4");
//! let data = fs.read_file(b"/etc/hostname").expect("read failed");
//! ```

#![no_std]

extern crate alloc;

pub mod bitmap;
pub mod block_group;
pub mod block_map;
pub mod crc32c;
pub mod dir;
pub mod encrypt;
pub mod extent;
pub mod htree;
pub mod inode;
pub mod journal;
pub mod readwrite;
pub mod superblock;

pub use readwrite::{BlockDevice, Ext4Fs, Ext4Error};
pub use superblock::Superblock;
pub use block_group::BlockGroupDesc;
pub use inode::Inode;
pub use dir::DirEntry;
pub use extent::{ExtentHeader, ExtentIndex, ExtentLeaf};
pub use bitmap::BitmapAllocator;
pub use crc32c::{crc32c, crc32c_seed, crc32c_uuid_seed};
pub use journal::{JournalSuperblock, JournalTransaction};
pub use htree::{dx_hash, EXT4_INDEX_FL};
pub use block_map::read_block_map;
pub use encrypt::{check_encryption, EXT4_ENCRYPT_FL};
