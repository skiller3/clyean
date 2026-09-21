// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The change plan catalog: `.clyean/plans/<date>-<slug>/v<N>.md` files whose versions are
//! never edited in place, plus the journal that makes a plan's workflow resumable.

use std::path::{Path, PathBuf};

use crate::layout::ProjectLayout;
use crate::{ProjectError, Result};

pub const JOURNAL_FILE_NAME: &str = "journal.json";
const MAX_SLUG_LENGTH: usize = 48;

/// Identifies a change plan directory, optionally pinned to one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReference {
    pub name: String,
    pub version: Option<u32>,
}

impl PlanReference {
    /// Parses `"<name>"` or `"<name>/v<N>"`.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim().trim_end_matches('/');
        let (name, version) = match text.split_once('/') {
            Some((name, version_text)) => {
                let version = version_text
                    .strip_prefix('v')
                    .and_then(|n| n.parse::<u32>().ok())
                    .ok_or_else(|| ProjectError::InvalidPlanReference(text.to_string()))?;
                (name, Some(version))
            }
            None => (text, None),
        };
        if !is_valid_plan_name(name) {
            return Err(ProjectError::InvalidPlanReference(text.to_string()));
        }
        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

impl std::fmt::Display for PlanReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.version {
            Some(version) => write!(f, "{}/v{version}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// One concrete version file of a change plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanVersion {
    pub name: String,
    pub version: u32,
    pub path: PathBuf,
}

impl PlanVersion {
    pub fn reference(&self) -> PlanReference {
        PlanReference {
            name: self.name.clone(),
            version: Some(self.version),
        }
    }
}

/// Reads and creates change plans under `.clyean/plans`.
#[derive(Debug, Clone)]
pub struct PlanCatalog {
    plans_dir: PathBuf,
}

impl PlanCatalog {
    pub fn new(layout: &ProjectLayout) -> Self {
        Self {
            plans_dir: layout.plans_dir(),
        }
    }

    pub fn plans_dir(&self) -> &Path {
        &self.plans_dir
    }

    pub fn plan_dir(&self, name: &str) -> PathBuf {
        self.plans_dir.join(name)
    }

    pub fn journal_path(&self, name: &str) -> PathBuf {
        self.plan_dir(name).join(JOURNAL_FILE_NAME)
    }

    /// Names of every plan directory, sorted.
    pub fn list(&self) -> Result<Vec<String>> {
        if !self.plans_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = std::fs::read_dir(&self.plans_dir)
            .map_err(|e| ProjectError::io(format!("listing {}", self.plans_dir.display()), e))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| is_valid_plan_name(name))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Every version file of a plan in ascending order.
    pub fn versions(&self, name: &str) -> Result<Vec<PlanVersion>> {
        let dir = self.plan_dir(name);
        if !dir.is_dir() {
            return Err(ProjectError::PlanNotFound(name.to_string()));
        }
        let mut versions: Vec<PlanVersion> = std::fs::read_dir(&dir)
            .map_err(|e| ProjectError::io(format!("listing {}", dir.display()), e))?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let file_name = entry.file_name();
                let version = parse_version_file_name(file_name.to_str()?)?;
                Some(PlanVersion {
                    name: name.to_string(),
                    version,
                    path: entry.path(),
                })
            })
            .collect();
        versions.sort_by_key(|version| version.version);
        Ok(versions)
    }

    pub fn latest_version(&self, name: &str) -> Result<Option<PlanVersion>> {
        Ok(self.versions(name)?.pop())
    }

    /// Resolves a reference to a concrete version (the latest when unpinned).
    pub fn resolve(&self, reference: &PlanReference) -> Result<PlanVersion> {
        let versions = self.versions(&reference.name)?;
        let found = match reference.version {
            Some(wanted) => versions.into_iter().find(|v| v.version == wanted),
            None => versions.into_iter().last(),
        };
        found.ok_or_else(|| ProjectError::PlanNotFound(reference.to_string()))
    }

    /// Creates the directory of a new plan named from today's date and a slug of `title`,
    /// de-duplicating with a numeric suffix, and returns the name.
    pub fn create_plan(&self, title: &str, date: &str) -> Result<String> {
        let base = format!("{date}-{}", slugify(title));
        let mut name = base.clone();
        let mut suffix = 2;
        while self.plan_dir(&name).exists() {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        std::fs::create_dir_all(self.plan_dir(&name))
            .map_err(|e| ProjectError::io(format!("creating plan {name}"), e))?;
        Ok(name)
    }

    /// The path of the next version file of a plan (v1 for a plan without versions).
    pub fn next_version_path(&self, name: &str) -> Result<PlanVersion> {
        let next = self
            .latest_version(name)?
            .map_or(1, |latest| latest.version + 1);
        Ok(PlanVersion {
            name: name.to_string(),
            version: next,
            path: self.plan_dir(name).join(format!("v{next}.md")),
        })
    }
}

fn parse_version_file_name(file_name: &str) -> Option<u32> {
    file_name
        .strip_prefix('v')?
        .strip_suffix(".md")?
        .parse::<u32>()
        .ok()
}

fn is_valid_plan_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

/// Lower-cases `text`, keeps ASCII letters and digits, and joins runs of anything else
/// with single hyphens, capped at a bounded length.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_hyphen = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
        if slug.len() >= MAX_SLUG_LENGTH {
            break;
        }
    }
    let trimmed = slug.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "change".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_produces_bounded_kebab_case() {
        assert_eq!(
            slugify("Add OAuth login to the API!"),
            "add-oauth-login-to-the-api"
        );
        assert_eq!(slugify("   "), "change");
        assert!(slugify(&"word ".repeat(40)).len() <= MAX_SLUG_LENGTH);
    }

    #[test]
    fn plan_reference_parses_with_and_without_version() {
        let unpinned = PlanReference::parse("2026-09-21-add-login").unwrap();
        assert_eq!(unpinned.version, None);
        let pinned = PlanReference::parse("2026-09-21-add-login/v3").unwrap();
        assert_eq!(pinned.version, Some(3));
        assert_eq!(pinned.to_string(), "2026-09-21-add-login/v3");
        assert!(PlanReference::parse("Bad Name").is_err());
        assert!(PlanReference::parse("name/3").is_err());
    }

    #[test]
    fn versions_never_clobber_and_resolve_latest() {
        let dir = tempfile::tempdir().unwrap();
        let layout = ProjectLayout::new(dir.path());
        let catalog = PlanCatalog::new(&layout);
        let name = catalog.create_plan("Add login", "2026-09-21").unwrap();
        assert_eq!(name, "2026-09-21-add-login");
        let duplicate = catalog.create_plan("Add login", "2026-09-21").unwrap();
        assert_eq!(duplicate, "2026-09-21-add-login-2");

        assert!(catalog.latest_version(&name).unwrap().is_none());
        let v1 = catalog.next_version_path(&name).unwrap();
        assert_eq!(v1.version, 1);
        std::fs::write(&v1.path, "# v1").unwrap();
        let v2 = catalog.next_version_path(&name).unwrap();
        assert_eq!(v2.version, 2);
        std::fs::write(&v2.path, "# v2").unwrap();

        let latest = catalog
            .resolve(&PlanReference::parse(&name).unwrap())
            .unwrap();
        assert_eq!(latest.version, 2);
        let pinned = catalog
            .resolve(&PlanReference::parse(&format!("{name}/v1")).unwrap())
            .unwrap();
        assert_eq!(pinned.version, 1);
        assert_eq!(catalog.list().unwrap().len(), 2);
    }
}
