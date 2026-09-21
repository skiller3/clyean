// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::sync::Arc;

use anyhow::{bail, Result};
use clyean_orchestrator::protocol::{Request, StreamedEvent};
use clyean_orchestrator::scaffold::{prepare_host_files, project_agent_profiles, PendingScaffold};
use clyean_orchestrator::service::{OrchestratorService, ProjectState, UnscaffoldedProject};
use clyean_project::ProjectType;
use serde_json::json;
use tokio::sync::broadcast;

use crate::cli::{ProjectArgs, ProjectTypeArg, ScaffoldArgs};
use crate::runtime::{ProjectRuntime, SandboxRenderer, SandboxSessionFactory};

pub async fn run(project: &ProjectArgs, args: ScaffoldArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(project)?;
    let pending = if runtime.is_scaffolded() {
        PendingScaffold::default()
    } else {
        runtime.pending_scaffold(args.worktrees, args.image, args.mounts)?
    };
    let (git, report) = prepare_host_files(&runtime.directory, &runtime.layout)?;
    if report.git_initialized {
        println!(
            "Initialized a Git repository in {}",
            runtime.directory.project().display()
        );
    }
    for path in &report.agent_files_created {
        println!(
            "Created {}",
            path.strip_prefix(runtime.layout.root())
                .unwrap_or(path)
                .display()
        );
    }
    let sandbox = runtime.sandbox_config(&pending)?;
    let (_, provisioned) = runtime.ensure_sandbox(&sandbox).await?;
    if provisioned {
        println!("Provisioned the sandbox root filesystem.");
    }
    project_agent_profiles(&runtime.layout, &runtime.user)?;

    let Some(project_type) = args.project_type else {
        if runtime.is_scaffolded() {
            println!("Project is already scaffolded; host scaffold refreshed.");
        } else {
            println!("Host scaffold prepared.  Run `clyean` to let the User Assistant finish scaffolding, or pass --project-type to scaffold now.");
        }
        return Ok(0);
    };
    if runtime.is_scaffolded() {
        bail!("the project is already scaffolded (.clyean/project.json exists)");
    }
    let project_type = match project_type {
        ProjectTypeArg::SoftwareEngineering => ProjectType::SoftwareEngineeringProject,
        ProjectTypeArg::Miscellaneous => ProjectType::MiscellaneousProject,
    };
    let context = runtime.launch_context(runtime.provisional_config(&pending), None);
    let service = Arc::new(OrchestratorService::new(
        ProjectState::Unscaffolded(Box::new(UnscaffoldedProject {
            directory: runtime.directory.clone(),
            layout: runtime.layout.clone(),
            git,
            pending,
            renderer: Arc::new(SandboxRenderer::new(context.clone())),
            factory: Arc::new(SandboxSessionFactory::new(context)),
            clyean_version: crate::cli::VERSION.to_string(),
        })),
        crate::cli::VERSION,
    ));
    let request = Request {
        id: "scaffold".into(),
        method: "project.scaffold".into(),
        params: json!({"project_type": project_type}),
    };
    let dispatch = service.dispatch(request).await;
    let response = serde_json::to_value(&dispatch.response)?;
    if let Some(error) = response.get("error") {
        bail!(
            "scaffolding failed: {}",
            error["message"].as_str().unwrap_or("unknown error")
        );
    }
    let Some(stream) = dispatch.stream else {
        bail!("scaffolding did not start");
    };
    print_stream(stream.receiver).await
}

/// Prints streamed events until a terminal one; returns the exit code.
pub async fn print_stream(mut receiver: broadcast::Receiver<StreamedEvent>) -> Result<i32> {
    loop {
        match receiver.recv().await {
            Ok(StreamedEvent::Progress { agent, text, .. }) => println!("[{agent}] {text}"),
            Ok(StreamedEvent::AgentOutput { agent, text, .. }) => print!("[{agent}] {text}"),
            Ok(StreamedEvent::InformationRequested { questions, .. }) => {
                println!(
                    "The workflow needs information that only the User Assistant can collect:"
                );
                for question in questions {
                    println!("  - {question}");
                }
                println!("Run `clyean` and ask the User Assistant to resume this work.");
                return Ok(2);
            }
            Ok(StreamedEvent::Completed { summary, .. }) => {
                println!("{summary}");
                return Ok(0);
            }
            Ok(StreamedEvent::Failed { message, .. }) => {
                eprintln!("error: {message}");
                return Ok(1);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return Ok(1),
        }
    }
}
