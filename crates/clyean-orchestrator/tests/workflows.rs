// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! End-to-end workflow tests over a real Git repository with scripted agents and a
//! no-op diagram renderer.

use std::path::Path;
use std::sync::Arc;

use clyean_agents::AgentId;
use clyean_git::GitRepository;
use clyean_orchestrator::agents::fake::ScriptedFactory;
use clyean_orchestrator::protocol::{Request, StreamedEvent};
use clyean_orchestrator::scaffold::{complete_scaffold, prepare_host_files, PendingScaffold};
use clyean_orchestrator::server::handle_connection;
use clyean_orchestrator::service::{
    NoopRenderer, OrchestratorService, ProjectServices, ProjectState, UnscaffoldedProject,
};
use clyean_orchestrator::StreamedEvent as Event;
use clyean_project::{PlanCatalog, ProjectDirectory, ProjectLayout, ProjectType};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn verdict(json: Value) -> String {
    format!("Reasoning...\n```json\n{json}\n```\n")
}

struct Fixture {
    _dir: tempfile::TempDir,
    layout: ProjectLayout,
    git: GitRepository,
    factory: Arc<ScriptedFactory>,
    service: Arc<OrchestratorService>,
}

async fn scaffolded_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let directory = ProjectDirectory::resolve(Some(dir.path()), None).unwrap();
    let layout = ProjectLayout::new(directory.project());
    let (git, _) = prepare_host_files(&directory, &layout).unwrap();
    let report = complete_scaffold(
        &directory,
        &layout,
        &git,
        &PendingScaffold::default(),
        ProjectType::SoftwareEngineeringProject,
        "0.1.0",
    )
    .unwrap();
    let factory = Arc::new(ScriptedFactory::new());
    let services = Arc::new(ProjectServices {
        directory,
        layout: layout.clone(),
        config: report.config,
        git: git.clone(),
        plans: PlanCatalog::new(&layout),
        renderer: Arc::new(NoopRenderer),
        factory: factory.clone(),
        clyean_version: "0.1.0".into(),
    });
    let service = Arc::new(OrchestratorService::new(
        ProjectState::Scaffolded(services),
        "0.1.0",
    ));
    Fixture {
        _dir: dir,
        layout,
        git,
        factory,
        service,
    }
}

/// Sends one request through the socket handler and returns the response and events.
async fn call(service: &Arc<OrchestratorService>, request: Value) -> (Value, Vec<Event>) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let service = service.clone();
    let task =
        tokio::spawn(async move { handle_connection(service, server_read, server_write).await });
    let (client_read, mut client_write) = tokio::io::split(client);
    client_write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    client_write.flush().await.unwrap();
    let mut lines = BufReader::new(client_read).lines();
    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let mut events = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let event: Event = serde_json::from_str(&line).unwrap();
        let closes = event.closes_connection();
        events.push(event);
        if closes {
            break;
        }
    }
    task.await.unwrap().unwrap();
    (response, events)
}

