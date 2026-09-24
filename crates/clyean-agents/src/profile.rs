// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Projection of an agent's tracked configuration into its harness profile directory
//! inside the sandbox root filesystem: `/home/<user>/.omp/profiles/<agent-id>/agent/`.

use std::path::{Path, PathBuf};

use clyean_project::local_overlay::deep_merge;
use clyean_project::ProjectLayout;
use serde_json::Value;

use crate::instructions::effective_instructions;
use crate::roster::AgentId;
use crate::settings::{
    effective_mcp_seed, effective_settings_overlay, read_json_if_present, write_json,
};
use crate::{AgentError, Result};

pub const OVERLAY_FILE_NAME: &str = "clyean-overlay.json";
pub const MCP_FILE_NAME: &str = "mcp.json";
pub const AGENTS_FILE_NAME: &str = "AGENTS.md";
pub const EXTENSIONS_DIR_NAME: &str = "extensions";
pub const EXTENSION_VERSION_MARKER: &str = "// CLYEAN_EXTENSION_VERSION=";

/// A Clyean-managed harness extension shipped into the User Assistant's profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedExtension {
    pub file_name: &'static str,
    pub source: &'static str,
}

impl ManagedExtension {
    pub fn version(&self) -> Option<u32> {
        extension_version(self.source)
    }
}

fn extension_version(source: &str) -> Option<u32> {
    source
        .lines()
        .take(16)
        .find_map(|line| line.trim().strip_prefix(EXTENSION_VERSION_MARKER))
        .and_then(|value| value.trim().parse().ok())
}

/// Container-side root of an agent's harness profile (`HOME` is `/home/<user>`).
pub fn container_profile_root(user: &str, agent: AgentId) -> String {
    format!("/home/{user}/.omp/profiles/{}", agent.id())
}

/// Container-side profile directory of an agent, where its settings and stores live.
pub fn container_profile_dir(user: &str, agent: AgentId) -> String {
    format!("{}/agent", container_profile_root(user, agent))
}

/// The directories where the harness keeps the sockets of its per-profile daemons.  Each
/// container gets private copies, so no container reaches another's daemons through the
/// shared root filesystem.
pub fn container_daemon_dirs(user: &str, agent: AgentId) -> [String; 2] {
    [
        format!("/home/{user}/.omp/run"),
        format!("{}/run", container_profile_root(user, agent)),
    ]
}

pub fn container_overlay_path(user: &str, agent: AgentId) -> String {
    format!("{}/{OVERLAY_FILE_NAME}", container_profile_dir(user, agent))
}

/// Host-side path of the same profile directory inside the root filesystem.
pub fn host_profile_dir(container_root: &Path, user: &str, agent: AgentId) -> PathBuf {
    container_root
        .join("home")
        .join(user)
        .join(".omp")
        .join("profiles")
        .join(agent.id())
        .join("agent")
}

/// Everything Clyean writes into one agent's profile before launching it.
#[derive(Debug, Clone)]
pub struct ProfileProjection {
    pub agent: AgentId,
    pub agents_md: String,
    pub settings_overlay: Value,
    pub mcp_seed: Option<Value>,
    pub extensions: Vec<ManagedExtension>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProfileWriteReport {
    pub written: Vec<PathBuf>,
    pub extensions_refreshed: Vec<&'static str>,
}

impl ProfileProjection {
    pub fn for_agent(
        layout: &ProjectLayout,
        agent: AgentId,
        extensions: Vec<ManagedExtension>,
    ) -> Result<Self> {
        Ok(Self {
            agent,
            agents_md: effective_instructions(layout, agent)?,
            settings_overlay: effective_settings_overlay(layout, agent)?,
            mcp_seed: effective_mcp_seed(layout, agent)?,
            extensions,
        })
    }

