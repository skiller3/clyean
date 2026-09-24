// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Where sandbox root filesystems live and the helper containers that create, populate,
//! list, and remove them.  Clyean keeps its roots directory beside Podman's own data, on a
//! Linux filesystem Podman already uses (inside the Podman machine on native Windows and
//! macOS), and only Clyean adds or removes anything in it.  Every operation that needs
//! container privileges runs in a helper container, so no path depends on `podman
//! unshare` or on the roots directory being reachable from this host.

use std::process::{Command, Stdio};

use clyean_project::SandboxId;
use serde::Deserialize;

use crate::environment::Topology;
use crate::fs::{HostDirectoryFs, PodmanHostFs, SandboxFs};
use crate::podman::Podman;
use crate::{Result, SandboxError};

/// A small image for helper containers, pinned by the digest of its multi-architecture
/// index, because the configured sandbox image might lack the tools helpers need.
pub const HELPER_IMAGE: &str =
    "docker.io/library/alpine@sha256:5291449c3df73caf6ed85e649dec1b9e818b39a5d8c871e97afc13e9cd5e8fa8";

/// The label every container running on a sandbox root filesystem carries.
pub const SANDBOX_LABEL: &str = "clyean.sandbox";

const ANCHOR_MOUNT: &str = "/clyean-anchor";
const ROOTS_MOUNT: &str = "/clyean-roots";

/// The directory that holds Clyean's `roots` directory on the Podman host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRoots {
    anchor: String,
}

impl SandboxRoots {
    /// `clyean` inside the data directory that holds Podman's `containers` directory, or,
    /// for a customized graph root, inside the directory that contains it.
    pub fn beside_graph_root(graph_root: &str) -> Self {
        let graph_root = graph_root.trim_end_matches('/');
        let anchor = graph_root
            .strip_suffix("/containers/storage")
            .unwrap_or_else(|| graph_root.rsplit_once('/').map_or("", |(parent, _)| parent));
        Self {
            anchor: if anchor.is_empty() {
                "/".to_string()
            } else {
                anchor.to_string()
            },
        }
    }

    pub fn anchor(&self) -> &str {
        &self.anchor
    }

    pub fn roots_dir(&self) -> String {
        join(&self.anchor, "clyean/roots")
    }

    pub fn location(&self, id: SandboxId, topology: Topology) -> SandboxLocation {
        SandboxLocation {
            root: join(&self.roots_dir(), id.as_str()),
            id,
            topology,
        }
    }
}

fn join(base: &str, relative: &str) -> String {
    format!("{}/{relative}", base.trim_end_matches('/'))
}

/// One project's sandbox root filesystem on the Podman host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLocation {
    pub id: SandboxId,
    /// The root filesystem's path on the Podman host, which `podman run --rootfs` takes.
    pub root: String,
    pub topology: Topology,
}

impl SandboxLocation {
    /// The label that marks a container as running on this root filesystem.
    pub fn label(&self) -> (String, String) {
        (SANDBOX_LABEL.to_string(), self.id.to_string())
    }

    /// File access to the root filesystem: direct on this host's kernel, through
    /// containers on a Podman machine.
    pub fn fs(&self, podman: &Podman) -> Box<dyn SandboxFs> {
        match self.topology {
            #[cfg(unix)]
            Topology::SharedKernel => Box::new(HostDirectoryFs::new(&self.root)),
            _ => Box::new(PodmanHostFs::new(podman.clone(), self)),
        }
    }
}

/// Runs helper containers against the roots directory of one Podman host.
#[derive(Debug, Clone)]
pub struct Helpers {
    podman: Podman,
    roots: SandboxRoots,
}

impl Helpers {
    pub fn new(podman: Podman, roots: SandboxRoots) -> Self {
        Self { podman, roots }
    }

    pub fn roots(&self) -> &SandboxRoots {
        &self.roots
    }

    fn ensure_image(&self) -> Result<()> {
        if self
            .podman
            .command(["image", "exists", HELPER_IMAGE])
            .status()
            .map_err(SandboxError::PodmanMissing)?
            .success()
        {
            return Ok(());
        }
        self.podman.output(["pull", "--quiet", HELPER_IMAGE])?;
        Ok(())
    }

