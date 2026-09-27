// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The orchestrator service: protocol methods over the project state, independent of the
//! socket transport so that it can be driven directly in tests and by the CLI.

use std::sync::Arc;

use clyean_agents::AgentId;
use clyean_git::GitRepository;
use clyean_harness::session::BoxFuture;
use clyean_plantuml::RenderReport;
use clyean_project::{
    PlanCatalog, ProjectConfig, ProjectDirectory, ProjectLayout, ProjectLock, ProjectLockError,
    ProjectType,
};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};

use crate::agents::{AgentSessionFactory, SubAgentPool};
use crate::events::EventSink;
use crate::journal::{journal_path, list_journals, WorkJournal, WorkKind, WorkStatus};
use crate::lease::UserAssistantLease;
use crate::protocol::{
    ConnectionOrigin, PromptType, ProvideInformationParams, Request, Response, ScaffoldParams,
    StreamedEvent, WorkIdParams, WorkStartParams,
};
use crate::relays::{opened_result, RelayError, RelayManager, RelayRequest, Requester};
use crate::scaffold::{complete_scaffold, PendingScaffold};
use crate::servers::{LaunchRequest, ServerError, ServerManager};
use crate::work::{AnswerDelivery, WorkHandle, WorkId, WorkRegistry};
use crate::workflows::{self, WorkContext};
use crate::{OrchestratorError, Result};

/// Renders the project's architecture diagrams inside the sandbox.
pub trait DiagramRenderer: Send + Sync {
    fn render<'a>(&'a self) -> BoxFuture<'a, Result<RenderReport>>;
}

/// A renderer that reports success without rendering, for tests and dry runs.
pub struct NoopRenderer;

impl DiagramRenderer for NoopRenderer {
    fn render<'a>(&'a self) -> BoxFuture<'a, Result<RenderReport>> {
        Box::pin(async { Ok(RenderReport::default()) })
    }
}

/// The dependencies of every workflow of a scaffolded project.
pub struct ProjectServices {
    pub directory: ProjectDirectory,
    pub layout: ProjectLayout,
    pub config: ProjectConfig,
    pub git: GitRepository,
    pub plans: PlanCatalog,
    pub renderer: Arc<dyn DiagramRenderer>,
    pub factory: Arc<dyn AgentSessionFactory>,
    pub clyean_version: String,
}

/// A project before its type is known: enough to answer status and to scaffold.
pub struct UnscaffoldedProject {
    pub directory: ProjectDirectory,
    pub layout: ProjectLayout,
    pub git: GitRepository,
    pub pending: PendingScaffold,
    pub renderer: Arc<dyn DiagramRenderer>,
    pub factory: Arc<dyn AgentSessionFactory>,
    pub clyean_version: String,
}

pub enum ProjectState {
    Unscaffolded(Box<UnscaffoldedProject>),
    Scaffolded(Arc<ProjectServices>),
}

/// The outcome of dispatching one request: the immediate response and, for streaming
/// methods, the subscription to forward until a closing event.
pub struct Dispatch {
    pub response: Response,
    pub stream: Option<StreamAttachment>,
}

pub struct StreamAttachment {
    pub receiver: broadcast::Receiver<StreamedEvent>,
    /// Events to send before the live ones (a pending information request on resume).
    pub replay: Vec<StreamedEvent>,
}

pub struct OrchestratorService {
    state: Mutex<ProjectState>,
    registry: Arc<WorkRegistry>,
    lease: Arc<UserAssistantLease>,
    relays: Option<Arc<RelayManager>>,
    servers: Option<Arc<ServerManager>>,
    version: String,
}

