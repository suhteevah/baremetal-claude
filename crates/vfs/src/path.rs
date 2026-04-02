//! Path handling for the VFS.
//!
//! Provides an owned, heap-allocated `Path` type with component iteration,
//! normalization (resolving `.` and `..`), joining, parent/filename/extension
//! extraction, and absolute-vs-relative detection. An optional case-sensitivity
//! flag controls comparison behavior for case-insensitive filesystems (FAT32, NTFS).

use alloc::string::String;
use alloc::vec::Vec;

/// Separator used throughout ClaudioOS VFS paths.
pub const SEPARATOR: char = '/';

/// An owned, heap-allocated filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    inner: String,
    /// When `true`, comparisons are case-insensitive (for FAT32 / NTFS mounts).
    pub case_insensitive: bool,
}

impl Path {
    /// Create a new path from a string slice.
    pub fn new(s: &str) -> Self {
        log::trace!("path: new from {:?}", s);
        Self {
            inner: String::from(s),
            case_insensitive: false,
        }
    }

    /// Create a path with explicit case-sensitivity setting.
    pub fn with_case_sensitivity(s: &str, case_insensitive: bool) -> Self {
        log::trace!(
            "path: new from {:?} (case_insensitive={})",
            s,
            case_insensitive
        );
        Self {
            inner: String::from(s),
            case_insensitive,
        }
    }

    /// Returns the raw string representation.
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Returns `true` if this path starts with `/`.
    pub fn is_absolute(&self) -> bool {
        self.inner.starts_with('/')
    }

    /// Returns `true` if this path does NOT start with `/`.
    pub fn is_relative(&self) -> bool {
        !self.is_absolute()
    }

    /// Split the path into its individual components.
    ///
    /// Leading `/` is not included as a component; trailing slashes are ignored.
    /// E.g. `"/foo/bar/baz"` -> `["foo", "bar", "baz"]`.
    pub fn components(&self) -> Vec<&str> {
        self.inner
            .split('/')
            .filter(|c| !c.is_empty())
            .collect()
    }

    /// Normalize the path by resolving `.` (current dir) and `..` (parent dir).
    ///
    /// - Collapses consecutive separators.
    /// - Preserves leading `/` for absolute paths.
    /// - `..` at the root is a no-op (stays at `/`).
    pub fn normalize(&self) -> Path {
        log::trace!("path: normalizing {:?}", self.inner);
        let absolute = self.is_absolute();
        let mut stack: Vec<&str> = Vec::new();

        for component in self.components() {
            match component {
                "." => { /* skip */ }
                ".." => {
                    if !stack.is_empty() && (!absolute || stack.len() > 0) {
                        stack.pop();
                    }
                }
                c => stack.push(c),
            }
        }

        let mut result = String::new();
        if absolute {
            result.push('/');
        }
        for (i, comp) in stack.iter().enumerate() {
            if i > 0 {
                result.push('/');
            }
            result.push_str(comp);
        }
        // Ensure absolute root is just "/"
        if absolute && result.len() == 0 {
            result.push('/');
        }

        log::trace!("path: normalized to {:?}", result);
        Path {
            inner: result,
            case_insensitive: self.case_insensitive,
        }
    }

    /// Join another path onto this one.
    ///
    /// If `other` is absolute, it replaces this path entirely.
    /// Otherwise it is appended with a separator.
    pub fn join(&self, other: &str) -> Path {
        log::trace!("path: joining {:?} with {:?}", self.inner, other);
        if other.starts_with('/') {
            return Path {
                inner: String::from(other),
                case_insensitive: self.case_insensitive,
            };
        }

        let mut result = self.inner.clone();
        if !result.ends_with('/') && !result.is_empty() {
            result.push('/');
        }
        result.push_str(other);
        Path {
            inner: result,
            case_insensitive: self.case_insensitive,
        }
    }

    /// Return the parent directory, or `None` if this is the root.
    pub fn parent(&self) -> Option<Path> {
        let norm = self.normalize();
        let s = norm.as_str();
        if s == "/" || s.is_empty() {
            return None;
        }
        match s.rfind('/') {
            Some(0) => Some(Path {
                inner: String::from("/"),
                case_insensitive: self.case_insensitive,
            }),
            Some(idx) => Some(Path {
                inner: String::from(&s[..idx]),
                case_insensitive: self.case_insensitive,
            }),
            None => Some(Path {
                inner: String::from(""),
                case_insensitive: self.case_insensitive,
            }),
        }
    }

