// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The planning workflow of `workflow-planning.mmd`: the Software Engineering Director
//! writes the plan overview (asking the user for information through the User
//! Assistant when needed), the Specifier and the Software Architect enrich the plan, and
//! the Director reviews each enrichment until it is aligned.

use std::path::{Path, PathBuf};

use clyean_agents::AgentId;
use clyean_project::plans::slugify;

use super::{
    plan_section, plan_skeleton, EnrichmentVerdict, InformationVerdict, ReviewVerdict, WorkContext,
    MAX_REVIEW_ROUNDS,
};
use crate::journal::{Phase, WorkJournal, WorkKind};
use crate::prompts;
use crate::{OrchestratorError, Result};

const MAX_OVERVIEW_CHARACTERS: usize = 1200;
const OVERVIEW_PLACEHOLDER: &str = "_To be written by the Software Engineering Director._";

/// Runs planning phases from the journal's current phase until planning is complete.
/// For a planning work the journal ends in `Done`; for an implementation work it ends in
/// `ImplementationSpecs`.
pub async fn run_planning_phases(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    loop {
        resume_pending_information(ctx, journal).await?;
        match journal.phase {
            Phase::Pending => setup_plan(ctx, journal).await?,
            Phase::PlanningOverviewInformation => overview_information(ctx, journal).await?,
            Phase::PlanningOverview => write_overview(ctx, journal).await?,
            Phase::PlanningSpecification => {
                enrich_section(ctx, journal, PlanSection::Specification).await?
            }
            Phase::PlanningSpecificationReview => {
                review_section(ctx, journal, PlanSection::Specification).await?
            }
            Phase::PlanningArchitecture => {
                enrich_section(ctx, journal, PlanSection::Architecture).await?
            }
            Phase::PlanningArchitectureReview => {
                review_section(ctx, journal, PlanSection::Architecture).await?
            }
            _ => return Ok(()),
        }
    }
}

/// Re-emits a pending information request after a restart and waits for its answers.
pub async fn resume_pending_information(
    ctx: &WorkContext,
    journal: &mut WorkJournal,
) -> Result<()> {
    let Some(pending) = journal.pending_information().cloned() else {
        return Ok(());
    };
    ctx.sink
        .information_requested(
            &pending.request_id,
            &pending.questions,
            &format!("asked by {}", pending.asked_by.display_name()),
        )
        .await;
    ctx.await_answers(journal, &pending.request_id).await?;
    Ok(())
}

/// The plan version file of the journal's plan: host path and project-relative path.
pub fn plan_paths(ctx: &WorkContext, journal: &WorkJournal) -> Result<(PathBuf, String)> {
    let reference = journal
        .plan
        .as_deref()
        .ok_or_else(|| OrchestratorError::Workflow("the work has no change plan".into()))?;
    let (name, version) = reference.split_once("/v").ok_or_else(|| {
        OrchestratorError::Workflow(format!("malformed plan reference {reference}"))
    })?;
    let relative = format!(".clyean/plans/{name}/v{version}.md");
    Ok((ctx.services.layout.root().join(&relative), relative))
}

pub fn plan_dir_relative(journal: &WorkJournal) -> Option<PathBuf> {
    let reference = journal.plan.as_deref()?;
    let name = reference.split('/').next()?;
    Some(Path::new(".clyean").join("plans").join(name))
}

fn plan_title(journal: &WorkJournal) -> String {
    let source = journal
        .original_prompt
        .as_deref()
        .unwrap_or(&journal.prompt);
    let first_line = source
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Change");
    let title: String = first_line.chars().take(72).collect();
    title.trim().to_string()
}

async fn setup_plan(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let previous_path = ctx.journal_path(journal);
    if journal.plan.is_none() {
        let title = plan_title(journal);
        let date = clyean_project::utc_now_rfc3339()[..10].to_string();
        let name = ctx.services.plans.create_plan(&slugify(&title), &date)?;
        let version = ctx.services.plans.next_version_path(&name)?;
        std::fs::write(&version.path, plan_skeleton(&title, version.version, None))
            .map_err(|e| OrchestratorError::io(format!("writing {}", version.path.display()), e))?;
        journal.plan = Some(format!("{name}/v{}", version.version));
        ctx.sink
            .progress(
                "orchestrator",
                Phase::Pending,
                format!(
                    "created change plan {}",
                    journal.plan.as_deref().unwrap_or_default()
                ),
            )
            .await;
    }
    journal.set_phase(Phase::PlanningOverviewInformation);
    ctx.save(journal)?;
    let new_path = ctx.journal_path(journal);
    if previous_path != new_path && previous_path.exists() {
        let _ = std::fs::remove_file(previous_path);
    }
    let plan_dir = plan_dir_relative(journal).expect("plan set above");
    ctx.commit_step(
        journal,
        &[&plan_dir],
        "Create change plan skeleton",
        AgentId::SoftwareEngineeringDirector,
    )
    .await
}

