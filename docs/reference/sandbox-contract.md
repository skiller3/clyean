# Sandbox contract

This page is the reference for how Clyean lays out an agent sandbox and what every agent process can rely on inside it.  It is the contract shared by the `clyean` host program, the harness extensions shipped into the sandbox, and the agent baseline instructions.

## Root filesystem

Every agent of a project runs in a Podman container whose root filesystem is the project's `.clyean/container-root` directory, passed to Podman as `--rootfs`.  The directory is populated from the image named in `.clyean/project.json` (`sandbox.image`, default `ubuntu:latest`) and then provisioned.  All agents of a project share this one root filesystem, so a package installed by one agent is visible to the next.

`podman run` is always invoked with `--init`, so PID 1 inside the container is Podman's init process and the agent's harness process is its sole direct child.  Every agent container runs with `--rm` and `--detach-keys=`: it is removed when it exits, and it cannot be detached from.

Provisioning writes a marker file, `/.clyean-sandbox.json`, recording the image reference, the image digest, the provisioning schema version, and the Clyean version that provisioned the root.  `clyean sandbox rebuild` discards and re-creates the whole root.

## User Assistant containers

Every invocation of `clyean` starts exactly one User Assistant, in a container of its own that ends when the invocation ends.  Several invocations of one project may run at once; each has its own container and session on the shared root filesystem.

| Property | Value |
| --- | --- |
| Name | `clyean-<project-id>-user-assistant-<launch-id>`, where `<launch-id>` is eight random hexadecimal characters chosen by the invocation. |
| Labels | `clyean.project=<project-id>`, `clyean.launch=<launch-id>`, `clyean.role=user-assistant` |
| Command | `/usr/local/libexec/clyean/clyean-bridge await --ready-file /run/clyean/bridge.ready -- /usr/local/bin/clyean <harness arguments>` |
| Terminal | Allocated for interactive launches, not for print mode. |

The container reaches its `clyean` process only through the bridge: once the container runs, `clyean` opens one `podman exec --interactive` session running `clyean-bridge bridge`, and multiplexes every connection to the bridge's sockets over the session's standard input and output.  The bridge serves two channels:

- `orchestrator`, at `/run/clyean/orchestrator.sock`, carrying the orchestrator protocol and nothing else.
- `herdr`, at `/run/herdr/herdr.sock`, relaying the host Herdr socket unmodified, only when `clyean` runs inside a Herdr pane.

The container's command is a start gate: it waits until the bridge creates `/run/clyean/bridge.ready`, which it does once both sockets accept connections, and then replaces itself with the harness, so the harness never runs without its bridge and is still the only child of init.  If the bridge is not ready within 30 seconds, the gate exits with status 125 and the container ends before the harness starts.

The User Assistant's orchestration extension holds a lease: one `session.lease` connection to the orchestrator for the life of the harness process.  When the `clyean` process ends for any reason, the session's input closes, the bridge exits, the lease connection ends, and the extension shuts the harness down, after which Podman removes the container.  A harness that has stopped responding cannot act on the lease; `clyean sandbox prune` removes such containers (see [the command-line reference](cli.md)).

## Fixed paths inside the container

| Path | Contents | Writable by agents |
| --- | --- | --- |
| `/home/<user>` | `HOME` of every agent process.  `<user>` is the host user name (lower-cased, non-alphanumerics replaced by `-`). | yes |
| `/home/<user>/workspace/<workspace-name>` | Bind mount of the host workspace directory.  `<workspace-name>` is the base name of the host workspace directory. | yes (read-write mount) |
| `/home/<user>/.omp/profiles/<agent-id>/agent/` | The harness profile of the container's own agent: `settings.json`, `AGENTS.md`, `extensions/`, its login store (`agent.db`), and the harness's own state (sessions, caches).  No other agent's profile is visible. | yes |
| `/home/<user>/.omp/profiles/<agent-id>/agent/clyean-credentials.json` | The credential copies delivered to a sub-agent, readable by its owner only, from the sub-agent's launch until its credentials extension imports and deletes them. | yes |
| `/usr/local/bin/clyean` | The contained harness binary (the Clyean fork of Oh-My-Pi, Linux build). | no by convention |
| `/opt/plantuml/plantuml-mit-<version>.jar` | The pinned MIT-licensed PlantUML distribution. | no by convention |
| `/opt/plantuml/plantuml.jar` | Symbolic link to the pinned jar. | no by convention |
| `/mnt/<name>` | One read-only bind mount per entry in `sandbox.mounts` of `project.json`; `<name>` is the base name of the host path. | no (read-only mount) |
| `/usr/local/libexec/clyean/clyean-bridge` | Read-only bind mount of the host's static bridge executable (User Assistant containers only). | no (read-only mount) |
| `/run/clyean/orchestrator.sock` | The orchestrator channel of the bridge (User Assistant containers only). | n/a |
| `/run/clyean/bridge.ready` | Created by the bridge once its sockets accept connections (User Assistant containers only). | n/a |
| `/run/herdr/herdr.sock` | The Herdr channel of the bridge (User Assistant containers only, only inside a Herdr pane). | n/a |
| `/usr/local/bin/herdr` | Read-only bind mount of the host `herdr` executable when it exists (User Assistant containers only, only inside a Herdr pane). | no |

