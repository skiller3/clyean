// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::layout::ProjectLayout;
use crate::{ProjectError, Result};

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

/// The identifier of a project's sandbox root filesystem: sixteen random hexadecimal
/// digits, kept in the local-only `.clyean/sandbox.local.json` so that the sandbox moves
/// with the project directory and every clone gets its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxId(String);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxIdentityFile {
    sandbox_id: SandboxId,
}

impl SandboxId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string()[..16].to_string())
    }

    /// The identifier recorded in the project, if any.
    pub fn load(layout: &ProjectLayout) -> Result<Option<Self>> {
        let path = layout.sandbox_identity_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ProjectError::io(
                    format!("reading {}", path.display()),
                    error,
                ))
            }
        };
        let file: SandboxIdentityFile =
            serde_json::from_str(&text).map_err(|source| ProjectError::Json { path, source })?;
        Ok(Some(file.sandbox_id))
    }

    /// The identifier recorded in the project, recording a new one when there is none.
    pub fn load_or_create(layout: &ProjectLayout) -> Result<Self> {
        if let Some(id) = Self::load(layout)? {
            return Ok(id);
        }
        let id = Self::generate();
        let path = layout.sandbox_identity_path();
        std::fs::create_dir_all(layout.clyean_dir())
            .map_err(|e| ProjectError::io("creating .clyean", e))?;
        let mut text = serde_json::to_string_pretty(&SandboxIdentityFile {
            sandbox_id: id.clone(),
        })
        .expect("the identity serializes");
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|e| ProjectError::io(format!("writing {}", path.display()), e))?;
        Ok(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, String> {
        let valid = value.len() == 16
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if valid {
            Ok(Self(value))
        } else {
            Err(format!(
                "{value:?} is not a sandbox identifier (sixteen lower-case hexadecimal digits)"
            ))
        }
    }
}

impl From<SandboxId> for String {
    fn from(id: SandboxId) -> Self {
        id.0
    }
}

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_identifier_is_recorded_once_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        assert_eq!(SandboxId::load(&layout).unwrap(), None);
        let created = SandboxId::load_or_create(&layout).unwrap();
        assert_eq!(created.as_str().len(), 16);
        assert_eq!(SandboxId::load_or_create(&layout).unwrap(), created);
        let text = std::fs::read_to_string(layout.sandbox_identity_path()).unwrap();
        assert_eq!(text, format!("{{\n  \"sandboxId\": \"{created}\"\n}}\n"));
        assert_ne!(SandboxId::generate(), created);
        assert!(SandboxId::try_from("3F9C2A7D1E4B8C05".to_string()).is_err());
        assert!(SandboxId::try_from("../../etc".to_string()).is_err());
    }

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
