// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::Path;

use anyhow::{bail, Context, Result};
use clyean_agents::AgentId;
use clyean_orchestrator::scaffold::{refresh_sandbox_files, PendingScaffold, SandboxPreparation};
use clyean_project::{ProjectLayout, SandboxId};
use clyean_sandbox::orphans::{list_user_assistant_containers, prune_orphaned_user_assistants};
use clyean_sandbox::rootfs::RootfsMarker;
use clyean_sandbox::roots::{StoredRoot, SANDBOX_LABEL};
use clyean_sandbox::{Helpers, HostOs, LaunchRole, Podman, PodmanEnvironment};

use crate::cli::{ProjectArgs, SandboxAction, SandboxArgs};
use crate::runtime::{PodmanSandbox, ProjectRuntime};

/// A root filesystem without a marker is an interrupted population once it is this old.
const INCOMPLETE_ROOT_AGE_SECONDS: u64 = 24 * 60 * 60;

pub async fn run(project: &ProjectArgs, args: SandboxArgs) -> Result<i32> {
    if let SandboxAction::Prune { remove } = args.action {
        return prune(&Podman::default(), remove);
    }
    let runtime = ProjectRuntime::resolve(project)?;
    let podman_sandbox = runtime.podman_sandbox()?;
    match args.action {
        SandboxAction::Status => status(&runtime, &podman_sandbox),
        SandboxAction::Build => build(&runtime, &podman_sandbox, false).await,
        SandboxAction::Rebuild => build(&runtime, &podman_sandbox, true).await,
        SandboxAction::Shell => shell(&runtime, &podman_sandbox),
        SandboxAction::Prune { .. } => unreachable!("handled above"),
    }
}

fn status(runtime: &ProjectRuntime, podman_sandbox: &PodmanSandbox) -> Result<i32> {
    let location = &podman_sandbox.location;
    println!("Sandbox:      {}", location.id);
    println!("Location:     {}", location.root);
    let fs = location.fs(&runtime.podman);
    let helpers = Helpers::new(
        runtime.podman.clone(),
        podman_sandbox.environment.sandbox_roots(),
    );
    let code = match RootfsMarker::read_existing(fs.as_ref(), &helpers, &location.id)? {
        Some(marker) => {
            println!("Image:        {} ({})", marker.image, marker.image_digest);
            println!(
                "Provisioned:  {} by clyean {} (schema {})",
                marker.provisioned_at, marker.clyean_version, marker.provisioning_version
            );
            match marker.harness_sha256.get(..12) {
                Some(digest) => {
                    println!("Harness:      {} (sha256 {digest})", marker.harness_version)
                }
                None => println!("Harness:      {}", marker.harness_version),
            }
            println!(
                "Last used:    {} from {}",
                marker.last_used_at, marker.project_dir
            );
            0
        }
        None => {
            println!("Not provisioned; run `clyean sandbox build` or just `clyean`.");
            1
        }
    };
    let assistants = list_user_assistant_containers(
        &runtime.podman,
        &[format!("{SANDBOX_LABEL}={}", location.id)],
    )?;
    let running: Vec<&str> = assistants
        .iter()
        .filter(|c| c.is_running())
        .map(|c| c.name())
        .collect();
    if running.is_empty() {
        println!("User Assistants: none running");
    } else {
        println!("User Assistants: {}", running.join(", "));
    }
    Ok(code)
}

async fn build(
    runtime: &ProjectRuntime,
    podman_sandbox: &PodmanSandbox,
    discard_first: bool,
) -> Result<i32> {
    let pending = PendingScaffold::default();
    let sandbox = runtime.sandbox_config(&pending)?;
    if discard_first {
        let location = &podman_sandbox.location;
        let in_use = runtime.podman.output([
            "ps",
            "--quiet",
            "--filter",
            &format!("label={SANDBOX_LABEL}={}", location.id),
        ])?;
        if !in_use.trim().is_empty() {
            bail!(
                "the sandbox {} is in use by running containers; end the clyean processes of this project first",
                location.id
            );
        }
        println!("Discarding {}", location.root);
        Helpers::new(
            runtime.podman.clone(),
            podman_sandbox.environment.sandbox_roots(),
        )
        .remove(&location.id)
        .context("removing the sandbox root filesystem")?;
    }
    println!("Ensuring the sandbox is provisioned from {}", sandbox.image);
    let (marker, preparation) = runtime.ensure_sandbox(podman_sandbox, &sandbox).await?;
    match &preparation {
        SandboxPreparation::Provisioned => println!(
            "Provisioned sandbox with harness {} at {}",
            marker.harness_version, marker.provisioned_at
        ),
        SandboxPreparation::HarnessReplaced { source } => {
            println!("Replaced the sandbox's harness with {}.", source.display())
        }
        SandboxPreparation::Current => println!(
            "Sandbox already current (provisioned {}).",
            marker.provisioned_at
        ),
    }
    refresh_sandbox_files(
        &runtime.layout,
        &runtime.user,
        podman_sandbox.location.fs(&runtime.podman).as_ref(),
        marker,
    )?;
    Ok(0)
}