fn write_plan_sections(layout: &ProjectLayout, plan: &str, overview: &str, spec: &str, arch: &str) {
    let (name, version) = plan.split_once("/v").unwrap();
    let path = layout.plans_dir().join(name).join(format!("v{version}.md"));
    let text = format!("# Title (v{version})\n\n## Overview\n\n{overview}\n\n## Specification Changes\n\n{spec}\n\n## Implementation Architecture\n\n{arch}\n");
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn a_lease_stays_open_until_the_user_assistant_closes_it() {
    let fixture = scaffolded_fixture().await;
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let service = fixture.service.clone();
    let task =
        tokio::spawn(async move { handle_connection(service, server_read, server_write).await });
    let (client_read, mut client_write) = tokio::io::split(client);
    client_write
        .write_all(b"{\"id\":\"lease-1\",\"method\":\"session.lease\",\"params\":{}}\n")
        .await
        .unwrap();
    let mut lines = BufReader::new(client_read).lines();
    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(
        response,
        json!({"id": "lease-1", "result": {"type": "lease"}})
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !task.is_finished(),
        "the lease ended while its holder was alive"
    );
    client_write.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(lines.next_line().await.unwrap(), None);
}

#[tokio::test]
async fn planning_asks_the_user_then_authors_and_reviews_every_section() {
    let fixture = scaffolded_fixture().await;
    let layout = fixture.layout.clone();
    // The Director asks for information once, then proceeds; section authors write the
    // plan file as a side effect, simulated here by scripting the file writes alongside.
    fixture
        .factory
        .script(
            AgentId::SoftwareEngineeringDirector,
            [
                verdict(json!({"decision": "needs_information", "questions": ["Which identity provider?"]})),
                verdict(json!({"decision": "proceed"})),
                verdict(json!({"decision": "overview_written"})),
                verdict(json!({"decision": "misaligned", "issues": ["Mention the token lifetime"]})),
                verdict(json!({"decision": "aligned"})),
                verdict(json!({"decision": "aligned"})),
            ],
        )
        .await;
    fixture
        .factory
        .script(
            AgentId::Specifier,
            [
                verdict(json!({"decision": "completed", "summary": "spec v1"})),
                verdict(json!({"decision": "completed", "summary": "spec v2"})),
            ],
        )
        .await;
    fixture
        .factory
        .script(
            AgentId::SoftwareArchitect,
            [verdict(json!({"decision": "completed", "summary": "arch"}))],
        )
        .await;

    let (response, events) = call(
        &fixture.service,
        json!({"id": "r1", "method": "work.start", "params": {
            "prompt_type": "SOFTWARE_ENGINEERING_PROJECT_PLANNING",
            "prompt": "Add OAuth login",
            "original_prompt": "add login pls",
            "session_id": "ua-session"
        }}),
    )
    .await;
    assert_eq!(response["result"]["type"], "work_accepted");
    let work_id = response["result"]["work_id"].as_str().unwrap().to_string();
    let request = events.last().unwrap().clone();
    let Event::InformationRequested {
        request_id,
        questions,
        ..
    } = request
    else {
        panic!("expected an information request, got {request:?}");
    };
    assert_eq!(questions, vec!["Which identity provider?"]);

    // While the workflow waits, fill the plan so the section checks pass, then answer.
    let plan = format!(
        "{}/v1",
        fixture
            .layout
            .plans_dir()
            .read_dir()
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| !n.starts_with('.'))
            .unwrap()
    );
    write_plan_sections(
        &layout,
        &plan,
        "Login via OAuth.",
        "Spec details.",
        "Arch details.",
    );
    let (response, events) = call(
        &fixture.service,
        json!({"id": "r2", "method": "work.provide_information", "params": {"work_id": work_id, "request_id": request_id, "answers": ["Okta"]}}),
    )
    .await;
    assert_eq!(response["result"]["type"], "work_resumed");
    let last = events.last().unwrap();
    let Event::Completed {
        plan: completed_plan,
        artifacts,
        ..
    } = last
    else {
        panic!("expected completion, got {last:?}")
    };
    assert_eq!(completed_plan.as_deref(), Some(plan.as_str()));
    assert_eq!(artifacts, &vec![format!(".clyean/plans/{}.md", plan)]);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Progress { text, .. } if text.contains("misaligned"))));

    // The Specifier was revised once with the Director's issue, and answers reached the Director.
    let specifier_prompts = fixture.factory.prompts_to(AgentId::Specifier).await;
    assert_eq!(specifier_prompts.len(), 2);
    assert!(specifier_prompts[1].contains("Mention the token lifetime"));
    let director_prompts = fixture
        .factory
        .prompts_to(AgentId::SoftwareEngineeringDirector)
        .await;
    assert!(director_prompts[1].contains("A: Okta"));

    // Commits carry the agent trailers and the journal is complete.
    let head = fixture.git.head_sha().unwrap().unwrap();
    assert!(fixture
        .git
        .commit_message(&head)
        .unwrap()
        .contains("Clyean-Agent: software-engineering-director"));
    let (_, status_events) = call(
        &fixture.service,
        json!({"id": "r3", "method": "project.status"}),
    )
    .await;
    assert!(status_events.is_empty());
}

