// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::{Path, PathBuf};

use crate::{ProjectError, Result};

/// A canonical, absolute project directory together with its workspace directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDirectory {
    project: PathBuf,
    workspace: PathBuf,
}

impl ProjectDirectory {
    /// Resolves `project` (defaulting to the current directory) and `workspace`
    /// (defaulting to the project directory), validating that the workspace is
    /// the project directory or one of its ancestors.
    pub fn resolve(project: Option<&Path>, workspace: Option<&Path>) -> Result<Self> {
        let project = match project {
            Some(path) => canonicalize(path)?,
            None => {
                let cwd = std::env::current_dir()
                    .map_err(|e| ProjectError::io("reading the current directory", e))?;
                canonicalize(&cwd)?
            }
        };
        let workspace = match workspace {
            Some(path) => canonicalize(path)?,
            None => project.clone(),
        };
        Self::with_paths(project, workspace)
    }

    pub fn with_paths(project: PathBuf, workspace: PathBuf) -> Result<Self> {
        if !project.starts_with(&workspace) {
            return Err(ProjectError::WorkspaceNotAncestor { workspace, project });
        }
        Ok(Self { project, workspace })
    }

    pub fn project(&self) -> &Path {
        &self.project
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The project directory relative to the workspace directory (empty when equal).
    pub fn project_relative_to_workspace(&self) -> &Path {
        self.project
            .strip_prefix(&self.workspace)
            .expect("validated at construction")
    }

    /// The base name of the workspace directory, used as its mount name in the sandbox.
    pub fn workspace_name(&self) -> String {
        self.workspace
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string())
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .map_err(|e| ProjectError::io(format!("resolving {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_defaults_to_the_project_directory() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = ProjectDirectory::resolve(Some(dir.path()), None).unwrap();
        assert_eq!(resolved.project(), resolved.workspace());
        assert_eq!(resolved.project_relative_to_workspace(), Path::new(""));
    }

    #[test]
    fn workspace_may_be_an_ancestor_but_not_a_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("nested").join("project");
        let sibling = dir.path().join("sibling");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let resolved = ProjectDirectory::resolve(Some(&project), Some(dir.path())).unwrap();
        assert_eq!(
            resolved.project_relative_to_workspace(),
            Path::new("nested/project")
        );
        assert!(matches!(
            ProjectDirectory::resolve(Some(&project), Some(&sibling)),
            Err(ProjectError::WorkspaceNotAncestor { .. })
        ));
    }
}