The project directory inside the container is the workspace mount joined with the project's path relative to the host workspace directory.  The path `.clyean/container-root` under the mounted project directory is masked with an empty `tmpfs` so that agents never see or traverse the root filesystem through the workspace mount.

Some directories get a private, empty `tmpfs` in each container, so that no container reaches another's sockets or login store through the shared root filesystem:

| Path | Containers |
| --- | --- |
| `/run/clyean`, `/run/herdr` | User Assistant |
| `/home/<user>/.omp/run` and `/home/<user>/.omp/profiles/<agent-id>/run`, where the harness keeps the sockets of its daemons | User Assistant and sub-agents |
| `/home/<user>/.omp/profiles`, with the container's own agent's profile directory bind-mounted back from the root filesystem at `/home/<user>/.omp/profiles/<agent-id>` | User Assistant and sub-agents; maintenance containers get the empty directory only |
| `/home/<user>/.omp/agent`, which no profile reads | every container |

Files that every profile reads, such as `~/.env` in the sandbox home, stay shared.  Clyean never writes one, and secrets do not belong there.

Agent processes run as UID 0 inside the container.  Under rootless Podman that UID is the host user, so every file an agent creates in the workspace is owned by the host user.

## Environment of an agent process

| Variable | Value | Present in |
| --- | --- | --- |
| `CLYEAN_AGENT` | The agent identifier (see below). | all agents |
| `CLYEAN_VERSION` | Version of the `clyean` host program that launched the container. | all agents |
| `CLYEAN_PROJECT_DIR` | Absolute container path of the project directory. | all agents |
| `CLYEAN_WORKSPACE_DIR` | Absolute container path of the workspace mount. | all agents |
| `CLYEAN_HOST_WORKSPACE_DIR` | Absolute host path of the workspace directory. | all agents |
| `CLYEAN_HOST_CONTAINER_ROOT` | Absolute host path of `.clyean/container-root`. | all agents |
| `CLYEAN_WORK_ID` | Identifier of the unit of work the agent was started for. | sub-agents |
| `CLYEAN_CREDENTIALS_BUNDLE` | Path of the agent's credential bundle file (see "Credentials"). | sub-agents |
| `CLYEAN_ORCHESTRATOR_SOCKET` | `/run/clyean/orchestrator.sock` | User Assistant |
| `CLYEAN_ORCHESTRATOR_LEASE` | `1`: hold the orchestrator lease and shut down when it ends. | User Assistant |
| `HOME` | `/home/<user>` | all agents |
| `OMP_PROFILE` | The agent identifier, selecting the agent's harness profile. | all agents |
| `GIT_AUTHOR_NAME`, `GIT_COMMITTER_NAME` | `Clyean <Agent Display Name>` | all agents |
| `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_EMAIL` | `<agent-id>@agents.clyean.com` | all agents |
| `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID` | Copied from the host environment. | User Assistant, inside Herdr |
| `HERDR_SOCKET_PATH` | `/run/herdr/herdr.sock` | User Assistant, inside Herdr |
| `HERDR_BIN_PATH` | `/usr/local/bin/herdr` | User Assistant, inside Herdr, when `herdr` exists on the host |

Host variables that carry credentials or configuration are selected per agent; see "Credentials".

Translating a container path to its host equivalent, which the Herdr reporter needs for session files, follows two rules: a path under `CLYEAN_WORKSPACE_DIR` maps to the same relative path under `CLYEAN_HOST_WORKSPACE_DIR`; any other path maps to the same path under `CLYEAN_HOST_CONTAINER_ROOT`.  Paths under `/run` and `/mnt` have no host equivalent.

## Credentials

Every agent has its own login store, the harness's credential store in its own profile.  Every sign-in is made in the User Assistant's store; the other agents hold copies.

