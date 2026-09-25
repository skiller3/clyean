// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The deterministic half of scaffolding.  `prepare_host_files` runs before the User
//! Assistant can start (Git repository, agent files, ignore rules), `ensure_sandbox` and
//! `refresh_sandbox_files` prepare the sandbox root filesystem, and `complete_scaffold`
//! runs once the project type is known and writes the remaining scaffold files; the
//! Scaffolder agent then does the research.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clyean_agents::{managed_extensions, AgentId, ProfileProjection};
use clyean_git::{agent_commit_message, GitRepository};
use clyean_plantuml::DIAGRAM_TYPES;
use clyean_project::config::DEFAULT_SANDBOX_IMAGE;
use clyean_project::ignore::ensure_ignore_rules;
use clyean_project::{ProjectConfig, ProjectDirectory, ProjectLayout, ProjectType, SandboxConfig};
use clyean_sandbox::provisioning::{
    ensure_plantuml_jar, provision, resolve_sandbox_executable, ProvisioningInputs, HARNESS,
    PROVISIONING_VERSION,
};
use clyean_sandbox::rootfs::RootfsMarker;
use clyean_sandbox::{
    ContainerUser, Helpers, Podman, PodmanEnvironment, SandboxArchive, SandboxFs, SandboxLocation,
};

use crate::{OrchestratorError, Result};

/// Settings chosen at launch for a project that is not scaffolded yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingScaffold {
    pub use_worktrees: bool,
    pub sandbox: SandboxConfig,
}

