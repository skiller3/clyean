// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A thin wrapper over the `git` command line with the standardized identity that every
//! Clyean agent uses when it authors commits.

use std::path::{Path, PathBuf};
use std::process::Command;

pub mod identity;

pub use identity::{agent_commit_message, CommitIdentity, AGENT_TRAILER_KEY};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed or not on PATH: {0}")]
    NotInstalled(std::io::Error),
    #[error("git {command} failed in {workdir}: {stderr}")]
    CommandFailed {
        command: String,
        workdir: PathBuf,
        stderr: String,
    },
    #[error("{0} is not inside a Git repository")]
    NotARepository(PathBuf),
}

pub type Result<T> = std::result::Result<T, GitError>;

/// A Git working directory together with the identity used for commits made through it.
#[derive(Debug, Clone)]
pub struct GitRepository {
    workdir: PathBuf,
}

/// The result of a commit attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// A commit was created with this full SHA.
    Committed(String),
    /// Nothing was staged, so no commit was created.
    NothingToCommit,
}

impl GitRepository {
    /// Wraps an existing working directory without validating it.
    pub fn at(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
        }
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Finds the repository containing `path`, if any.
    pub fn discover(path: &Path) -> Result<Option<Self>> {
        let output = git_output(path, &["rev-parse", "--show-toplevel"]);
        match output {
            Ok(top_level) => Ok(Some(Self::at(top_level.trim()))),
            Err(GitError::CommandFailed { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Initializes a repository at `path` with `main` as the initial branch.
    pub fn init(path: &Path) -> Result<Self> {
        git_output(path, &["init", "--initial-branch=main", "--quiet"])?;
        Ok(Self::at(path))
    }

    /// Whether Git's effective ignore rules cover `path` (relative to the working directory).
    pub fn is_ignored(&self, path: &Path) -> Result<bool> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.workdir)
            .args(["check-ignore", "-q", "--"])
            .arg(path)
            .output()
            .map_err(GitError::NotInstalled)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitError::CommandFailed {
                command: format!("check-ignore {}", path.display()),
                workdir: self.workdir.clone(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
        }
    }

    /// Whether the working directory is a linked worktree rather than the main checkout.
    pub fn is_linked_worktree(&self) -> Result<bool> {
        let git_dir = git_output(&self.workdir, &["rev-parse", "--git-dir"])?;
        let common_dir = git_output(&self.workdir, &["rev-parse", "--git-common-dir"])?;
        Ok(git_dir.trim() != common_dir.trim())
    }

    /// Stages `paths` (relative to the working directory), tolerating deleted files.
    pub fn stage(&self, paths: &[&Path]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args: Vec<&std::ffi::OsStr> = vec!["add".as_ref(), "--all".as_ref(), "--".as_ref()];
        args.extend(paths.iter().map(|p| p.as_os_str()));
        git_output_os(&self.workdir, &args).map(|_| ())
    }

    /// Whether the index holds staged changes.
    pub fn has_staged_changes(&self) -> Result<bool> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.workdir)
            .args(["diff", "--cached", "--quiet"])
            .output()
            .map_err(GitError::NotInstalled)?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(GitError::CommandFailed {
                command: "diff --cached --quiet".to_string(),
                workdir: self.workdir.clone(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
        }
    }

    /// Commits the index with the given identity and message, skipping when nothing is staged.
    pub fn commit(&self, identity: &CommitIdentity, message: &str) -> Result<CommitOutcome> {
        if !self.has_staged_changes()? {
            return Ok(CommitOutcome::NothingToCommit);
        }
        let author = identity.as_author_string();
        git_output(
            &self.workdir,
            &[
                "-c",
                &format!("user.name={}", identity.name),
                "-c",
                &format!("user.email={}", identity.email),
                "commit",
                "--quiet",
                "--no-verify",
                "--author",
                &author,
                "--message",
                message,
            ],
        )?;
        let sha = git_output(&self.workdir, &["rev-parse", "HEAD"])?;
        Ok(CommitOutcome::Committed(sha.trim().to_string()))
    }

    /// Stages `paths` and commits them in one step.
    pub fn stage_and_commit(
        &self,
        paths: &[&Path],
        identity: &CommitIdentity,
        message: &str,
    ) -> Result<CommitOutcome> {
        self.stage(paths)?;
        self.commit(identity, message)
    }

    pub fn head_sha(&self) -> Result<Option<String>> {
        match git_output(&self.workdir, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            Ok(sha) => Ok(Some(sha.trim().to_string())),
            Err(GitError::CommandFailed { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// The subject line of the commit at `sha`.
    pub fn commit_subject(&self, sha: &str) -> Result<String> {
        git_output(&self.workdir, &["log", "-1", "--format=%s", sha]).map(|s| s.trim().to_string())
    }

    /// The full message body of the commit at `sha`.
    pub fn commit_message(&self, sha: &str) -> Result<String> {
        git_output(&self.workdir, &["log", "-1", "--format=%B", sha])
    }
}

fn git_output(workdir: &Path, args: &[&str]) -> Result<String> {
    let os_args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
    git_output_os(workdir, &os_args)
}

fn git_output_os(workdir: &Path, args: &[&std::ffi::OsStr]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(GitError::NotInstalled)?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            command: args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            workdir: workdir.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CommitIdentity {
        CommitIdentity::new("Clyean Programmer", "programmer@agents.clyean.com")
    }

    #[test]
    fn init_stage_commit_and_discover() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepository::init(dir.path()).unwrap();
        assert_eq!(repo.head_sha().unwrap(), None);
        assert_eq!(
            repo.commit(&identity(), "empty").unwrap(),
            CommitOutcome::NothingToCommit
        );

        std::fs::write(dir.path().join("hello.txt"), "hi\n").unwrap();
        let message = agent_commit_message("Add greeting", None, "programmer");
        let outcome = repo
            .stage_and_commit(&[Path::new("hello.txt")], &identity(), &message)
            .unwrap();
        let sha = match outcome {
            CommitOutcome::Committed(sha) => sha,
            other => panic!("expected a commit, got {other:?}"),
        };
        assert_eq!(repo.commit_subject(&sha).unwrap(), "Add greeting");
        assert!(repo
            .commit_message(&sha)
            .unwrap()
            .contains("Clyean-Agent: programmer"));

        let nested = dir.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        let discovered = GitRepository::discover(&nested).unwrap().unwrap();
        assert_eq!(
            std::fs::canonicalize(discovered.workdir()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert!(!repo.is_linked_worktree().unwrap());
    }

    #[test]
    fn check_ignore_consults_effective_rules() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        assert!(repo.is_ignored(Path::new("ignored/file.txt")).unwrap());
        assert!(!repo.is_ignored(Path::new("kept/file.txt")).unwrap());
    }

    #[test]
    fn discover_outside_a_repository_is_none() {
        let dir = tempfile::tempdir().unwrap();
        // A temp dir under /tmp is not inside a repository unless the machine is unusual.
        let result = GitRepository::discover(dir.path()).unwrap();
        if let Some(repo) = result {
            assert!(!dir.path().starts_with(repo.workdir()) || repo.workdir() != dir.path());
        }
    }
}