    /// Writes the projection into the root filesystem.  `AGENTS.md` and the overlay are
    /// always replaced; `mcp.json` keeps servers the user added through the harness and
    /// layers the tracked seed on top; extensions are refreshed only when their version
    /// marker is behind the embedded one.
    pub fn write(&self, container_root: &Path, user: &str) -> Result<ProfileWriteReport> {
        let profile_dir = host_profile_dir(container_root, user, self.agent);
        std::fs::create_dir_all(&profile_dir)
            .map_err(|e| AgentError::io(format!("creating {}", profile_dir.display()), e))?;
        let mut report = ProfileWriteReport::default();

        let agents_path = profile_dir.join(AGENTS_FILE_NAME);
        std::fs::write(&agents_path, &self.agents_md)
            .map_err(|e| AgentError::io(format!("writing {}", agents_path.display()), e))?;
        report.written.push(agents_path);

        let overlay_path = profile_dir.join(OVERLAY_FILE_NAME);
        write_json(&overlay_path, &self.settings_overlay)?;
        report.written.push(overlay_path);

        if let Some(seed) = &self.mcp_seed {
            let mcp_path = profile_dir.join(MCP_FILE_NAME);
            let existing =
                read_json_if_present(&mcp_path)?.unwrap_or(Value::Object(Default::default()));
            write_json(&mcp_path, &deep_merge(existing, seed.clone()))?;
            report.written.push(mcp_path);
        }

        let extensions_dir = profile_dir.join(EXTENSIONS_DIR_NAME);
        for extension in &self.extensions {
            let path = extensions_dir.join(extension.file_name);
            if extension_is_current(&path, extension) {
                continue;
            }
            std::fs::create_dir_all(&extensions_dir)
                .map_err(|e| AgentError::io(format!("creating {}", extensions_dir.display()), e))?;
            std::fs::write(&path, extension.source)
                .map_err(|e| AgentError::io(format!("writing {}", path.display()), e))?;
            report.extensions_refreshed.push(extension.file_name);
            report.written.push(path);
        }
        Ok(report)
    }
}

/// Files that make up the harness's credential store inside a profile.
pub const CREDENTIAL_STORE_FILES: [&str; 3] = ["agent.db", "agent.db-wal", "agent.db-shm"];

/// Copies the credential store of `from` into the profile of `to`, replacing what is there.
/// Returns whether a store existed to copy.
pub fn inherit_credentials(
    container_root: &Path,
    user: &str,
    from: AgentId,
    to: AgentId,
) -> Result<bool> {
    let source_dir = host_profile_dir(container_root, user, from);
    if !source_dir.join(CREDENTIAL_STORE_FILES[0]).is_file() {
        return Ok(false);
    }
    let target_dir = host_profile_dir(container_root, user, to);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| AgentError::io(format!("creating {}", target_dir.display()), e))?;
    for file_name in CREDENTIAL_STORE_FILES {
        let source = source_dir.join(file_name);
        let target = target_dir.join(file_name);
        if source.is_file() {
            std::fs::copy(&source, &target).map_err(|e| {
                AgentError::io(
                    format!("copying {} to {}", source.display(), target.display()),
                    e,
                )
            })?;
        } else if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|e| AgentError::io(format!("removing stale {}", target.display()), e))?;
        }
    }
    Ok(true)
}

