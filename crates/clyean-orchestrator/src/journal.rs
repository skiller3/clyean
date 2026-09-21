// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The durable record of one unit of work.  Every phase transition is written to disk
//! (and, for plans, committed to Git) so that an interrupted workflow resumes where it
//! stopped instead of starting over.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clyean_agents::AgentId;
use serde::{Deserialize, Serialize};

use crate::protocol::PromptType;
use crate::{OrchestratorError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Scaffold,
    Research,
    Planning,
    Implementation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Running,
    AwaitingInformation,
    Completed,
    Failed,
    Cancelled,
}

impl WorkStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// The steps of the planning and implementation workflows, in the order the workflow
/// diagrams prescribe.  Phases that loop carry their iteration count in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Pending,
    Scaffolding,
    Research,
    PlanningOverviewInformation,
    PlanningOverview,
    PlanningSpecification,
    PlanningSpecificationReview,
    PlanningArchitecture,
    PlanningArchitectureReview,
    ImplementationSpecs,
    ImplementationArchitecture,
    ImplementationProgramming,
    ImplementationReview,
    ImplementationRemediation,
    Done,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Scaffolding => "scaffolding",
            Self::Research => "research",
            Self::PlanningOverviewInformation => "planning.overview_information",
            Self::PlanningOverview => "planning.overview",
            Self::PlanningSpecification => "planning.specification",
            Self::PlanningSpecificationReview => "planning.specification_review",
            Self::PlanningArchitecture => "planning.architecture",
            Self::PlanningArchitectureReview => "planning.architecture_review",
            Self::ImplementationSpecs => "implementation.specs",
            Self::ImplementationArchitecture => "implementation.architecture",
            Self::ImplementationProgramming => "implementation.programming",
            Self::ImplementationReview => "implementation.review",
            Self::ImplementationRemediation => "implementation.remediation",
            Self::Done => "done",
        }
    }
}

/// One round of questions relayed to the user and, once available, their answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InformationExchange {
    pub request_id: String,
    pub asked_by: AgentId,
    pub phase: Phase,
    pub questions: Vec<String>,
    #[serde(default)]
    pub answers: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    #[serde(default)]
    pub session_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkJournal {
    pub work_id: String,
    pub kind: WorkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_type: Option<PromptType>,
    pub prompt: String,
    #[serde(default)]
    pub original_prompt: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub phase: Phase,
    pub status: WorkStatus,
    /// `<plan name>/v<N>` of the plan version being authored or implemented.
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub information: Vec<InformationExchange>,
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionRecord>,
    #[serde(default)]
    pub commits: Vec<String>,
    #[serde(default)]
    pub iterations: BTreeMap<String, u32>,
    /// Concerns added to the change by blocking implementation issues, newest last.
    #[serde(default)]
    pub blocking_issues: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Step-local data carried between phases (section summaries, review issues).
    #[serde(default)]
    pub scratch: BTreeMap<String, serde_json::Value>,
}

impl WorkJournal {
    pub fn new(work_id: String, kind: WorkKind, prompt: String) -> Self {
        let now = clyean_project::utc_now_rfc3339();
        Self {
            work_id,
            kind,
            prompt_type: None,
            prompt,
            original_prompt: None,
            session_id: None,
            created_at: now.clone(),
            updated_at: now,
            phase: Phase::Pending,
            status: WorkStatus::Running,
            plan: None,
            information: Vec::new(),
            sessions: BTreeMap::new(),
            commits: Vec::new(),
            iterations: BTreeMap::new(),
            blocking_issues: Vec::new(),
            summary: None,
            error: None,
            artifacts: Vec::new(),
            scratch: BTreeMap::new(),
        }
    }

    pub fn scratch_string(&self, key: &str) -> Option<String> {
        self.scratch
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    pub fn scratch_strings(&self, key: &str) -> Option<Vec<String>> {
        self.scratch
            .get(key)
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .map(str::to_string)
                    .collect()
            })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| OrchestratorError::io(format!("reading {}", path.display()), e))?;
        serde_json::from_str(&text).map_err(|source| OrchestratorError::Json {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.updated_at = clyean_project::utc_now_rfc3339();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| OrchestratorError::io(format!("creating {}", parent.display()), e))?;
        }
        let mut text = serde_json::to_string_pretty(self).expect("journal serializes");
        text.push('\n');
        std::fs::write(path, text)
            .map_err(|e| OrchestratorError::io(format!("writing {}", path.display()), e))
    }

    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.status = WorkStatus::Running;
    }

    /// Increments and returns the iteration counter of `phase`.
    pub fn next_iteration(&mut self, phase: Phase) -> u32 {
        let counter = self
            .iterations
            .entry(phase.label().to_string())
            .or_insert(0);
        *counter += 1;
        *counter
    }

    pub fn pending_information(&self) -> Option<&InformationExchange> {
        self.information
            .iter()
            .rev()
            .find(|exchange| exchange.answers.is_none())
    }

    pub fn record_information_request(&mut self, exchange: InformationExchange) {
        self.information.push(exchange);
        self.status = WorkStatus::AwaitingInformation;
    }

    pub fn record_answers(&mut self, request_id: &str, answers: Vec<String>) -> Result<()> {
        let exchange = self
            .information
            .iter_mut()
            .find(|exchange| exchange.request_id == request_id && exchange.answers.is_none())
            .ok_or_else(|| OrchestratorError::RequestNotFound {
                work_id: self.work_id.clone(),
                request_id: request_id.to_string(),
            })?;
        exchange.answers = Some(answers);
        self.status = WorkStatus::Running;
        Ok(())
    }

    /// All answered exchanges rendered as a block the agents can read.
    pub fn answered_information_block(&self) -> String {
        let mut block = String::new();
        for exchange in self.information.iter().filter(|e| e.answers.is_some()) {
            let answers = exchange.answers.as_ref().expect("filtered");
            for (question, answer) in exchange.questions.iter().zip(answers) {
                block.push_str(&format!("Q: {question}\nA: {answer}\n\n"));
            }
        }
        block
    }

    pub fn complete(&mut self, summary: String, artifacts: Vec<String>) {
        self.phase = Phase::Done;
        self.status = WorkStatus::Completed;
        self.summary = Some(summary);
        self.artifacts = artifacts;
    }

    pub fn fail(&mut self, error: String) {
        self.status = WorkStatus::Failed;
        self.error = Some(error);
    }

    pub fn cancel(&mut self) {
        self.status = WorkStatus::Cancelled;
    }

    pub fn is_resumable(&self) -> bool {
        !self.status.is_terminal()
    }
}

