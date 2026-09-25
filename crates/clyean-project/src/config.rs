// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::ProjectLayout;
use crate::local_overlay::deep_merge;
use crate::{ProjectError, Result};

/// Schema version of `project.json`; bump when its shape changes incompatibly.
pub const PROJECT_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_SANDBOX_IMAGE: &str = "docker.io/library/ubuntu:latest";

/// The two mutually exclusive kinds of Clyean project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectType {
    SoftwareEngineeringProject,
    MiscellaneousProject,
}

impl ProjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SoftwareEngineeringProject => "SOFTWARE_ENGINEERING_PROJECT",
            Self::MiscellaneousProject => "MISCELLANEOUS_PROJECT",
        }
    }
}

impl std::str::FromStr for ProjectType {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "SOFTWARE_ENGINEERING_PROJECT" => Ok(Self::SoftwareEngineeringProject),
            "MISCELLANEOUS_PROJECT" => Ok(Self::MiscellaneousProject),
            other => Err(format!("unknown project type {other:?}")),
        }
    }
}

/// Top-level configuration of a Clyean project, persisted as `.clyean/project.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub schema_version: u32,
    /// Version of `clyean` that generated the scaffolding.
    pub clyean_version: String,
    /// UTC timestamp (RFC 3339) at which the scaffolding was generated.
    pub scaffolded_at: String,
    pub project_type: ProjectType,
    pub workspace: WorkspaceConfig,
    pub git: GitConfig,
    pub sandbox: SandboxConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfig {
    /// Absolute host path of the workspace directory (the project directory or an ancestor).
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitConfig {
    /// Whether Clyean may use Git worktrees for concurrent work.
    pub use_worktrees: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxConfig {
    /// Image the agent root filesystem is populated from.
    pub image: String,
    /// Additional host paths mounted read-only under `/mnt/<name>`.
    #[serde(default)]
    pub mounts: Vec<PathBuf>,
    /// Extra arguments appended to every `podman run` invocation.
    #[serde(default)]
    pub podman_run_args: Vec<String>,
    /// Overrides where the harness binary is taken from; `None` selects the release download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_binary: Option<PathBuf>,
    /// Host environment variables passed into agent containers, beyond each agent's
    /// provider credentials.  Exact names or `*` glob patterns.
    #[serde(default)]
    pub passthrough_env: Vec<PassthroughEntry>,
}

/// One host environment variable, or `*` glob pattern, that the project passes through:
/// written as a string, it reaches the User Assistant only; written as an object, it
/// reaches the agents it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PassthroughEntry {
    UserAssistant(String),
    Assigned { name: String, agents: Vec<String> },
}

impl PassthroughEntry {
    /// Whether the variable or pattern reaches the agent `agent_id`.
    pub fn reaches(&self, agent_id: &str, is_user_assistant: bool) -> bool {
        match self {
            Self::UserAssistant(_) => is_user_assistant,
            Self::Assigned { agents, .. } => agents.iter().any(|agent| agent == agent_id),
        }
    }

    pub fn pattern(&self) -> &str {
        match self {
            Self::UserAssistant(name) | Self::Assigned { name, .. } => name,
        }
    }
}

impl SandboxConfig {
    pub fn with_image(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            mounts: Vec::new(),
            podman_run_args: Vec::new(),
            harness_binary: None,
            passthrough_env: Vec::new(),
        }
    }
}

impl ProjectConfig {
    pub fn new(
        clyean_version: impl Into<String>,
        project_type: ProjectType,
        workspace: impl Into<PathBuf>,
        use_worktrees: bool,
        sandbox: SandboxConfig,
    ) -> Self {
        Self {
            schema_version: PROJECT_CONFIG_SCHEMA_VERSION,
            clyean_version: clyean_version.into(),
            scaffolded_at: crate::utc_now_rfc3339(),
            project_type,
            workspace: WorkspaceConfig {
                path: workspace.into(),
            },
            git: GitConfig { use_worktrees },
            sandbox,
        }
    }

