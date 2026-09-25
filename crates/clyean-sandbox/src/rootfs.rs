// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The provisioning marker at the top of a sandbox root filesystem: what populated and
//! provisioned it, and which project used it last.

use std::path::Path;

use serde::{Deserialize, Serialize};

use clyean_project::SandboxId;

use crate::fs::{SandboxArchive, SandboxFs};
use crate::roots::Helpers;
use crate::{Result, SandboxError};

pub const MARKER_PATH: &str = "/.clyean-sandbox.json";

/// Recorded at the root of the sandbox filesystem once provisioning completed, and
/// updated by every launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootfsMarker {
    pub sandbox_id: String,
    pub image: String,
    pub image_digest: String,
    pub provisioning_version: u32,
    pub clyean_version: String,
    pub provisioned_at: String,
    pub harness_version: String,
    /// The host path of the project that used the root filesystem last.
    pub project_dir: String,
    pub last_used_at: String,
}

impl RootfsMarker {
    pub fn read(fs: &dyn SandboxFs) -> Result<Option<Self>> {
        let Some(bytes) = fs.read_file(MARKER_PATH)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            SandboxError::Invalid(format!("{MARKER_PATH} is not a valid sandbox marker: {e}"))
        })
    }

    /// The marker of the root filesystem `id`, which may not exist yet: on a Podman
    /// machine, reading runs a container on the root filesystem, which fails without one.
    pub fn read_existing(
        fs: &dyn SandboxFs,
        helpers: &Helpers,
        id: &SandboxId,
    ) -> Result<Option<Self>> {
        match Self::read(fs) {
            Ok(marker) => Ok(marker),
            Err(error) if helpers.is_populated(id)? => Err(error),
            Err(_) => Ok(None),
        }
    }

    /// The marker after a use by the project at `project_dir`.
    pub fn used_by(mut self, project_dir: &Path) -> Self {
        self.project_dir = project_dir.to_string_lossy().into_owned();
        self.last_used_at = clyean_project::utc_now_rfc3339();
        self
    }

    pub fn add_to(&self, archive: &mut SandboxArchive) {
        let mut text = serde_json::to_string_pretty(self).expect("marker serializes");
        text.push('\n');
        archive.file(MARKER_PATH, text, 0o644);
    }

    pub fn write(&self, fs: &dyn SandboxFs) -> Result<()> {
        let mut archive = SandboxArchive::new();
        self.add_to(&mut archive);
        fs.write(archive)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::fs::HostDirectoryFs;

    #[test]
    fn marker_round_trips_and_records_its_last_use() {
        let dir = tempfile::tempdir().unwrap();
        let fs = HostDirectoryFs::new(dir.path());
        assert!(RootfsMarker::read(&fs).unwrap().is_none());
        let marker = RootfsMarker {
            sandbox_id: "3f9c2a7d1e4b8c05".into(),
            image: "docker.io/library/ubuntu:latest".into(),
            image_digest: "sha256:abc".into(),
            provisioning_version: 1,
            clyean_version: "0.1.0".into(),
            provisioned_at: "2026-09-21T00:00:00Z".into(),
            harness_version: "18.2.7".into(),
            project_dir: "/home/skye/old".into(),
            last_used_at: "2026-09-21T00:00:00Z".into(),
        };
        let used = marker.clone().used_by(Path::new("/home/skye/app"));
        assert_eq!(used.project_dir, "/home/skye/app");
        assert_ne!(used.last_used_at, marker.last_used_at);
        used.write(&fs).unwrap();
        assert_eq!(RootfsMarker::read(&fs).unwrap(), Some(used));
    }
}