fn extension_is_current(path: &Path, extension: &ManagedExtension) -> bool {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return false;
    };
    match (extension_version(&existing), extension.version()) {
        (Some(installed), Some(embedded)) => installed >= embedded,
        _ => existing == extension.source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instructions::write_baseline_if_missing;
    use crate::settings::write_seeds_if_missing;
    use serde_json::json;

    const EXTENSION_V1: &str =
        "// managed by clyean\n// CLYEAN_EXTENSION_VERSION=1\nexport default function () {}\n";
    const EXTENSION_V2: &str =
        "// managed by clyean\n// CLYEAN_EXTENSION_VERSION=2\nexport default function () {}\n";

    fn scaffolded_layout(dir: &Path) -> ProjectLayout {
        let layout = ProjectLayout::new(dir);
        write_baseline_if_missing(&layout, AgentId::UserAssistant).unwrap();
        write_seeds_if_missing(&layout, AgentId::UserAssistant).unwrap();
        layout
    }

    #[test]
    fn container_paths_follow_the_sandbox_contract() {
        assert_eq!(
            container_profile_dir("skye", AgentId::Programmer),
            "/home/skye/.omp/profiles/programmer/agent"
        );
        assert_eq!(
            container_overlay_path("skye", AgentId::UserAssistant),
            "/home/skye/.omp/profiles/user-assistant/agent/clyean-overlay.json"
        );
    }

    #[test]
    fn projection_writes_profile_files_and_preserves_user_added_mcp_servers() {
        let project = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let layout = scaffolded_layout(project.path());
        std::fs::write(
            layout.agents_dir().join("USER_ASSISTANT.mcp.json"),
            r#"{"mcpServers": {"tracked": {"type": "stdio", "command": "x"}}}"#,
        )
        .unwrap();
        let profile_dir = host_profile_dir(root.path(), "skye", AgentId::UserAssistant);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(
            profile_dir.join(MCP_FILE_NAME),
            r#"{"mcpServers": {"added-by-user": {"type": "stdio", "command": "y"}}}"#,
        )
        .unwrap();

        let projection = ProfileProjection::for_agent(
            &layout,
            AgentId::UserAssistant,
            vec![ManagedExtension {
                file_name: "clyean-test.ts",
                source: EXTENSION_V1,
            }],
        )
        .unwrap();
        let report = projection.write(root.path(), "skye").unwrap();
        assert_eq!(report.extensions_refreshed, vec!["clyean-test.ts"]);

        let agents_md = std::fs::read_to_string(profile_dir.join(AGENTS_FILE_NAME)).unwrap();
        assert!(agents_md.contains("# User Assistant"));
        let mcp: Value = serde_json::from_str(
            &std::fs::read_to_string(profile_dir.join(MCP_FILE_NAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(mcp["mcpServers"]["tracked"]["command"], "x");
        assert_eq!(mcp["mcpServers"]["added-by-user"]["command"], "y");
        let overlay: Value = serde_json::from_str(
            &std::fs::read_to_string(profile_dir.join(OVERLAY_FILE_NAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(overlay, json!({}));
    }

    #[test]
    fn credential_store_is_copied_from_the_user_assistant_profile() {
        let root = tempfile::tempdir().unwrap();
        assert!(!inherit_credentials(
            root.path(),
            "skye",
            AgentId::UserAssistant,
            AgentId::Programmer
        )
        .unwrap());
        let source = host_profile_dir(root.path(), "skye", AgentId::UserAssistant);
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("agent.db"), b"db").unwrap();
        std::fs::write(source.join("agent.db-wal"), b"wal").unwrap();
        let target = host_profile_dir(root.path(), "skye", AgentId::Programmer);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("agent.db-shm"), b"stale").unwrap();
        assert!(inherit_credentials(
            root.path(),
            "skye",
            AgentId::UserAssistant,
            AgentId::Programmer
        )
        .unwrap());
        assert_eq!(std::fs::read(target.join("agent.db")).unwrap(), b"db");
        assert_eq!(std::fs::read(target.join("agent.db-wal")).unwrap(), b"wal");
        assert!(!target.join("agent.db-shm").exists());
    }

    #[test]
    fn extensions_are_refreshed_only_when_the_embedded_version_is_newer() {
        let project = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let layout = scaffolded_layout(project.path());
        let write_with = |source: &'static str| {
            ProfileProjection::for_agent(
                &layout,
                AgentId::UserAssistant,
                vec![ManagedExtension {
                    file_name: "clyean-test.ts",
                    source,
                }],
            )
            .unwrap()
            .write(root.path(), "skye")
            .unwrap()
            .extensions_refreshed
            .len()
        };
        assert_eq!(write_with(EXTENSION_V2), 1);
        assert_eq!(
            write_with(EXTENSION_V1),
            0,
            "older embedded version must not downgrade"
        );
        assert_eq!(write_with(EXTENSION_V2), 0, "same version is left alone");
    }
}
