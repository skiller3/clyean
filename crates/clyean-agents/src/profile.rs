// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Projection of an agent's tracked configuration into its harness profile directory
//! inside the sandbox root filesystem: `/home/<user>/.omp/profiles/<agent-id>/agent/`.
//! The projection only plans: it names the profile files it must read first and returns
//! the files to write, and the caller moves them through the sandbox filesystem.

use std::collections::HashMap;

use clyean_project::local_overlay::deep_merge;
use clyean_project::ProjectLayout;
use serde_json::Value;

use crate::instructions::effective_instructions;
use crate::roster::AgentId;
use crate::settings::{effective_mcp_seed, effective_settings_overlay};
use crate::{AgentError, Result};

pub const OVERLAY_FILE_NAME: &str = "clyean-overlay.json";
pub const MCP_FILE_NAME: &str = "mcp.json";
pub const AGENTS_FILE_NAME: &str = "AGENTS.md";
pub const EXTENSIONS_DIR_NAME: &str = "extensions";
pub const EXTENSION_VERSION_MARKER: &str = "// CLYEAN_EXTENSION_VERSION=";
/// The file a sub-agent's credential copies are delivered in, inside its own profile.
pub const CREDENTIALS_BUNDLE_FILE_NAME: &str = "clyean-credentials.json";

/// A Clyean-managed harness extension shipped into an agent's profile.
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

pub fn container_credentials_bundle_path(user: &str, agent: AgentId) -> String {
    format!(
        "{}/{CREDENTIALS_BUNDLE_FILE_NAME}",
        container_profile_dir(user, agent)
    )
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

/// A file to write into the root filesystem: its absolute path there, contents, and mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFile {
    pub path: String,
    pub contents: Vec<u8>,
    pub mode: u32,
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

    /// The profile files the projection depends on: `mcp.json` and every managed
    /// extension it may replace.
    pub fn reads(&self, user: &str) -> Vec<String> {
        let profile_dir = container_profile_dir(user, self.agent);
        let mut paths = vec![format!("{profile_dir}/{MCP_FILE_NAME}")];
        paths.extend(self.extensions.iter().map(|extension| {
            format!(
                "{profile_dir}/{EXTENSIONS_DIR_NAME}/{}",
                extension.file_name
            )
        }));
        paths
    }

    /// The files to write, given the current contents of the paths `reads` named.
    /// `AGENTS.md` and the overlay are always replaced; `mcp.json` keeps servers the user
    /// added through the harness and layers the tracked seed on top; extensions are
    /// written only when their version marker is behind the embedded one.
    pub fn files(
        &self,
        user: &str,
        existing: &HashMap<String, Vec<u8>>,
    ) -> Result<Vec<ProfileFile>> {
        let profile_dir = container_profile_dir(user, self.agent);
        let mut files = vec![
            ProfileFile {
                path: format!("{profile_dir}/{AGENTS_FILE_NAME}"),
                contents: self.agents_md.clone().into_bytes(),
                mode: 0o644,
            },
            ProfileFile {
                path: format!("{profile_dir}/{OVERLAY_FILE_NAME}"),
                contents: json_bytes(&self.settings_overlay),
                mode: 0o644,
            },
        ];
        if let Some(seed) = &self.mcp_seed {
            let path = format!("{profile_dir}/{MCP_FILE_NAME}");
            let current = match existing.get(&path) {
                Some(bytes) => {
                    serde_json::from_slice(bytes).map_err(|source| AgentError::Json {
                        path: path.clone().into(),
                        source,
                    })?
                }
                None => Value::Object(Default::default()),
            };
            files.push(ProfileFile {
                contents: json_bytes(&deep_merge(current, seed.clone())),
                path,
                mode: 0o644,
            });
        }
        for extension in &self.extensions {
            let path = format!(
                "{profile_dir}/{EXTENSIONS_DIR_NAME}/{}",
                extension.file_name
            );
            let installed = existing
                .get(&path)
                .map(|bytes| String::from_utf8_lossy(bytes));
            if installed.is_some_and(|source| extension_is_current(&source, extension)) {
                continue;
            }
            files.push(ProfileFile {
                path,
                contents: extension.source.as_bytes().to_vec(),
                mode: 0o644,
            });
        }
        Ok(files)
    }
}