/// Where the journal of a unit of work lives.
pub fn journal_path(layout: &clyean_project::ProjectLayout, journal: &WorkJournal) -> PathBuf {
    match (&journal.kind, &journal.plan) {
        (WorkKind::Planning | WorkKind::Implementation, Some(plan)) => {
            let name = plan.split('/').next().unwrap_or(plan);
            layout
                .plans_dir()
                .join(name)
                .join(format!("journal-{}.json", journal.work_id))
        }
        _ => layout.work_dir().join(format!("{}.json", journal.work_id)),
    }
}

fn is_journal_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("journal-") && n.ends_with(".json"))
}

/// Every journal under `.clyean/work` and `.clyean/plans/*/journal-*.json`.
pub fn list_journals(
    layout: &clyean_project::ProjectLayout,
) -> Result<Vec<(PathBuf, WorkJournal)>> {
    let mut journals = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(layout.work_dir()) {
        candidates.extend(
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "json")),
        );
    }
    if let Ok(plan_dirs) = std::fs::read_dir(layout.plans_dir()) {
        for plan_dir in plan_dirs.filter_map(|e| e.ok()).map(|e| e.path()) {
            if let Ok(entries) = std::fs::read_dir(&plan_dir) {
                candidates.extend(
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| is_journal_file(p)),
                );
            }
        }
    }
    candidates.sort();
    for path in candidates {
        match WorkJournal::load(&path) {
            Ok(journal) => journals.push((path, journal)),
            Err(error) => {
                tracing::warn!(target: "clyean::orchestrator", %error, "skipping unreadable journal")
            }
        }
    }
    Ok(journals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_round_trips_and_tracks_information() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let mut journal = WorkJournal::new("w1".into(), WorkKind::Planning, "add login".into());
        journal.set_phase(Phase::PlanningOverviewInformation);
        journal.record_information_request(InformationExchange {
            request_id: "r1".into(),
            asked_by: AgentId::SoftwareEngineeringDirector,
            phase: Phase::PlanningOverviewInformation,
            questions: vec!["Which provider?".into()],
            answers: None,
        });
        assert_eq!(journal.status, WorkStatus::AwaitingInformation);
        assert!(journal.pending_information().is_some());
        journal.save(&path).unwrap();

        let mut loaded = WorkJournal::load(&path).unwrap();
        assert_eq!(loaded.phase, Phase::PlanningOverviewInformation);
        assert!(loaded.record_answers("nope", vec![]).is_err());
        loaded.record_answers("r1", vec!["OAuth".into()]).unwrap();
        assert!(loaded.pending_information().is_none());
        assert_eq!(loaded.status, WorkStatus::Running);
        assert!(loaded.answered_information_block().contains("A: OAuth"));
        assert_eq!(loaded.next_iteration(Phase::PlanningSpecificationReview), 1);
        assert_eq!(loaded.next_iteration(Phase::PlanningSpecificationReview), 2);
        loaded.complete("done".into(), vec![".clyean/plans/x/v1.md".into()]);
        assert!(!loaded.is_resumable());
    }

    #[test]
    fn journal_paths_depend_on_the_kind_of_work() {
        let layout = clyean_project::ProjectLayout::new("/p");
        let mut planning = WorkJournal::new("w1".into(), WorkKind::Planning, "x".into());
        planning.plan = Some("2026-09-21-add-login/v1".into());
        assert_eq!(
            journal_path(&layout, &planning),
            PathBuf::from("/p/.clyean/plans/2026-09-21-add-login/journal-w1.json")
        );
        let research = WorkJournal::new("w2".into(), WorkKind::Research, "x".into());
        assert_eq!(
            journal_path(&layout, &research),
            PathBuf::from("/p/.clyean/work/w2.json")
        );
    }
}
