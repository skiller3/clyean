// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! `clyean server`: the project's server containers, from the host.

use std::sync::Arc;

use anyhow::Result;
use clyean_orchestrator::scaffold::PendingScaffold;
use clyean_orchestrator::ServerManager;
use serde_json::Value;

use crate::cli::{ProjectArgs, ServerAction, ServerArgs};
use crate::runtime::ProjectRuntime;
use crate::servers::PodmanServerRuntime;

pub async fn run(project: &ProjectArgs, args: ServerArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(project)?;
    let podman_sandbox = runtime.podman_sandbox()?;
    let config = if runtime.is_scaffolded() {
        runtime.load_config()?
    } else {
        runtime.provisional_config(&PendingScaffold::default())
    };
    let context = runtime.launch_context(config, &podman_sandbox);
    let servers = ServerManager::new(Arc::new(PodmanServerRuntime::new(context)), None, "");
    match args.action {
        ServerAction::List => {
            let listing = servers.list().await?;
            print!("{}", render_listing(&listing));
        }
        ServerAction::Stop { all: true, .. } => {
            let stopped = servers.stop_all().await?;
            if stopped.is_empty() {
                println!("No servers were running.");
            } else {
                println!("Stopped {}", stopped.join(", "));
            }
        }
        ServerAction::Stop { name, .. } => {
            let name = name.unwrap_or_default();
            servers.stop(&name).await?;
            println!("Server {name} is stopped.");
        }
    }
    Ok(0)
}

fn render_listing(listing: &Value) -> String {
    let servers = listing["servers"].as_array().cloned().unwrap_or_default();
    if servers.is_empty() {
        return "No servers are running for this project.\n".to_string();
    }
    let mut text = String::new();
    for server in servers {
        let lifetime = if server["persistent"] == true {
            "persistent"
        } else {
            "stops with its clyean process"
        };
        text.push_str(&format!(
            "{}  ({lifetime}; requested by {}; started {})\n",
            server["name"].as_str().unwrap_or_default(),
            server["requested_by"].as_str().unwrap_or("unknown"),
            server["started_at"]
                .as_str()
                .unwrap_or("at an unknown time"),
        ));
        let cwd = server["cwd"]
            .as_str()
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or(".");
        text.push_str(&format!(
            "  {}   (in {cwd})\n  {}\n",
            server["command"].as_str().unwrap_or_default(),
            server["summary"].as_str().unwrap_or_default(),
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn listings_show_each_server_and_how_to_reach_it() {
        assert_eq!(
            render_listing(&json!({"servers": []})),
            "No servers are running for this project.\n"
        );
        let text = render_listing(&json!({"servers": [{"name": "web", "persistent": true,
            "requested_by": "programmer", "started_at": "2026-09-26T18:21:45Z",
            "command": "npm run dev", "cwd": "app", "summary": "Server web is ready."}]}));
        assert_eq!(
            text,
            "web  (persistent; requested by programmer; started 2026-09-26T18:21:45Z)\n  npm run dev   (in app)\n  Server web is ready.\n"
        );
    }
}
