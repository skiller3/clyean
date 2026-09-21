// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The default command: prepare the host scaffold and sandbox, serve the orchestrator,
//! and attach the terminal to the User Assistant agent running in its container.

use std::sync::Arc;

use anyhow::{Context, Result};
use clyean_agents::AgentId;
use clyean_orchestrator::scaffold::{prepare_host_files, project_agent_profiles, PendingScaffold};
use clyean_orchestrator::server;
use clyean_orchestrator::service::{
    OrchestratorService, ProjectServices, ProjectState, UnscaffoldedProject,
};
use clyean_project::identity::orchestrator_socket_path;
use clyean_project::PlanCatalog;
use clyean_sandbox::{AgentContainerSpec, LaunchContext, LaunchRole};
use tokio_util::sync::CancellationToken;

use crate::cli::{LaunchArgs, ProjectArgs, VERSION};
use crate::runtime::{ProjectRuntime, SandboxRenderer, SandboxSessionFactory};

pub async fn run(project: &ProjectArgs, args: LaunchArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(project)?;
    let pending = if runtime.is_scaffolded() {
        PendingScaffold::default()
    } else {
        runtime.pending_scaffold(args.worktrees, args.image.clone(), args.mounts.clone())?
    };
    let (git, report) = prepare_host_files(&runtime.directory, &runtime.layout)?;
    if report.git_initialized {
        eprintln!(
            "Initialized a Git repository in {}",
            runtime.directory.project().display()
        );
    }
    let sandbox = runtime.sandbox_config(&pending)?;
    let (marker, provisioned) = runtime.ensure_sandbox(&sandbox).await?;
    if provisioned {
        eprintln!(
            "Provisioned the Podman sandbox (harness {}).",
            marker.harness_version
        );
    }
    project_agent_profiles(&runtime.layout, &runtime.user)?;

    let socket_path = orchestrator_socket_path(&runtime.project_id);
    let config = if runtime.is_scaffolded() {
        runtime.load_config()?
    } else {
        runtime.provisional_config(&pending)
    };
    let context = runtime.launch_context(config.clone(), Some(socket_path.clone()));
    let renderer = Arc::new(SandboxRenderer::new(context.clone()));
    let factory = Arc::new(SandboxSessionFactory::new(context.clone()));
    let state = if runtime.is_scaffolded() {
        ProjectState::Scaffolded(Arc::new(ProjectServices {
            directory: runtime.directory.clone(),
            layout: runtime.layout.clone(),
            config,
            git,
            plans: PlanCatalog::new(&runtime.layout),
            renderer,
            factory,
            clyean_version: VERSION.to_string(),
        }))
    } else {
        ProjectState::Unscaffolded(Box::new(UnscaffoldedProject {
            directory: runtime.directory.clone(),
            layout: runtime.layout.clone(),
            git,
            pending,
            renderer,
            factory,
            clyean_version: VERSION.to_string(),
        }))
    };
    let service = Arc::new(OrchestratorService::new(state, VERSION));
    let shutdown = CancellationToken::new();
    let server_task = {
        let service = service.clone();
        let shutdown = shutdown.clone();
        let socket_path = socket_path.clone();
        tokio::spawn(async move { server::serve(service, &socket_path, shutdown).await })
    };
    wait_for_socket(&socket_path).await?;

    let harness_args = user_assistant_harness_args(&context, &args);
    let exit_code = if args.print {
        run_print_mode(&runtime, &context, harness_args).await?
    } else {
        run_interactive(&runtime, &context, harness_args).await?
    };

    shutdown.cancel();
    let _ = server_task.await;
    Ok(exit_code)
}

fn user_assistant_harness_args(context: &LaunchContext, args: &LaunchArgs) -> Vec<String> {
    let mut harness_args = context.base_harness_args(AgentId::UserAssistant);
    if args.r#continue {
        harness_args.push("--continue".into());
    }
    if let Some(session) = &args.resume {
        harness_args.push("--resume".into());
        if !session.is_empty() {
            harness_args.push(session.clone());
        }
    }
    if let Some(model) = &args.model {
        harness_args.push("--model".into());
        harness_args.push(model.clone());
    }
    if args.no_session {
        harness_args.push("--no-session".into());
    }
    if args.print {
        harness_args.push("--print".into());
    }
    if !args.prompt.is_empty() {
        harness_args.push("--".into());
        harness_args.push(args.prompt.join(" "));
    }
    harness_args
}

async fn wait_for_socket(path: &std::path::Path) -> Result<()> {
    for _ in 0..100 {
        if path.exists() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    anyhow::bail!("the orchestrator socket {} did not appear", path.display())
}

async fn run_print_mode(
    runtime: &ProjectRuntime,
    context: &LaunchContext,
    harness_args: Vec<String>,
) -> Result<i32> {
    let mut spec = context.agent_container_spec(
        AgentId::UserAssistant,
        LaunchRole::UserAssistant,
        harness_args,
    );
    spec.tty = false;
    spec.remove_on_exit = true;
    spec.name = format!("{}-print-{}", spec.name, std::process::id());
    let podman = runtime.podman.clone();
    let status =
        tokio::task::spawn_blocking(move || podman.run_interactive(spec.run_args())).await??;
    Ok(status.code().unwrap_or(1))
}

async fn run_interactive(
    runtime: &ProjectRuntime,
    context: &LaunchContext,
    harness_args: Vec<String>,
) -> Result<i32> {
    let spec = context.agent_container_spec(
        AgentId::UserAssistant,
        LaunchRole::UserAssistant,
        harness_args,
    );
    let podman = runtime.podman.clone();
    ignore_interrupts();
    if podman.container_is_running(&spec.name)? {
        eprintln!(
            "Attaching to the running User Assistant ({}). Detach with Ctrl-p Ctrl-q.",
            spec.name
        );
        let name = spec.name.clone();
        let status = tokio::task::spawn_blocking(move || {
            podman.run_interactive(["attach", "--detach-keys", "ctrl-p,ctrl-q", &name])
        })
        .await??;
        return Ok(status.code().unwrap_or(1));
    }
    podman
        .remove_container(&spec.name)
        .context("removing the previous User Assistant container")?;
    start_attached(&podman, &spec).await
}

async fn start_attached(podman: &clyean_sandbox::Podman, spec: &AgentContainerSpec) -> Result<i32> {
    podman
        .output(spec.create_args())
        .context("creating the User Assistant container")?;
    let podman_for_task = podman.clone();
    let name = spec.name.clone();
    let status = tokio::task::spawn_blocking(move || {
        podman_for_task.run_interactive([
            "start",
            "--attach",
            "--interactive",
            "--detach-keys",
            "ctrl-p,ctrl-q",
            &name,
        ])
    })
    .await??;
    if !podman.container_is_running(&spec.name)? {
        let _ = podman.remove_container(&spec.name);
    } else {
        eprintln!(
            "The User Assistant keeps running in container {}; run `clyean` again to re-attach.",
            spec.name
        );
    }
    Ok(status.code().unwrap_or(1))
}

/// Ctrl-C belongs to the attached terminal session, not to the host launcher.
fn ignore_interrupts() {
    tokio::spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
        }
    });
}
