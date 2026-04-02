# ClaudioOS Filesystem Documentation

## Overview

ClaudioOS implements a complete filesystem stack: a VFS abstraction layer on top,
multiple filesystem implementations (ext4, btrfs, NTFS, FAT32), and GPT/MBR
partition detection. All crates are `#![no_std]`.

```
+---------------------------------------------------------------+
|                  VFS (claudio-vfs)                             |
|  Mount table | Path handling | POSIX-like file API | FDs      |
+-------+--------+--------+--------+-----------------------+----+
| ext4  | btrfs  | NTFS   | FAT32  | GPT/MBR partition     |
| 3,013 | 4,006  | 3,561  |  (fatfs | auto-detection        |
| lines | lines  | lines  |  crate) |                       |
+-------+--------+--------+--------+-----------------------+
|              BlockDevice trait                              |
|        (implemented by AHCI, NVMe, VirtIO-blk)             |
+------------------------------------------------------------+
```

---

## VFS Layer (`crates/vfs/`, 1,930 lines)

The Virtual Filesystem unifies all filesystem implementations behind a single API.

### Modules

| Module | Purpose |
|--------|---------|
| `path.rs` | Heap-allocated paths: normalization, component iteration, join, parent, extension |
| `mount.rs` | Mount table: longest-prefix-match resolution, read-only flag, ordered listing |
| `file.rs` | File descriptors, open flags (READ, WRITE, CREATE, APPEND), seek whence, stat |
| `dir.rs` | Directory entries: name, file type, size |
| `fs_trait.rs` | `Filesystem` trait that ext4/btrfs/NTFS/FAT32 implement |
| `device.rs` | `BlockDevice` trait, GPT and MBR partition table parsing, fs type auto-detection |
| `vfs.rs` | Top-level `Vfs` struct: global init, working directory, full POSIX-like API |

### API

```rust
use claudio_vfs::{Vfs, OpenFlags};

let mut vfs = Vfs::new();
vfs.mount("/data", my_ext4_fs, MountOptions::default());

let fd = vfs.open("/data/hello.txt", OpenFlags::READ)?;
let mut buf = [0u8; 4096];
let n = vfs.read(fd, &mut buf)?;
vfs.close(fd)?;
```

### Filesystem Trait

Every filesystem crate implements this trait:

```rust
pub trait Filesystem {
    fn fs_type(&self) -> FsType;
    fn read_file(&self, path: &[u8]) -> Result<Vec<u8>, ...>;
    fn write_file(&mut self, path: &[u8], data: &[u8]) -> Result<(), ...>;
    fn list_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, ...>;
    fn mkdir(&mut self, path: &[u8]) -> Result<(), ...>;
    fn stat(&self, path: &[u8]) -> Result<FileInfo, ...>;
    // ... rmdir, unlink, rename
}
```

### GPT/MBR Partition Detection

The `device.rs` module parses partition tables from block devices:

- **GPT** (GUID Partition Table): reads protective MBR + GPT header at LBA 1,
  iterates partition entries, detects filesystem type from partition type GUID
- **MBR** (Master Boot Record): reads legacy MBR at sector 0, iterates up to 4
  primary partition entries, detects type from partition type byte

Filesystem type auto-detection reads the first few sectors of each partition to
identify the filesystem (ext4 superblock magic, btrfs magic, NTFS boot sector, etc.).

---

## ext4 (`crates/ext4/`, 3,013 lines)

Full read-write ext4 implementation.

### Modules

| Module | Purpose |
|--------|---------|
| `superblock.rs` | Superblock at offset 1024: magic (0xEF53), block size, inode count, features |
| `block_group.rs` | Block group descriptor table: inode/block bitmaps, inode table location |
| `inode.rs` | Inode structure: mode, size, timestamps, extent tree or block pointers |
| `extent.rs` | Extent tree: header, index nodes (internal), leaf nodes (physical blocks) |
| `dir.rs` | Directory entries: inode number, name length, file type, rec_len for iteration |
| `bitmap.rs` | Block and inode bitmap allocation: find free, mark used, mark free |
| `readwrite.rs` | High-level `Ext4Fs` API: mount, read_file, write_file, mkdir, list_dir |

### On-Disk Layout

```
Block 0:       Boot sector (1024 bytes, unused by ext4)
Block 0+1024:  Superblock (magic 0xEF53)
Block 1+:      Block group descriptor table
  For each block group:
    Block bitmap (1 block)
    Inode bitmap (1 block)
    Inode table (N blocks)
    Data blocks

Inode 2: root directory (always)
```

### Extent Tree

ext4 uses an extent tree for efficient mapping of logical to physical blocks:

```
Extent Header (12 bytes):
  magic: 0xF30A
  entries: number of entries following
  max: max entries that fit
  depth: 0 = leaf, >0 = internal

Extent Index (12 bytes, depth > 0):
  block: first logical block
  leaf: physical block of child node

Extent Leaf (12 bytes, depth = 0):
  block: first logical block
  len: number of contiguous blocks
  start_hi/start_lo: physical block number
```