fn json_bytes(value: &Value) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(value).expect("JSON values serialize");
    text.push('\n');
    text.into_bytes()
}

fn extension_is_current(installed: &str, extension: &ManagedExtension) -> bool {
    match (extension_version(installed), extension.version()) {
        (Some(installed), Some(embedded)) => installed >= embedded,
        _ => installed == extension.source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instructions::write_baseline_if_missing;
    use crate::settings::write_seeds_if_missing;
    use serde_json::json;
    use std::path::Path;

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

    fn by_path(files: &[ProfileFile]) -> HashMap<String, Vec<u8>> {
        files
            .iter()
            .map(|file| (file.path.clone(), file.contents.clone()))
            .collect()
    }

    #[test]
    fn projection_plans_profile_files_and_preserves_user_added_mcp_servers() {
        let project = tempfile::tempdir().unwrap();
        let layout = scaffolded_layout(project.path());
        std::fs::write(
            layout.agents_dir().join("USER_ASSISTANT.mcp.json"),
            r#"{"mcpServers": {"tracked": {"type": "stdio", "command": "x"}}}"#,
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
        let profile_dir = "/home/skye/.omp/profiles/user-assistant/agent";
        assert_eq!(
            projection.reads("skye"),
            [
                format!("{profile_dir}/mcp.json"),
                format!("{profile_dir}/extensions/clyean-test.ts")
            ]
        );
        let existing = HashMap::from([(
            format!("{profile_dir}/mcp.json"),
            br#"{"mcpServers": {"added-by-user": {"type": "stdio", "command": "y"}}}"#.to_vec(),
        )]);
        let files = by_path(&projection.files("skye", &existing).unwrap());
        let agents_md = String::from_utf8_lossy(&files[&format!("{profile_dir}/AGENTS.md")]);
        assert!(agents_md.contains("# User Assistant"));
        let mcp: Value =
            serde_json::from_slice(&files[&format!("{profile_dir}/mcp.json")]).unwrap();
        assert_eq!(mcp["mcpServers"]["tracked"]["command"], "x");
        assert_eq!(mcp["mcpServers"]["added-by-user"]["command"], "y");
        let overlay: Value =
            serde_json::from_slice(&files[&format!("{profile_dir}/clyean-overlay.json")]).unwrap();
        assert_eq!(overlay, json!({}));
        assert_eq!(
            files[&format!("{profile_dir}/extensions/clyean-test.ts")],
            EXTENSION_V1.as_bytes()
        );
    }

    #[test]
    fn extensions_are_written_only_when_the_embedded_version_is_newer() {
        let project = tempfile::tempdir().unwrap();
        let layout = scaffolded_layout(project.path());
        let path = "/home/skye/.omp/profiles/user-assistant/agent/extensions/clyean-test.ts";
        let writes = |embedded: &'static str, installed: Option<&str>| {
            let projection = ProfileProjection::for_agent(
                &layout,
                AgentId::UserAssistant,
                vec![ManagedExtension {
                    file_name: "clyean-test.ts",
                    source: embedded,
                }],
            )
            .unwrap();
            let existing: HashMap<String, Vec<u8>> = installed
                .map(|source| (path.to_string(), source.as_bytes().to_vec()))
                .into_iter()
                .collect();
            projection
                .files("skye", &existing)
                .unwrap()
                .iter()
                .any(|file| file.path == path)
        };
        assert!(writes(EXTENSION_V2, None));
        assert!(writes(EXTENSION_V2, Some(EXTENSION_V1)));
        assert!(
            !writes(EXTENSION_V1, Some(EXTENSION_V2)),
            "an older embedded version must not downgrade"
        );
        assert!(!writes(EXTENSION_V2, Some(EXTENSION_V2)));
    }
}
