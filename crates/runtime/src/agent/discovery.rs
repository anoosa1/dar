//! CLI executable discovery (`next/runtime/agent/discovery.ts`).

use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
}

const X_OK: i32 = 1;

/// systemd does not inherit an interactive shell's PATH. Check standard system
/// profiles and the service user's installations, without scanning other homes
/// or selecting an arbitrary version from the Nix store.
pub fn codex_search_directories(environment_path: &str, service_home: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = environment_path
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect();
    candidates.extend([
        PathBuf::from("/run/current-system/sw/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        service_home.join(".nix-profile").join("bin"),
        service_home.join(".local").join("bin"),
    ]);

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for dir in candidates {
        if dir.is_absolute() && seen.insert(dir.clone()) {
            result.push(dir);
        }
    }
    result
}

/// Executable profile symlinks are normal on NixOS; their targets must be files.
pub async fn find_codex_executable(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    for directory in directories {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(name);
        let Ok(cstr) = CString::new(candidate.as_os_str().as_bytes()) else {
            continue;
        };
        let executable = unsafe { access(cstr.as_ptr(), X_OK) == 0 };
        if !executable {
            continue;
        }
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return Some(candidate);
        }
    }
    None
}
