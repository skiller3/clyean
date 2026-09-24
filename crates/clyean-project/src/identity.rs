// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::Path;

use sha2::{Digest, Sha256};

/// A short, stable identifier of a project derived from its canonical path.  Used to
/// name containers, whose names should stay short.
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

/// The identifier of one `clyean` invocation, which names its User Assistant container.
/// Eight hexadecimal characters taken from the random bits of a version 7 UUID, so two
/// concurrent invocations of one project never share a container name in practice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchId(String);

impl LaunchId {
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::now_v7().simple().to_string();
        Self(uuid[uuid.len() - 8..].to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LaunchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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
    fn launch_ids_are_eight_hex_characters_and_distinct() {
        let first = LaunchId::generate();
        let second = LaunchId::generate();
        assert_eq!(first.as_str().len(), 8);
        assert!(first.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