async fn overview_information(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let prompt = prompts::director_overview_information(journal, &relative);
    let (verdict, _) = ctx
        .ask_agent::<InformationVerdict>(
            AgentId::SoftwareEngineeringDirector,
            journal,
            Phase::PlanningOverviewInformation,
            prompt,
        )
        .await?;
    match verdict {
        InformationVerdict::NeedsInformation { questions } if !questions.is_empty() => {
            ctx.request_information(
                journal,
                AgentId::SoftwareEngineeringDirector,
                Phase::PlanningOverviewInformation,
                questions,
            )
            .await?;
        }
        _ => {
            journal.set_phase(Phase::PlanningOverview);
            ctx.save(journal)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
enum OverviewVerdict {
    OverviewWritten,
    #[serde(other)]
    Other,
}

async fn write_overview(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (host_path, relative) = plan_paths(ctx, journal)?;
    let prompt = prompts::director_write_overview(journal, &relative);
    ctx.ask_agent::<OverviewVerdict>(
        AgentId::SoftwareEngineeringDirector,
        journal,
        Phase::PlanningOverview,
        prompt,
    )
    .await?;
    let overview = read_section(&host_path, "Overview")?;
    if overview.is_empty() || overview == OVERVIEW_PLACEHOLDER {
        return Err(OrchestratorError::Workflow(
            "the Software Engineering Director did not write the plan Overview section".into(),
        ));
    }
    if overview.chars().count() > MAX_OVERVIEW_CHARACTERS {
        let shorten = format!(
            "The Overview section of `{relative}` is {} characters; shorten it to at most {MAX_OVERVIEW_CHARACTERS} characters without losing the substance, then reply with the verdict {{\"decision\": \"overview_written\"}}.",
            overview.chars().count()
        );
        ctx.ask_agent::<OverviewVerdict>(
            AgentId::SoftwareEngineeringDirector,
            journal,
            Phase::PlanningOverview,
            shorten,
        )
        .await?;
    }
    let plan_dir = plan_dir_relative(journal).expect("plan set");
    ctx.commit_step(
        journal,
        &[&plan_dir],
        "Write change plan overview",
        AgentId::SoftwareEngineeringDirector,
    )
    .await?;
    journal.set_phase(Phase::PlanningSpecification);
    ctx.save(journal)
}

fn read_section(plan_path: &Path, section: &str) -> Result<String> {
    let text = std::fs::read_to_string(plan_path)
        .map_err(|e| OrchestratorError::io(format!("reading {}", plan_path.display()), e))?;
    Ok(plan_section(&text, section).unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanSection {
    Specification,
    Architecture,
}

impl PlanSection {
    fn heading(self) -> &'static str {
        match self {
            Self::Specification => "Specification Changes",
            Self::Architecture => "Implementation Architecture",
        }
    }

    fn agent(self) -> AgentId {
        match self {
            Self::Specification => AgentId::Specifier,
            Self::Architecture => AgentId::SoftwareArchitect,
        }
    }

    fn enrich_phase(self) -> Phase {
        match self {
            Self::Specification => Phase::PlanningSpecification,
            Self::Architecture => Phase::PlanningArchitecture,
        }
    }

    fn review_phase(self) -> Phase {
        match self {
            Self::Specification => Phase::PlanningSpecificationReview,
            Self::Architecture => Phase::PlanningArchitectureReview,
        }
    }

    fn summary_key(self) -> String {
        format!("{}.summary", self.enrich_phase().label())
    }

    fn issues_key(self) -> String {
        format!("{}.issues", self.enrich_phase().label())
    }

    fn prompt(self, journal: &WorkJournal, relative: &str, issues: Option<&[String]>) -> String {
        match self {
            Self::Specification => prompts::specifier_enrich(journal, relative, issues),
            Self::Architecture => prompts::architect_enrich(journal, relative, issues),
        }
    }
}

async fn enrich_section(
    ctx: &WorkContext,
    journal: &mut WorkJournal,
    section: PlanSection,
) -> Result<()> {
    let (host_path, relative) = plan_paths(ctx, journal)?;
    let issues = journal.scratch_strings(&section.issues_key());
    let prompt = section.prompt(journal, &relative, issues.as_deref());
    let (verdict, _) = ctx
        .ask_agent::<EnrichmentVerdict>(section.agent(), journal, section.enrich_phase(), prompt)
        .await?;
    match verdict {
        EnrichmentVerdict::NeedsInformation { questions } if !questions.is_empty() => {
            ctx.resolve_questions(journal, section.agent(), section.enrich_phase(), questions)
                .await?;
            Ok(())
        }
        EnrichmentVerdict::NeedsInformation { .. } | EnrichmentVerdict::Completed { .. } => {
            let summary = match verdict {
                EnrichmentVerdict::Completed { summary } => summary,
                _ => String::new(),
            };
            if read_section(&host_path, section.heading())?.is_empty() {
                return Err(OrchestratorError::Workflow(format!(
                    "the {} did not write the {} section",
                    section.agent().display_name(),
                    section.heading()
                )));
            }
            journal
                .scratch
                .insert(section.summary_key(), serde_json::Value::String(summary));
            journal.scratch.remove(&section.issues_key());
            let plan_dir = plan_dir_relative(journal).expect("plan set");
            ctx.commit_step(
                journal,
                &[&plan_dir],
                &format!("Write change plan {}", section.heading().to_lowercase()),
                section.agent(),
            )
            .await?;
            journal.set_phase(section.review_phase());
            ctx.save(journal)
        }
    }
}

async fn review_section(
    ctx: &WorkContext,
    journal: &mut WorkJournal,
    section: PlanSection,
) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let summary = journal
        .scratch_string(&section.summary_key())
        .unwrap_or_default();
    let prompt = prompts::director_review_section(section.heading(), &relative, &summary);
    let (verdict, _) = ctx
        .ask_agent::<ReviewVerdict>(
            AgentId::SoftwareEngineeringDirector,
            journal,
            section.review_phase(),
            prompt,
        )
        .await?;
    match verdict {
        ReviewVerdict::Aligned => {
            ctx.sink
                .progress(
                    "orchestrator",
                    section.review_phase(),
                    format!("{} aligned", section.heading()),
                )
                .await;
            match section {
                PlanSection::Specification => journal.set_phase(Phase::PlanningArchitecture),
                PlanSection::Architecture => finish_planning(ctx, journal).await?,
            }
            ctx.save(journal)
        }
        ReviewVerdict::Misaligned { issues } => {
            let round = journal.next_iteration(section.review_phase());
            if round > MAX_REVIEW_ROUNDS {
                return Err(OrchestratorError::Workflow(format!(
                    "the {} section was still misaligned after {MAX_REVIEW_ROUNDS} revisions: {}",
                    section.heading(),
                    issues.join("; ")
                )));
            }
            ctx.sink
                .progress(
                    "orchestrator",
                    section.review_phase(),
                    format!(
                        "{} misaligned (round {round}); asking for a revision",
                        section.heading()
                    ),
                )
                .await;
            journal
                .scratch
                .insert(section.issues_key(), serde_json::json!(issues));
            journal.set_phase(section.enrich_phase());
            ctx.save(journal)
        }
    }
}

async fn finish_planning(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    if journal.kind == WorkKind::Planning {
        let (_, relative) = plan_paths(ctx, journal)?;
        let summary = format!(
            "Change plan {} is authored at {relative} and ready for review.",
            journal.plan.as_deref().unwrap_or_default()
        );
        journal.complete(summary.clone(), vec![relative.clone()]);
        ctx.save(journal)?;
        let plan_dir = plan_dir_relative(journal).expect("plan set");
        ctx.commit_step(
            journal,
            &[&plan_dir],
            "Complete change plan",
            AgentId::SoftwareEngineeringDirector,
        )
        .await?;
        ctx.sink
            .completed(&summary, &[relative], journal.plan.as_deref())
            .await;
    } else {
        journal.set_phase(Phase::ImplementationSpecs);
    }
    Ok(())
}
