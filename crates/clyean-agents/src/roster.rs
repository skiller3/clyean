// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use clyean_git::CommitIdentity;
use serde::{Deserialize, Serialize};

/// Every agent named in `AGENT_SPECS.md`.  Placeholders are known so that configuration
/// and documentation can name them, but they have no behavior until specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    UserAssistant,
    Scaffolder,
    SoftwareEngineeringDirector,
    Specifier,
    SoftwareArchitect,
    Programmer,
    CodeReviewer,
    AutomatedTestProgrammer,
    MutantKiller,
    CrapReducer,
    QaTester,
    CiCdProgrammer,
    DeploymentAnalyst,
    SecurityEngineer,
    WhiteHatHacker,
    DocumentationAuthor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Implemented,
    Placeholder,
}

impl AgentId {
    pub const ALL: [AgentId; 16] = [
        Self::UserAssistant,
        Self::Scaffolder,
        Self::SoftwareEngineeringDirector,
        Self::Specifier,
        Self::SoftwareArchitect,
        Self::Programmer,
        Self::CodeReviewer,
        Self::AutomatedTestProgrammer,
        Self::MutantKiller,
        Self::CrapReducer,
        Self::QaTester,
        Self::CiCdProgrammer,
        Self::DeploymentAnalyst,
        Self::SecurityEngineer,
        Self::WhiteHatHacker,
        Self::DocumentationAuthor,
    ];

    pub fn implemented() -> impl Iterator<Item = AgentId> {
        Self::ALL
            .into_iter()
            .filter(|agent| agent.status() == AgentStatus::Implemented)
    }

    /// The kebab-case identifier used for `CLYEAN_AGENT`, profile names, and trailers.
    pub fn id(self) -> &'static str {
        match self {
            Self::UserAssistant => "user-assistant",
            Self::Scaffolder => "scaffolder",
            Self::SoftwareEngineeringDirector => "software-engineering-director",
            Self::Specifier => "specifier",
            Self::SoftwareArchitect => "software-architect",
            Self::Programmer => "programmer",
            Self::CodeReviewer => "code-reviewer",
            Self::AutomatedTestProgrammer => "automated-test-programmer",
            Self::MutantKiller => "mutant-killer",
            Self::CrapReducer => "crap-reducer",
            Self::QaTester => "qa-tester",
            Self::CiCdProgrammer => "ci-cd-programmer",
            Self::DeploymentAnalyst => "deployment-analyst",
            Self::SecurityEngineer => "security-engineer",
            Self::WhiteHatHacker => "white-hat-hacker",
            Self::DocumentationAuthor => "documentation-author",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::UserAssistant => "User Assistant",
            Self::Scaffolder => "Scaffolder",
            Self::SoftwareEngineeringDirector => "Software Engineering Director",
            Self::Specifier => "Specifier",
            Self::SoftwareArchitect => "Software Architect",
            Self::Programmer => "Programmer",
            Self::CodeReviewer => "Code Reviewer",
            Self::AutomatedTestProgrammer => "Automated Test Programmer",
            Self::MutantKiller => "Mutant Killer",
            Self::CrapReducer => "CRAP Reducer",
            Self::QaTester => "QA Tester",
            Self::CiCdProgrammer => "CI/CD Programmer",
            Self::DeploymentAnalyst => "Deployment Analyst",
            Self::SecurityEngineer => "Security Engineer",
            Self::WhiteHatHacker => "White-Hat Hacker",
            Self::DocumentationAuthor => "Documentation Author",
        }
    }

    /// The `SCREAMING_SNAKE_CASE` stem of the agent's files under `.clyean/agents`.
    pub fn file_stem(self) -> String {
        self.id().to_ascii_uppercase().replace('-', "_")
    }

    pub fn instruction_file_name(self) -> String {
        format!("AGENTS__{}.md", self.file_stem())
    }

    pub fn settings_file_name(self) -> String {
        format!("{}.omp.json", self.file_stem())
    }

    pub fn status(self) -> AgentStatus {
        match self {
            Self::UserAssistant
            | Self::Scaffolder
            | Self::SoftwareEngineeringDirector
            | Self::Specifier
            | Self::SoftwareArchitect
            | Self::Programmer => AgentStatus::Implemented,
            _ => AgentStatus::Placeholder,
        }
    }

    pub fn git_identity(self) -> CommitIdentity {
        CommitIdentity::for_agent(self.id(), self.display_name())
    }

    pub fn parse(id: &str) -> Option<AgentId> {
        Self::ALL.into_iter().find(|agent| agent.id() == id)
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_file_names_follow_the_documented_pattern() {
        let agent = AgentId::SoftwareEngineeringDirector;
        assert_eq!(agent.id(), "software-engineering-director");
        assert_eq!(agent.file_stem(), "SOFTWARE_ENGINEERING_DIRECTOR");
        assert_eq!(
            agent.instruction_file_name(),
            "AGENTS__SOFTWARE_ENGINEERING_DIRECTOR.md"
        );
        assert_eq!(
            agent.settings_file_name(),
            "SOFTWARE_ENGINEERING_DIRECTOR.omp.json"
        );
        assert_eq!(AgentId::parse("programmer"), Some(AgentId::Programmer));
        assert_eq!(AgentId::parse("nope"), None);
    }

    #[test]
    fn six_agents_are_implemented_and_the_rest_are_placeholders() {
        assert_eq!(AgentId::implemented().count(), 6);
        assert_eq!(AgentId::CodeReviewer.status(), AgentStatus::Placeholder);
        assert_eq!(
            serde_json::to_string(&AgentId::UserAssistant).unwrap(),
            "\"user-assistant\""
        );
    }

    #[test]
    fn git_identity_uses_the_standard_form() {
        assert_eq!(
            AgentId::Programmer.git_identity().as_author_string(),
            "Clyean Programmer <programmer@agents.clyean.com>"
        );
    }
}
