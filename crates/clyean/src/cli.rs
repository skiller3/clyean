// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// The release tag's version, which release builds stamp through `CLYEAN_RELEASE_VERSION`, or
/// the workspace version for any other build.
pub const VERSION: &str = match option_env!("CLYEAN_RELEASE_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Clyean: the slop-scrubbing agentic coding harness.
///
/// Without a subcommand, `clyean` scaffolds the current directory when needed and starts a
/// User Assistant agent of its own in the project's Podman sandbox.
#[derive(Debug, Parser)]
#[command(name = "clyean", version = VERSION, about, long_about = None, arg_required_else_help = false)]
pub struct Cli {
    /// Print detailed diagnostics to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(flatten)]
    pub project: ProjectArgs,

    #[command(flatten)]
    pub launch: LaunchArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args, Clone)]
pub struct ProjectArgs {
    /// Project directory (default: the current directory).
    #[arg(long, global = true, value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Workspace directory: the project directory or one of its ancestors.  It is mounted
    /// read-write into the sandbox.
    #[arg(long, global = true, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct LaunchArgs {
    /// Continue the User Assistant's previous session.
    #[arg(short = 'c', long)]
    pub r#continue: bool,

    /// Resume a User Assistant session by id prefix or path, or open the picker.
    #[arg(short = 'r', long, value_name = "SESSION", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Model or configured role for the User Assistant, for this launch.
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// Non-interactive mode: send the prompt to the User Assistant, print the result, and exit.
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Do not save the User Assistant session.
    #[arg(long)]
    pub no_session: bool,

    /// Answer the Git worktree question for a new project.
    #[arg(long, value_enum, value_name = "yes|no")]
    pub worktrees: Option<YesNo>,

    /// Image to populate a new project's sandbox from (default: ubuntu:latest).
    #[arg(long, value_name = "IMAGE")]
    pub image: Option<String>,

    /// Host path to mount read-only under /mnt in the sandbox (repeatable).
    #[arg(long = "mount", value_name = "PATH")]
    pub mounts: Vec<PathBuf>,

    /// Initial prompt for the User Assistant.
    #[arg(value_name = "PROMPT", trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum YesNo {
    Yes,
    No,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold the project without launching the User Assistant.
    Scaffold(ScaffoldArgs),
    /// Inspect or rebuild the project's Podman sandbox.
    Sandbox(SandboxArgs),
    /// List the Clyean agents and their status.
    Agents,
    /// List the project's change plans.
    Plans,
    /// List units of work recorded for the project.
    Work,
}

#[derive(Debug, Args, Clone)]
pub struct ScaffoldArgs {
    /// Project type; when omitted, only the host-side scaffold (Git, agent files, sandbox) is prepared.
    #[arg(long, value_enum)]
    pub project_type: Option<ProjectTypeArg>,

    /// Answer the Git worktree question for a new project.
    #[arg(long, value_enum, value_name = "yes|no")]
    pub worktrees: Option<YesNo>,

    /// Image to populate the sandbox from (default: ubuntu:latest).
    #[arg(long, value_name = "IMAGE")]
    pub image: Option<String>,

    /// Host path to mount read-only under /mnt in the sandbox (repeatable).
    #[arg(long = "mount", value_name = "PATH")]
    pub mounts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProjectTypeArg {
    #[value(name = "software-engineering")]
    SoftwareEngineering,
    #[value(name = "miscellaneous")]
    Miscellaneous,
}

#[derive(Debug, Args, Clone)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub action: SandboxAction,
}

#[derive(Debug, Subcommand, Clone)]
pub enum SandboxAction {
    /// Show the project's sandbox identifier and location, what provisioned it, and its
    /// running User Assistants.
    Status,
    /// Populate and provision the sandbox root filesystem if it is missing or outdated.
    Build,
    /// Discard the sandbox root filesystem and provision it again, unless it is in use.
    Rebuild,
    /// Open an interactive shell inside the sandbox.
    Shell,
    /// Remove User Assistant containers, in every project, whose clyean process is gone,
    /// and list sandbox root filesystems whose project is gone.
    Prune {
        /// Also remove the orphaned sandbox root filesystems.
        #[arg(long)]
        remove: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_invocation_launches_with_a_prompt() {
        let cli = Cli::try_parse_from(["clyean", "fix", "the", "build"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.launch.prompt, vec!["fix", "the", "build"]);
    }

    #[test]
    fn subcommands_and_flags_parse() {
        let cli = Cli::try_parse_from([
            "clyean",
            "--cwd",
            "/p",
            "scaffold",
            "--project-type",
            "software-engineering",
            "--worktrees",
            "no",
        ])
        .unwrap();
        assert_eq!(cli.project.cwd.as_deref(), Some(std::path::Path::new("/p")));
        match cli.command {
            Some(Command::Scaffold(args)) => {
                assert_eq!(args.project_type, Some(ProjectTypeArg::SoftwareEngineering));
                assert_eq!(args.worktrees, Some(YesNo::No));
            }
            other => panic!("unexpected {other:?}"),
        }
        let cli = Cli::try_parse_from(["clyean", "-r"]).unwrap();
        assert_eq!(cli.launch.resume.as_deref(), Some(""));
        let cli = Cli::try_parse_from(["clyean", "sandbox", "rebuild"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Sandbox(SandboxArgs {
                action: SandboxAction::Rebuild
            }))
        ));
        let cli = Cli::try_parse_from(["clyean", "sandbox", "prune", "--remove"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Sandbox(SandboxArgs {
                action: SandboxAction::Prune { remove: true }
            }))
        ));
    }
}
