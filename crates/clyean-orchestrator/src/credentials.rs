// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The credential authority: the User Assistant's harness, which holds every sign-in and
//! answers the orchestrator's requests for credential copies over its lease.  From the
//! answers, the orchestrator plans what each sub-agent receives when it starts and renews
//! copies while it works.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use clyean_agents::{AgentId, AgentNeeds};
use clyean_harness::session::BoxFuture;
use clyean_harness::{RpcFrame, UiRequestHandler};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::lease::{LeaseError, UserAssistantLease};
use crate::{OrchestratorError, Result};

pub const RESOLVE_METHOD: &str = "credentials.resolve";
pub const VARIABLES_METHOD: &str = "credentials.variables";
pub const COPIES_METHOD: &str = "credentials.copies";

/// The title of the extension UI request with which a sub-agent's credentials extension
/// asks for renewed copies; the placeholder carries what it asks for.
pub const RENEWAL_REQUEST_TITLE: &str = "clyean:credentials";

/// How long the User Assistant may take to answer, which covers refreshing its sign-ins.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("no User Assistant is connected to supply credentials")]
    Unavailable,
    #[error("the User Assistant's connection ended before it answered")]
    Closed,
    #[error("the User Assistant did not answer within {} seconds", REQUEST_TIMEOUT.as_secs())]
    Timeout,
    #[error("the User Assistant could not supply credentials: {0}")]
    Refused(String),
    #[error("the User Assistant's answer was malformed: {0}")]
    Malformed(String),
}

impl From<LeaseError> for AuthorityError {
    fn from(error: LeaseError) -> Self {
        match error {
            LeaseError::Unavailable => Self::Unavailable,
            LeaseError::Closed => Self::Closed,
            LeaseError::Timeout(_) => Self::Timeout,
            LeaseError::Refused { message, .. } => Self::Refused(message),
            LeaseError::Malformed(message) => Self::Malformed(message),
        }
    }
}

impl From<AuthorityError> for OrchestratorError {
    fn from(error: AuthorityError) -> Self {
        Self::Credentials(error.to_string())
    }
}

/// Asks the User Assistant, over its lease, about models and for credential copies.
pub struct CredentialAuthority {
    lease: Arc<UserAssistantLease>,
}

impl CredentialAuthority {
    pub fn new(lease: Arc<UserAssistantLease>) -> Self {
        Self { lease }
    }

    pub fn is_connected(&self) -> bool {
        self.lease.is_connected()
    }

    /// Sends one request to the User Assistant and waits for its result.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> std::result::Result<Value, AuthorityError> {
        Ok(self.lease.request(method, params, REQUEST_TIMEOUT).await?)
    }

    /// Maps model patterns to their providers and reports the User Assistant's model.
    pub async fn resolve(
        &self,
        patterns: &[String],
    ) -> std::result::Result<ModelResolution, AuthorityError> {
        let result = self
            .request(RESOLVE_METHOD, json!({"patterns": patterns}))
            .await?;
        parse(result)
    }

    /// The names of the environment variables the harness reads for each provider.
    pub async fn variables(
        &self,
        providers: &[String],
    ) -> std::result::Result<HashMap<String, Vec<String>>, AuthorityError> {
        #[derive(Deserialize)]
        struct Variables {
            #[serde(default)]
            variables: HashMap<String, Vec<String>>,
        }
        let result = self
            .request(VARIABLES_METHOD, json!({"providers": providers}))
            .await?;
        parse::<Variables>(result).map(|answer| answer.variables)
    }

    /// Copies of the User Assistant's credentials for `providers` and `mcp_servers`, made
    /// for `agent`: refreshed when close to expiry, without refresh tokens, and with MCP
    /// credentials keyed to `agent`'s profile.
    pub async fn copies(
        &self,
        agent: AgentId,
        providers: &[String],
        mcp_servers: &[String],
    ) -> std::result::Result<CredentialCopies, AuthorityError> {
        let result = self
            .request(
                COPIES_METHOD,
                json!({"agent": agent.id(), "providers": providers, "mcp_servers": mcp_servers}),
            )
            .await?;
        parse(result)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> std::result::Result<T, AuthorityError> {
    serde_json::from_value(value).map_err(|error| AuthorityError::Malformed(error.to_string()))
}

/// One model pattern as the User Assistant's harness resolved it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResolvedModel {
    pub provider: String,
    /// A selector naming exactly this model (with the pattern's thinking level), usable
    /// with `--model`; absent for patterns that name a provider rather than a model.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ModelResolution {
    /// Every pattern asked about, with `None` for patterns that name nothing known.
    #[serde(default)]
    pub models: HashMap<String, Option<ResolvedModel>>,
    /// The User Assistant's current model.
    #[serde(default)]
    pub current: Option<ResolvedModel>,
}

