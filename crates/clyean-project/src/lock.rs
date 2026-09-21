// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The project lock that serializes potentially conflicting Clyean activity.  Projects
//! managed with Git worktrees do not need it, so acquiring the lock is a no-op there.

use std::fs::{File, OpenOptions};

use fd_lock::RwLock;

use crate::layout::ProjectLayout;

#[derive(Debug, thiserror::Error)]
pub enum ProjectLockError {
    #[error("the project is locked by another Clyean process ({0})")]
    Locked(std::path::PathBuf),
    #[error("cannot open lock file {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Holds the project lock until dropped.  `NoOp` is returned for worktree-managed projects.
#[derive(Debug)]
pub enum ProjectLock {
    NoOp,
    Held(HeldLock),
}

/// An exclusive advisory lock on `.clyean/lock`.
#[derive(Debug)]
pub struct HeldLock {
    _lock: RwLock<File>,
}

impl ProjectLock {
    /// Tries to take the lock without blocking.
    pub fn acquire(layout: &ProjectLayout, use_worktrees: bool) -> Result<Self, ProjectLockError> {
        if use_worktrees {
            return Ok(Self::NoOp);
        }
        let path = layout.lock_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectLockError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| ProjectLockError::Io {
                path: path.clone(),
                source,
            })?;
        let mut lock = RwLock::new(file);
        let guard = lock
            .try_write()
            .map_err(|_| ProjectLockError::Locked(path.clone()))?;
        // The guard borrows the lock; forgetting it keeps the OS lock until the file closes.
        std::mem::forget(guard);
        Ok(Self::Held(HeldLock { _lock: lock }))
    }

    /// Like [`ProjectLock::acquire`], but tolerates the brief window in which a child
    /// process forked by this process still holds an inherited copy of a lock that was
    /// just released: a genuinely held lock persists far longer than `attempts * delay`.
    pub fn acquire_with_retry(
        layout: &ProjectLayout,
        use_worktrees: bool,
        attempts: u32,
        delay: std::time::Duration,
    ) -> Result<Self, ProjectLockError> {
        let mut remaining = attempts.max(1);
        loop {
            match Self::acquire(layout, use_worktrees) {
                Err(ProjectLockError::Locked(path)) if remaining > 1 => {
                    remaining -= 1;
                    std::thread::sleep(delay);
                    if remaining == 1 {
                        return Self::acquire(layout, use_worktrees)
                            .map_err(|_| ProjectLockError::Locked(path));
                    }
                }
                other => return other,
            }
        }
    }

    pub fn is_no_op(&self) -> bool {
        matches!(self, Self::NoOp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_projects_do_not_lock() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let lock = ProjectLock::acquire(&layout, true).unwrap();
        assert!(lock.is_no_op());
        assert!(!layout.lock_path().exists());
    }

    #[test]
    fn second_acquisition_fails_while_the_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let first = ProjectLock::acquire(&layout, false).unwrap();
        assert!(!first.is_no_op());
        assert!(matches!(
            ProjectLock::acquire(&layout, false),
            Err(ProjectLockError::Locked(_))
        ));
        drop(first);
        assert!(ProjectLock::acquire(&layout, false).is_ok());
    }

    #[test]
    fn retrying_acquisition_gives_up_after_the_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let _held = ProjectLock::acquire(&layout, false).unwrap();
        let started = std::time::Instant::now();
        let result =
            ProjectLock::acquire_with_retry(&layout, false, 3, std::time::Duration::from_millis(5));
        assert!(matches!(result, Err(ProjectLockError::Locked(_))));
        assert!(started.elapsed() >= std::time::Duration::from_millis(10));
    }
}
