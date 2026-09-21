// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The agentic half of scaffolding: after Clyean's deterministic logic has created the
//! scaffold, the Scaffolder researches the project and authors its specifications and
//! architecture, which are then rendered and committed.

use std::path::Path;

use clyean_agents::AgentId;

use super::{CompletionVerdict, WorkContext};
use crate::journal::{Phase, WorkJournal};
use crate::prompts;
use crate::{OrchestratorError, Result};

pub const PROJECT_TYPE_KEY: &str = "scaffold.project_type";

pub async fn run_scaffold(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    journal.set_phase(Phase::Scaffolding);
    ctx.save(journal)?;
    let project_type = journal
        .scratch_string(PROJECT_TYPE_KEY)
        .unwrap_or_else(|| ctx.services.config.project_type.as_str().to_string());
    let mut prompt = prompts::scaffolder_research(&project_type);
    for attempt in 1..=3u32 {
        let (verdict, _) = ctx
            .ask_agent::<CompletionVerdict>(
                AgentId::Scaffolder,
                journal,
                Phase::Scaffolding,
                prompt.clone(),
            )
            .await?;
        let summary = match verdict {
            CompletionVerdict::Completed { summary } => summary,
            CompletionVerdict::Blocked { issue } => {
                return Err(OrchestratorError::Workflow(format!(
                    "the Scaffolder was blocked: {issue}"
                )))
            }
        };
        let report = ctx.services.renderer.render().await?;
        if report.all_succeeded() {
            ctx.commit_step(
                journal,
                &[
                    Path::new(".clyean/SPECS.md"),
                    Path::new(".clyean/architecture"),
                ],
                "Author project specifications and architecture",
                AgentId::Scaffolder,
            )
            .await?;
            let artifacts = vec![
                ".clyean/SPECS.md".to_string(),
                ".clyean/architecture".to_string(),
            ];
            let summary = format!("Project scaffolded. {summary}");
            journal.complete(summary.clone(), artifacts.clone());
            ctx.save(journal)?;
            ctx.sink.completed(&summary, &artifacts, None).await;
            return Ok(());
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
                Phase::Scaffolding,
                format!(
                    "{} diagram(s) failed to render (attempt {attempt})",
                    failures.len()
                ),
            )
            .await;
        prompt = format!(
            "# Step: fix diagrams that failed to render\n\nThe following PlantUML sources failed to render:\n\n{}\n\nFix them so that every diagram renders, then reply with the verdict {{\"decision\": \"completed\", \"summary\": \"...\"}}.",
            failures.join("\n")
        );
    }
    Err(OrchestratorError::Workflow(
        "architecture diagrams still fail to render after three attempts".into(),
    ))
}
