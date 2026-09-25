// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! What an agent's configuration says it needs credentials for: the models its settings
//! overlay names and the remote MCP servers its seed lists.  Computed on the host from the
//! agent's files under `.clyean/agents`, which are the agent's configuration.

use std::collections::HashSet;

use clyean_project::ProjectLayout;
use serde_json::{Map, Value};

use crate::roster::AgentId;
use crate::settings::{effective_mcp_seed, effective_settings_overlay};
use crate::Result;

/// The prefix of a model role alias (`@smol`) and its legacy form (`pi/smol`).
const ROLE_ALIAS_PREFIXES: [&str; 2] = ["@", "pi/"];
/// The alias of the default model role.
const DEFAULT_ROLE_ALIAS: &str = "*";
const DEFAULT_ROLE: &str = "default";

/// The credential-bearing references in one agent's configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentNeeds {
    /// Every model pattern in the agent's model roles and fallback chains, as written
    /// (`provider/id`, a bare id, a provider wildcard, or any of these with a thinking
    /// suffix), without role aliases or duplicates.
    pub model_patterns: Vec<String>,
    /// The patterns of the agent's default model role in priority order, with role aliases
    /// expanded; empty when the agent's configuration names no default model.
    pub default_model_patterns: Vec<String>,
    /// The addresses of the agent's remote MCP servers.
    pub mcp_server_urls: Vec<String>,
}

impl AgentNeeds {
    pub fn for_agent(layout: &ProjectLayout, agent: AgentId) -> Result<Self> {
        let overlay = effective_settings_overlay(layout, agent)?;
        let seed = effective_mcp_seed(layout, agent)?.unwrap_or(Value::Null);
        Ok(Self::from_configuration(&overlay, &seed))
    }

    pub fn from_configuration(overlay: &Value, mcp_seed: &Value) -> Self {
        let empty = Map::new();
        let roles = overlay
            .get("modelRoles")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let mut needs = Self {
            default_model_patterns: expand_role(roles, DEFAULT_ROLE, &mut HashSet::new()),
            ..Self::default()
        };
        let chains = overlay
            .pointer("/retry/fallbackChains")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        for value in roles.values().chain(chains.values()) {
            for pattern in patterns(value) {
                if role_alias(&pattern).is_none() {
                    push_unique(&mut needs.model_patterns, &pattern);
                }
            }
        }
        if let Some(servers) = mcp_seed.get("mcpServers").and_then(Value::as_object) {
            for server in servers.values() {
                if let Some(url) = server.get("url").and_then(Value::as_str) {
                    push_unique(&mut needs.mcp_server_urls, url);
                }
            }
        }
        needs
    }
}

/// The concrete patterns of `role`, following aliases to other roles of the same agent.
fn expand_role(
    roles: &Map<String, Value>,
    role: &str,
    visited: &mut HashSet<String>,
) -> Vec<String> {
    if !visited.insert(role.to_string()) {
        return Vec::new();
    }
    let mut expanded = Vec::new();
    for pattern in roles.get(role).map(patterns).unwrap_or_default() {
        match role_alias(&pattern) {
            Some(target) => {
                for inner in expand_role(roles, &target, visited) {
                    push_unique(&mut expanded, &inner);
                }
            }
            None => push_unique(&mut expanded, &pattern),
        }
    }
    expanded
}

/// The patterns of a role value or fallback chain: a comma-separated string or a list.
fn patterns(value: &Value) -> Vec<String> {
    let items: Vec<&str> = match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    items
        .into_iter()
        .flat_map(|item| item.split(','))
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_string)
        .collect()
}

/// The role a pattern refers to, when it is a role alias rather than a model.
fn role_alias(pattern: &str) -> Option<String> {
    if pattern == DEFAULT_ROLE_ALIAS {
        return Some(DEFAULT_ROLE.to_string());
    }
    ROLE_ALIAS_PREFIXES.iter().find_map(|prefix| {
        let role = pattern.strip_prefix(prefix)?;
        let role = role.split_once(':').map_or(role, |(name, _)| name);
        (!role.is_empty()).then(|| role.to_string())
    })
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|existing| existing == value) {
        list.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roles_fallback_chains_and_remote_servers_are_collected_without_aliases() {
        let overlay = json!({
            "modelRoles": {
                "default": "anthropic/claude-sonnet-4-5:high",
                "smol": "openrouter/meta-llama/llama-4, gpt-5-mini",
                "commit": "@smol"
            },
            "retry": {"fallbackChains": {
                "default": ["openai/gpt-5", "google/*"],
                "anthropic/*": ["anthropic/claude-sonnet-4-5:high"]
            }},
            "tools": {"approvalMode": "yolo"}
        });
        let seed = json!({"mcpServers": {
            "docs": {"url": "https://mcp.example.com/sse"},
            "local": {"command": "mcp-local"}
        }});
        let needs = AgentNeeds::from_configuration(&overlay, &seed);
        let mut patterns = needs.model_patterns.clone();
        patterns.sort();
        assert_eq!(
            patterns,
            [
                "anthropic/claude-sonnet-4-5:high",
                "google/*",
                "gpt-5-mini",
                "openai/gpt-5",
                "openrouter/meta-llama/llama-4"
            ]
        );
        assert_eq!(
            needs.default_model_patterns,
            ["anthropic/claude-sonnet-4-5:high"]
        );
        assert_eq!(needs.mcp_server_urls, ["https://mcp.example.com/sse"]);
    }

    #[test]
    fn an_unconfigured_agent_needs_nothing_and_names_no_default() {
        let needs = AgentNeeds::from_configuration(&json!({}), &json!({"mcpServers": {}}));
        assert_eq!(needs, AgentNeeds::default());
    }

    #[test]
    fn a_default_that_is_an_alias_follows_the_agents_own_roles() {
        let overlay = json!({"modelRoles": {
            "default": "@slow, pi/smol",
            "slow": "openai/gpt-5:xhigh",
            "smol": ["gpt-5-mini", "*"]
        }});
        let needs = AgentNeeds::from_configuration(&overlay, &Value::Null);
        assert_eq!(
            needs.default_model_patterns,
            ["openai/gpt-5:xhigh", "gpt-5-mini"]
        );
        let cyclic = json!({"modelRoles": {"default": "@plan", "plan": "*"}});
        let needs = AgentNeeds::from_configuration(&cyclic, &Value::Null);
        assert!(needs.default_model_patterns.is_empty());
    }
}
