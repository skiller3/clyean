// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use anyhow::{Context, Result};
use clyean_agents::AgentId;
use clyean_orchestrator::scaffold::{project_agent_profiles, PendingScaffold};
use clyean_sandbox::rootfs::{self, RootfsMarker};
use clyean_sandbox::LaunchRole;

use crate::cli::{ProjectArgs, SandboxAction, SandboxArgs};
use crate::runtime::ProjectRuntime;

pub async fn run(project: &ProjectArgs, args: SandboxArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(project)?;
    match args.action {
        SandboxAction::Status => status(&runtime),
        SandboxAction::Build => build(&runtime, false).await,
        SandboxAction::Rebuild => build(&runtime, true).await,
        SandboxAction::Shell => shell(&runtime),
    }
}

fn status(runtime: &ProjectRuntime) -> Result<i32> {
    let root = runtime.layout.container_root_dir();
    match RootfsMarker::read(&root)? {
        Some(marker) => {
            println!("Sandbox root: {}", root.display());
            println!("Image:        {} ({})", marker.image, marker.image_digest);
            println!(
                "Provisioned:  {} by clyean {} (schema {})",
                marker.provisioned_at, marker.clyean_version, marker.provisioning_version
            );
            println!("Harness:      {}", marker.harness_version);
            Ok(0)
        }
        None if rootfs::is_populated(&root) => {
            println!(
                "Sandbox root {} is populated but not provisioned; run `clyean sandbox build`.",
                root.display()
            );
            Ok(1)
        }
        None => {
            println!(
                "No sandbox root at {}; run `clyean sandbox build` or just `clyean`.",
                root.display()
            );
            Ok(1)
        }
    }
}

async fn build(runtime: &ProjectRuntime, discard_first: bool) -> Result<i32> {
    let pending = PendingScaffold::default();
    let sandbox = runtime.sandbox_config(&pending)?;
    if discard_first {
        println!(
            "Discarding {}",
            runtime.layout.container_root_dir().display()
        );
        rootfs::remove(&runtime.podman, &runtime.layout.container_root_dir())
            .context("removing the sandbox root")?;
    }
    println!("Ensuring the sandbox is provisioned from {}", sandbox.image);
    let (marker, provisioned) = runtime.ensure_sandbox(&sandbox).await?;
    project_agent_profiles(&runtime.layout, &runtime.user)?;
    if provisioned {
        println!(
            "Provisioned sandbox with harness {} at {}",
            marker.harness_version, marker.provisioned_at
        );
    } else {
        println!(
            "Sandbox already current (provisioned {}).",
            marker.provisioned_at
        );
    }
    Ok(0)
}

fn shell(runtime: &ProjectRuntime) -> Result<i32> {
    let pending = PendingScaffold::default();
    let config = if runtime.is_scaffolded() {
        runtime.load_config()?
    } else {
        runtime.provisional_config(&pending)
    };
    let context = runtime.launch_context(config, None);
    let mut spec = context.agent_container_spec(
        AgentId::SoftwareArchitect,
        LaunchRole::Maintenance,
        Vec::new(),
    );
    spec.tty = true;
    spec.command = vec![
        "/bin/sh".into(),
        "-lc".into(),
        "exec bash 2>/dev/null || exec sh".into(),
    ];
    let status = runtime.podman.run_interactive(spec.run_args())?;
    Ok(status.code().unwrap_or(1))
}