---

## btrfs (`crates/btrfs/`, 4,006 lines)

Read-write btrfs with B-tree traversal, copy-on-write, and CRC32C checksums.

### Modules

| Module | Purpose |
|--------|---------|
| `superblock.rs` | Superblock at offset 0x10000: magic `_BHRfS_M`, chunk tree, root tree |
| `key.rs` | `BtrfsKey` (objectid, type, offset): used in all B-tree lookups |
| `tree.rs` | B-tree traversal: search, insert, split nodes/leaves |
| `item.rs` | B-tree node/leaf structures: header, key pointers, item data |
| `inode.rs` | Inode item: generation, size, mode, nlink, timestamps |
| `dir.rs` | Directory item: name hash (CRC32C), name, child objectid |
| `extent.rs` | File extent item: inline data, regular extents, preallocated |
| `chunk.rs` | Chunk/device mapping: logical address -> physical stripe mapping |
| `crc32c.rs` | CRC32C implementation for checksums and directory name hashing |
| `readwrite.rs` | High-level `BtrFs` API: mount, read_file, write_file, mkdir |

### B-Tree Structure

```
Root Node (internal)
  +-- Key Pointer [objectid=256, type=DIR_ITEM, offset=0]
  |     +-- Leaf: [DirItem: name="etc", child=257]
  |     +-- Leaf: [DirItem: name="home", child=258]
  +-- Key Pointer [objectid=257, type=INODE_ITEM, offset=0]
        +-- Leaf: [InodeItem: mode=0o755, size=4096]
        +-- Leaf: [FileExtentItem: disk_bytenr=0x1000000, num_bytes=4096]
```

### Copy-on-Write

btrfs never overwrites data in place. Writes allocate new blocks, update parent
pointers, and free old blocks. This provides atomic updates and easy snapshots.

---

## NTFS (`crates/ntfs/`, 3,561 lines)

Read-write NTFS with MFT parsing, data run decoding, and B+ tree indexes.

### Modules

| Module | Purpose |
|--------|---------|
| `boot_sector.rs` | NTFS boot sector: BPB, bytes per sector, MFT start cluster |
| `mft.rs` | Master File Table entry parsing: header, fixup array, attribute iteration |
| `attribute.rs` | Attribute parsing: resident (inline data) and non-resident (data runs) |
| `data_runs.rs` | Data run decoding: variable-length (offset, length) pairs for cluster chains |
| `filename.rs` | $FILE_NAME attribute: UTF-16LE name, parent MFT ref, timestamps |
| `index.rs` | B+ tree index: $INDEX_ROOT, $INDEX_ALLOCATION, index entry iteration |
| `upcase.rs` | $UpCase table: case-insensitive filename comparison (Unicode) |
| `readwrite.rs` | High-level `NtfsFs` API: mount, read_file, write_file, list_dir |

### MFT Layout

```
MFT Entry (typically 1024 bytes):
  +0x00: Signature "FILE"
  +0x04: Fixup offset and count
  +0x14: First attribute offset
  +0x16: Flags (IN_USE, DIRECTORY)
  +0x18: Used size
  +0x1C: Allocated size

Well-known MFT entries:
  Entry 0: $MFT itself
  Entry 1: $MFTMirr
  Entry 2: $LogFile
  Entry 3: $Volume
  Entry 4: $AttrDef
  Entry 5: Root directory (\)
  Entry 6: $Bitmap
  Entry 7: $Boot
  Entry 8: $BadClus
```

### Data Run Encoding

Non-resident attributes store data in "runs" (contiguous cluster ranges):

```
Header byte: [offset_size:4][length_size:4]
  length_size bytes: cluster count (little-endian)
  offset_size bytes: cluster offset, signed delta from previous run

Example: 31 01 40 00 = 1 cluster at offset 0x4000
         31 02 00 01 = 2 clusters at offset (previous + 0x100)
```

---

## FAT32

ClaudioOS uses the [`fatfs`](https://docs.rs/fatfs/0.3) crate (v0.3) for FAT32
support. This is used for the `fs-persist` layer (token storage, config files,
conversation logs). The `fatfs` crate operates on any `Read + Write + Seek` backend.

---

## Storage Drivers

The filesystem layer sits on top of block device drivers:

| Driver | Crate | Interface |
|--------|-------|-----------|
| AHCI/SATA | `claudio-ahci` | `AhciDisk::read_sectors()`, `write_sectors()` |
| NVMe | `claudio-nvme` | `NvmeDisk::read_sectors()`, `write_sectors()` |
| VirtIO-blk | (planned) | Would provide virtual block device in QEMU |

All drivers implement a common `BlockDevice` trait:

```rust
pub trait BlockDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), Error>;
    fn sector_size(&self) -> u32;  // typically 512
    fn total_sectors(&self) -> u64;
}
```