    /// Loads `project.json` and applies `project.local.json` on top when it exists.
    pub fn load(layout: &ProjectLayout) -> Result<Self> {
        let path = layout.project_config_path();
        if !path.is_file() {
            return Err(ProjectError::NotScaffolded(layout.root().to_path_buf()));
        }
        let mut merged = read_json(&path)?;
        let local_path = layout.project_local_config_path();
        if local_path.is_file() {
            merged = deep_merge(merged, read_json(&local_path)?);
        }
        serde_json::from_value(merged).map_err(|source| ProjectError::Json { path, source })
    }

    /// Writes `project.json` (never the local override file).
    pub fn save(&self, layout: &ProjectLayout) -> Result<()> {
        let path = layout.project_config_path();
        let parent = path.parent().expect("project.json has a parent directory");
        std::fs::create_dir_all(parent)
            .map_err(|e| ProjectError::io(format!("creating {}", parent.display()), e))?;
        let mut text = serde_json::to_string_pretty(self).expect("ProjectConfig serializes");
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|e| ProjectError::io(format!("writing {}", path.display()), e))
    }
}

fn read_json(path: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProjectError::io(format!("reading {}", path.display()), e))?;
    serde_json::from_str(&text).map_err(|source| ProjectError::Json {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(workspace: &Path) -> ProjectConfig {
        ProjectConfig::new(
            "0.1.0",
            ProjectType::SoftwareEngineeringProject,
            workspace,
            false,
            SandboxConfig::with_image(DEFAULT_SANDBOX_IMAGE),
        )
    }

    #[test]
    fn round_trips_through_project_json() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let config = sample(dir.path());
        config.save(&layout).unwrap();
        let loaded = ProjectConfig::load(&layout).unwrap();
        assert_eq!(loaded, config);
        let text = std::fs::read_to_string(layout.project_config_path()).unwrap();
        assert!(text.contains("\"projectType\": \"SOFTWARE_ENGINEERING_PROJECT\""));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn local_override_is_merged_over_the_tracked_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        sample(dir.path()).save(&layout).unwrap();
        std::fs::write(
            layout.project_local_config_path(),
            r#"{"sandbox": {"image": "docker.io/library/debian:12", "mounts": ["/data"]}}"#,
        )
        .unwrap();
        let loaded = ProjectConfig::load(&layout).unwrap();
        assert_eq!(loaded.sandbox.image, "docker.io/library/debian:12");
        assert_eq!(loaded.sandbox.mounts, vec![PathBuf::from("/data")]);
        assert_eq!(loaded.project_type, ProjectType::SoftwareEngineeringProject);
    }

    #[test]
    fn passthrough_entries_are_names_for_the_user_assistant_or_assigned_objects() {
        let sandbox: SandboxConfig = serde_json::from_str(
            r#"{"image": "i", "passthroughEnv": ["CORP_*", {"name": "GH_TOKEN", "agents": ["software-engineering-director"]}]}"#,
        )
        .unwrap();
        let [corp, token] = sandbox.passthrough_env.as_slice() else {
            panic!("two entries")
        };
        assert_eq!(corp.pattern(), "CORP_*");
        assert!(corp.reaches("user-assistant", true));
        assert!(!corp.reaches("programmer", false));
        assert_eq!(token.pattern(), "GH_TOKEN");
        assert!(token.reaches("software-engineering-director", false));
        assert!(!token.reaches("user-assistant", true));
    }

    #[test]
    fn missing_project_json_reports_not_scaffolded() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        assert!(matches!(
            ProjectConfig::load(&layout),
            Err(ProjectError::NotScaffolded(_))
        ));
    }

    #[test]
    fn project_type_parses_its_wire_form() {
        assert_eq!(
            "MISCELLANEOUS_PROJECT".parse::<ProjectType>().unwrap(),
            ProjectType::MiscellaneousProject
        );
        assert!("nope".parse::<ProjectType>().is_err());
    }
}