/// Credential copies, grouped as the sub-agent's credentials extension imports them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CredentialCopies {
    /// Copies per model provider, each a list of the harness's stored credentials.
    #[serde(default)]
    pub providers: Map<String, Value>,
    /// MCP server credentials by the identifier the receiving agent's harness looks up.
    #[serde(default)]
    pub mcp: Map<String, Value>,
    /// Providers asked about for which the User Assistant itself has no credential.
    #[serde(default)]
    pub unavailable: Vec<String>,
}

impl CredentialCopies {
    /// The contents of a sub-agent's bundle file, and of a renewal answer.
    pub fn bundle(&self) -> Value {
        json!({"version": 1, "providers": self.providers, "mcp": self.mcp})
    }
}

/// What a sub-agent receives when it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    /// The credential copies written to the agent's bundle file.
    pub bundle: Value,
    /// The host variables, by exact name or `*` pattern, its container receives.
    pub secrets: Vec<String>,
    /// The model it runs with, passed to its harness with `--model`.
    pub model: Option<String>,
    /// The providers and MCP servers renewals may ask for.
    pub providers: Vec<String>,
    pub mcp_servers: Vec<String>,
}

impl Delivery {
    /// For a sub-agent started while no User Assistant runs (`clyean scaffold
    /// --project-type`): nothing can resolve its needs or supply copies, so it receives
    /// the host variables the User Assistant would, and its earlier copies are cleared.
    pub fn without_authority(builtin_patterns: &[&str]) -> Self {
        Self {
            bundle: CredentialCopies::default().bundle(),
            secrets: builtin_patterns.iter().map(|p| p.to_string()).collect(),
            model: None,
            providers: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }
}

/// Works out what `agent` receives from its needs, the User Assistant's answers, and the
/// names of the variables set in the host environment.  An agent whose model cannot be
/// supplied by the User Assistant or the host is not started.
pub async fn plan_delivery(
    authority: &CredentialAuthority,
    agent: AgentId,
    needs: &AgentNeeds,
    host_variables: &HashSet<String>,
) -> Result<Delivery> {
    let resolution = authority.resolve(&needs.model_patterns).await?;
    let mut providers: Vec<String> = Vec::new();
    for pattern in &needs.model_patterns {
        if let Some(Some(resolved)) = resolution.models.get(pattern) {
            push_unique(&mut providers, &resolved.provider);
        }
    }
    let candidates = model_candidates(agent, needs, &resolution)?;
    for (_, provider) in &candidates {
        push_unique(&mut providers, provider);
    }
    let variables = authority.variables(&providers).await?;
    let copies = authority
        .copies(agent, &providers, &needs.mcp_server_urls)
        .await?;
    let can_supply = |provider: &str| {
        !copies.unavailable.iter().any(|p| p == provider)
            || variables
                .get(provider)
                .is_some_and(|names| names.iter().any(|name| host_variables.contains(name)))
    };
    let Some((model, _)) = candidates.iter().find(|(_, provider)| can_supply(provider)) else {
        return Err(missing_sign_in(agent, &candidates, &variables));
    };
    let mut secrets = Vec::new();
    for provider in &providers {
        for name in variables.get(provider).into_iter().flatten() {
            push_unique(&mut secrets, name);
        }
    }
    Ok(Delivery {
        bundle: copies.bundle(),
        secrets,
        model: Some(model.clone()),
        providers,
        mcp_servers: needs.mcp_server_urls.clone(),
    })
}

/// The models `agent` may run with, in order, as (`--model` selector, provider): its
/// configured default patterns that name a model, or else the User Assistant's model.
fn model_candidates(
    agent: AgentId,
    needs: &AgentNeeds,
    resolution: &ModelResolution,
) -> Result<Vec<(String, String)>> {
    if needs.default_model_patterns.is_empty() {
        let Some(ResolvedModel {
            provider,
            model: Some(model),
        }) = &resolution.current
        else {
            return Err(OrchestratorError::Credentials(format!(
                "the {} agent's configuration names no default model, so it runs with the User Assistant's model, and the User Assistant has none selected; choose one with /model",
                agent.id()
            )));
        };
        return Ok(vec![(model.clone(), provider.clone())]);
    }
    let candidates: Vec<(String, String)> = needs
        .default_model_patterns
        .iter()
        .filter_map(|pattern| match resolution.models.get(pattern) {
            Some(Some(ResolvedModel {
                provider,
                model: Some(model),
            })) => Some((model.clone(), provider.clone())),
            _ => None,
        })
        .collect();
    if candidates.is_empty() {
        return Err(OrchestratorError::Credentials(format!(
            "none of the default models of the {} agent ({}) is a model the harness knows; correct modelRoles.default in .clyean/agents/{}",
            agent.id(),
            needs.default_model_patterns.join(", "),
            agent.settings_file_name()
        )));
    }
    Ok(candidates)
}

fn missing_sign_in(
    agent: AgentId,
    candidates: &[(String, String)],
    variables: &HashMap<String, Vec<String>>,
) -> OrchestratorError {
    let mut providers: Vec<String> = Vec::new();
    for (_, provider) in candidates {
        push_unique(&mut providers, provider);
    }
    let names: Vec<String> = providers
        .iter()
        .flat_map(|provider| variables.get(provider).into_iter().flatten().cloned())
        .collect();
    let host_hint = if names.is_empty() {
        String::new()
    } else {
        format!(", or set {} on the host", names.join(" or "))
    };
    OrchestratorError::Credentials(format!(
        "the {} agent cannot start: its model needs a sign-in for {}, which neither the User Assistant nor the host environment has; sign in with /login in the User Assistant{host_hint}, then resume the work",
        agent.id(),
        providers.join(" or ")
    ))
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|existing| existing == value) {
        list.push(value.to_string());
    }
}