impl OrchestratorService {
    pub fn new(state: ProjectState, version: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(state),
            registry: Arc::new(WorkRegistry::default()),
            lease: Arc::default(),
            relays: None,
            servers: None,
            version: version.into(),
        }
    }

    /// Tracks running work in `registry`, which the handlers of sub-agents' requests share.
    pub fn with_registry(mut self, registry: Arc<WorkRegistry>) -> Self {
        self.registry = registry;
        self
    }

    /// Uses `lease`, which the credential authority of the sub-agent session factory
    /// shares, as the User Assistant's lease.
    pub fn with_lease(mut self, lease: Arc<UserAssistantLease>) -> Self {
        self.lease = lease;
        if let Some(relays) = &self.relays {
            self.lease.set_request_handler(relays.clone());
        }
        self
    }

    /// Serves `relay.*` methods through `relays`, which also answers the extension's
    /// requests on the lease.
    pub fn with_relays(mut self, relays: Arc<RelayManager>) -> Self {
        self.lease.set_request_handler(relays.clone());
        self.relays = Some(relays);
        self
    }

    /// Serves `server.*` methods through `servers`.
    pub fn with_servers(mut self, servers: Arc<ServerManager>) -> Self {
        self.servers = Some(servers);
        self
    }

    pub fn lease(&self) -> &Arc<UserAssistantLease> {
        &self.lease
    }

    pub async fn services(&self) -> Result<Arc<ProjectServices>> {
        match &*self.state.lock().await {
            ProjectState::Scaffolded(services) => Ok(services.clone()),
            ProjectState::Unscaffolded(_) => Err(OrchestratorError::NotScaffolded),
        }
    }

    pub async fn is_scaffolded(&self) -> bool {
        matches!(&*self.state.lock().await, ProjectState::Scaffolded(_))
    }

    /// Routes one request to its handler, converting errors to protocol errors.
    pub async fn dispatch(&self, request: Request, origin: &ConnectionOrigin) -> Dispatch {
        let id = request.id.clone();
        if request.method.starts_with("relay.") {
            return self.dispatch_relay(&id, &request, origin).await;
        }
        if request.method.starts_with("server.") {
            return self.dispatch_server(&id, request, origin).await;
        }
        let result = match request.method.as_str() {
            "ping" => Ok(Dispatch {
                response: Response::result(&id, json!({"type": "pong", "version": self.version})),
                stream: None,
            }),
            "project.status" => self.project_status(&id).await,
            "project.scaffold" => match serde_json::from_value::<ScaffoldParams>(request.params) {
                Ok(params) => self.project_scaffold(&id, params).await,
                Err(e) => Ok(invalid(&id, e)),
            },
            "work.start" => match serde_json::from_value::<WorkStartParams>(request.params) {
                Ok(params) => self.work_start(&id, params).await,
                Err(e) => Ok(invalid(&id, e)),
            },
            "work.provide_information" => {
                match serde_json::from_value::<ProvideInformationParams>(request.params) {
                    Ok(params) => self.work_provide_information(&id, params).await,
                    Err(e) => Ok(invalid(&id, e)),
                }
            }
            "work.resume" => match serde_json::from_value::<WorkIdParams>(request.params) {
                Ok(params) => self.work_resume(&id, &params.work_id).await,
                Err(e) => Ok(invalid(&id, e)),
            },
            "work.cancel" => match serde_json::from_value::<WorkIdParams>(request.params) {
                Ok(params) => self.work_cancel(&id, &params.work_id).await,
                Err(e) => Ok(invalid(&id, e)),
            },
            other => Ok(Dispatch {
                response: Response::error(
                    &id,
                    "unknown_method",
                    format!("unknown method: {other}"),
                ),
                stream: None,
            }),
        };
        result.unwrap_or_else(|error| Dispatch {
            response: Response::error(&id, error.code(), error.to_string()),
            stream: None,
        })
    }

    async fn dispatch_relay(
        &self,
        id: &str,
        request: &Request,
        origin: &ConnectionOrigin,
    ) -> Dispatch {
        let answer = match &self.relays {
            None => Err(RelayError::unavailable(
                "relays need the User Assistant's bridge, which this process does not run",
            )),
            Some(relays) => self.relay_method(relays, request, origin).await,
        };
        let response = match answer {
            Ok(result) => Response::result(id, result),
            Err(error) => Response::error_with_reason(id, error.code, error.message, error.reason),
        };
        Dispatch {
            response,
            stream: None,
        }
    }

    async fn relay_method(
        &self,
        relays: &RelayManager,
        request: &Request,
        origin: &ConnectionOrigin,
    ) -> std::result::Result<Value, RelayError> {
        match request.method.as_str() {
            "relay.open" => {
                if request.params.get("purpose").is_some() {
                    return Err(RelayError::lease_only("relay.open for a sign-in"));
                }
                let relay = RelayRequest::from_params(&request.params)?;
                let requester = Requester::Process {
                    pid: origin.peer.as_ref().map(|peer| peer.pid),
                    command: origin.peer.as_ref().map(|peer| peer.command.clone()),
                };
                let opened = relays.open(relay, requester).await?;
                Ok(opened_result(&opened))
            }
            "relay.close" => {
                let relay_id = request
                    .params
                    .get("relay_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RelayError::invalid("relay.close needs a relay_id"))?;
                relays.close(relay_id);
                Ok(json!({"type": "relay_closed", "relay_id": relay_id}))
            }
            "relay.list" => Ok(json!({"type": "relay_list", "relays": relays.list()})),
            method if method.starts_with("relay.allowlist.") => Err(RelayError::lease_only(method)),
            other => Err(RelayError::new_unknown(other)),
        }
    }

    async fn dispatch_server(
        &self,
        id: &str,
        request: Request,
        origin: &ConnectionOrigin,
    ) -> Dispatch {
        let answer = match &self.servers {
            None => Err(ServerError::failed(
                "server containers are not available in this process",
            )),
            Some(servers) => server_method(servers, request, origin).await,
        };
        let response = match answer {
            Ok(result) => Response::result(id, result),
            Err(error) => Response::error_with_reason(id, error.code, error.message, error.reason),
        };
        Dispatch {
            response,
            stream: None,
        }
    }

    async fn project_status(&self, id: &str) -> Result<Dispatch> {
        let state = self.state.lock().await;
        let (layout, project_type, use_worktrees) = match &*state {
            ProjectState::Scaffolded(services) => (
                services.layout.clone(),
                Some(services.config.project_type),
                services.config.git.use_worktrees,
            ),
            ProjectState::Unscaffolded(project) => {
                (project.layout.clone(), None, project.pending.use_worktrees)
            }
        };
        drop(state);
        let locked =
            match ProjectLock::acquire_with_retry(&layout, use_worktrees, 4, LOCK_RETRY_DELAY) {
                Ok(_) => false,
                Err(ProjectLockError::Locked(_)) => true,
                Err(other) => return Err(OrchestratorError::Workflow(other.to_string())),
            };
        let running = self.registry.running_ids().await;
        let incomplete: Vec<Value> = list_journals(&layout)?
            .into_iter()
            .filter(|(_, journal)| journal.is_resumable())
            .map(|(_, journal)| {
                json!({
                    "work_id": journal.work_id,
                    "kind": journal.kind,
                    "prompt_type": journal.prompt_type.map(|p| p.as_str()),
                    "session_id": journal.session_id,
                    "phase": journal.phase.label(),
                    "status": journal.status,
                    "running": running.contains(&journal.work_id),
                    "plan": journal.plan,
                    "started_at": journal.created_at,
                })
            })
            .collect();
        Ok(Dispatch {
            response: Response::result(
                id,
                json!({
                    "type": "project_status",
                    "scaffolded": project_type.is_some(),
                    "project_type": project_type.map(ProjectType::as_str),
                    "locked": locked || !running.is_empty(),
                    "incomplete_work": incomplete,
                }),
            ),
            stream: None,
        })
    }

    async fn project_scaffold(&self, id: &str, params: ScaffoldParams) -> Result<Dispatch> {
        let services = {
            let mut state = self.state.lock().await;
            match &*state {
                ProjectState::Scaffolded(_) => {
                    return Ok(Dispatch {
                        response: Response::error(
                            id,
                            "invalid_request",
                            "the project is already scaffolded",
                        ),
                        stream: None,
                    })
                }
                ProjectState::Unscaffolded(project) => {
                    let _lock =
                        acquire_project_lock(&project.layout, project.pending.use_worktrees)?;
                    let report = complete_scaffold(
                        &project.directory,
                        &project.layout,
                        &project.git,
                        &project.pending,
                        params.project_type,
                        &project.clyean_version,
                    )?;
                    let services = Arc::new(ProjectServices {
                        directory: project.directory.clone(),
                        layout: project.layout.clone(),
                        config: report.config,
                        git: project.git.clone(),
                        plans: PlanCatalog::new(&project.layout),
                        renderer: project.renderer.clone(),
                        factory: project.factory.clone(),
                        clyean_version: project.clyean_version.clone(),
                    });
                    *state = ProjectState::Scaffolded(services.clone());
                    services
                }
            }
        };
        let mut journal = WorkJournal::new(
            WorkId::generate().to_string(),
            WorkKind::Scaffold,
            format!("Scaffold the project as a {}", params.project_type.as_str()),
        );
        journal.session_id = params.session_id;
        journal.scratch.insert(
            workflows::scaffold::PROJECT_TYPE_KEY.into(),
            Value::String(params.project_type.as_str().into()),
        );
        journal.save(&journal_path(&services.layout, &journal))?;
        let attachment = self.spawn_work(services, journal.clone()).await?;
        Ok(Dispatch {
            response: Response::result(
                id,
                json!({"type": "work_accepted", "work_id": journal.work_id}),
            ),
            stream: Some(attachment),
        })
    }

    async fn work_start(&self, id: &str, params: WorkStartParams) -> Result<Dispatch> {
        let services = self.services().await?;
        let kind = match params.prompt_type {
            PromptType::SoftwareEngineeringProjectResearch => WorkKind::Research,
            PromptType::SoftwareEngineeringProjectPlanning => WorkKind::Planning,
            PromptType::SoftwareEngineeringProjectImplementation => WorkKind::Implementation,
        };
        let mut journal = WorkJournal::new(WorkId::generate().to_string(), kind, params.prompt);
        journal.prompt_type = Some(params.prompt_type);
        journal.original_prompt = params.original_prompt;
        journal.session_id = params.session_id;
        journal.plan = params.plan.filter(|p| !p.trim().is_empty());
        journal.save(&journal_path(&services.layout, &journal))?;
        let attachment = self.spawn_work(services, journal.clone()).await?;
        Ok(Dispatch {
            response: Response::result(
                id,
                json!({"type": "work_accepted", "work_id": journal.work_id}),
            ),
            stream: Some(attachment),
        })
    }

    async fn work_provide_information(
        &self,
        id: &str,
        params: ProvideInformationParams,
    ) -> Result<Dispatch> {
        let handle = match self.registry.get(&params.work_id).await {
            Some(handle) => handle,
            None => {
                self.resume_from_journal(&params.work_id).await?;
                self.registry
                    .get(&params.work_id)
                    .await
                    .ok_or_else(|| OrchestratorError::WorkNotFound(params.work_id.clone()))?
            }
        };
        let receiver = handle.subscribe();
        let delivered = handle
            .deliver_answers(AnswerDelivery {
                request_id: params.request_id.clone(),
                answers: params.answers,
            })
            .await;
        if !delivered {
            return Err(OrchestratorError::RequestNotFound {
                work_id: params.work_id,
                request_id: params.request_id,
            });
        }
        Ok(Dispatch {
            response: Response::result(
                id,
                json!({"type": "work_resumed", "work_id": params.work_id}),
            ),
            stream: Some(StreamAttachment {
                receiver,
                replay: Vec::new(),
            }),
        })
    }

    async fn work_resume(&self, id: &str, work_id: &str) -> Result<Dispatch> {
        let handle = match self.registry.get(work_id).await {
            Some(handle) => handle,
            None => self.resume_from_journal(work_id).await?,
        };
        let receiver = handle.subscribe();
        let replay = handle
            .pending_information_request()
            .await
            .into_iter()
            .collect();
        Ok(Dispatch {
            response: Response::result(id, json!({"type": "work_resumed", "work_id": work_id})),
            stream: Some(StreamAttachment { receiver, replay }),
        })
    }

    async fn work_cancel(&self, id: &str, work_id: &str) -> Result<Dispatch> {
        let handle = self
            .registry
            .get(work_id)
            .await
            .ok_or_else(|| OrchestratorError::WorkNotFound(work_id.to_string()))?;
        handle.cancellation.cancel();
        Ok(Dispatch {
            response: Response::result(id, json!({"type": "work_cancelled", "work_id": work_id})),
            stream: None,
        })
    }

    async fn resume_from_journal(&self, work_id: &str) -> Result<WorkHandle> {
        let services = self.services().await?;
        let (_, journal) = list_journals(&services.layout)?
            .into_iter()
            .find(|(_, journal)| journal.work_id == work_id)
            .ok_or_else(|| OrchestratorError::WorkNotFound(work_id.to_string()))?;
        if !journal.is_resumable() {
            return Err(OrchestratorError::Workflow(format!(
                "work {work_id} already finished with status {:?}",
                journal.status
            )));
        }
        let attachment = self.spawn_work(services, journal).await?;
        drop(attachment);
        self.registry
            .get(work_id)
            .await
            .ok_or_else(|| OrchestratorError::WorkNotFound(work_id.to_string()))
    }

    /// Starts the task that runs `journal`'s workflow, holding the project lock for its
    /// duration, and returns a subscription created before the first event.
    async fn spawn_work(
        &self,
        services: Arc<ProjectServices>,
        mut journal: WorkJournal,
    ) -> Result<StreamAttachment> {
        let lock = acquire_project_lock(&services.layout, services.config.git.use_worktrees)?;
        let (handle, answers) = WorkHandle::new(WorkId::from_string(journal.work_id.clone()));
        handle.record_phase(journal.phase);
        let receiver = handle.subscribe();
        self.registry.insert(handle.clone()).await;
        let registry = self.registry.clone();
        let ctx = WorkContext {
            pool: SubAgentPool::new(services.factory.clone(), journal.work_id.clone()),
            sink: EventSink::new(handle.clone()),
            handle: handle.clone(),
            answers: Mutex::new(answers),
            services: services.clone(),
        };
        tokio::spawn(async move {
            let _lock = lock;
            journal.status = WorkStatus::Running;
            let outcome = match journal.kind {
                WorkKind::Scaffold => workflows::scaffold::run_scaffold(&ctx, &mut journal).await,
                WorkKind::Research => workflows::research::run_research(&ctx, &mut journal).await,
                WorkKind::Planning => {
                    workflows::planning::run_planning_phases(&ctx, &mut journal).await
                }
                WorkKind::Implementation => {
                    workflows::implementation::run_implementation(&ctx, &mut journal).await
                }
            };
            match outcome {
                Ok(()) => {}
                Err(OrchestratorError::Cancelled) => {
                    journal.cancel();
                    let _ = ctx.save(&mut journal);
                    ctx.sink.failed("cancelled", "the work was cancelled").await;
                }
                Err(error) => {
                    tracing::error!(target: "clyean::orchestrator", work_id = %journal.work_id, %error, "work failed");
                    journal.fail(error.to_string());
                    let _ = ctx.save(&mut journal);
                    ctx.sink.failed(error.code(), &error.to_string()).await;
                }
            }
            ctx.pool.shutdown_all().await;
            registry.remove(journal.work_id.as_str()).await;
        });
        Ok(StreamAttachment {
            receiver,
            replay: Vec::new(),
        })
    }
}