#[tokio::test]
async fn implementation_replans_after_a_blocking_issue_and_remediates_review_findings() {
    let fixture = scaffolded_fixture().await;
    let layout = fixture.layout.clone();
    let plans = PlanCatalog::new(&layout);
    let name = plans.create_plan("add search", "2026-09-21").unwrap();
    write_plan_sections(&layout, &format!("{name}/v1"), "Search.", "Spec.", "Arch.");
    let plan_dir = format!(".clyean/plans/{name}");
    fixture
        .git
        .stage_and_commit(
            &[Path::new(&plan_dir)],
            &AgentId::SoftwareEngineeringDirector.git_identity(),
            "plan v1\n\nClyean-Agent: software-engineering-director\n",
        )
        .unwrap();

    fixture
        .factory
        .script(
            AgentId::Specifier,
            [
                verdict(json!({"decision": "completed", "summary": "specs applied"})),
                verdict(json!({"decision": "completed", "summary": "spec v2"})),
                verdict(json!({"decision": "completed", "summary": "specs applied again"})),
            ],
        )
        .await;
    fixture.factory.script(AgentId::SoftwareArchitect, [
        verdict(json!({"decision": "completed", "summary": "diagrams"})),
        verdict(json!({"decision": "completed", "summary": "arch v2"})),
        verdict(json!({"decision": "completed", "summary": "diagrams again"})),
        verdict(json!({"decision": "issues", "issues": [{"severity": "major", "location": "search.rs", "description": "missing index"}]})),
        verdict(json!({"decision": "approved", "summary": "looks good"})),
    ]).await;
    fixture.factory.script(AgentId::Programmer, [
        verdict(json!({"decision": "blocked", "issue": "no database migrations exist", "suggested_resolution": "add a migration tool"})),
        verdict(json!({"decision": "ready_for_review", "summary": "implemented search"})),
        verdict(json!({"decision": "ready_for_review", "summary": "added the index"})),
    ]).await;
    fixture
        .factory
        .script(
            AgentId::SoftwareEngineeringDirector,
            [
                verdict(json!({"decision": "proceed"})),
                verdict(json!({"decision": "overview_written"})),
                verdict(json!({"decision": "aligned"})),
                verdict(json!({"decision": "aligned"})),
            ],
        )
        .await;

    let (response, events) = call(
        &fixture.service,
        json!({"id": "r1", "method": "work.start", "params": {
            "prompt_type": "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION",
            "prompt": "Implement search",
            "plan": name,
        }}),
    )
    .await;
    assert_eq!(response["result"]["type"], "work_accepted");
    let last = events.last().unwrap();
    let Event::Completed { plan, summary, .. } = last else {
        panic!("expected completion, got {last:?}")
    };
    assert_eq!(plan.as_deref(), Some(format!("{name}/v2").as_str()));
    assert!(summary.contains("added the index"));
    assert!(
        layout.plans_dir().join(&name).join("v2.md").is_file(),
        "a new plan version was created"
    );
    assert!(
        layout.plans_dir().join(&name).join("v1.md").is_file(),
        "the old version was kept"
    );

    let programmer_prompts = fixture.factory.prompts_to(AgentId::Programmer).await;
    assert_eq!(programmer_prompts.len(), 3);
    assert!(programmer_prompts[2].contains("missing index"));
    let director_prompts = fixture
        .factory
        .prompts_to(AgentId::SoftwareEngineeringDirector)
        .await;
    assert!(director_prompts[0].contains("no database migrations exist"));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Progress { text, .. } if text.contains("re-planning"))));
    assert!(events.iter().any(
        |e| matches!(e, Event::Progress { text, .. } if text.contains("remediation round 1"))
    ));
}

#[tokio::test]
async fn research_returns_the_directors_answer_and_journals_under_work() {
    let fixture = scaffolded_fixture().await;
    fixture
        .factory
        .script(
            AgentId::SoftwareEngineeringDirector,
            ["The build uses cargo.\n"],
        )
        .await;
    let (_, events) = call(
        &fixture.service,
        json!({"id": "r1", "method": "work.start", "params": {"prompt_type": "SOFTWARE_ENGINEERING_PROJECT_RESEARCH", "prompt": "How is it built?"}}),
    )
    .await;
    let Event::Completed { summary, .. } = events.last().unwrap() else {
        panic!()
    };
    assert_eq!(summary, "The build uses cargo.\n");
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::AgentOutput { text, .. } if text.contains("cargo"))));
    let journals: Vec<_> = std::fs::read_dir(fixture.layout.work_dir())
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(journals.len(), 1);
}

