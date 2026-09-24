// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The Git ignore rules Clyean needs for its scaffold, written to `.clyean/.gitignore`
//! so that a `.gitignore` maintained by the user is never edited.

use std::path::Path;

use crate::layout::ProjectLayout;
use crate::{ProjectError, Result};

/// One ignore rule together with a representative path used to probe whether Git
/// already ignores it through rules the user placed elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRule {
    /// Pattern relative to `.clyean/`.
    pub pattern: &'static str,
    /// A path relative to `.clyean/` that the pattern must cover.
    pub probe: &'static str,
}

pub const REQUIRED_IGNORE_RULES: &[IgnoreRule] = &[
    IgnoreRule {
        pattern: "*.local.json",
        probe: "project.local.json",
    },
    IgnoreRule {
        pattern: "*.local.md",
        probe: "agents/AGENTS__PROGRAMMER.local.md",
    },
    IgnoreRule {
        pattern: "/lock",
        probe: "lock",
    },
    IgnoreRule {
        pattern: "/logs/",
        probe: "logs/orchestrator.log",
    },
    IgnoreRule {
        pattern: "/work/",
        probe: "work/example.json",
    },
];

/// Appends every rule whose probe path `is_ignored` reports as not yet ignored.
/// `is_ignored` receives project-relative paths and must consult Git's effective rules.
pub fn ensure_ignore_rules(
    layout: &ProjectLayout,
    mut is_ignored: impl FnMut(&Path) -> Result<bool>,
) -> Result<Vec<&'static str>> {
    let mut added = Vec::new();
    let gitignore_path = layout.clyean_gitignore_path();
    let mut contents = if gitignore_path.is_file() {
        std::fs::read_to_string(&gitignore_path)
            .map_err(|e| ProjectError::io(format!("reading {}", gitignore_path.display()), e))?
    } else {
        String::new()
    };
    for rule in REQUIRED_IGNORE_RULES {
        let probe = Path::new(".clyean").join(rule.probe);
        if is_ignored(&probe)? {
            continue;
        }
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(rule.pattern);
        contents.push('\n');
        added.push(rule.pattern);
    }
    if !added.is_empty() {
        std::fs::create_dir_all(layout.clyean_dir())
            .map_err(|e| ProjectError::io("creating .clyean", e))?;
        std::fs::write(&gitignore_path, &contents)
            .map_err(|e| ProjectError::io(format!("writing {}", gitignore_path.display()), e))?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_the_rules_git_does_not_already_honor() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let added =
            ensure_ignore_rules(&layout, |path| Ok(path.to_string_lossy().ends_with("lock")))
                .unwrap();
        assert!(!added.contains(&"/lock"));
        assert!(added.contains(&"*.local.json"));
        let written = std::fs::read_to_string(layout.clyean_gitignore_path()).unwrap();
        assert!(!written.contains("/lock"));
        assert!(written.contains("*.local.md\n"));
        assert!(written.ends_with('\n'));
    }

    #[test]
    fn is_a_no_op_when_everything_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let added = ensure_ignore_rules(&layout, |_| Ok(true)).unwrap();
        assert!(added.is_empty());
        assert!(!layout.clyean_gitignore_path().exists());
    }
}
