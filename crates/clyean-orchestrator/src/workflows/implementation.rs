// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The implementation workflow of `workflow-implementation.mmd`: specifications, then
//! architecture, then programming, then an architecture review with bounded remediation.
//! A blocking issue found while programming re-runs planning into a new plan version.

use std::path::Path;

use clyean_agents::AgentId;
use clyean_project::PlanReference;

use super::planning::{
    plan_dir_relative, plan_paths, resume_pending_information, run_planning_phases,
};
use super::{
    plan_skeleton, CompletionVerdict, ImplementationReviewVerdict, ProgrammerVerdict, WorkContext,
    MAX_REPLANNING_CYCLES, MAX_REVIEW_ROUNDS,
};
use crate::journal::{Phase, WorkJournal};
use crate::prompts;
use crate::{OrchestratorError, Result};

const REVIEW_ISSUES_KEY: &str = "implementation.review.issues";
const PROGRAMMER_SUMMARY_KEY: &str = "implementation.programming.summary";
const MAX_RENDER_ATTEMPTS: u32 = 3;

pub async fn run_implementation(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    loop {
        resume_pending_information(ctx, journal).await?;
        match journal.phase {
            Phase::Pending => setup(ctx, journal).await?,
            Phase::PlanningOverviewInformation
            | Phase::PlanningOverview
            | Phase::PlanningSpecification
            | Phase::PlanningSpecificationReview
            | Phase::PlanningArchitecture
            | Phase::PlanningArchitectureReview => run_planning_phases(ctx, journal).await?,
            Phase::ImplementationSpecs => apply_specs(ctx, journal).await?,
            Phase::ImplementationArchitecture => apply_architecture(ctx, journal).await?,
            Phase::ImplementationProgramming | Phase::ImplementationRemediation => {
                program(ctx, journal).await?
            }
            Phase::ImplementationReview => review(ctx, journal).await?,
            Phase::Done => return Ok(()),
            other => {
                return Err(OrchestratorError::Workflow(format!(
                    "phase {} does not belong to the implementation workflow",
                    other.label()
                )))
            }
        }
    }
}

async fn setup(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    match journal.plan.clone() {
        Some(reference) if !reference.contains("/v") => {
            let resolved = ctx
                .services
                .plans
                .resolve(&PlanReference::parse(&reference)?)?;
            let previous_path = ctx.journal_path(journal);
            journal.plan = Some(format!("{}/v{}", resolved.name, resolved.version));
            journal.set_phase(Phase::ImplementationSpecs);
            ctx.save(journal)?;
            let new_path = ctx.journal_path(journal);
            if previous_path != new_path && previous_path.exists() {
                let _ = std::fs::remove_file(previous_path);
            }
            ctx.sink
                .progress(
                    "orchestrator",
                    Phase::Pending,
                    format!(
                        "implementing existing change plan {}",
                        journal.plan.as_deref().unwrap_or_default()
                    ),
                )
                .await;
            Ok(())
        }
        Some(_) => {
            journal.set_phase(Phase::ImplementationSpecs);
            ctx.save(journal)
        }
        None => {
            ctx.sink
                .progress(
                    "orchestrator",
                    Phase::Pending,
                    "no change plan was referenced; planning first",
                )
                .await;
            run_planning_phases(ctx, journal).await
        }
    }
}

async fn apply_specs(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let (verdict, _) = ctx
        .ask_agent::<CompletionVerdict>(
            AgentId::Specifier,
            journal,
            Phase::ImplementationSpecs,
            prompts::specifier_apply_specs(&relative),
        )
        .await?;
    match verdict {
        CompletionVerdict::Completed { .. } => {
            ctx.commit_step(
                journal,
                &[Path::new(".clyean/SPECS.md")],
                "Update specifications for change plan",
                AgentId::Specifier,
            )
            .await?;
            journal.set_phase(Phase::ImplementationArchitecture);
            ctx.save(journal)
        }
        CompletionVerdict::Blocked { issue } => Err(OrchestratorError::Workflow(format!(
            "the Specifier could not update SPECS.md: {issue}"
        ))),
    }
}