#[tokio::test]
async fn unscaffolded_projects_report_status_and_scaffold_through_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let directory = ProjectDirectory::resolve(Some(dir.path()), None).unwrap();
    let layout = ProjectLayout::new(directory.project());
    let (git, _) = prepare_host_files(&directory, &layout).unwrap();
    let factory = Arc::new(ScriptedFactory::new());
    factory
        .script(
            AgentId::Scaffolder,
            [verdict(
                json!({"decision": "completed", "summary": "documented the CLI"}),
            )],
        )
        .await;
    let service = Arc::new(OrchestratorService::new(
        ProjectState::Unscaffolded(Box::new(UnscaffoldedProject {
            directory,
            layout: layout.clone(),
            git: git.clone(),
            pending: PendingScaffold::default(),
            renderer: Arc::new(NoopRenderer),
            factory,
            clyean_version: "0.1.0".into(),
        })),
        "0.1.0",
    ));
    let (status, _) = call(&service, json!({"id": "s", "method": "project.status"})).await;
    assert_eq!(status["result"]["scaffolded"], false);
    assert_eq!(status["result"]["project_type"], Value::Null);

    let (response, events) = call(&service, json!({"id": "x", "method": "project.scaffold", "params": {"project_type": "SOFTWARE_ENGINEERING_PROJECT"}})).await;
    assert_eq!(
        response["result"]["type"], "work_accepted",
        "unexpected response: {response}"
    );
    let Event::Completed { summary, .. } = events.last().unwrap() else {
        panic!("{events:?}")
    };
    assert!(summary.contains("documented the CLI"));
    assert!(layout.is_scaffolded());
    let (status, _) = call(&service, json!({"id": "s2", "method": "project.status"})).await;
    assert_eq!(
        status["result"]["project_type"],
        "SOFTWARE_ENGINEERING_PROJECT"
    );
    let (again, _) = call(&service, json!({"id": "y", "method": "project.scaffold", "params": {"project_type": "MISCELLANEOUS_PROJECT"}})).await;
    assert_eq!(again["error"]["code"], "invalid_request");
    let (unknown, _) = call(&service, json!({"id": "z", "method": "nope"})).await;
    assert_eq!(unknown["error"]["code"], "unknown_method");
    let (missing, _) = call(
        &service,
        json!({"id": "m", "method": "work.resume", "params": {"work_id": "nothing"}}),
    )
    .await;
    assert_eq!(missing["error"]["code"], "work_not_found");
}

#[tokio::test]
async fn work_can_be_cancelled_while_waiting_for_information() {
    let fixture = scaffolded_fixture().await;
    fixture
        .factory
        .script(
            AgentId::SoftwareEngineeringDirector,
            [verdict(
                json!({"decision": "needs_information", "questions": ["q?"]}),
            )],
        )
        .await;
    let (response, events) = call(
        &fixture.service,
        json!({"id": "r1", "method": "work.start", "params": {"prompt_type": "SOFTWARE_ENGINEERING_PROJECT_PLANNING", "prompt": "x"}}),
    )
    .await;
    let work_id = response["result"]["work_id"].as_str().unwrap().to_string();
    assert!(matches!(
        events.last(),
        Some(Event::InformationRequested { .. })
    ));
    let (status, _) = call(
        &fixture.service,
        json!({"id": "s", "method": "project.status"}),
    )
    .await;
    assert_eq!(
        status["result"]["incomplete_work"][0]["status"],
        "awaiting_information"
    );
    assert_eq!(status["result"]["locked"], true);
    let (resumed, replay) = call(
        &fixture.service,
        json!({"id": "r2", "method": "work.resume", "params": {"work_id": work_id}}),
    )
    .await;
    assert_eq!(resumed["result"]["type"], "work_resumed");
    assert!(
        matches!(replay.first(), Some(Event::InformationRequested { .. })),
        "pending request is replayed"
    );
    let (cancelled, _) = call(
        &fixture.service,
        json!({"id": "r3", "method": "work.cancel", "params": {"work_id": work_id}}),
    )
    .await;
    assert_eq!(cancelled["result"]["type"], "work_cancelled");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (status, _) = call(
        &fixture.service,
        json!({"id": "s2", "method": "project.status"}),
    )
    .await;
    assert_eq!(
        status["result"]["incomplete_work"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "cancelled work is terminal"
    );
    assert_eq!(status["result"]["locked"], false);
}

#[test]
fn request_type_is_reexported() {
    let request: Request = serde_json::from_value(json!({"id": "1", "method": "ping"})).unwrap();
    assert_eq!(request.method, "ping");
    let _ = StreamedEvent::Failed {
        work_id: String::new(),
        seq: 0,
        code: String::new(),
        message: String::new(),
    };
}
