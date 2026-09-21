// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The workflows the orchestrator executes, each written as a resumable state machine
//! over the phases recorded in a work journal.

pub mod implementation;
pub mod planning;
pub mod research;
pub mod scaffold;

use std::path::Path;
use std::sync::Arc;

use clyean_agents::AgentId;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::{mpsc, Mutex};

use crate::agents::SubAgentPool;
use crate::events::EventSink;
use crate::journal::{journal_path, InformationExchange, Phase, WorkJournal};
use crate::service::ProjectServices;
use crate::verdict::extract_verdict;
use crate::work::{AnswerDelivery, WorkHandle};
use crate::{OrchestratorError, Result};

/// Everything a running workflow needs.
pub struct WorkContext {
    pub services: Arc<ProjectServices>,
    pub pool: SubAgentPool,
    pub sink: EventSink,
    pub handle: WorkHandle,
    pub answers: Mutex<mpsc::Receiver<AnswerDelivery>>,
}

/// The verdict shapes shared by several steps.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum InformationVerdict {
    NeedsInformation { questions: Vec<String> },
    Proceed,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum EnrichmentVerdict {
    NeedsInformation {
        questions: Vec<String>,
    },
    Completed {
        #[serde(default)]
        summary: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AnswerVerdict {
    Answered { answers: Vec<String> },
    Escalate { questions: Vec<String> },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ReviewVerdict {
    Aligned,
    Misaligned { issues: Vec<String> },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CompletionVerdict {
    Completed {
        #[serde(default)]
        summary: String,
    },
    Blocked {
        issue: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ProgrammerVerdict {
    ReadyForReview {
        #[serde(default)]
        summary: String,
    },
    Blocked {
        issue: String,
        #[serde(default)]
        suggested_resolution: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ReviewIssue {
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default)]
    pub location: String,
    pub description: String,
}

fn default_severity() -> String {
    "major".to_string()
}

impl ReviewIssue {
    pub fn blocks(&self) -> bool {
        matches!(
            self.severity.to_ascii_lowercase().as_str(),
            "blocker" | "major"
        )
    }

    pub fn render(&self) -> String {
        format!(
            "- [{}] {}: {}",
            self.severity, self.location, self.description
        )
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ImplementationReviewVerdict {
    Approved {
        #[serde(default)]
        summary: String,
    },
    Issues {
        issues: Vec<ReviewIssue>,
    },
}

/// Upper bound on review or remediation rounds before the workflow gives up.
pub const MAX_REVIEW_ROUNDS: u32 = 3;
/// Upper bound on re-planning cycles caused by blocking implementation issues.
pub const MAX_REPLANNING_CYCLES: u32 = 2;

impl WorkContext {
    pub fn journal_path(&self, journal: &WorkJournal) -> std::path::PathBuf {
        journal_path(&self.services.layout, journal)
    }

    pub fn save(&self, journal: &mut WorkJournal) -> Result<()> {
        let path = self.journal_path(journal);
        journal.save(&path)
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.handle.cancellation.is_cancelled() {
            return Err(OrchestratorError::Cancelled);
        }
        Ok(())
    }

    /// Sends `prompt` to `agent` and returns the extracted verdict and the full reply.
    /// A reply without a verdict block earns one retry asking for the block alone.
    pub async fn ask_agent<V: DeserializeOwned>(
        &self,
        agent: AgentId,
        journal: &mut WorkJournal,
        phase: Phase,
        prompt: String,
    ) -> Result<(V, String)> {
        self.check_cancelled()?;
        let (progress, relay) = self.sink.turn_progress_channel(agent.id(), phase);
        let outcome = self.pool.prompt(agent, journal, prompt, progress).await;
        relay.finish().await;
        let outcome = outcome?;
        self.save(journal)?;
        match extract_verdict::<V>(agent.id(), &outcome.assistant_text) {
            Ok(verdict) => Ok((verdict, outcome.assistant_text)),
            Err(first_error) => {
                self.sink
                    .progress(
                        agent.id(),
                        phase,
                        "reply lacked a verdict block; asking again",
                    )
                    .await;
                let nudge = "Your previous reply did not end with the required verdict block.  Reply now with only the fenced json verdict block for the step you just performed.".to_string();
                let (retry_progress, retry_relay) =
                    self.sink.turn_progress_channel(agent.id(), phase);
                let retry = self
                    .pool
                    .prompt(agent, journal, nudge, retry_progress)
                    .await;
                retry_relay.finish().await;
                let retry = retry?;
                match extract_verdict::<V>(agent.id(), &retry.assistant_text) {
                    Ok(verdict) => Ok((
                        verdict,
                        format!("{}\n{}", outcome.assistant_text, retry.assistant_text),
                    )),
                    Err(_) => Err(first_error),
                }
            }
        }
    }

    /// Sends a free-form prompt whose reply needs no verdict.
    pub async fn ask_agent_freeform(
        &self,
        agent: AgentId,
        journal: &mut WorkJournal,
        phase: Phase,
        prompt: String,
    ) -> Result<String> {
        self.check_cancelled()?;
        let (progress, relay) = self.sink.turn_progress_channel(agent.id(), phase);
        let outcome = self.pool.prompt(agent, journal, prompt, progress).await;
        relay.finish().await;
        let outcome = outcome?;
        self.save(journal)?;
        Ok(outcome.assistant_text)
    }

    /// Relays `questions` to the user through the User Assistant and waits for the
    /// answers, keeping every sub-agent session alive meanwhile.
    pub async fn request_information(
        &self,
        journal: &mut WorkJournal,
        asked_by: AgentId,
        phase: Phase,
        questions: Vec<String>,
    ) -> Result<Vec<String>> {
        let request_id = uuid::Uuid::now_v7().simple().to_string();
        journal.record_information_request(InformationExchange {
            request_id: request_id.clone(),
            asked_by,
            phase,
            questions: questions.clone(),
            answers: None,
        });
        self.save(journal)?;
        self.sink
            .information_requested(
                &request_id,
                &questions,
                &format!("asked by {}", asked_by.display_name()),
            )
            .await;
        self.await_answers(journal, &request_id).await
    }

    /// Waits for the answers to the pending request `request_id` (used both right after
    /// asking and when resuming a work that was waiting).
    pub async fn await_answers(
        &self,
        journal: &mut WorkJournal,
        request_id: &str,
    ) -> Result<Vec<String>> {
        loop {
            let delivery = {
                let mut answers = self.answers.lock().await;
                tokio::select! {
                    delivery = answers.recv() => delivery,
                    _ = self.handle.cancellation.cancelled() => return Err(OrchestratorError::Cancelled),
                }
            };
            let Some(delivery) = delivery else {
                return Err(OrchestratorError::Cancelled);
            };
            if delivery.request_id != request_id {
                continue;
            }
            journal.record_answers(request_id, delivery.answers.clone())?;
            self.handle.clear_pending_information_request().await;
            self.save(journal)?;
            return Ok(delivery.answers);
        }
    }

    /// Stages `paths` (project-relative) and commits them under `agent`'s identity,
    /// recording the commit in the journal.  Nothing to commit is not an error.
    pub async fn commit_step(
        &self,
        journal: &mut WorkJournal,
        paths: &[&Path],
        subject: &str,
        agent: AgentId,
    ) -> Result<()> {
        self.save(journal)?;
        let journal_relative = self
            .journal_path(journal)
            .strip_prefix(self.services.layout.root())
            .map(Path::to_path_buf)
            .ok();
        let mut all_paths: Vec<&Path> = paths.to_vec();
        if let Some(path) = journal_relative.as_deref() {
            if !self.services.git.is_ignored(path)? {
                all_paths.push(path);
            }
        }
        let message = clyean_git::agent_commit_message(subject, None, agent.id());
        let outcome =
            self.services
                .git
                .stage_and_commit(&all_paths, &agent.git_identity(), &message)?;
        if let clyean_git::CommitOutcome::Committed(sha) = outcome {
            journal.commits.push(sha.clone());
            self.save(journal)?;
            self.sink
                .progress(
                    "orchestrator",
                    journal.phase,
                    format!("committed {} ({subject})", &sha[..8.min(sha.len())]),
                )
                .await;
        }
        Ok(())
    }

    /// Asks the Software Engineering Director to answer a sub-agent's questions, escalating
    /// to the user when the Director cannot.  Returns the answers.
    pub async fn resolve_questions(
        &self,
        journal: &mut WorkJournal,
        asking_agent: AgentId,
        phase: Phase,
        questions: Vec<String>,
    ) -> Result<Vec<String>> {
        let prompt =
            crate::prompts::director_answer_questions(asking_agent.display_name(), &questions);
        let (verdict, _) = self
            .ask_agent::<AnswerVerdict>(
                AgentId::SoftwareEngineeringDirector,
                journal,
                phase,
                prompt,
            )
            .await?;
        match verdict {
            AnswerVerdict::Answered { answers } => {
                journal.information.push(InformationExchange {
                    request_id: uuid::Uuid::now_v7().simple().to_string(),
                    asked_by: asking_agent,
                    phase,
                    questions,
                    answers: Some(answers.clone()),
                });
                self.save(journal)?;
                Ok(answers)
            }
            AnswerVerdict::Escalate {
                questions: escalated,
            } => {
                self.request_information(journal, asking_agent, phase, escalated)
                    .await
            }
        }
    }
}

/// Reads the plan file and returns the body of `## <section>`.
pub fn plan_section(plan_text: &str, section: &str) -> Option<String> {
    let heading = format!("## {section}");
    let mut collecting = false;
    let mut body = String::new();
    for line in plan_text.lines() {
        if line.trim_end() == heading {
            collecting = true;
            continue;
        }
        if collecting && line.starts_with("## ") {
            break;
        }
        if collecting {
            body.push_str(line);
            body.push('\n');
        }
    }
    if collecting {
        Some(body.trim().to_string())
    } else {
        None
    }
}

pub const PLAN_SECTIONS: [&str; 3] = [
    "Overview",
    "Specification Changes",
    "Implementation Architecture",
];

/// The skeleton every change plan version starts from.
pub fn plan_skeleton(title: &str, version: u32, previous_content: Option<&str>) -> String {
    if let Some(previous) = previous_content {
        let mut text = previous.to_string();
        if let Some(rest) = text.strip_prefix("# ") {
            if let Some(newline) = rest.find('\n') {
                let heading = format!("# {title} (v{version})");
                text = format!("{heading}{}", &rest[newline..]);
            }
        }
        return text;
    }
    format!(
        "# {title} (v{version})\n\n## Overview\n\n_To be written by the Software Engineering Director._\n\n\
         ## Specification Changes\n\n_To be written by the Specifier._\n\n\
         ## Implementation Architecture\n\n_To be written by the Software Architect._\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_sections_are_parsed_and_skeletons_carry_every_heading() {
        let skeleton = plan_skeleton("Add login", 1, None);
        for section in PLAN_SECTIONS {
            assert!(plan_section(&skeleton, section).is_some(), "{section}");
        }
        let text = "# T (v1)\n\n## Overview\n\nSummary here.\n\n## Specification Changes\n\nSpec.\n\n## Implementation Architecture\n\nArch.\n";
        assert_eq!(
            plan_section(text, "Overview").as_deref(),
            Some("Summary here.")
        );
        assert_eq!(
            plan_section(text, "Implementation Architecture").as_deref(),
            Some("Arch.")
        );
        assert_eq!(plan_section(text, "Nope"), None);
        let v2 = plan_skeleton("Add login", 2, Some(text));
        assert!(v2.starts_with("# Add login (v2)\n"));
        assert!(v2.contains("Summary here."));
    }

    #[test]
    fn review_issues_block_only_when_severe() {
        let blocker: ReviewIssue =
            serde_json::from_str(r#"{"severity":"Blocker","location":"a.rs","description":"x"}"#)
                .unwrap();
        let minor: ReviewIssue =
            serde_json::from_str(r#"{"severity":"minor","description":"y"}"#).unwrap();
        let unspecified: ReviewIssue = serde_json::from_str(r#"{"description":"z"}"#).unwrap();
        assert!(blocker.blocks());
        assert!(!minor.blocks());
        assert!(unspecified.blocks());
        assert!(minor.render().starts_with("- [minor]"));
    }
}