async fn apply_architecture(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let mut render_failures: Option<String> = None;
    for attempt in 1..=MAX_RENDER_ATTEMPTS {
        let prompt = prompts::architect_apply_architecture(&relative, render_failures.as_deref());
        let (verdict, _) = ctx
            .ask_agent::<CompletionVerdict>(
                AgentId::SoftwareArchitect,
                journal,
                Phase::ImplementationArchitecture,
                prompt,
            )
            .await?;
        if let CompletionVerdict::Blocked { issue } = verdict {
            return Err(OrchestratorError::Workflow(format!(
                "the Software Architect could not update the architecture materials: {issue}"
            )));
        }
        let report = ctx.services.renderer.render().await?;
        if report.all_succeeded() {
            ctx.commit_step(
                journal,
                &[Path::new(".clyean/architecture")],
                "Update architecture diagrams for change plan",
                AgentId::SoftwareArchitect,
            )
            .await?;
            journal.set_phase(Phase::ImplementationProgramming);
            return ctx.save(journal);
        }
        let failures: Vec<String> = report
            .failures()
            .map(|f| match &f.result {
                clyean_plantuml::RenderResult::Failed { exit_code, stderr } => {
                    format!(
                        "- {} (exit {:?}): {}",
                        f.source_file_name,
                        exit_code,
                        stderr.trim()
                    )
                }
                clyean_plantuml::RenderResult::Rendered => String::new(),
            })
            .collect();
        ctx.sink
            .progress(
                "orchestrator",
                Phase::ImplementationArchitecture,
                format!(
                    "{} diagram(s) failed to render (attempt {attempt})",
                    failures.len()
                ),
            )
            .await;
        render_failures = Some(failures.join("\n"));
    }
    Err(OrchestratorError::Workflow(format!(
        "architecture diagrams still fail to render after {MAX_RENDER_ATTEMPTS} attempts: {}",
        render_failures.unwrap_or_default()
    )))
}

async fn program(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let issues = journal.scratch_string(REVIEW_ISSUES_KEY);
    let phase = journal.phase;
    let prompt = prompts::programmer_implement(&relative, issues.as_deref());
    let (verdict, _) = ctx
        .ask_agent::<ProgrammerVerdict>(AgentId::Programmer, journal, phase, prompt)
        .await?;
    match verdict {
        ProgrammerVerdict::ReadyForReview { summary } => {
            journal.scratch.insert(
                PROGRAMMER_SUMMARY_KEY.into(),
                serde_json::Value::String(summary),
            );
            journal.scratch.remove(REVIEW_ISSUES_KEY);
            ctx.commit_step(
                journal,
                &[Path::new(".")],
                "Implement change plan",
                AgentId::Programmer,
            )
            .await?;
            journal.set_phase(Phase::ImplementationReview);
            ctx.save(journal)
        }
        ProgrammerVerdict::Blocked {
            issue,
            suggested_resolution,
        } => replan_after_blocking_issue(ctx, journal, issue, suggested_resolution).await,
    }
}

async fn replan_after_blocking_issue(
    ctx: &WorkContext,
    journal: &mut WorkJournal,
    issue: String,
    suggested_resolution: String,
) -> Result<()> {
    let cycle = journal.next_iteration(Phase::ImplementationRemediation);
    if cycle > MAX_REPLANNING_CYCLES {
        return Err(OrchestratorError::Workflow(format!(
            "implementation stayed blocked after {MAX_REPLANNING_CYCLES} re-planning cycles: {issue}"
        )));
    }
    let concern = if suggested_resolution.trim().is_empty() {
        issue.clone()
    } else {
        format!("{issue} (suggested resolution: {suggested_resolution})")
    };
    journal.blocking_issues.push(concern);
    let (host_path, _) = plan_paths(ctx, journal)?;
    let previous = std::fs::read_to_string(&host_path)
        .map_err(|e| OrchestratorError::io(format!("reading {}", host_path.display()), e))?;
    let name = journal
        .plan
        .as_deref()
        .and_then(|p| p.split('/').next())
        .unwrap_or_default()
        .to_string();
    let next = ctx.services.plans.next_version_path(&name)?;
    let title = previous
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("# "))
        .map(|t| {
            t.rsplit_once(" (v")
                .map(|(t, _)| t)
                .unwrap_or(t)
                .to_string()
        })
        .unwrap_or_else(|| name.clone());
    std::fs::write(
        &next.path,
        plan_skeleton(&title, next.version, Some(&previous)),
    )
    .map_err(|e| OrchestratorError::io(format!("writing {}", next.path.display()), e))?;
    journal.plan = Some(format!("{name}/v{}", next.version));
    journal
        .scratch
        .retain(|key, _| !key.starts_with("planning."));
    ctx.sink
        .progress(
            "orchestrator",
            Phase::ImplementationProgramming,
            format!(
                "blocking issue reported; re-planning as {} (cycle {cycle})",
                journal.plan.as_deref().unwrap_or_default()
            ),
        )
        .await;
    let plan_dir = plan_dir_relative(journal).expect("plan set");
    ctx.commit_step(
        journal,
        &[&plan_dir],
        "Start new change plan version after blocking issue",
        AgentId::SoftwareEngineeringDirector,
    )
    .await?;
    journal.set_phase(Phase::PlanningOverviewInformation);
    ctx.save(journal)
}

