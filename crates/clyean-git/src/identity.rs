// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The standardized way every Clyean agent identifies itself in Git history: an author
//! identity per agent and a `Clyean-Agent:` trailer on every commit message.

/// The trailer key that names the authoring agent in every Clyean commit message.
pub const AGENT_TRAILER_KEY: &str = "Clyean-Agent";
pub const AGENT_EMAIL_DOMAIN: &str = "agents.clyean.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

impl CommitIdentity {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
        }
    }

    /// The identity of a Clyean agent: `Clyean <Display Name> <agent-id@agents.clyean.com>`.
    pub fn for_agent(agent_id: &str, display_name: &str) -> Self {
        Self {
            name: format!("Clyean {display_name}"),
            email: format!("{agent_id}@{AGENT_EMAIL_DOMAIN}"),
        }
    }

    pub fn as_author_string(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }

    /// Environment variables that make Git inside a sandbox use this identity.
    pub fn environment(&self) -> [(&'static str, String); 4] {
        [
            ("GIT_AUTHOR_NAME", self.name.clone()),
            ("GIT_AUTHOR_EMAIL", self.email.clone()),
            ("GIT_COMMITTER_NAME", self.name.clone()),
            ("GIT_COMMITTER_EMAIL", self.email.clone()),
        ]
    }
}

/// Builds a commit message with the mandatory agent trailer.
pub fn agent_commit_message(subject: &str, body: Option<&str>, agent_id: &str) -> String {
    let mut message = subject.trim().to_string();
    if let Some(body) = body.map(str::trim).filter(|b| !b.is_empty()) {
        message.push_str("\n\n");
        message.push_str(body);
    }
    message.push_str("\n\n");
    message.push_str(AGENT_TRAILER_KEY);
    message.push_str(": ");
    message.push_str(agent_id);
    message.push('\n');
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_identity_follows_the_standard_form() {
        let identity = CommitIdentity::for_agent("software-architect", "Software Architect");
        assert_eq!(
            identity.as_author_string(),
            "Clyean Software Architect <software-architect@agents.clyean.com>"
        );
        assert_eq!(identity.environment()[0].0, "GIT_AUTHOR_NAME");
    }

    #[test]
    fn commit_message_ends_with_the_agent_trailer() {
        assert_eq!(
            agent_commit_message("Subject", None, "specifier"),
            "Subject\n\nClyean-Agent: specifier\n"
        );
        assert_eq!(
            agent_commit_message(" Subject ", Some("Body text"), "specifier"),
            "Subject\n\nBody text\n\nClyean-Agent: specifier\n"
        );
    }
}