impl Default for PendingScaffold {
    fn default() -> Self {
        Self {
            use_worktrees: false,
            sandbox: SandboxConfig::with_image(DEFAULT_SANDBOX_IMAGE),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostScaffoldReport {
    pub git_initialized: bool,
    pub agent_files_created: Vec<PathBuf>,
    pub ignore_rules_added: Vec<&'static str>,
}

/// Ensures the repository, the agent files, and the ignore rules exist.  Idempotent.
pub fn prepare_host_files(
    directory: &ProjectDirectory,
    layout: &ProjectLayout,
) -> Result<(GitRepository, HostScaffoldReport)> {
    let mut report = HostScaffoldReport::default();
    let git = match GitRepository::discover(directory.project())? {
        Some(repository) => repository,
        None => {
            report.git_initialized = true;
            GitRepository::init(directory.project())?
        }
    };
    std::fs::create_dir_all(layout.agents_dir())
        .map_err(|e| OrchestratorError::io("creating .clyean/agents", e))?;
    for agent in AgentId::implemented() {
        if clyean_agents::instructions::write_baseline_if_missing(layout, agent)? {
            report
                .agent_files_created
                .push(layout.agents_dir().join(agent.instruction_file_name()));
        }
        report
            .agent_files_created
            .extend(clyean_agents::settings::write_seeds_if_missing(
                layout, agent,
            )?);
    }
    let git_for_ignore = git.clone();
    let root = layout.root().to_path_buf();
    report.ignore_rules_added = ensure_ignore_rules(layout, |path| {
        let relative = path.strip_prefix(&root).unwrap_or(path);
        git_for_ignore.is_ignored(relative).map_err(|e| {
            clyean_project::ProjectError::io(
                "git check-ignore",
                std::io::Error::other(e.to_string()),
            )
        })
    })?;
    Ok((git, report))
}

/// Inputs of the sandbox part of host scaffolding.
pub struct SandboxInputs<'a> {
    pub podman: &'a Podman,
    pub environment: &'a PodmanEnvironment,
    pub location: &'a SandboxLocation,
    pub layout: &'a ProjectLayout,
    pub user: &'a ContainerUser,
    pub sandbox: &'a SandboxConfig,
    pub clyean_version: &'a str,
    pub cache_dir: &'a Path,
}

/// Populates and provisions the project's sandbox root filesystem unless a current one
/// exists.  Returns the marker and whether provisioning ran.
pub async fn ensure_sandbox(inputs: &SandboxInputs<'_>) -> Result<(RootfsMarker, bool)> {
    let fs = inputs.location.fs(inputs.podman);
    let helpers = Helpers::new(inputs.podman.clone(), inputs.environment.sandbox_roots());
    let existing = RootfsMarker::read_existing(fs.as_ref(), &helpers, &inputs.location.id)?;
    if let Some(marker) = &existing {
        if marker.provisioning_version >= PROVISIONING_VERSION
            && marker.image == inputs.sandbox.image
        {
            return Ok((marker.clone(), false));
        }
        tracing::info!(target: "clyean::scaffold", "sandbox is outdated; re-provisioning");
    }
    let arch_tag = inputs.environment.arch_tag()?;
    let id = inputs.location.id.clone();
    let image = inputs.sandbox.image.clone();
    let image_digest = match existing {
        Some(marker) if marker.image == inputs.sandbox.image => marker.image_digest,
        _ => {
            tracing::info!(target: "clyean::scaffold", image = %image, root = %inputs.location.root, "populating the sandbox root filesystem");
            tokio::task::spawn_blocking(move || {
                if helpers.is_populated(&id)? {
                    helpers.remove(&id)?;
                }
                helpers.populate(&id, &image)
            })
            .await
            .map_err(|e| OrchestratorError::Workflow(format!("populate task failed: {e}")))??
        }
    };
    let jar = ensure_plantuml_jar(inputs.cache_dir).await?;
    let harness = resolve_sandbox_executable(
        &HARNESS,
        inputs.sandbox.harness_binary.as_deref(),
        inputs.clyean_version,
        arch_tag,
        inputs.cache_dir,
    )
    .await?;
    tracing::info!(target: "clyean::scaffold", harness = %harness.path.display(), origin = ?harness.origin, "provisioning the sandbox");
    let podman = inputs.podman.clone();
    let location = inputs.location.clone();
    let user = inputs.user.clone();
    let image = inputs.sandbox.image.clone();
    let clyean_version = inputs.clyean_version.to_string();
    let project_dir = inputs.layout.root().to_path_buf();
    let marker = tokio::task::spawn_blocking(move || {
        provision(&ProvisioningInputs {
            podman: &podman,
            location: &location,
            fs: fs.as_ref(),
            user: &user,
            image: &image,
            image_digest: &image_digest,
            clyean_version: &clyean_version,
            plantuml_jar: &jar,
            harness_binary: &harness.path,
            project_dir: &project_dir,
        })
    })
    .await
    .map_err(|e| OrchestratorError::Workflow(format!("provisioning task failed: {e}")))??;
    Ok((marker, true))
}

/// Projects every implemented agent's profile into the root filesystem, once the project
/// has agent files, and records this project's use in the marker, in one write.
pub fn refresh_sandbox_files(
    layout: &ProjectLayout,
    user: &ContainerUser,
    fs: &dyn SandboxFs,
    marker: RootfsMarker,
) -> Result<()> {
    let projections = if layout.agents_dir().is_dir() {
        AgentId::implemented()
            .map(|agent| ProfileProjection::for_agent(layout, agent, managed_extensions(agent)))
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let reads: Vec<String> = projections
        .iter()
        .flat_map(|projection| projection.reads(user.name()))
        .collect();
    let contents = fs.read_files(&reads.iter().map(String::as_str).collect::<Vec<_>>())?;
    let existing: HashMap<String, Vec<u8>> = reads
        .into_iter()
        .zip(contents)
        .filter_map(|(path, contents)| contents.map(|contents| (path, contents)))
        .collect();
    let mut archive = SandboxArchive::new();
    for projection in &projections {
        for file in projection.files(user.name(), &existing)? {
            archive.file(&file.path, file.contents, file.mode);
        }
    }
    marker.used_by(layout.root()).add_to(&mut archive);
    fs.write(archive)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldReport {
    pub config: ProjectConfig,
    pub created: Vec<PathBuf>,
    pub commit: Option<String>,
}

/// Writes `project.json`, the specification skeleton, the diagram skeletons, and the
/// plans directory, then commits the scaffold.  Idempotent for files that already exist.
pub fn complete_scaffold(
    directory: &ProjectDirectory,
    layout: &ProjectLayout,
    git: &GitRepository,
    pending: &PendingScaffold,
    project_type: ProjectType,
    clyean_version: &str,
) -> Result<ScaffoldReport> {
    let mut created = Vec::new();
    let config = ProjectConfig::new(
        clyean_version,
        project_type,
        directory.workspace(),
        pending.use_worktrees,
        pending.sandbox.clone(),
    );
    config.save(layout)?;
    created.push(layout.project_config_path());

    let project_name = directory
        .project()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    if !layout.specs_path().exists() {
        std::fs::write(
            layout.specs_path(),
            specs_skeleton(&project_name, project_type),
        )
        .map_err(|e| OrchestratorError::io("writing .clyean/SPECS.md", e))?;
        created.push(layout.specs_path());
    }
    std::fs::create_dir_all(layout.architecture_dir())
        .map_err(|e| OrchestratorError::io("creating .clyean/architecture", e))?;
    for diagram in DIAGRAM_TYPES {
        let path = layout.architecture_dir().join(diagram.source_file_name());
        if !path.exists() {
            std::fs::write(&path, diagram.render_template(&project_name))
                .map_err(|e| OrchestratorError::io(format!("writing {}", path.display()), e))?;
            created.push(path);
        }
    }
    std::fs::create_dir_all(layout.plans_dir())
        .map_err(|e| OrchestratorError::io("creating .clyean/plans", e))?;
    let keep = layout.plans_dir().join(".gitkeep");
    if !keep.exists() {
        std::fs::write(&keep, "")
            .map_err(|e| OrchestratorError::io("writing .clyean/plans/.gitkeep", e))?;
        created.push(keep);
    }

    let message = agent_commit_message(
        "Scaffold Clyean project",
        Some(&format!("Project type: {}.", project_type.as_str())),
        AgentId::Scaffolder.id(),
    );
    let outcome = git.stage_and_commit(
        &[Path::new(".clyean")],
        &AgentId::Scaffolder.git_identity(),
        &message,
    )?;
    let commit = match outcome {
        clyean_git::CommitOutcome::Committed(sha) => Some(sha),
        clyean_git::CommitOutcome::NothingToCommit => None,
    };
    Ok(ScaffoldReport {
        config,
        created,
        commit,
    })
}

fn specs_skeleton(project_name: &str, project_type: ProjectType) -> String {
    let nature = match project_type {
        ProjectType::SoftwareEngineeringProject => {
            "the software system's externally legible behavior"
        }
        ProjectType::MiscellaneousProject => "the purpose and structure of the project's materials",
    };
    format!(
        "# {project_name} specifications\n\n\
         This file states the current requirements of {project_name}: {nature}.  It is maintained by Clyean's Specifier agent and describes the project as it is, not as it might become.\n\n\
         ## Purpose\n\n_To be researched and written by the Scaffolder agent._\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_files_and_scaffold_completion_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let directory = ProjectDirectory::resolve(Some(dir.path()), None).unwrap();
        let layout = ProjectLayout::new(directory.project());
        let (git, report) = prepare_host_files(&directory, &layout).unwrap();
        assert!(report.git_initialized);
        assert_eq!(report.agent_files_created.len(), 6 * 3);
        assert!(report.ignore_rules_added.contains(&"*.local.json"));
        let (_, second) = prepare_host_files(&directory, &layout).unwrap();
        assert!(!second.git_initialized);
        assert!(second.agent_files_created.is_empty());
        assert!(second.ignore_rules_added.is_empty());
        assert!(git
            .is_ignored(Path::new(".clyean/sandbox.local.json"))
            .unwrap());

        let report = complete_scaffold(
            &directory,
            &layout,
            &git,
            &PendingScaffold::default(),
            ProjectType::SoftwareEngineeringProject,
            "0.1.0",
        )
        .unwrap();
        assert!(layout.is_scaffolded());
        assert!(report.commit.is_some());
        assert_eq!(report.created.len(), 1 + 1 + 14 + 1);
        assert!(layout.architecture_dir().join("timing.puml").is_file());
        let again = complete_scaffold(
            &directory,
            &layout,
            &git,
            &PendingScaffold::default(),
            ProjectType::SoftwareEngineeringProject,
            "0.1.0",
        )
        .unwrap();
        assert_eq!(again.created.len(), 1, "only project.json is rewritten");
        let subject = git
            .commit_subject(report.commit.as_deref().unwrap())
            .unwrap();
        assert_eq!(subject, "Scaffold Clyean project");
    }
}