When a sub-agent starts, the orchestrator reads the agent's configuration from `.clyean/agents` on the host: the model patterns in the model roles and fallback chains of its settings overlay, and the remote servers of its MCP seed.  It then asks the User Assistant, over the User Assistant's lease connection (see [the orchestrator protocol](orchestrator-protocol.md#credential-requests-on-the-lease)), for three things:

1. `credentials.resolve`: the provider of each model pattern, resolved with the harness's own resolver, and the User Assistant's current model.
2. `credentials.variables`: the environment variables the harness reads for each of those providers.
3. `credentials.copies`: copies of the User Assistant's credentials for those providers and servers.  The User Assistant first refreshes any sign-in that expires within fifteen minutes.  Copies carry an empty refresh token and no client secret, and each MCP credential is keyed to the receiving agent's profile (`mcp_oauth:profile:<agent-id>:<server address>`).

The model the sub-agent runs with is the first pattern of its default model role that names a model the User Assistant or the host environment can supply, or, when its overlay names no default model, the User Assistant's current model.  It is passed to the harness with `--model`.  When no candidate can be supplied, the sub-agent does not start and the unit of work fails with a message naming the sign-in to add.

The orchestrator writes the copies to the sub-agent's bundle file with mode `0600` and starts the container with `CLYEAN_CREDENTIALS_BUNDLE` set.  The agent's credentials extension, which loads before the harness decides which models are available, deletes every credential its store holds, imports the copies, and deletes the file.

A copy is renewed rather than refreshed.  The credentials extension registers each model provider it holds an OAuth copy for, so when the harness would refresh that copy, it instead sends an extension UI request with the title `clyean:credentials` and a placeholder naming what it needs (`{"providers": [...]}` or `{"mcp_servers": [...]}`).  It sends the same request ten minutes before an MCP copy expires.  The orchestrator answers only for the providers and servers computed when the agent started, with fresh copies from the User Assistant, and ignores other UI requests.

Host variables reach agents as follows:

| Agent | Host variables it receives |
| --- | --- |
| User Assistant | Names matching `*_API_KEY`, `*_API_TOKEN`, or `*_BASE_URL`, the AWS, Azure OpenAI, and Google Cloud credential variables, and every `sandbox.passthroughEnv` entry written as a string |
| Sub-agent | The variables the harness reads for its models' providers, and every `sandbox.passthroughEnv` entry written as an object that lists the agent |
| Maintenance container | None |

The harness's auth broker variables never pass through on their own.  `clyean scaffold --project-type` starts the Scaffolder without a User Assistant; it then receives the User Assistant's host variables and an empty bundle, which clears copies from earlier runs.

## Agent identifiers

| Agent | `CLYEAN_AGENT` and profile name | Instruction file | Display name used in Git identity |
| --- | --- | --- | --- |
| User Assistant | `user-assistant` | `AGENTS__USER_ASSISTANT.md` | `Clyean User Assistant` |
| Scaffolder | `scaffolder` | `AGENTS__SCAFFOLDER.md` | `Clyean Scaffolder` |
| Software Engineering Director | `software-engineering-director` | `AGENTS__SOFTWARE_ENGINEERING_DIRECTOR.md` | `Clyean Software Engineering Director` |
| Specifier | `specifier` | `AGENTS__SPECIFIER.md` | `Clyean Specifier` |
| Software Architect | `software-architect` | `AGENTS__SOFTWARE_ARCHITECT.md` | `Clyean Software Architect` |
| Programmer | `programmer` | `AGENTS__PROGRAMMER.md` | `Clyean Programmer` |

Agents listed in `AGENT_SPECS.md` as placeholders are known to the roster but have no instruction file, profile, or container until they are specified.

## Harness profile projection

Before an agent container starts, the host projects the agent's configuration into its profile directory:

- `.clyean/agents/AGENTS__<NAME>.md` followed by `.clyean/agents/AGENTS__<NAME>.local.md` (when present) becomes `/home/<user>/.omp/profiles/<agent-id>/agent/AGENTS.md`.
- `.clyean/agents/<NAME>.omp.json` deep-merged with `.clyean/agents/<NAME>.omp.local.json` (when present) becomes `/home/<user>/.omp/profiles/<agent-id>/agent/settings.json`.  The file uses the harness's own settings schema, so MCP servers, models, and tool settings are scoped to the agent that owns the file.
- The Clyean-managed extensions are written to the profile's `extensions/` directory: the Herdr reporter and the orchestration extension for the User Assistant, and the credentials extension for every other agent.  Each carries a `CLYEAN_EXTENSION_VERSION` marker and is overwritten whenever the marker is behind the running `clyean` version.

## Git identity of commits

Every commit made by Clyean, whether by deterministic logic on the host or by an agent inside a container, uses the author identity shown in the agent table and ends its message with the trailer `Clyean-Agent: <agent-id>`.  Commits made by host logic on behalf of the orchestrator use the identity of the Software Engineering Director.

## Sanctioned routes out of the sandbox

Two features deliberately let an agent affect state outside the workspace mount and are documented as such:

1. The Herdr channel the bridge relays into the User Assistant container grants the full Herdr socket API, including workspace, tab, and pane mutation and control of panes that do not belong to the project.
2. Remote repository access by the Software Engineering Director agent, once it is implemented.

No other mount is writable, and no host socket is mounted: the bridge's channels are the only connections from a container to its `clyean` process.
