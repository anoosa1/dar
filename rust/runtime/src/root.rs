//! The storage root (`root.ts`), its single-runtime lock (`lock.ts`) and durable private
//! files (`atomic.ts`).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

fn other(message: &str) -> io::Error {
    io::Error::other(message.to_owned())
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

fn temporary(filename: &Path) -> PathBuf {
    let name = filename.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    filename.with_file_name(format!("{name}.{}.tmp", uuid()))
}

/// A random v4 UUID.
pub fn uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("randomness");
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Publishes a new private file; never replaces an existing one or follows a link.
pub fn create_durable_file(filename: &Path, content: &[u8]) -> io::Result<()> {
    let staged = temporary(filename);
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc_nofollow()).open(&staged)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&staged, filename)?;
        sync_directory(filename.parent().unwrap_or(Path::new(".")))
    })();
    let _ = fs::remove_file(&staged);
    result
}

/// Atomically replaces a private regular file.
pub fn replace_durable_file(filename: &Path, content: &[u8]) -> io::Result<()> {
    let old = fs::symlink_metadata(filename)?;
    if !old.is_file() || old.mode() & 0o077 != 0 {
        return Err(other("Unsafe private state file."));
    }
    let staged = temporary(filename);
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc_nofollow()).open(&staged)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staged, filename)?;
        sync_directory(filename.parent().unwrap_or(Path::new(".")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// Creates or replaces a private file.
pub fn write_durable_file(filename: &Path, content: &[u8]) -> io::Result<()> {
    match fs::symlink_metadata(filename) {
        Ok(_) => replace_durable_file(filename, content),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_durable_file(filename, content),
        Err(e) => Err(e),
    }
}

const fn libc_nofollow() -> i32 {
    0o400000 // O_NOFOLLOW on Linux
}

/// Resolves the root (it cannot be a link), marks it and creates the managed directories.
pub fn open_root(root: &Path) -> io::Result<PathBuf> {
    let root = std::path::absolute(root)?;
    let parent = root.parent().ok_or_else(|| other("Invalid storage root."))?;
    fs::create_dir_all(parent)?;
    let root = fs::canonicalize(parent)?.join(root.file_name().ok_or_else(|| other("Invalid storage root."))?);
    match fs::symlink_metadata(&root) {
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err(other("Managed storage root cannot be a symlink or non-directory.")),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::DirBuilder::new().mode(0o700).create(&root)?,
        Err(e) => return Err(e),
    }
    let marker = root.join("instance.json");
    match fs::symlink_metadata(&marker) {
        Ok(m) => {
            let format = fs::read_to_string(&marker).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["format"].as_str().map(str::to_owned));
            if !m.is_file() || format.as_deref() != Some("dar-spacetime-v1") {
                return Err(other("Unsupported storage root."));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if fs::read_dir(&root)?.next().is_some() {
                return Err(other("Refusing an unknown non-empty storage root."));
            }
            create_durable_file(&marker, br#"{"format":"dar-spacetime-v1"}"#)?;
        }
        Err(e) => return Err(e),
    }
    for name in ["secrets", "blobs", "uploads", "agent-workspaces", "photo-cache", "book-cache", "git"] {
        let directory = root.join(name);
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let m = fs::symlink_metadata(&directory)?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(other("Unsafe managed storage directory."));
        }
    }
    Ok(root)
}

use std::os::unix::fs::DirBuilderExt;

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned()).unwrap_or_default()
}

/// The root's single-runtime lock (`runtime.lock`); released when dropped.
pub struct RootLock(PathBuf);

impl RootLock {
    pub fn acquire(root: &Path) -> io::Result<Self> {
        let filename = root.join("runtime.lock");
        match fs::symlink_metadata(&filename) {
            Ok(m) => {
                if !m.is_file() {
                    return Err(other("Unsafe runtime lock."));
                }
                let record: serde_json::Value = serde_json::from_str(&fs::read_to_string(&filename)?).map_err(|_| other("Unrecognized runtime lock."))?;
                let pid = record["pid"].as_u64().filter(|p| *p > 0).ok_or_else(|| other("Unrecognized runtime lock."))?;
                if record["host"].as_str() != Some(hostname().as_str()) {
                    return Err(other("Unrecognized runtime lock."));
                }
                if Path::new(&format!("/proc/{pid}")).exists() {
                    return Err(other("Another runtime owns this storage root."));
                }
                fs::remove_file(&filename)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        create_durable_file(&filename, serde_json::json!({ "pid": std::process::id(), "host": hostname() }).to_string().as_bytes())?;
        Ok(Self(filename))
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_marked_and_locked_once() {
        let base = std::env::temp_dir().join(format!("dar-root-{}", uuid()));
        let root = open_root(&base.join("state")).unwrap();
        assert!(root.join("instance.json").is_file() && root.join("blobs").is_dir());
        let lock = RootLock::acquire(&root).unwrap();
        assert!(RootLock::acquire(&root).is_err());
        drop(lock);
        assert!(!root.join("runtime.lock").exists());
        // A stale lock from a stopped process is replaced.
        create_durable_file(&root.join("runtime.lock"), serde_json::json!({ "pid": 999_999_999u64, "host": hostname() }).to_string().as_bytes()).unwrap();
        drop(RootLock::acquire(&root).unwrap());
        fs::write(base.join("other"), "x").unwrap();
        assert!(open_root(&base).is_err());
        fs::remove_dir_all(&base).unwrap();
    }
}
