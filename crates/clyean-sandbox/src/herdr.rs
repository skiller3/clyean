// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Host-side Herdr detection and the environment and executable mount that let the User
//! Assistant use Herdr from inside its container; the bridge relays the socket itself.

use std::path::{Path, PathBuf};

pub const CONTAINER_SOCKET_PATH: &str = "/run/herdr/herdr.sock";
pub const CONTAINER_BIN_PATH: &str = "/usr/local/bin/herdr";

/// The Herdr pane Clyean was launched in, resolved from the host environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrHostContext {
    pub pane_id: String,
    pub tab_id: Option<String>,
    pub workspace_id: Option<String>,
    pub socket_path: PathBuf,
    pub bin_path: Option<PathBuf>,
}

impl HerdrHostContext {
    /// Detects a Herdr pane exactly like the harness does: `HERDR_ENV=1` or any identity
    /// variable signals a pane; client-side variables never do.  Reporting additionally
    /// needs `HERDR_ENV=1`, a pane id, and a socket path, so this returns `None` unless
    /// all three are present.
    pub fn detect(env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let inside_pane = env("HERDR_ENV").as_deref() == Some("1")
            || non_empty(env("HERDR_PANE_ID")).is_some()
            || non_empty(env("HERDR_TAB_ID")).is_some()
            || non_empty(env("HERDR_WORKSPACE_ID")).is_some();
        if !inside_pane || env("HERDR_ENV").as_deref() != Some("1") {
            return None;
        }
        let pane_id = non_empty(env("HERDR_PANE_ID"))?;
        let socket_path = PathBuf::from(non_empty(env("HERDR_SOCKET_PATH"))?);
        let bin_path = non_empty(env("HERDR_BIN_PATH"))
            .map(PathBuf::from)
            .or_else(|| find_on_path("herdr"))
            .filter(|path| path.is_file());
        Some(Self {
            pane_id,
            tab_id: non_empty(env("HERDR_TAB_ID")),
            workspace_id: non_empty(env("HERDR_WORKSPACE_ID")),
            socket_path,
            bin_path,
        })
    }

    pub fn detect_from_process_environment() -> Option<Self> {
        Self::detect(&|name| std::env::var(name).ok())
    }

    /// Environment variables to propagate into the User Assistant container.
    pub fn container_environment(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("HERDR_ENV".to_string(), "1".to_string()),
            ("HERDR_PANE_ID".to_string(), self.pane_id.clone()),
            (
                "HERDR_SOCKET_PATH".to_string(),
                CONTAINER_SOCKET_PATH.to_string(),
            ),
        ];
        if let Some(tab) = &self.tab_id {
            env.push(("HERDR_TAB_ID".to_string(), tab.clone()));
        }
        if let Some(workspace) = &self.workspace_id {
            env.push(("HERDR_WORKSPACE_ID".to_string(), workspace.clone()));
        }
        if self.bin_path.is_some() {
            env.push(("HERDR_BIN_PATH".to_string(), CONTAINER_BIN_PATH.to_string()));
        }
        env
    }

    /// The `herdr` executable, mounted read-only when present.  The socket is not
    /// mounted: the bridge relays it (the sanctioned sandbox exception).
    pub fn executable_mount(
        &self,
        host_paths: crate::environment::HostPathMapper,
    ) -> Option<crate::container::MountSpec> {
        self.bin_path.as_ref().map(|bin| {
            crate::container::MountSpec::read_only(host_paths.map(bin), CONTAINER_BIN_PATH)
        })
    }
}

/// The Herdr release the host runs, from `herdr --version` (`herdr 0.7.4`).
pub fn host_version(bin: &Path) -> Option<String> {
    let output = std::process::Command::new(bin)
        .arg("--version")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .nth(1)
        .map(|version| version.trim_start_matches('v').to_string())
}

const HERDR_RELEASES: &str = "https://api.github.com/repos/herdrdev/herdr/releases";

