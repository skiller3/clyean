// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! User Assistant containers under real Podman: the bridge over a `podman exec` session,
//! the container ending with its bridge, private sockets over one shared root filesystem,
//! and the orphan backstop.  The container's "harness" here is `clyean-bridge connect`,
//! which, like the harness's lease, ends when the orchestrator connection ends.
//!
//! These tests run only when `CLYEAN_PODMAN_TESTS=1` and `CLYEAN_BRIDGE_BINARY` names a
//! static Linux build of `clyean-bridge`.  `CLYEAN_TEST_IMAGE` overrides the small image
//! the root filesystem is populated from.

#![cfg(unix)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use clyean_agents::AgentId;
use clyean_bridge::host::{serve, ConnectFuture, Connector, LocalStream};
use clyean_project::{LaunchId, ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout};
use clyean_project::{ProjectType, SandboxConfig};
use clyean_sandbox::orphans::{has_running_bridge, lists_a_bridge, prune_matching};
use clyean_sandbox::rootfs::{populate_from_image, remove};
use clyean_sandbox::{AgentContainerSpec, ContainerUser, LaunchContext, LaunchRole, Podman};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};

const DEFAULT_IMAGE: &str = "docker.io/library/alpine:3.22";
const TEST_LABEL: &str = "clyean.test";

/// The container's command after the start gate: a client of the orchestrator socket.
const FAKE_HARNESS: &str =
    "#!/bin/sh\nexec /usr/local/libexec/clyean/clyean-bridge connect /run/clyean/orchestrator.sock\n";

