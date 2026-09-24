// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The Clyean-managed harness extensions, embedded at build time from the repository's
//! `extensions/` directory and installed into the agents' profiles.

use crate::profile::ManagedExtension;
use crate::roster::AgentId;

pub const HERDR_REPORTER: ManagedExtension = ManagedExtension {
    file_name: "clyean-herdr-reporter.ts",
    source: include_str!("../../../extensions/clyean-herdr-reporter.ts"),
};

pub const ORCHESTRATION: ManagedExtension = ManagedExtension {
    file_name: "clyean-orchestration.ts",
    source: include_str!("../../../extensions/clyean-orchestration.ts"),
};

pub const CREDENTIALS: ManagedExtension = ManagedExtension {
    file_name: "clyean-credentials.ts",
    source: include_str!("../../../extensions/clyean-credentials.ts"),
};

/// The extensions installed for `agent`.  Only the User Assistant talks to Herdr and to
/// the host orchestrator, and holds the sign-ins; every other agent imports and renews
/// the copies it receives.
pub fn managed_extensions(agent: AgentId) -> Vec<ManagedExtension> {
    match agent {
        AgentId::UserAssistant => vec![HERDR_REPORTER, ORCHESTRATION],
        _ => vec![CREDENTIALS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_extensions_carry_version_markers() {
        assert_eq!(HERDR_REPORTER.version(), Some(1));
        assert_eq!(ORCHESTRATION.version(), Some(3));
        assert_eq!(CREDENTIALS.version(), Some(1));
        assert_eq!(managed_extensions(AgentId::UserAssistant).len(), 2);
        assert_eq!(managed_extensions(AgentId::Programmer), [CREDENTIALS]);
    }
}