    fn run_args(&self, mount: String, script: &str, args: &[&str]) -> Vec<String> {
        let mut run = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--interactive".to_string(),
            "--network".to_string(),
            "none".to_string(),
            "--security-opt".to_string(),
            "label=disable".to_string(),
            "--volume".to_string(),
            mount,
            HELPER_IMAGE.to_string(),
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
            "sh".to_string(),
        ];
        run.extend(args.iter().map(|a| a.to_string()));
        run
    }

    /// Runs `script` with the roots directory mounted, creating it when it is missing.
    fn in_roots(&self, script: &str, args: &[&str]) -> Result<String> {
        self.ensure_directories()?;
        self.ensure_image()?;
        self.podman.output(self.run_args(
            format!("{}:{ROOTS_MOUNT}", self.roots.roots_dir()),
            script,
            args,
        ))
    }

    /// Creates the roots directory beside Podman's data.
    pub fn ensure_directories(&self) -> Result<()> {
        self.ensure_image()?;
        self.podman
            .output(self.run_args(
                format!("{}:{ANCHOR_MOUNT}", self.roots.anchor()),
                &format!("mkdir -p {ANCHOR_MOUNT}/clyean/roots"),
                &[],
            ))
            .map_err(|error| {
                SandboxError::Invalid(format!(
                    "Clyean cannot create its sandbox directory {} on the Podman host: {error}",
                    self.roots.roots_dir()
                ))
            })?;
        Ok(())
    }

    /// Whether the root filesystem `id` holds an extracted image.
    pub fn is_populated(&self, id: &SandboxId) -> Result<bool> {
        let answer = self.in_roots(
            "if [ -d \"$1/etc\" ]; then echo yes; else echo no; fi",
            &[&format!("{ROOTS_MOUNT}/{id}")],
        )?;
        Ok(answer.trim() == "yes")
    }

    /// Pulls `image` and extracts it into the root filesystem `id`, streaming `podman
    /// export` into a helper that unpacks it.  Returns the image's digest.
    pub fn populate(&self, id: &SandboxId, image: &str) -> Result<String> {
        if self.is_populated(id)? {
            return Err(SandboxError::Invalid(format!(
                "{} already holds a root filesystem; run `clyean sandbox rebuild` to replace it",
                join(&self.roots.roots_dir(), id.as_str())
            )));
        }
        self.podman.output(["pull", "--quiet", image])?;
        let digest = self
            .podman
            .output(["image", "inspect", "--format", "{{.Digest}}", image])?;
        let container = self.podman.output(["create", image])?.trim().to_string();
        let extracted = self.extract(id, &container);
        self.podman.remove_container(&container)?;
        extracted?;
        Ok(digest.trim().to_string())
    }

    fn extract(&self, id: &SandboxId, container: &str) -> Result<()> {
        let mut export = self
            .podman
            .command(["export", container])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(SandboxError::PodmanMissing)?;
        let stream = export.stdout.take().expect("stdout is piped");
        let target = format!("{ROOTS_MOUNT}/{id}");
        let args = self.run_args(
            format!("{}:{ROOTS_MOUNT}", self.roots.roots_dir()),
            "mkdir -p \"$1\" && tar -x -f - -C \"$1\" --numeric-owner",
            &[&target],
        );
        let unpacked = Command::new(self.podman.binary())
            .args(&args)
            .stdin(Stdio::from(stream))
            .stdout(Stdio::null())
            .output()
            .map_err(SandboxError::PodmanMissing)?;
        let exported = export
            .wait_with_output()
            .map_err(SandboxError::PodmanMissing)?;
        for (step, status, stderr) in [
            ("export", exported.status, exported.stderr),
            ("extracting the image", unpacked.status, unpacked.stderr),
        ] {
            if !status.success() {
                return Err(SandboxError::PodmanFailed {
                    command: step.to_string(),
                    stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
                });
            }
        }
        Ok(())
    }

    /// Removes the root filesystem `id`, whose files belong to container users.
    pub fn remove(&self, id: &SandboxId) -> Result<()> {
        self.in_roots("rm -rf -- \"$1\"", &[&format!("{ROOTS_MOUNT}/{id}")])?;
        Ok(())
    }

    /// Verifies that the Podman host sees every bind-mount source in `sources`, naming the
    /// ones it does not.
    pub fn check_visible(&self, sources: &[String]) -> Result<()> {
        self.ensure_image()?;
        let visible = |sources: &[String]| {
            let mut args = vec![
                "run".to_string(),
                "--rm".to_string(),
                "--network".to_string(),
                "none".to_string(),
                "--security-opt".to_string(),
                "label=disable".to_string(),
            ];
            for (index, source) in sources.iter().enumerate() {
                args.push("--volume".to_string());
                args.push(format!("{source}:/clyean-check/{index}:ro"));
            }
            args.extend([HELPER_IMAGE.to_string(), "true".to_string()]);
            self.podman.output(args).is_ok()
        };
        if visible(sources) {
            return Ok(());
        }
        let missing: Vec<&str> = sources
            .iter()
            .filter(|source| !visible(std::slice::from_ref(source)))
            .map(String::as_str)
            .collect();
        Err(SandboxError::Invalid(format!(
            "the Podman machine cannot see {}; on macOS, directories outside the machine's shared directories need a machine volume (`podman machine init --volume <dir>:<dir>`), and on Windows, the drive must be mounted in the machine",
            missing.join(", ")
        )))
    }

    /// Every root filesystem in the roots directory, with its marker, age, and size.
    pub fn list(&self) -> Result<Vec<StoredRoot>> {
        let script = "cd /clyean-roots || exit 0\n\
            now=$(date +%s)\n\
            for dir in */; do\n\
              [ -d \"$dir\" ] || continue\n\
              id=${dir%/}\n\
              size=$(du -sk \"$id\" 2>/dev/null | cut -f1)\n\
              modified=$(stat -c %Y \"$id\")\n\
              marker=$(cat \"$id/.clyean-sandbox.json\" 2>/dev/null | tr -d '\\n')\n\
              printf '{\"id\":\"%s\",\"sizeKib\":%s,\"ageSeconds\":%s,\"marker\":%s}\\n' \"$id\" \"${size:-0}\" \"$((now - modified))\" \"${marker:-null}\"\n\
            done\n";
        let listing = self.in_roots(script, &[])?;
        Ok(listing
            .lines()
            .filter_map(|line| serde_json::from_str::<StoredRoot>(line).ok())
            .collect())
    }
}