fn shell(runtime: &ProjectRuntime, podman_sandbox: &PodmanSandbox) -> Result<i32> {
    let pending = PendingScaffold::default();
    let config = if runtime.is_scaffolded() {
        runtime.load_config()?
    } else {
        runtime.provisional_config(&pending)
    };
    let context = runtime.launch_context(config, podman_sandbox);
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

/// Removes User Assistant containers, in every project, whose `clyean` process is gone,
/// and lists sandbox root filesystems whose project is gone, removing them on request.
fn prune(podman: &Podman, remove: bool) -> Result<i32> {
    let removed = prune_orphaned_user_assistants(podman)
        .context("removing orphaned User Assistant containers")?;
    if removed.is_empty() {
        println!("No orphaned User Assistant containers.");
    }
    for name in removed {
        println!("Removed orphaned User Assistant container {name}");
    }
    let host = HostOs::current();
    let environment =
        PodmanEnvironment::detect(podman, host).context("asking Podman about its environment")?;
    environment.check(host)?;
    let helpers = Helpers::new(podman.clone(), environment.sandbox_roots());
    let in_use = podman.output([
        "ps",
        "--filter",
        &format!("label={SANDBOX_LABEL}"),
        "--format",
        &format!("{{{{index .Labels \"{SANDBOX_LABEL}\"}}}}"),
    ])?;
    let in_use: Vec<&str> = in_use.lines().map(str::trim).collect();
    let roots = helpers.list().context("listing sandbox root filesystems")?;
    println!(
        "Sandbox root filesystems in {}:",
        environment.sandbox_roots().roots_dir()
    );
    if roots.is_empty() {
        println!("  none");
    }
    let mut orphans = Vec::new();
    for root in &roots {
        let used = in_use.contains(&root.id.as_str());
        let verdict = classify(root, used);
        let project = root
            .marker
            .as_ref()
            .and_then(|marker| marker["projectDir"].as_str())
            .unwrap_or("unknown project");
        let last_use = root
            .marker
            .as_ref()
            .and_then(|marker| marker["lastUsedAt"].as_str())
            .unwrap_or("never");
        println!(
            "  {}  {project}  last used {last_use}  {} MiB  {}",
            root.id,
            root.size_kib / 1024,
            verdict.describe()
        );
        if verdict.is_orphan() {
            orphans.push(root.id.clone());
        }
    }
    if orphans.is_empty() {
        return Ok(0);
    }
    if !remove {
        println!(
            "{} orphaned; run `clyean sandbox prune --remove` to delete them.  A project that was moved looks orphaned until its next launch.",
            orphans.len()
        );
        return Ok(0);
    }
    for id in orphans {
        let id = SandboxId::try_from(id).map_err(anyhow::Error::msg)?;
        helpers
            .remove(&id)
            .with_context(|| format!("removing the sandbox root filesystem {id}"))?;
        println!("Removed the sandbox root filesystem {id}");
    }
    Ok(0)
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    InUse,
    Kept,
    ProjectMissing,
    ProjectMoved,
    Incomplete,
}

impl Verdict {
    fn is_orphan(&self) -> bool {
        matches!(
            self,
            Self::ProjectMissing | Self::ProjectMoved | Self::Incomplete
        )
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::InUse => "in use",
            Self::Kept => "kept",
            Self::ProjectMissing => "orphaned: the project directory is gone",
            Self::ProjectMoved => "orphaned: the project directory holds another sandbox",
            Self::Incomplete => "orphaned: an interrupted population",
        }
    }
}

/// Whether a root filesystem still belongs to the project that last used it.
fn classify(root: &StoredRoot, used: bool) -> Verdict {
    if used {
        return Verdict::InUse;
    }
    let Some(marker) = &root.marker else {
        return if root.age_seconds > INCOMPLETE_ROOT_AGE_SECONDS {
            Verdict::Incomplete
        } else {
            Verdict::Kept
        };
    };
    let Some(project_dir) = marker["projectDir"].as_str() else {
        return Verdict::Kept;
    };
    let project_dir = Path::new(project_dir);
    if !project_dir.is_dir() {
        return Verdict::ProjectMissing;
    }
    match SandboxId::load(&ProjectLayout::new(project_dir)) {
        Ok(Some(id)) if id.as_str() == root.id => Verdict::Kept,
        Ok(_) => Verdict::ProjectMoved,
        Err(_) => Verdict::Kept,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root(id: &str, age_seconds: u64, marker: Option<serde_json::Value>) -> StoredRoot {
        StoredRoot {
            id: id.to_string(),
            size_kib: 2048,
            age_seconds,
            marker,
        }
    }

    #[test]
    fn roots_are_orphaned_only_when_their_project_no_longer_claims_them() {
        let project = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(project.path());
        let id = SandboxId::load_or_create(&layout).unwrap();
        let marker = |dir: &Path| Some(json!({"projectDir": dir.to_string_lossy()}));

        let owned = root(id.as_str(), 10, marker(project.path()));
        assert_eq!(classify(&owned, false), Verdict::Kept);
        assert_eq!(classify(&owned, true), Verdict::InUse);

        let copied_away = root("0000000000000000", 10, marker(project.path()));
        assert_eq!(classify(&copied_away, false), Verdict::ProjectMoved);

        let deleted = root(id.as_str(), 10, marker(&project.path().join("gone")));
        assert_eq!(classify(&deleted, false), Verdict::ProjectMissing);

        assert_eq!(classify(&root("1", 60, None), false), Verdict::Kept);
        assert_eq!(
            classify(&root("1", 2 * INCOMPLETE_ROOT_AGE_SECONDS, None), false),
            Verdict::Incomplete
        );
        assert!(!Verdict::InUse.is_orphan());
        assert!(Verdict::ProjectMoved.is_orphan());
    }
}
