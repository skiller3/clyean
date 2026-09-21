// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

/// The exact PlantUML distribution Clyean installs into every sandbox.  The MIT
/// distribution omits only the ditaa, jcckit, and sudoku integrations, none of which
/// Clyean uses.  Pinning the version keeps rendered diagrams stable across projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlantUmlDistribution {
    pub version: &'static str,
    pub sha256: &'static str,
}

pub const PINNED_DISTRIBUTION: PlantUmlDistribution = PlantUmlDistribution {
    version: "1.2026.8",
    sha256: "3629c9cd017c7f73e6450396eea0040216c7e1eef8473ce33cc1aad469dab2f9",
};

/// Directory inside the sandbox that holds the jar.
pub const SANDBOX_INSTALL_DIR: &str = "/opt/plantuml";
/// Stable symbolic link name inside the sandbox.
pub const SANDBOX_JAR_LINK: &str = "/opt/plantuml/plantuml.jar";
/// Layout engine that needs no Graphviz installation.
pub const LAYOUT_ENGINE_ARGUMENT: &str = "-Playout=smetana";

impl PlantUmlDistribution {
    pub fn jar_file_name(&self) -> String {
        format!("plantuml-mit-{}.jar", self.version)
    }

    pub fn download_url(&self) -> String {
        format!(
            "https://github.com/plantuml/plantuml/releases/download/v{}/{}",
            self.version,
            self.jar_file_name()
        )
    }

    pub fn sandbox_jar_path(&self) -> String {
        format!("{SANDBOX_INSTALL_DIR}/{}", self.jar_file_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_distribution_resolves_to_the_mit_jar() {
        assert_eq!(
            PINNED_DISTRIBUTION.jar_file_name(),
            "plantuml-mit-1.2026.8.jar"
        );
        assert_eq!(
            PINNED_DISTRIBUTION.download_url(),
            "https://github.com/plantuml/plantuml/releases/download/v1.2026.8/plantuml-mit-1.2026.8.jar"
        );
        assert_eq!(
            PINNED_DISTRIBUTION.sandbox_jar_path(),
            "/opt/plantuml/plantuml-mit-1.2026.8.jar"
        );
        assert_eq!(PINNED_DISTRIBUTION.sha256.len(), 64);
    }
}
