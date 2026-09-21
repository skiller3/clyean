# Sandbox contract

This page is the reference for how Clyean lays out an agent sandbox and what every agent process can rely on inside it.  It is the contract shared by the `clyean` host program, the harness extensions shipped into the sandbox, and the agent baseline instructions.

## Root filesystem

Every agent of a project runs in a Podman container whose root filesystem is the project's `.clyean/container-root` directory, passed to Podman as `--rootfs`.  The directory is populated from the image named in `.clyean/project.json` (`sandbox.image`, default `ubuntu:latest`) and then provisioned.  All agents of a project share this one root filesystem, so a package installed by one agent is visible to the next.

`podman run` is always invoked with `--init`, so PID 1 inside the container is Podman's init process and the agent's harness process is its sole direct child.

Provisioning writes a marker file, `/.clyean-sandbox.json`, recording the image reference, the image digest, the provisioning schema version, and the Clyean version that provisioned the root.  `clyean sandbox rebuild` discards and re-creates the whole root.

## Fixed paths inside the container

| Path | Contents | Writable by agents |
| --- | --- | --- |
| `/home/<user>` | `HOME` of every agent process.  `<user>` is the host user name (lower-cased, non-alphanumerics replaced by `-`). | yes |
| `/home/<user>/workspace/<workspace-name>` | Bind mount of the host workspace directory.  `<workspace-name>` is the base name of the host workspace directory. | yes (read-write mount) |
| `/home/<user>/.omp/profiles/<agent-id>/agent/` | The harness profile of one agent: `settings.json`, `AGENTS.md`, `extensions/`, and the harness's own state (sessions, caches). | yes |
| `/usr/local/bin/clyean` | The contained harness binary (the Clyean fork of Oh-My-Pi, Linux build). | no by convention |
| `/opt/plantuml/plantuml-mit-<version>.jar` | The pinned MIT-licensed PlantUML distribution. | no by convention |
| `/opt/plantuml/plantuml.jar` | Symbolic link to the pinned jar. | no by convention |
| `/mnt/<name>` | One read-only bind mount per entry in `sandbox.mounts` of `project.json`; `<name>` is the base name of the host path. | no (read-only mount) |
| `/run/clyean/orchestrator.sock` | Unix socket to the host orchestrator (User Assistant container only). | n/a |
| `/run/herdr/herdr.sock` | Bind mount of the host Herdr socket (User Assistant container only, only inside a Herdr pane). | n/a |
| `/usr/local/bin/herdr` | Read-only bind mount of the host `herdr` executable when it exists (User Assistant container only, only inside a Herdr pane). | no |

The project directory inside the container is the workspace mount joined with the project's path relative to the host workspace directory.  The path `.clyean/container-root` under the mounted project directory is masked with an empty `tmpfs` so that agents never see or traverse the root filesystem through the workspace mount.

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
| `CLYEAN_ORCHESTRATOR_SOCKET` | `/run/clyean/orchestrator.sock` | User Assistant |
| `HOME` | `/home/<user>` | all agents |
| `OMP_PROFILE` | The agent identifier, selecting the agent's harness profile. | all agents |
| `GIT_AUTHOR_NAME`, `GIT_COMMITTER_NAME` | `Clyean <Agent Display Name>` | all agents |
| `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_EMAIL` | `<agent-id>@agents.clyean.com` | all agents |
| `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID` | Copied from the host environment. | User Assistant, inside Herdr |
| `HERDR_SOCKET_PATH` | `/run/herdr/herdr.sock` | User Assistant, inside Herdr |
| `HERDR_BIN_PATH` | `/usr/local/bin/herdr` | User Assistant, inside Herdr, when `herdr` exists on the host |

Provider credentials reach the containers two ways.  First, host environment variables whose names match `*_API_KEY`, `*_API_TOKEN`, or `*_BASE_URL`, the AWS, Azure OpenAI, and Google Cloud credential variables, and the auth broker variables are passed into every agent container; `sandbox.passthroughEnv` in `project.json` adds exact names or `*` glob patterns.  Second, unless `sandbox.inheritCredentials` is `false`, every sub-agent starts with a copy of the User Assistant's credential store (`agent.db` and its journal files) taken from the User Assistant's profile, so a `/login` performed in the User Assistant applies to the other agents from their next start.  An agent whose profile should hold different credentials needs `inheritCredentials` off and its own login.

Translating a container path to its host equivalent, which the Herdr reporter needs for session files, follows two rules: a path under `CLYEAN_WORKSPACE_DIR` maps to the same relative path under `CLYEAN_HOST_WORKSPACE_DIR`; any other path maps to the same path under `CLYEAN_HOST_CONTAINER_ROOT`.  Paths under `/run` and `/mnt` have no host equivalent.

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
- The Clyean-managed extensions are written to `/home/<user>/.omp/profiles/user-assistant/agent/extensions/`.  Each carries a `CLYEAN_EXTENSION_VERSION` marker and is overwritten whenever the marker is behind the running `clyean` version.

## Git identity of commits

Every commit made by Clyean, whether by deterministic logic on the host or by an agent inside a container, uses the author identity shown in the agent table and ends its message with the trailer `Clyean-Agent: <agent-id>`.  Commits made by host logic on behalf of the orchestrator use the identity of the Software Engineering Director.

## Sanctioned routes out of the sandbox

Two features deliberately let an agent affect state outside the workspace mount and are documented as such:

1. The Herdr socket mounted into the User Assistant container grants the full Herdr socket API, including workspace, tab, and pane mutation and control of panes that do not belong to the project.
2. Remote repository access by the Product Director agent, once that agent is specified.

No other mount is writable, and no other socket is mounted.