/// One entry of the roots directory as a helper container reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRoot {
    pub id: String,
    pub size_kib: u64,
    pub age_seconds: u64,
    /// The provisioning marker, absent while a population is incomplete.
    pub marker: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_sit_beside_podmans_data_directory() {
        let cases = [
            (
                "/home/skye/.local/share/containers/storage",
                "/home/skye/.local/share/clyean/roots",
            ),
            ("/var/lib/containers/storage/", "/var/lib/clyean/roots"),
            ("/srv/podman", "/srv/clyean/roots"),
            ("/podman", "/clyean/roots"),
        ];
        for (graph_root, roots_dir) in cases {
            assert_eq!(
                SandboxRoots::beside_graph_root(graph_root).roots_dir(),
                roots_dir,
                "{graph_root}"
            );
        }
        let roots =
            SandboxRoots::beside_graph_root("/var/home/core/.local/share/containers/storage");
        let id = SandboxId::try_from("3f9c2a7d1e4b8c05".to_string()).unwrap();
        let location = roots.location(id, Topology::VirtualMachine);
        assert_eq!(
            location.root,
            "/var/home/core/.local/share/clyean/roots/3f9c2a7d1e4b8c05"
        );
        assert_eq!(
            location.label(),
            ("clyean.sandbox".to_string(), "3f9c2a7d1e4b8c05".to_string())
        );
    }

    #[test]
    fn listed_roots_parse_with_and_without_a_marker() {
        let line = r#"{"id":"3f9c2a7d1e4b8c05","sizeKib":1024,"ageSeconds":90000,"marker":null}"#;
        let root: StoredRoot = serde_json::from_str(line).unwrap();
        assert_eq!(root.marker, None);
        let line = r#"{"id":"3f9c2a7d1e4b8c05","sizeKib":1,"ageSeconds":1,"marker":{"sandboxId":"3f9c2a7d1e4b8c05"}}"#;
        let root: StoredRoot = serde_json::from_str(line).unwrap();
        assert_eq!(root.marker.unwrap()["sandboxId"], "3f9c2a7d1e4b8c05");
    }
}