/// The Linux build of Herdr `version` for containers of architecture `arch_tag`, which
/// replaces the host's own executable on a Podman machine: downloaded once from Herdr's
/// GitHub release, verified against the digest GitHub records for it, and cached.
pub async fn linux_cli(version: &str, arch_tag: &str, cache_dir: &Path) -> crate::Result<PathBuf> {
    let asset = match arch_tag {
        "x64" => "herdr-linux-x86_64",
        _ => "herdr-linux-aarch64",
    };
    let cached = cache_dir.join("herdr").join(version).join(asset);
    if cached.is_file() {
        return Ok(cached);
    }
    let release_url = format!("{HERDR_RELEASES}/tags/v{version}");
    let failed = |reason: String| crate::SandboxError::Download {
        url: release_url.clone(),
        reason,
    };
    let client = reqwest::Client::builder()
        .user_agent("clyean")
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let text = client
        .get(&release_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| failed(e.to_string()))?
        .text()
        .await
        .map_err(|e| failed(e.to_string()))?;
    let release: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| failed(e.to_string()))?;
    let (url, expected) = release_asset(&release, asset).ok_or_else(|| {
        failed(format!(
            "the release has no {asset} with a digest GitHub records"
        ))
    })?;
    crate::provisioning::download_verified(&url, &cached, &expected).await?;
    Ok(cached)
}

/// The download address and SHA-256 digest of `asset` in a GitHub release description.
fn release_asset(release: &serde_json::Value, asset: &str) -> Option<(String, String)> {
    let entry = release["assets"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == asset)?;
    let digest = entry["digest"].as_str()?.strip_prefix("sha256:")?;
    Some((
        entry["browser_download_url"].as_str()?.to_string(),
        digest.to_string(),
    ))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[allow(dead_code)]
fn is_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn detect(map: &HashMap<String, String>) -> Option<HerdrHostContext> {
        HerdrHostContext::detect(&|name| map.get(name).cloned())
    }

    #[test]
    fn requires_env_pane_and_socket_for_reporting() {
        assert!(detect(&env(&[])).is_none());
        assert!(detect(&env(&[("HERDR_ENV", "1")])).is_none());
        assert!(detect(&env(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")])).is_none());
        assert!(detect(&env(&[
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/tmp/h.sock")
        ]))
        .is_none());
        let context = detect(&env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_TAB_ID", "w1:t1"),
            ("HERDR_SOCKET_PATH", "/tmp/h.sock"),
        ]))
        .unwrap();
        assert_eq!(context.pane_id, "w1:p1");
        assert_eq!(context.tab_id.as_deref(), Some("w1:t1"));
        assert_eq!(context.socket_path, PathBuf::from("/tmp/h.sock"));
    }

    #[test]
    fn client_side_variables_alone_are_not_detection_signals() {
        let map = env(&[
            ("HERDR_SOCKET_PATH", "/tmp/h.sock"),
            ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ("HERDR_SESSION", "s"),
            ("HERDR_CONFIG_PATH", "/c"),
            ("HERDR_CLIENT_SOCKET_PATH", "/x"),
        ]);
        assert!(detect(&map).is_none());
    }

    #[test]
    fn release_assets_are_found_with_their_digests() {
        let release = serde_json::json!({"assets": [
            {"name": "herdr-linux-aarch64", "digest": "sha256:aa", "browser_download_url": "https://x/aarch64"},
            {"name": "herdr-linux-x86_64", "digest": "sha256:bb", "browser_download_url": "https://x/x86_64"},
            {"name": "herdr-macos-x86_64", "browser_download_url": "https://x/macos"}
        ]});
        assert_eq!(
            release_asset(&release, "herdr-linux-x86_64"),
            Some(("https://x/x86_64".to_string(), "bb".to_string()))
        );
        assert_eq!(release_asset(&release, "herdr-macos-x86_64"), None);
        assert_eq!(release_asset(&release, "herdr-linux-riscv64"), None);
    }

    #[test]
    fn container_environment_rewrites_socket_and_bin_paths() {
        let context = HerdrHostContext {
            pane_id: "w1:p1".into(),
            tab_id: None,
            workspace_id: Some("w1".into()),
            socket_path: PathBuf::from("/home/u/.config/herdr/sessions/x/herdr.sock"),
            bin_path: Some(PathBuf::from("/usr/local/bin/herdr")),
        };
        let env = context.container_environment();
        assert!(env.contains(&("HERDR_SOCKET_PATH".into(), "/run/herdr/herdr.sock".into())));
        assert!(env.contains(&("HERDR_BIN_PATH".into(), "/usr/local/bin/herdr".into())));
        assert!(env.contains(&("HERDR_WORKSPACE_ID".into(), "w1".into())));
        let mount = context
            .executable_mount(crate::environment::HostPathMapper::Identity)
            .unwrap();
        assert!(mount.read_only);
        assert_eq!(mount.target, CONTAINER_BIN_PATH);
    }
}