    /// Return the final component (file or directory name), or `None` for root.
    pub fn filename(&self) -> Option<&str> {
        let trimmed = self.inner.trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.rfind('/') {
            Some(idx) => Some(&trimmed[idx + 1..]),
            None => Some(trimmed),
        }
    }

    /// Return the file extension (without the leading dot), or `None`.
    pub fn extension(&self) -> Option<&str> {
        let name = self.filename()?;
        // Ignore leading dot (hidden files like `.bashrc`)
        let search = if name.starts_with('.') { &name[1..] } else { name };
        match search.rfind('.') {
            Some(idx) => Some(&search[idx + 1..]),
            None => None,
        }
    }

    /// Strip a prefix from this path, returning the remainder.
    ///
    /// Used by the mount table to compute the relative path within a filesystem.
    pub fn strip_prefix(&self, prefix: &str) -> Option<Path> {
        let self_str = if self.case_insensitive {
            self.inner.to_ascii_lowercase()
        } else {
            self.inner.clone()
        };
        let prefix_str = if self.case_insensitive {
            String::from(prefix).to_ascii_lowercase()
        } else {
            String::from(prefix)
        };

        if self_str.starts_with(&prefix_str) {
            let remainder = &self.inner[prefix.len()..];
            let remainder = remainder.trim_start_matches('/');
            if remainder.is_empty() {
                Some(Path::new("/"))
            } else {
                let mut result = String::from("/");
                result.push_str(remainder);
                Some(Path {
                    inner: result,
                    case_insensitive: self.case_insensitive,
                })
            }
        } else {
            None
        }
    }

    /// Check equality respecting the case-sensitivity flag.
    pub fn equals(&self, other: &Path) -> bool {
        if self.case_insensitive || other.case_insensitive {
            self.inner.eq_ignore_ascii_case(&other.inner)
        } else {
            self.inner == other.inner
        }
    }
}

impl core::fmt::Display for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absolute_relative() {
        assert!(Path::new("/foo").is_absolute());
        assert!(Path::new("foo").is_relative());
    }

    #[test]
    fn test_components() {
        let p = Path::new("/foo/bar/baz");
        assert_eq!(p.components(), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_normalize() {
        let p = Path::new("/foo/./bar/../baz");
        assert_eq!(p.normalize().as_str(), "/foo/baz");
    }

    #[test]
    fn test_normalize_root_dotdot() {
        let p = Path::new("/../..");
        assert_eq!(p.normalize().as_str(), "/");
    }

    #[test]
    fn test_join_absolute_replaces() {
        let base = Path::new("/mnt/data");
        let joined = base.join("/etc/config");
        assert_eq!(joined.as_str(), "/etc/config");
    }

    #[test]
    fn test_join_relative() {
        let base = Path::new("/mnt/data");
        let joined = base.join("file.txt");
        assert_eq!(joined.as_str(), "/mnt/data/file.txt");
    }

    #[test]
    fn test_parent() {
        assert_eq!(Path::new("/foo/bar").parent().unwrap().as_str(), "/foo");
        assert_eq!(Path::new("/foo").parent().unwrap().as_str(), "/");
        assert!(Path::new("/").parent().is_none());
    }

    #[test]
    fn test_filename() {
        assert_eq!(Path::new("/foo/bar.txt").filename(), Some("bar.txt"));
        assert_eq!(Path::new("/").filename(), None);
    }

    #[test]
    fn test_extension() {
        assert_eq!(Path::new("/foo/bar.txt").extension(), Some("txt"));
        assert_eq!(Path::new("/foo/.bashrc").extension(), None);
        assert_eq!(Path::new("/foo/archive.tar.gz").extension(), Some("gz"));
    }

    #[test]
    fn test_strip_prefix() {
        let p = Path::new("/mnt/data/hello.txt");
        let rel = p.strip_prefix("/mnt/data").unwrap();
        assert_eq!(rel.as_str(), "/hello.txt");
    }

    #[test]
    fn test_case_insensitive() {
        let a = Path::with_case_sensitivity("/FOO/BAR", true);
        let b = Path::with_case_sensitivity("/foo/bar", true);
        assert!(a.equals(&b));
    }
}
