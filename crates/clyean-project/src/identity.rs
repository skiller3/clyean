// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// A short, stable identifier of a project derived from its canonical path.  Used to
/// name containers and runtime sockets, which both have tight length limits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn of(project_dir: &Path) -> Self {
        let digest = Sha256::digest(project_dir.to_string_lossy().as_bytes());
        Self(hex::encode(&digest[..6]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Per-user runtime directory for sockets.  Kept short because `AF_UNIX` socket paths
/// are limited to roughly one hundred bytes.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("clyean");
    }
    let uid = current_user_id();
    std::env::temp_dir().join(format!("clyean-{uid}"))
}

/// Host path of the orchestrator socket of a project.
pub fn orchestrator_socket_path(project_id: &ProjectId) -> PathBuf {
    runtime_dir().join(format!("{project_id}.sock"))
}

#[cfg(unix)]
fn current_user_id() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc_getuid() }
}

#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    getuid()
}

#[cfg(not(unix))]
fn current_user_id() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_twelve_hex_characters_and_stable() {
        let id = ProjectId::of(Path::new("/home/someone/workspace/example"));
        assert_eq!(id.as_str().len(), 12);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            id,
            ProjectId::of(Path::new("/home/someone/workspace/example"))
        );
        assert_ne!(
            id,
            ProjectId::of(Path::new("/home/someone/workspace/other"))
        );
    }

    #[test]
    fn socket_path_lives_in_the_runtime_directory() {
        let id = ProjectId::of(Path::new("/x"));
        let path = orchestrator_socket_path(&id);
        assert!(path.starts_with(runtime_dir()));
        assert!(path.to_string_lossy().ends_with(&format!("{id}.sock")));
    }
}
