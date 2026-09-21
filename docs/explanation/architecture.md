# Architecture

Clyean is two programs and a contract between them.  On the host runs `clyean`, a Rust program that owns the project's scaffold, the Podman sandbox, and the orchestration of agents.  Inside the sandbox runs the contained harness, Clyean's fork of Oh-My-Pi, one process per agent.  The contract is the [sandbox layout](../reference/sandbox-contract.md) every agent sees and the [socket protocol](../reference/orchestrator-protocol.md) the User Assistant uses to reach the host.

## The host program

`clyean` is a Cargo workspace whose crates separate concerns and depend on each other in one direction only: project state (`clyean-project`), Git (`clyean-git`), the agent roster and profile projection (`clyean-agents`), PlantUML (`clyean-plantuml`), the Podman sandbox (`clyean-sandbox`), the harness RPC client (`clyean-harness`), orchestration (`clyean-orchestrator`), and the command line (`clyean`).  Nothing in the host program talks to a model.  Everything that requires judgment is delegated to an agent; everything that must be exact (layout, locking, commits, rendering, workflow order, bounded retries) is deterministic Rust.

## One root filesystem, one container per agent process

All agents of a project share one root filesystem, the directory `.clyean/container-root`, which Clyean populates by exporting the configured image inside Podman's user namespace and then provisions.  Every agent container is started with `podman run --init --rootfs <that directory>`, so PID 1 is Podman's init and the harness process is its only child.  A directory rather than an image layer has three consequences that the design wants: packages one agent installs are there for the next; the whole sandbox is a plain directory you can inspect, back up, or delete; and nothing an agent does can escape it, because the only writable paths outside the root are the workspace mount and the sanctioned sockets.

The workspace is mounted read-write at `/home/<user>/workspace/<name>`; additional host paths are mounted read-only under `/mnt`.  The path of `.clyean/container-root` inside the workspace mount is masked with an empty `tmpfs`, so agents never traverse the root filesystem through the workspace.  Under rootless Podman the container's root is your own user, which keeps ownership of every file an agent writes sane.

Each agent runs under its own harness profile (`OMP_PROFILE=<agent-id>`), which is how models, credentials, MCP servers, and sessions stay separate per agent without changing the harness's configuration model.  Clyean projects the tracked files under `.clyean/agents` into those profiles before every start, layering the project's settings overlay over each profile's own persisted state through the harness's `--config` flag.  Provider credentials reach the containers by passing through the host variables that match the credential patterns and, by default, by copying the User Assistant's credential store into each sub-agent's profile when it starts.

## The User Assistant and the orchestrator

The User Assistant is an interactive harness session in its own container; your terminal is attached to it.  It cannot start containers itself (the Podman socket is deliberately not mounted), so it delegates through tools provided by a Clyean-managed extension.  Those tools speak newline-delimited JSON over a Unix socket that the host creates and bind-mounts into the User Assistant container at `/run/clyean/orchestrator.sock`.  One request per connection; the host answers immediately and then streams progress until the work completes, fails, or needs information from the user.

The orchestrator runs inside the host `clyean` process while you are attached.  It takes the project lock, opens one harness session per sub-agent per unit of work (each in its own container, driven over the harness's RPC mode), sends one instruction per workflow step, reads the verdict from the last fenced `json` block of the reply, commits after each step, and relays the sub-agents' output back to the User Assistant.  When a step needs an answer from you, the orchestrator suspends the workflow with the sub-agent sessions still alive, the User Assistant asks you, and the answer flows back through the same socket.

## Sessions and context

The Clyean session is the User Assistant's harness session, which is why `--continue` and `--resume` work as they do in the harness.  Every other agent gets a fresh session for each unit of work and keeps it for the whole unit, including across information requests.  Resetting sub-agent context for every prompt is a deliberate defense against technical debt: an agent that carries the accumulated context of earlier changes drifts toward the shortcuts it took before, while an agent that starts from the specification, the architecture, and the plan reasons from the recorded state of the project.

## Durable execution

Every unit of work has a journal that records its phase, the questions and answers so far, the sub-agent session files, and the commits made after each step.  The journal is saved on every transition and, for plans, committed to Git with the plan.  A workflow that is interrupted resumes from its recorded phase, re-opening the recorded sessions, and a workflow that is waiting for information replays its questions.  Git is the record of the work itself: each step's output is a commit authored by the responsible agent with a `Clyean-Agent:` trailer.
