//! Mount table management.
//!
//! Tracks which filesystems are mounted at which path prefixes. Provides
//! longest-prefix-match resolution so that `/mnt/data/subdir/file.txt` resolves
//! to the filesystem mounted at `/mnt/data` with relative path `/subdir/file.txt`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fs_trait::Filesystem;
use crate::path::Path;

/// Options for a mount operation.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// If `true`, the mount is read-only — write/create/delete operations will fail.
    pub read_only: bool,
    /// If `true`, path comparisons on this mount are case-insensitive.
    pub case_insensitive: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            read_only: false,
            case_insensitive: false,
        }
    }
}

/// A single mount point: a path prefix bound to a filesystem handle.
#[derive(Debug)]
pub struct MountPoint {
    /// The absolute path prefix where this filesystem is mounted (e.g. `/mnt/data`).
    pub path: String,
    /// The filesystem implementation backing this mount.
    pub filesystem: &'static dyn Filesystem,
    /// Mount options.
    pub options: MountOptions,
}

/// Ordered collection of mount points, supporting longest-prefix-match resolution.
pub struct MountTable {
    mounts: Vec<MountPoint>,
}

/// Result of resolving a path against the mount table.
pub struct ResolvedMount<'a> {
    /// The filesystem that owns this path.
    pub filesystem: &'a dyn Filesystem,
    /// The path relative to the mount point's root.
    pub relative_path: Path,
    /// Whether this mount is read-only.
    pub read_only: bool,
}

impl MountTable {
    /// Create an empty mount table.
    pub fn new() -> Self {
        log::info!("mount: creating empty mount table");
        Self {
            mounts: Vec::new(),
        }
    }

    /// Mount a filesystem at the given path.
    ///
    /// The path must be absolute. If a filesystem is already mounted at exactly
    /// this path, it is replaced (the old one is effectively unmounted).
    pub fn mount(
        &mut self,
        path: &str,
        filesystem: &'static dyn Filesystem,
        options: MountOptions,
    ) -> Result<(), MountError> {
        if !path.starts_with('/') {
            log::error!("mount: path {:?} is not absolute", path);
            return Err(MountError::NotAbsolute);
        }

        // Normalize: strip trailing slash (unless root)
        let normalized = if path.len() > 1 {
            path.trim_end_matches('/')
        } else {
            path
        };

        // Check for duplicate mount point
        if let Some(idx) = self.mounts.iter().position(|m| m.path == normalized) {
            log::warn!(
                "mount: replacing existing mount at {:?}",
                normalized
            );
            self.mounts.remove(idx);
        }

        log::info!(
            "mount: mounting filesystem at {:?} (read_only={}, case_insensitive={})",
            normalized,
            options.read_only,
            options.case_insensitive
        );

        self.mounts.push(MountPoint {
            path: String::from(normalized),
            filesystem,
            options,
        });

        // Sort by path length descending so longest prefix is checked first.
        self.mounts.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

        log::debug!(
            "mount: table now has {} mount(s)",
            self.mounts.len()
        );
        Ok(())
    }

    /// Unmount the filesystem at the given path.
    pub fn umount(&mut self, path: &str) -> Result<(), MountError> {
        let normalized = if path.len() > 1 {
            path.trim_end_matches('/')
        } else {
            path
        };

        if let Some(idx) = self.mounts.iter().position(|m| m.path == normalized) {
            let removed = self.mounts.remove(idx);
            log::info!("mount: unmounted filesystem from {:?}", removed.path);
            Ok(())
        } else {
            log::error!("mount: no filesystem mounted at {:?}", normalized);
            Err(MountError::NotMounted)
        }
    }

    /// Resolve a path to a mounted filesystem and the relative path within it.
    ///
    /// Uses longest-prefix-match: `/mnt/data/foo` matches `/mnt/data` before `/mnt`.
    pub fn resolve(&self, path: &str) -> Result<ResolvedMount<'_>, MountError> {
        log::trace!("mount: resolving path {:?}", path);

        let lookup_path = Path::new(path).normalize();
        let lookup_str = lookup_path.as_str();

        for mount in &self.mounts {
            let prefix = mount.path.as_str();
            let matches = if mount.options.case_insensitive {
                lookup_str
                    .to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            } else {
                lookup_str.starts_with(prefix)
            };

            if matches {
                // Make sure it's a proper prefix boundary (not `/mnt/data` matching `/mnt/datastore`)
                let rest = &lookup_str[prefix.len()..];
                if !rest.is_empty() && !rest.starts_with('/') && prefix != "/" {
                    continue;
                }

                let relative = if prefix == "/" {
                    Path::new(lookup_str)
                } else {
                    lookup_path.strip_prefix(prefix).unwrap_or(Path::new("/"))
                };

                log::debug!(
                    "mount: resolved {:?} -> mount={:?}, relative={:?}",
                    path,
                    mount.path,
                    relative.as_str()
                );

                return Ok(ResolvedMount {
                    filesystem: mount.filesystem,
                    relative_path: relative,
                    read_only: mount.options.read_only,
                });
            }
        }

        log::error!("mount: no mount point found for {:?}", path);
        Err(MountError::NoMount)
    }

    /// Return a slice of all current mount points (sorted longest-first).
    pub fn list(&self) -> &[MountPoint] {
        &self.mounts
    }

    /// Return the number of active mounts.
    pub fn count(&self) -> usize {
        self.mounts.len()
    }
}

/// Errors that can occur during mount operations.
#[derive(Debug)]
pub enum MountError {
    /// The path is not absolute (doesn't start with `/`).
    NotAbsolute,
    /// No filesystem is mounted at the specified path.
    NotMounted,
    /// No mount point was found that matches the given path.
    NoMount,
}

impl core::fmt::Display for MountError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MountError::NotAbsolute => write!(f, "mount path must be absolute"),
            MountError::NotMounted => write!(f, "no filesystem mounted at path"),
            MountError::NoMount => write!(f, "no mount point matches path"),
        }
    }
}
