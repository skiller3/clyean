// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Population and removal of the shared root filesystem under `.clyean/container-root`.
//! The image is exported and extracted inside Podman's user namespace so that file
//! ownership inside the sandbox is correct under rootless Podman.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::podman::Podman;
use crate::{Result, SandboxError};

pub const MARKER_FILE_NAME: &str = ".clyean-sandbox.json";

/// Recorded at the root of the sandbox filesystem once provisioning completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootfsMarker {
    pub image: String,
    pub image_digest: String,
    pub provisioning_version: u32,
    pub clyean_version: String,
    pub provisioned_at: String,
    pub harness_version: String,
}

impl RootfsMarker {
    pub fn read(root: &Path) -> Result<Option<Self>> {
        let path = root.join(MARKER_FILE_NAME);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| SandboxError::io(format!("reading {}", path.display()), e))?;
        serde_json::from_str(&text).map(Some).map_err(|e| {
            SandboxError::Invalid(format!("{} is not a valid marker: {e}", path.display()))
        })
    }

    pub fn write(&self, root: &Path) -> Result<()> {
        let path = root.join(MARKER_FILE_NAME);
        let mut text = serde_json::to_string_pretty(self).expect("marker serializes");
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|e| SandboxError::io(format!("writing {}", path.display()), e))
    }
}

/// Whether `root` holds an extracted filesystem (the marker may not exist yet).
pub fn is_populated(root: &Path) -> bool {
    root.join("etc").is_dir() && (root.join("bin").exists() || root.join("usr").is_dir())
}

/// Pulls `image`, exports a container created from it, and extracts the archive into
/// `root` inside Podman's user namespace.  Returns the image digest.
pub fn populate_from_image(podman: &Podman, image: &str, root: &Path) -> Result<String> {
    std::fs::create_dir_all(root)
        .map_err(|e| SandboxError::io(format!("creating {}", root.display()), e))?;
    if is_populated(root) {
        return Err(SandboxError::Invalid(format!(
            "{} already holds a root filesystem; run `clyean sandbox rebuild` to replace it",
            root.display()
        )));
    }
    podman.output(["pull", "--quiet", image])?;
    let digest = podman.output(["image", "inspect", "--format", "{{.Digest}}", image])?;
    let container_id = podman.output(["create", image])?.trim().to_string();
    let archive = tempfile::Builder::new()
        .prefix("clyean-rootfs-")
        .suffix(".tar")
        .tempfile()
        .map_err(|e| SandboxError::io("creating a temporary archive", e))?;
    let export = podman.output([
        "export",
        "--output",
        &archive.path().to_string_lossy(),
        &container_id,
    ]);
    podman.remove_container(&container_id)?;
    export?;
    podman.output([
        "unshare",
        "tar",
        "--extract",
        "--preserve-permissions",
        "--numeric-owner",
        "--file",
        &archive.path().to_string_lossy(),
        "--directory",
        &root.to_string_lossy(),
    ])?;
    Ok(digest.trim().to_string())
}

/// Removes the root filesystem, again inside the user namespace, because files created
/// by container users other than root are owned by subordinate UIDs on the host.
pub fn remove(podman: &Podman, root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    podman.output(["unshare", "rm", "-rf", &root.to_string_lossy()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(RootfsMarker::read(dir.path()).unwrap().is_none());
        let marker = RootfsMarker {
            image: "docker.io/library/ubuntu:latest".into(),
            image_digest: "sha256:abc".into(),
            provisioning_version: 1,
            clyean_version: "0.1.0".into(),
            provisioned_at: "2026-09-21T00:00:00Z".into(),
            harness_version: "18.2.7".into(),
        };
        marker.write(dir.path()).unwrap();
        assert_eq!(RootfsMarker::read(dir.path()).unwrap(), Some(marker));
    }

    #[test]
    fn populated_detection_requires_a_filesystem_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_populated(dir.path()));
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        std::fs::create_dir_all(dir.path().join("usr")).unwrap();
        assert!(is_populated(dir.path()));
    }
}