/// Answers a sub-agent's renewal requests with fresh copies from the User Assistant, for
/// the providers and MCP servers computed when the sub-agent started and nothing else.
pub struct CredentialRenewals {
    pub authority: Arc<CredentialAuthority>,
    pub agent: AgentId,
    pub providers: Vec<String>,
    pub mcp_servers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RenewalRequest {
    #[serde(default)]
    providers: Vec<String>,
    #[serde(default)]
    mcp_servers: Vec<String>,
}

impl CredentialRenewals {
    async fn renew(&self, request: &RpcFrame) -> Value {
        let wanted: RenewalRequest = match request
            .0
            .get("placeholder")
            .and_then(Value::as_str)
            .map(serde_json::from_str)
        {
            Some(Ok(wanted)) => wanted,
            _ => return json!({"error": "the renewal request was malformed"}),
        };
        let outside: Vec<&str> = wanted
            .providers
            .iter()
            .filter(|p| !self.providers.contains(p))
            .chain(
                wanted
                    .mcp_servers
                    .iter()
                    .filter(|s| !self.mcp_servers.contains(s)),
            )
            .map(String::as_str)
            .collect();
        if !outside.is_empty() {
            return json!({"error": format!(
                "the {} agent's configuration does not name {}",
                self.agent.id(),
                outside.join(", ")
            )});
        }
        match self
            .authority
            .copies(self.agent, &wanted.providers, &wanted.mcp_servers)
            .await
        {
            Ok(copies) => copies.bundle(),
            Err(error) => json!({"error": error.to_string()}),
        }
    }
}

impl UiRequestHandler for CredentialRenewals {
    fn answer<'a>(&'a self, request: &'a RpcFrame) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            if request.0.get("title").and_then(Value::as_str) != Some(RENEWAL_REQUEST_TITLE) {
                return None;
            }
            let answer = self.renew(request).await;
            Some(json!({"value": answer.to_string()}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A User Assistant on the far end of a lease connection that answers every request
    /// with `answer(method, params)`.
    fn user_assistant(
        answer: impl Fn(&str, &Value) -> Value + Send + 'static,
    ) -> (Arc<CredentialAuthority>, tokio::task::JoinHandle<()>) {
        let lease = Arc::new(UserAssistantLease::default());
        let authority = Arc::new(CredentialAuthority::new(lease.clone()));
        let (orchestrator_end, assistant_end) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(orchestrator_end);
        tokio::spawn(async move {
            let _ = lease.serve(BufReader::new(reader).lines(), writer).await;
        });
        let assistant = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(assistant_end);
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap().to_string();
                let result = answer(&method, &request["params"]);
                let reply = if result.get("refuse").is_some() {
                    json!({"id": request["id"], "error": {"code": "refused", "message": result["refuse"]}})
                } else {
                    json!({"id": request["id"], "result": result})
                };
                writer
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        (authority, assistant)
    }

    async fn connected(authority: &CredentialAuthority) {
        for _ in 0..100 {
            if authority.is_connected() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the lease connection never registered");
    }

    fn answers(method: &str, params: &Value) -> Value {
        match method {
            RESOLVE_METHOD => json!({
                "type": "models_resolved",
                "models": {
                    "anthropic/claude-sonnet-4-5": {"provider": "anthropic", "model": "anthropic/claude-sonnet-4-5"},
                    "gpt-5": {"provider": "openai", "model": "openai/gpt-5"},
                    "nothing-known": null
                },
                "current": {"provider": "openai", "model": "openai/gpt-5.5"}
            }),
            VARIABLES_METHOD => {
                let mut variables = Map::new();
                for provider in params["providers"].as_array().unwrap() {
                    let provider = provider.as_str().unwrap();
                    variables.insert(
                        provider.into(),
                        json!([format!("{}_API_KEY", provider.to_uppercase())]),
                    );
                }
                json!({"type": "credential_variables", "variables": variables})
            }
            COPIES_METHOD => {
                assert_eq!(params["agent"], "programmer");
                json!({
                    "type": "credential_copies",
                    "providers": {"anthropic": [{"type": "oauth", "access": "a", "refresh": "", "expires": 1}]},
                    "mcp": {"mcp_oauth:profile:programmer:https://mcp.example.com": {"type": "oauth", "access": "m", "refresh": "", "expires": 1}},
                    "unavailable": params["providers"].as_array().unwrap().iter()
                        .filter(|p| p.as_str() == Some("openai")).cloned().collect::<Vec<_>>()
                })
            }
            _ => json!({"refuse": "unknown method"}),
        }
    }

    fn needs(defaults: &[&str], patterns: &[&str]) -> AgentNeeds {
        AgentNeeds {
            model_patterns: patterns.iter().map(|p| p.to_string()).collect(),
            default_model_patterns: defaults.iter().map(|p| p.to_string()).collect(),
            mcp_server_urls: vec!["https://mcp.example.com".into()],
        }
    }

    #[tokio::test]
    async fn a_configured_default_is_delivered_with_its_copies_and_variables() {
        let (authority, _assistant) = user_assistant(answers);
        connected(&authority).await;
        let needs = needs(
            &["anthropic/claude-sonnet-4-5"],
            &["anthropic/claude-sonnet-4-5", "gpt-5", "nothing-known"],
        );
        let delivery = plan_delivery(&authority, AgentId::Programmer, &needs, &HashSet::new())
            .await
            .unwrap();
        assert_eq!(
            delivery.model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(delivery.providers, ["anthropic", "openai"]);
        assert_eq!(delivery.secrets, ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]);
        assert_eq!(delivery.bundle["version"], 1);
        assert_eq!(delivery.bundle["providers"]["anthropic"][0]["access"], "a");
        assert!(delivery.bundle["mcp"]
            .get("mcp_oauth:profile:programmer:https://mcp.example.com")
            .is_some());
        assert!(delivery.bundle.get("unavailable").is_none());
    }

    #[tokio::test]
    async fn an_unconfigured_agent_runs_with_the_user_assistants_model() {
        let (authority, _assistant) = user_assistant(answers);
        connected(&authority).await;
        let host = HashSet::from(["OPENAI_API_KEY".to_string()]);
        let delivery = plan_delivery(&authority, AgentId::Programmer, &needs(&[], &[]), &host)
            .await
            .unwrap();
        assert_eq!(delivery.model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(delivery.providers, ["openai"]);
        assert_eq!(delivery.secrets, ["OPENAI_API_KEY"]);
    }

    #[tokio::test]
    async fn a_model_nothing_can_supply_stops_the_agent_and_names_the_sign_in() {
        let (authority, _assistant) = user_assistant(answers);
        connected(&authority).await;
        let error = plan_delivery(
            &authority,
            AgentId::Programmer,
            &needs(&["gpt-5"], &["gpt-5"]),
            &HashSet::new(),
        )
        .await
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("sign-in for openai"), "{message}");
        assert!(message.contains("OPENAI_API_KEY"), "{message}");
        assert_eq!(error.code(), "credentials_unavailable");
    }

    #[tokio::test]
    async fn a_later_default_is_chosen_when_an_earlier_one_cannot_be_supplied() {
        let (authority, _assistant) = user_assistant(answers);
        connected(&authority).await;
        let delivery = plan_delivery(
            &authority,
            AgentId::Programmer,
            &needs(
                &["gpt-5", "anthropic/claude-sonnet-4-5"],
                &["gpt-5", "anthropic/claude-sonnet-4-5"],
            ),
            &HashSet::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            delivery.model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[tokio::test]
    async fn requests_fail_cleanly_without_a_user_assistant_or_after_it_leaves() {
        let authority = CredentialAuthority::new(Arc::new(UserAssistantLease::default()));
        assert!(matches!(
            authority.resolve(&[]).await,
            Err(AuthorityError::Unavailable)
        ));
        let (authority, assistant) = user_assistant(answers);
        connected(&authority).await;
        assistant.abort();
        let _ = assistant.await;
        for _ in 0..100 {
            if !authority.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(matches!(
            authority.variables(&[]).await,
            Err(AuthorityError::Unavailable | AuthorityError::Closed)
        ));
    }

    #[tokio::test]
    async fn renewals_are_answered_only_within_the_agents_needs() {
        let (authority, _assistant) = user_assistant(answers);
        connected(&authority).await;
        let renewals = CredentialRenewals {
            authority,
            agent: AgentId::Programmer,
            providers: vec!["anthropic".into()],
            mcp_servers: vec!["https://mcp.example.com".into()],
        };
        let request = |placeholder: Value| {
            RpcFrame(
                json!({"type": "extension_ui_request", "id": "u1", "method": "input",
                "title": RENEWAL_REQUEST_TITLE, "placeholder": placeholder.to_string()}),
            )
        };
        let answer = renewals
            .answer(&request(json!({"providers": ["anthropic"]})))
            .await
            .unwrap();
        let value: Value = serde_json::from_str(answer["value"].as_str().unwrap()).unwrap();
        assert_eq!(value["providers"]["anthropic"][0]["access"], "a");

        let answer = renewals
            .answer(&request(json!({"providers": ["openai"]})))
            .await
            .unwrap();
        let value: Value = serde_json::from_str(answer["value"].as_str().unwrap()).unwrap();
        assert!(value["error"]
            .as_str()
            .unwrap()
            .contains("does not name openai"));

        let other = RpcFrame(
            json!({"type": "extension_ui_request", "id": "u2", "method": "input", "title": "Something else"}),
        );
        assert!(renewals.answer(&other).await.is_none());
    }
}