/// Server methods asked through the orchestrator socket, which only processes in the User
/// Assistant's container reach.
async fn server_method(
    servers: &ServerManager,
    request: Request,
    origin: &ConnectionOrigin,
) -> std::result::Result<Value, ServerError> {
    match request.method.as_str() {
        "server.launch" => {
            let launch = LaunchRequest::from_params(request.params)?;
            let requester = Requester::Process {
                pid: origin.peer.as_ref().map(|peer| peer.pid),
                command: origin.peer.as_ref().map(|peer| peer.command.clone()),
            };
            servers
                .launch(launch, AgentId::UserAssistant, requester)
                .await
        }
        "server.stop" => {
            let name = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ServerError::invalid("server.stop needs a name"))?;
            servers.stop(name).await
        }
        "server.list" => servers.list().await,
        other => Err(ServerError {
            code: "unknown_method",
            message: format!("unknown method: {other}"),
            reason: None,
        }),
    }
}

/// How long a lock attempt tolerates the exec window of a child process that inherited
/// a just-released lock before reporting the project as locked.
const LOCK_RETRY_ATTEMPTS: u32 = 40;
const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

fn acquire_project_lock(layout: &ProjectLayout, use_worktrees: bool) -> Result<ProjectLock> {
    ProjectLock::acquire_with_retry(layout, use_worktrees, LOCK_RETRY_ATTEMPTS, LOCK_RETRY_DELAY)
        .map_err(|error| match error {
            ProjectLockError::Locked(_) => OrchestratorError::ProjectLocked,
            other => OrchestratorError::Workflow(other.to_string()),
        })
}

fn invalid(id: &str, error: serde_json::Error) -> Dispatch {
    Dispatch {
        response: Response::error(
            id,
            "invalid_request",
            format!("invalid parameters: {error}"),
        ),
        stream: None,
    }
}
