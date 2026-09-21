// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Research prompts: a single Software Engineering Director session answers them.

use clyean_agents::AgentId;

use super::WorkContext;
use crate::journal::{Phase, WorkJournal};
use crate::prompts;
use crate::Result;

pub async fn run_research(ctx: &WorkContext, journal: &mut WorkJournal) -> Result<()> {
    journal.set_phase(Phase::Research);
    ctx.save(journal)?;
    let answer = ctx
        .ask_agent_freeform(
            AgentId::SoftwareEngineeringDirector,
            journal,
            Phase::Research,
            prompts::director_research(journal),
        )
        .await?;
    journal.complete(answer.clone(), Vec::new());
    ctx.save(journal)?;
    ctx.sink.completed(&answer, &[], None).await;
    Ok(())
}
