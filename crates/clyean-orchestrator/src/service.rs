// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The orchestrator service: protocol methods over the project state, independent of the
//! socket transport so that it can be driven directly in tests and by the CLI.

use std::sync::Arc;

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
use crate::protocol::{
    PromptType, ProvideInformationParams, Request, Response, ScaffoldParams, StreamedEvent,
    WorkIdParams, WorkStartParams,
};
use crate::scaffold::{complete_scaffold, PendingScaffold};
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
    version: String,
}

impl OrchestratorService {
    pub fn new(state: ProjectState, version: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(state),
            registry: Arc::new(WorkRegistry::default()),
            version: version.into(),
        }
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
    pub async fn dispatch(&self, request: Request) -> Dispatch {
        let id = request.id.clone();
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
                        ProjectLock::acquire(&project.layout, project.pending.use_worktrees)
                            .map_err(|_| OrchestratorError::ProjectLocked)?;
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
