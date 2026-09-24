// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! User Assistant containers whose `clyean` process is gone.  A User Assistant normally
//! shuts down when its bridge ends, but a harness that has stopped responding cannot, and
//! its container stays up without a bridge.  Such a container is recognised by its age and
//! by the absence of a bridge process in its process list.  Podman's list of exec sessions
//! cannot serve, because it keeps an entry after the session's process exits.

use std::time::Duration;

use serde::Deserialize;

use crate::launch::{ROLE_LABEL, USER_ASSISTANT_ROLE};
use crate::podman::Podman;
use crate::{Result, SandboxError};

/// How old a User Assistant container must be before it can count as orphaned.  A new
/// container gets its bridge within the bridge's start timeout, which is shorter.
pub const MINIMUM_AGE: Duration = Duration::from_secs(60);

/// One User Assistant container as `podman ps` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserAssistantContainer {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Names", default)]
    pub names: Vec<String>,
    #[serde(rename = "State", default)]
    pub state: String,
    /// Creation time in seconds since the Unix epoch.
    #[serde(rename = "Created", default)]
    pub created: i64,
}

impl UserAssistantContainer {
    pub fn name(&self) -> &str {
        self.names.first().map(String::as_str).unwrap_or(&self.id)
    }

    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

/// Every User Assistant container of every project, running or not.
pub fn list_user_assistant_containers(podman: &Podman) -> Result<Vec<UserAssistantContainer>> {
    let label = format!("label={ROLE_LABEL}={USER_ASSISTANT_ROLE}");
    let text = podman.output(["ps", "--all", "--filter", &label, "--format", "json"])?;
    parse_container_list(&text)
}

pub fn parse_container_list(text: &str) -> Result<Vec<UserAssistantContainer>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text)
        .map_err(|error| SandboxError::Invalid(format!("unexpected podman ps output: {error}")))
}

/// Whether the container's process list includes a running bridge session.
pub fn has_running_bridge(podman: &Podman, container_id: &str) -> Result<bool> {
    let listing = podman.output(["top", container_id, "args"])?;
    Ok(lists_a_bridge(&listing))
}

/// Whether a `podman top ... args` listing includes `clyean-bridge bridge ...`.
pub fn lists_a_bridge(listing: &str) -> bool {
    listing.lines().any(|line| {
        let mut words = line.split_whitespace();
        let program = words.next().unwrap_or_default();
        program.ends_with("clyean-bridge") && words.next() == Some("bridge")
    })
}

/// The containers old enough to judge that are stopped or have no bridge.
pub fn select_orphans(
    containers: &[UserAssistantContainer],
    now_unix: i64,
    mut has_bridge: impl FnMut(&UserAssistantContainer) -> bool,
) -> Vec<&UserAssistantContainer> {
    let minimum_age = MINIMUM_AGE.as_secs() as i64;
    containers
        .iter()
        .filter(|container| now_unix - container.created >= minimum_age)
        .filter(|container| !container.is_running() || !has_bridge(container))
        .collect()
}

/// Removes every orphaned User Assistant container and returns their names.
pub fn prune_orphaned_user_assistants(podman: &Podman) -> Result<Vec<String>> {
    let containers = list_user_assistant_containers(podman)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default();
    let orphans = select_orphans(&containers, now, |container| {
        // A container whose processes cannot be listed is left alone.
        has_running_bridge(podman, &container.id).unwrap_or(true)
    });
    let mut removed = Vec::new();
    for orphan in orphans {
        podman.output(["rm", "--force", "--ignore", &orphan.id])?;
        removed.push(orphan.name().to_string());
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(name: &str, state: &str, created: i64) -> UserAssistantContainer {
        UserAssistantContainer {
            id: format!("id-{name}"),
            names: vec![name.to_string()],
            state: state.to_string(),
            created,
        }
    }

    #[test]
    fn podman_ps_json_is_parsed() {
        let text = r#"[{"Id":"abc","Names":["clyean-p-user-assistant-1234abcd"],"State":"running","Created":1790000000,"Labels":{"clyean.role":"user-assistant"}}]"#;
        let containers = parse_container_list(text).unwrap();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name(), "clyean-p-user-assistant-1234abcd");
        assert!(containers[0].is_running());
        assert_eq!(containers[0].created, 1_790_000_000);
        assert!(parse_container_list("").unwrap().is_empty());
        assert!(parse_container_list("[]").unwrap().is_empty());
    }

    #[test]
    fn only_a_bridge_session_counts_as_a_bridge() {
        let listing = "COMMAND\n/usr/local/libexec/clyean/clyean-bridge bridge --ready-file /run/clyean/bridge.ready\n/usr/local/bin/clyean --cwd /p\n";
        assert!(lists_a_bridge(listing));
        let gate_only = "COMMAND\n/usr/local/libexec/clyean/clyean-bridge await --ready-file /run/clyean/bridge.ready -- /usr/local/bin/clyean\n";
        assert!(!lists_a_bridge(gate_only));
        assert!(!lists_a_bridge("COMMAND\n/usr/local/bin/clyean --cwd /p\n"));
    }

    #[test]
    fn young_containers_and_bridged_ones_are_kept() {
        let now = 10_000;
        let containers = vec![
            container("young-without-bridge", "running", now - 5),
            container("old-with-bridge", "running", now - 600),
            container("old-without-bridge", "running", now - 600),
            container("old-exited", "exited", now - 600),
            container("young-created", "created", now - 5),
        ];
        let orphans = select_orphans(&containers, now, |c| c.name() == "old-with-bridge");
        let names: Vec<&str> = orphans.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["old-without-bridge", "old-exited"]);
    }
}