struct Fixture {
    podman: Podman,
    context: LaunchContext,
    bridge_binary: PathBuf,
    label: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Option<Self> {
        if std::env::var("CLYEAN_PODMAN_TESTS").as_deref() != Ok("1") {
            eprintln!("skipping: set CLYEAN_PODMAN_TESTS=1 to run the Podman tests");
            return None;
        }
        let Some(bridge_binary) = std::env::var_os("CLYEAN_BRIDGE_BINARY").map(PathBuf::from)
        else {
            eprintln!("skipping: set CLYEAN_BRIDGE_BINARY to a static build of clyean-bridge");
            return None;
        };
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let project = workspace.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let directory = ProjectDirectory::resolve(Some(&project), Some(&workspace)).unwrap();
        let layout = ProjectLayout::new(directory.project());
        let podman = Podman::default();
        let image = std::env::var("CLYEAN_TEST_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.into());
        let root = layout.container_root_dir();
        populate_from_image(&podman, &image, &root).unwrap();
        install_fake_harness(&root);
        let config = ProjectConfig::new(
            "0.1.0",
            ProjectType::SoftwareEngineeringProject,
            directory.workspace(),
            false,
            SandboxConfig::with_image(image),
        );
        let context = LaunchContext {
            podman: podman.clone(),
            project_id: ProjectId::of(directory.project()),
            directory,
            layout,
            config,
            user: ContainerUser::from_host_user_name("clyean-test"),
            clyean_version: "0.1.0".into(),
            herdr: None,
        };
        Some(Self {
            podman,
            context,
            bridge_binary,
            label: format!("{TEST_LABEL}={}", LaunchId::generate()),
            _dir: dir,
        })
    }

    /// Starts a User Assistant container of this project and waits until it runs.
    async fn launch(&self) -> Launched {
        let role = LaunchRole::UserAssistant {
            launch_id: LaunchId::generate(),
            bridge_binary: self.bridge_binary.clone(),
        };
        let mut spec = self
            .context
            .agent_container_spec(AgentId::UserAssistant, role, Vec::new());
        spec.tty = false;
        let (key, value) = self.label.split_once('=').unwrap();
        spec.labels.push((key.to_string(), value.to_string()));
        let mut container = Command::new(self.podman.binary())
            .args(spec.run_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        wait_until_running(&self.podman, &spec.name).await;
        let input = container.stdin.take().unwrap();
        let output = BufReader::new(container.stdout.take().unwrap()).lines();
        Launched {
            spec,
            container,
            input,
            output,
        }
    }

    /// Opens the bridge of `launched`, answering orchestrator lines with `tag:<line>`.
    async fn open_bridge(&self, launched: &Launched, tag: &str) -> Child {
        let mut session = Command::new(self.podman.binary())
            .args(self.context.bridge_exec_args(&launched.spec.name))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (introduced, version) = tokio::sync::oneshot::channel();
        tokio::spawn(serve(
            session.stdout.take().unwrap(),
            session.stdin.take().unwrap(),
            Arc::new(TaggingConnector(tag.to_string())),
            introduced,
        ));
        let version = tokio::time::timeout(Duration::from_secs(30), version)
            .await
            .expect("the bridge introduced itself in time")
            .unwrap();
        assert_eq!(version, clyean_bridge::VERSION);
        session
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(ids) = self.podman.output([
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("label={}", self.label),
        ]) {
            for id in ids.split_whitespace() {
                let _ = self.podman.output(["rm", "--force", "--ignore", id]);
            }
        }
        let _ = remove(&self.podman, &self.context.layout.container_root_dir());
    }
}

struct Launched {
    spec: AgentContainerSpec,
    container: Child,
    input: tokio::process::ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

impl Launched {
    async fn exchange(&mut self, line: &str) -> String {
        self.input
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        self.input.flush().await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), self.output.next_line())
            .await
            .expect("a reply in time")
            .unwrap()
            .expect("a reply line")
    }
}

/// Answers each line on the orchestrator channel with `tag:<line>`.
struct TaggingConnector(String);

impl Connector for TaggingConnector {
    fn connect<'a>(&'a self, channel: &'a str) -> ConnectFuture<'a> {
        Box::pin(async move {
            if channel != clyean_bridge::ORCHESTRATOR_CHANNEL {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such channel"));
            }
            let (client, server) = tokio::io::duplex(4096);
            let tag = self.0.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if writer
                        .write_all(format!("{tag}:{line}\n").as_bytes())
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
            Ok(Box::new(client) as Box<dyn LocalStream>)
        })
    }
}

fn install_fake_harness(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let harness = root.join("usr/local/bin/clyean");
    std::fs::create_dir_all(harness.parent().unwrap()).unwrap();
    std::fs::write(&harness, FAKE_HARNESS).unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn wait_until_running(podman: &Podman, name: &str) {
    for _ in 0..600 {
        let state = podman.output([
            "container",
            "inspect",
            "--format",
            "{{.State.Status}}",
            name,
        ]);
        if matches!(state.as_deref().map(str::trim), Ok("running")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("container {name} never ran");
}

async fn wait_until_gone(podman: &Podman, name: &str) {
    for _ in 0..200 {
        if !podman.container_exists(name).unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("container {name} was not removed");
}

#[tokio::test]
async fn a_user_assistant_reaches_the_host_through_its_bridge_and_ends_with_it() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let mut launched = fixture.launch().await;
    let mut bridge = fixture.open_bridge(&launched, "host").await;
    assert_eq!(launched.exchange("ping").await, "host:ping");
    assert_eq!(launched.exchange("again").await, "host:again");

    let listing = fixture
        .podman
        .output(["top", &launched.spec.name, "args"])
        .unwrap();
    assert!(
        lists_a_bridge(&listing),
        "podman top shows the bridge:\n{listing}"
    );

    bridge.start_kill().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(30), launched.container.wait())
        .await
        .expect("the container ended once its bridge was gone")
        .unwrap();
    assert!(status.success(), "the lease ended cleanly: {status}");
    wait_until_gone(&fixture.podman, &launched.spec.name).await;
}

#[tokio::test]
async fn launches_share_the_root_filesystem_but_not_their_sockets() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let mut first = fixture.launch().await;
    let mut second = fixture.launch().await;
    assert_ne!(first.spec.name, second.spec.name);
    let _first_bridge = fixture.open_bridge(&first, "first").await;
    let _second_bridge = fixture.open_bridge(&second, "second").await;
    assert_eq!(first.exchange("hello").await, "first:hello");
    assert_eq!(second.exchange("hello").await, "second:hello");
}

#[tokio::test]
async fn prune_removes_a_user_assistant_without_a_bridge_and_keeps_one_with_a_bridge() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let bridged = fixture.launch().await;
    let _bridge = fixture.open_bridge(&bridged, "kept").await;
    let unbridged = fixture.launch().await;
    assert!(has_running_bridge(&fixture.podman, &bridged.spec.name).unwrap());
    assert!(!has_running_bridge(&fixture.podman, &unbridged.spec.name).unwrap());

    let removed = prune_matching(
        &fixture.podman,
        std::slice::from_ref(&fixture.label),
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(removed, vec![unbridged.spec.name.clone()]);
    wait_until_gone(&fixture.podman, &unbridged.spec.name).await;
    assert!(fixture
        .podman
        .container_is_running(&bridged.spec.name)
        .unwrap());
}
