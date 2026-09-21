// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Per-agent harness configuration tracked in `.clyean/agents`: a settings overlay in the
//! harness's `config.yml` schema (`<NAME>.omp.json`) and an MCP server seed
//! (`<NAME>.mcp.json`), each with a local-only companion merged on top.

use std::path::{Path, PathBuf};

use clyean_project::local_overlay::{deep_merge, local_companion};
use clyean_project::ProjectLayout;
use serde_json::{json, Value};

use crate::roster::AgentId;
use crate::{AgentError, Result};

pub fn mcp_seed_file_name(agent: AgentId) -> String {
    format!("{}.mcp.json", agent.file_stem())
}

/// The settings overlay a freshly scaffolded project starts with for `agent`.
pub fn default_settings_overlay(agent: AgentId) -> Value {
    match agent {
        AgentId::UserAssistant => json!({}),
        _ => json!({"tools": {"approvalMode": "yolo"}}),
    }
}

pub fn default_mcp_seed() -> Value {
    json!({"mcpServers": {}})
}

/// Writes both seed files of `agent` unless they already exist; returns the paths created.
pub fn write_seeds_if_missing(layout: &ProjectLayout, agent: AgentId) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(layout.agents_dir())
        .map_err(|e| AgentError::io("creating .clyean/agents", e))?;
    let mut created = Vec::new();
    let seeds = [
        (
            layout.agents_dir().join(agent.settings_file_name()),
            default_settings_overlay(agent),
        ),
        (
            layout.agents_dir().join(mcp_seed_file_name(agent)),
            default_mcp_seed(),
        ),
    ];
    for (path, value) in seeds {
        if path.exists() {
            continue;
        }
        write_json(&path, &value)?;
        created.push(path);
    }
    Ok(created)
}

/// The tracked settings overlay merged with its local companion.
pub fn effective_settings_overlay(layout: &ProjectLayout, agent: AgentId) -> Result<Value> {
    read_with_local_companion(&layout.agents_dir().join(agent.settings_file_name()))
        .map(|value| value.unwrap_or_else(|| json!({})))
}

/// The tracked MCP seed merged with its local companion, when either file exists.
pub fn effective_mcp_seed(layout: &ProjectLayout, agent: AgentId) -> Result<Option<Value>> {
    read_with_local_companion(&layout.agents_dir().join(mcp_seed_file_name(agent)))
}

fn read_with_local_companion(path: &Path) -> Result<Option<Value>> {
    let tracked = read_json_if_present(path)?;
    let local = read_json_if_present(&local_companion(path))?;
    Ok(match (tracked, local) {
        (None, None) => None,
        (Some(tracked), None) => Some(tracked),
        (None, Some(local)) => Some(local),
        (Some(tracked), Some(local)) => Some(deep_merge(tracked, local)),
    })
}

pub(crate) fn read_json_if_present(path: &Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| AgentError::io(format!("reading {}", path.display()), e))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| AgentError::Json {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AgentError::io(format!("creating {}", parent.display()), e))?;
    }
    let mut text = serde_json::to_string_pretty(value).expect("JSON values serialize");
    text.push('\n');
    std::fs::write(path, text).map_err(|e| AgentError::io(format!("writing {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_written_once_and_merged_with_local_companions() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let created = write_seeds_if_missing(&layout, AgentId::Programmer).unwrap();
        assert_eq!(created.len(), 2);
        assert!(write_seeds_if_missing(&layout, AgentId::Programmer)
            .unwrap()
            .is_empty());

        std::fs::write(
            layout.agents_dir().join("PROGRAMMER.omp.local.json"),
            r#"{"modelRoles": {"default": "anthropic/claude-opus-5"}}"#,
        )
        .unwrap();
        let overlay = effective_settings_overlay(&layout, AgentId::Programmer).unwrap();
        assert_eq!(overlay["tools"]["approvalMode"], "yolo");
        assert_eq!(overlay["modelRoles"]["default"], "anthropic/claude-opus-5");

        let mcp = effective_mcp_seed(&layout, AgentId::Programmer)
            .unwrap()
            .unwrap();
        assert_eq!(mcp, json!({"mcpServers": {}}));
        assert!(effective_mcp_seed(&layout, AgentId::Specifier)
            .unwrap()
            .is_none());
    }

    #[test]
    fn user_assistant_keeps_the_harness_approval_default() {
        assert_eq!(default_settings_overlay(AgentId::UserAssistant), json!({}));
        assert_eq!(
            default_settings_overlay(AgentId::Specifier)["tools"]["approvalMode"],
            "yolo"
        );
    }
}
