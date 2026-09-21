// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::path::{Path, PathBuf};

/// Name of the directory holding every Clyean scaffold resource of a project.
pub const CLYEAN_DIR_NAME: &str = ".clyean";
pub const PROJECT_CONFIG_FILE_NAME: &str = "project.json";
pub const PROJECT_LOCAL_CONFIG_FILE_NAME: &str = "project.local.json";
pub const AGENTS_DIR_NAME: &str = "agents";
pub const ARCHITECTURE_DIR_NAME: &str = "architecture";
pub const SPECS_FILE_NAME: &str = "SPECS.md";
pub const PLANS_DIR_NAME: &str = "plans";
pub const CONTAINER_ROOT_DIR_NAME: &str = "container-root";
pub const LOCK_FILE_NAME: &str = "lock";
pub const WORK_DIR_NAME: &str = "work";
pub const LOGS_DIR_NAME: &str = "logs";

/// Resolves the paths of the scaffold resources of one project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLayout {
    root: PathBuf,
}

impl ProjectLayout {
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: project_dir.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn clyean_dir(&self) -> PathBuf {
        self.root.join(CLYEAN_DIR_NAME)
    }

    pub fn project_config_path(&self) -> PathBuf {
        self.clyean_dir().join(PROJECT_CONFIG_FILE_NAME)
    }

    pub fn project_local_config_path(&self) -> PathBuf {
        self.clyean_dir().join(PROJECT_LOCAL_CONFIG_FILE_NAME)
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.clyean_dir().join(AGENTS_DIR_NAME)
    }

    pub fn architecture_dir(&self) -> PathBuf {
        self.clyean_dir().join(ARCHITECTURE_DIR_NAME)
    }

    pub fn specs_path(&self) -> PathBuf {
        self.clyean_dir().join(SPECS_FILE_NAME)
    }

    pub fn plans_dir(&self) -> PathBuf {
        self.clyean_dir().join(PLANS_DIR_NAME)
    }

    pub fn container_root_dir(&self) -> PathBuf {
        self.clyean_dir().join(CONTAINER_ROOT_DIR_NAME)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.clyean_dir().join(LOCK_FILE_NAME)
    }

    pub fn work_dir(&self) -> PathBuf {
        self.clyean_dir().join(WORK_DIR_NAME)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.clyean_dir().join(LOGS_DIR_NAME)
    }

    pub fn clyean_gitignore_path(&self) -> PathBuf {
        self.clyean_dir().join(".gitignore")
    }

    /// A project counts as scaffolded once its `project.json` exists.
    pub fn is_scaffolded(&self) -> bool {
        self.project_config_path().is_file()
    }

    /// The path of `path` relative to the project root, when it lies inside it.
    pub fn relative_to_root<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.root).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_every_scaffold_path_under_the_clyean_directory() {
        let layout = ProjectLayout::new("/tmp/example");
        assert_eq!(layout.clyean_dir(), PathBuf::from("/tmp/example/.clyean"));
        assert_eq!(
            layout.project_config_path(),
            PathBuf::from("/tmp/example/.clyean/project.json")
        );
        assert_eq!(
            layout.agents_dir(),
            PathBuf::from("/tmp/example/.clyean/agents")
        );
        assert_eq!(
            layout.specs_path(),
            PathBuf::from("/tmp/example/.clyean/SPECS.md")
        );
        assert_eq!(
            layout.container_root_dir(),
            PathBuf::from("/tmp/example/.clyean/container-root")
        );
        assert_eq!(
            layout.lock_path(),
            PathBuf::from("/tmp/example/.clyean/lock")
        );
    }

    #[test]
    fn unscaffolded_directory_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        assert!(!layout.is_scaffolded());
        std::fs::create_dir_all(layout.clyean_dir()).unwrap();
        std::fs::write(layout.project_config_path(), "{}").unwrap();
        assert!(layout.is_scaffolded());
    }
}