async fn review(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    let (_, relative) = plan_paths(ctx, journal)?;
    let summary = journal
        .scratch_string(PROGRAMMER_SUMMARY_KEY)
        .unwrap_or_default();
    let prompt = prompts::architect_review_implementation(&relative, &summary);
    let (verdict, _) = ctx
        .ask_agent::<ImplementationReviewVerdict>(
            AgentId::SoftwareArchitect,
            journal,
            Phase::ImplementationReview,
            prompt,
        )
        .await?;
    let minor_notes = match &verdict {
        ImplementationReviewVerdict::Issues { issues } if !issues.iter().any(|i| i.blocks()) => {
            Some(
                issues
                    .iter()
                    .map(|i| i.render())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        _ => None,
    };
    match verdict {
        ImplementationReviewVerdict::Issues { issues } if minor_notes.is_none() => {
            let round = journal.next_iteration(Phase::ImplementationReview);
            if round > MAX_REVIEW_ROUNDS {
                return Err(OrchestratorError::Workflow(format!(
                    "the implementation still had blocking review issues after {MAX_REVIEW_ROUNDS} remediation rounds"
                )));
            }
            let rendered = issues
                .iter()
                .map(|i| i.render())
                .collect::<Vec<_>>()
                .join("\n");
            ctx.sink
                .progress(
                    "orchestrator",
                    Phase::ImplementationReview,
                    format!(
                        "review found {} issue(s); remediation round {round}",
                        issues.len()
                    ),
                )
                .await;
            journal.scratch.insert(
                REVIEW_ISSUES_KEY.into(),
                serde_json::Value::String(rendered),
            );
            journal.set_phase(Phase::ImplementationRemediation);
            ctx.save(journal)
        }
        ImplementationReviewVerdict::Approved {
            summary: review_summary,
        } => complete(ctx, journal, &relative, &summary, &review_summary, None).await,
        ImplementationReviewVerdict::Issues { .. } => {
            complete(
                ctx,
                journal,
                &relative,
                &summary,
                "approved with minor notes",
                minor_notes.as_deref(),
            )
            .await
        }
    }
}

async fn complete(
    ctx: &WorkContext,
    journal: &mut WorkJournal,
    plan_relative: &str,
    programmer_summary: &str,
    review_summary: &str,
    minor_notes: Option<&str>,
) -> Result<()> {
    let mut summary = format!(
        "Change plan {plan_relative} is implemented and approved by the Software Architect. Programmer: {programmer_summary} Review: {review_summary}"
    );
    if let Some(notes) = minor_notes {
        summary.push_str("\nMinor review notes:\n");
        summary.push_str(notes);
    }
    let artifacts = vec![
        plan_relative.to_string(),
        ".clyean/SPECS.md".to_string(),
        ".clyean/architecture".to_string(),
    ];
    journal.complete(summary.clone(), artifacts.clone());
    ctx.save(journal)?;
    ctx.commit_step(
        journal,
        &[Path::new(".")],
        "Complete change plan implementation",
        AgentId::SoftwareEngineeringDirector,
    )
    .await?;
    ctx.sink
        .completed(&summary, &artifacts, journal.plan.as_deref())
        .await;
    Ok(())
}
