// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use anyhow::Result;
use clyean_agents::{AgentId, AgentStatus};
use clyean_orchestrator::journal::list_journals;
use clyean_project::PlanCatalog;

use crate::cli::ProjectArgs;
use crate::runtime::ProjectRuntime;

pub fn agents(args: &ProjectArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(args)?;
    println!("{:<32} {:<32} {:<12} INSTRUCTIONS", "ID", "NAME", "STATUS");
    for agent in AgentId::ALL {
        let status = match agent.status() {
            AgentStatus::Implemented => "implemented",
            AgentStatus::Placeholder => "placeholder",
        };
        let instructions = runtime
            .layout
            .agents_dir()
            .join(agent.instruction_file_name());
        let presence = if instructions.is_file() {
            format!(".clyean/agents/{}", agent.instruction_file_name())
        } else {
            "-".to_string()
        };
        println!(
            "{:<32} {:<32} {:<12} {}",
            agent.id(),
            agent.display_name(),
            status,
            presence
        );
    }
    Ok(0)
}

pub fn plans(args: &ProjectArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(args)?;
    let catalog = PlanCatalog::new(&runtime.layout);
    let names = catalog.list()?;
    if names.is_empty() {
        println!(
            "No change plans under {}.",
            runtime.layout.plans_dir().display()
        );
        return Ok(0);
    }
    println!("{:<48} {:<10} LATEST", "PLAN", "VERSIONS");
    for name in names {
        let versions = catalog.versions(&name)?;
        let latest = versions
            .last()
            .map(|v| {
                v.path
                    .strip_prefix(runtime.layout.root())
                    .unwrap_or(&v.path)
                    .display()
                    .to_string()
            })
            .unwrap_or_else(|| "-".to_string());
        println!("{:<48} {:<10} {}", name, versions.len(), latest);
    }
    Ok(0)
}

pub fn work(args: &ProjectArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(args)?;
    let journals = list_journals(&runtime.layout)?;
    if journals.is_empty() {
        println!("No units of work recorded for this project.");
        return Ok(0);
    }
    println!(
        "{:<34} {:<16} {:<22} {:<38} PLAN",
        "WORK", "KIND", "STATUS", "PHASE"
    );
    for (_, journal) in journals {
        println!(
            "{:<34} {:<16} {:<22} {:<38} {}",
            journal.work_id,
            format!("{:?}", journal.kind).to_lowercase(),
            format!("{:?}", journal.status).to_lowercase(),
            journal.phase.label(),
            journal.plan.as_deref().unwrap_or("-")
        );
    }
    Ok(0)
}
