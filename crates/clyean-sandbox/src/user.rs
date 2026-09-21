// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

/// The user name under which agents appear inside the sandbox: `HOME` is `/home/<name>`.
/// Agents run as UID 0, which rootless Podman maps to the host user, so the name is
/// purely presentational and derived from the host user name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerUser {
    name: String,
}

impl ContainerUser {
    pub fn from_host_user_name(host_user: &str) -> Self {
        Self {
            name: sanitize(host_user),
        }
    }

    pub fn from_environment() -> Self {
        let host_user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "clyean".to_string());
        Self::from_host_user_name(&host_user)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn home(&self) -> String {
        format!("/home/{}", self.name)
    }

    pub fn workspace_parent(&self) -> String {
        format!("{}/workspace", self.home())
    }
}

fn sanitize(host_user: &str) -> String {
    let sanitized: String = host_user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "clyean".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_user_names_are_lower_cased_and_sanitized() {
        assert_eq!(
            ContainerUser::from_host_user_name("Skye Isard").name(),
            "skye-isard"
        );
        assert_eq!(
            ContainerUser::from_host_user_name("skyei").home(),
            "/home/skyei"
        );
        assert_eq!(ContainerUser::from_host_user_name("???").name(), "clyean");
        assert_eq!(
            ContainerUser::from_host_user_name("skyei").workspace_parent(),
            "/home/skyei/workspace"
        );
    }
}
