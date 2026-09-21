// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Baseline instructions of every implemented agent, embedded at build time and written
//! into `.clyean/agents/AGENTS__<NAME>.md` when a project is scaffolded.

use std::path::Path;

use clyean_project::local_overlay::read_markdown_with_local_enhancement;
use clyean_project::ProjectLayout;

use crate::roster::AgentId;
use crate::{AgentError, Result};

/// Shared preamble prepended to every agent's baseline instructions.
pub const COMMON_PREAMBLE: &str = include_str!("../instructions/COMMON.md");

pub fn baseline(agent: AgentId) -> Result<&'static str> {
    Ok(match agent {
        AgentId::UserAssistant => include_str!("../instructions/AGENTS__USER_ASSISTANT.md"),
        AgentId::Scaffolder => include_str!("../instructions/AGENTS__SCAFFOLDER.md"),
        AgentId::SoftwareEngineeringDirector => {
            include_str!("../instructions/AGENTS__SOFTWARE_ENGINEERING_DIRECTOR.md")
        }
        AgentId::Specifier => include_str!("../instructions/AGENTS__SPECIFIER.md"),
        AgentId::SoftwareArchitect => include_str!("../instructions/AGENTS__SOFTWARE_ARCHITECT.md"),
        AgentId::Programmer => include_str!("../instructions/AGENTS__PROGRAMMER.md"),
        other => return Err(AgentError::NotImplemented(other.id())),
    })
}

/// The full baseline file content written at scaffold time: preamble plus agent section.
pub fn scaffold_file_content(agent: AgentId) -> Result<String> {
    let body = baseline(agent)?;
    Ok(format!(
        "{}\n{}",
        COMMON_PREAMBLE
            .replace("{{AGENT_ID}}", agent.id())
            .replace("{{AGENT_NAME}}", agent.display_name()),
        body
    ))
}

/// Writes the baseline instruction file of `agent` unless it already exists.
pub fn write_baseline_if_missing(layout: &ProjectLayout, agent: AgentId) -> Result<bool> {
    let path = layout.agents_dir().join(agent.instruction_file_name());
    if path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(layout.agents_dir())
        .map_err(|e| AgentError::io("creating .clyean/agents", e))?;
    std::fs::write(&path, scaffold_file_content(agent)?)
        .map_err(|e| AgentError::io(format!("writing {}", path.display()), e))?;
    Ok(true)
}

/// The instructions an agent runs with: the tracked file plus its local enhancement.
pub fn effective_instructions(layout: &ProjectLayout, agent: AgentId) -> Result<String> {
    let path = layout.agents_dir().join(agent.instruction_file_name());
    read_instruction_file(&path)
}

fn read_instruction_file(path: &Path) -> Result<String> {
    read_markdown_with_local_enhancement(path)
        .map_err(|e| AgentError::io(format!("reading {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_implemented_agent_has_non_trivial_instructions() {
        for agent in AgentId::implemented() {
            let content = scaffold_file_content(agent).unwrap();
            assert!(content.len() > 800, "{} instructions are too short", agent);
            assert!(content.contains(agent.display_name()), "{}", agent);
            assert!(content.contains(agent.id()), "{}", agent);
            assert!(!content.contains("{{AGENT"), "{}", agent);
        }
        assert!(baseline(AgentId::CodeReviewer).is_err());
    }

    #[test]
    fn local_enhancement_is_appended_to_the_tracked_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        assert!(write_baseline_if_missing(&layout, AgentId::Programmer).unwrap());
        assert!(!write_baseline_if_missing(&layout, AgentId::Programmer).unwrap());
        std::fs::write(
            layout.agents_dir().join("AGENTS__PROGRAMMER.local.md"),
            "Always run the linter.\n",
        )
        .unwrap();
        let effective = effective_instructions(&layout, AgentId::Programmer).unwrap();
        assert!(effective.ends_with("Always run the linter.\n"));
        assert!(effective.contains("# Programmer"));
    }
}
