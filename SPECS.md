# Basic User Experience
From a UX perspective, `clyean` is primarily a CLI program that launches a terminal UI (TUI) in which the user performs agentic programming (a.k.a vibe coding) and other LLM-based tasks (e.g. research, modeling, authoring).  The CLI/TUI should behave identically to Oh-My-Pi (`https://omp.sh/`; `https://github.com/can1357/oh-my-pi`) with the following modifications:
  - Titles, headers, and text should be rebranded from "omp" and "Oh-My-Pi" to "clyean" and "Clyean".  However, the TUI welcome page should prominently display the text "Clyean uses the fabulous Oh-My-Pi (https://omp.sh/) and Pi (https://pi.dev/) projects!"
  - CLI help text and messages should be rebranded from "omp" and "Oh-My-Pi" to "clyean" and "Clyean".
  - The "Pi" symbol logo should be replaced with a "Clyean" logo (and iconography) you invent.  The project's logo should be reminiscent of a bar of soap.
  - `omp` commandline arguments/options that are non-coherent given Clyean's goals or complicated to implement due to its architecture should be eliminated.
  - `omp` TUI configuration, options, commands (including slash commands), and overall functionality that is non-coherent given the Clyean's goals or complicated to implement given its architecture should be eliminated.
  - MCP (`/mcp`) commands and server connections should be supported with the enhancement of generally being scoped to particular agents (it shouldn't be assumed that all agents that coordinate to perform a unit of work should have access to the same MCP servers).
  - Model (`/model`) and switch (`/switch`) commands need to be adjusted to ensure model connections, authentication mechanisms, and usage parameters are scoped to specific agents.

IMPORTANT: When the user passes prompts to the Clyean CLI/TUI, they should be directly interacting with the "Chief-of-Staff" agent, which is itself one of several agents specified in this file's "Agents" section.

# Installation
Users are expected to install the latest release (or any specific release via a passed release version argument) via one of two scripts:
- `install.sh` - Bourne script expected to be used for installation within Linux and MacOS environments, often via a command like `curl -fsSL https://clyean.com/install.sh | sh`.
- `install.ps1` - PowerShell script expected to be used within Windows environments (outside of WSL), often via a command like `irm https://clyean.com/install.ps1 | iex`.

The preceding scripts are expected to install any necessary required dependencies (e.g. Podman), and broadly comply with software installation norms for their respective OS environments (for example, the `clyean` executable should be installed by default within `/home/<user>/.local/bin/` within Ubuntu Linux environments).  When in doubt about correct design and behavior of these scripts, please mirror the design and behavior of `omp`'s installation scripts at `https://omp.sh/install` and `https://omp.sh/install.ps1`.

No `clyean.com` website is currently launched; please just author the two install scripts at the top level of the `clyean` repository for now.

# Projects & Scaffolding
Unless otherwise specified via arguments in a fashion that mirrors `omp`'s behavior, the `clyean` executable should assume the current working directory of its process represents a "project" for which it is going to be used to perform work.  If a `.clyean-project.json` file exists within the project directory, Clyean should consider the project to be scaffolded; if the file doesn't exist, then `clyean` should immediately scaffold the project by creating the following resources in the project's top-level directory:

- `.clyean-project.json` JSON file - Contains the overall top-level configuration of the Clyean project.  Among other information, the file must contain (i) the version of `clyean` that initially generated the scaffolding, (ii) the UTC timestamp at which the scaffolding was initially generated, (iii) the initial Podman/Docker image used for the project's agent sandboxes, (iv) the workspace path to be used in conjunction with the project, (v) any additional mounts and podman run arguments that should be used for running agents on behalf of the project.  Unless specified by the user, the initial image used for a project's agent sandboxes should be `ubuntu:latest`.
- `.clyean-agents` directory - Contains base instructions (per typical `AGENTS.md` files) and omp-compatible configurations of the various agents comprising `clyean` as a system.  The base instruction files are expected to adhere to the pattern `AGENTS__<AGENT_NAME>.md` (e.g. `AGENTS__CHIEF_OF_STAFF.md`).
- `.clyean-architecture` directory - Contains up-to-date UML architecture diagrams spanning all 14 official UML diagram types for the project.  The UML diagrams should be represented both as (i) Mermaid (*.mermaid) files and (ii) PDF (*.pdf) files generated directly from the Mermaid files.
- `.clyean-specs.md` markdown file - Contains up-to-date specifications (i.e. requirements) for the project.
- `.clyean-container-root` directory - Contains file materials to be mounted at the root of Podman containers used to run Clyean agents.

All of the preceding `.clyean-*` scaffold materials are intended to be modifiable by the user and Clyean itself — and with the exception of the `.clyean-container-root` content — subjected to version control locally and within remote project repositories.  However, Clyean should also support user-provided local-only (i.e. not pushed to remote repository) (i) enhancements of agent baseline instructions, (ii) overrides of agent configurations, and (iii) overrides of Clyean project configuration.  These local-only enhancements and overrides should be implemented by mirorring Claude Code semantics for `*.local.<ext>` files.  

# Sandboxing, Workspaces & Mounts
Clyean must sandbox all agent activity via Podman (https://podman.io/) containers, and users are expected to be aware that Podman-based sandboxing is occurring (do not try to abstract them aware from this fact).  To be architecturally specific, any Clyean agent should always run as the sole direct child process of an `init` root process (PID 1) within a Podman container.

All agents used within a "project" should share the same container image and file system.  The entire agent container file system should be mounted from the project's `.clyean-container-root` directory.

Clyean should also contain the formal user-exposed notion of a "workspace" directory, and the workspace directory must either be the project directory itself or be an ancestor directory of the project directory.  By default, the workspace directory should be mounted into the agent container file tree with read-write access at `/home/<user>/workspace/<host-workspace-directory-name>`.

The user should also be able to configure any number (up to practical limits imposed by Linux and Podman) of mounts of local OS file system content into the Podman guest OS file system.  The mount points inside the container should reside in `/mnt/`, follow the naming convention `/mnt/<host-directory-or-file-name>`, and be read-only.

Clyean's sandboxing behavior should be enforced as follows:
- The Podman-based architecture should deterministaclly ensure agents NEVER modify file content outside of the specified workspace.
- By default, all agents should be instructed to never modify file system content outside of the project directory unless directly and explicitly instructed to do so.
- With the exception of the "Product Director" agent being capable of pushing changes to remote repositories and issuing pull requests, all agents should be instructed by default to never modify the state of any connected system (via MCP, API, UI, or otherwise) unless directly and explicitly instructed to do so. 
- As a further exception, the "Chief of Staff" agent is granted the full Herdr socket API when Clyean runs inside a Herdr pane, which lets it mutate the user's terminal workspace outside the project.  See the "Herdr Compatibility" section for the required socket mount and the capabilities it grants.

# Oh-My-Pi (omp) Architectural Relationship & Usage

Clyean *contains* a fork of `omp` and sits above it: Clyean's own code is an orchestration layer that coordinates communication between multiple agents, each of which runs independently on the contained harness.  The rebranding, option-pruning, and per-agent scoping requirements in the "Basic User Experience" section describe changes to the contained harness; the user is expected to interact with a "Chief of Staff" agent via `podman exec -it`.

The `omp` fork is vendored into this repository at `vendor/omp` as a `git subtree`, tracked against the `upstream` remote (`https://github.com/can1357/oh-my-pi`, push disabled).  Upstream changes are taken with `git subtree pull --prefix=vendor/omp upstream main`.  Upstream tags are deliberately not fetched so they cannot collide with Clyean's own semantic version tags.  Modifications to the harness are made in place under `vendor/omp` and should be kept as narrow and as well isolated as practical, since every additional point of divergence is a conflict to resolve on each upstream pull.

The contained harness deliberately keeps `omp`'s configuration discovery model rather than rebranding it.  Retaining the upstream paths and environment variables keeps the divergence from upstream small and should help preserve compatibility with the config files `omp` already imports from other tools to the maximum extent practical.  Specifically:
  - The user-level config root remains `~/.omp` (with the agent directory at `~/.omp/agent` and named profiles at `~/.omp/profiles/<name>`).
  - Project-level configuration remains the `.omp/` directory discovered by walking up from the working directory.
  - Environment variables retain their `OMP_*` and `PI_*` prefixes (for example `OMP_PROFILE`, `PI_CONFIG_DIR`, `PI_CODING_AGENT_DIR`).
  - Package and module identifiers inherited from upstream (for example `@oh-my-pi/*` imports) are not renamed.

These paths and variables are an explicit exception to the rebranding requirements in the "Basic User Experience" section.  Rebranding applies to what the user reads (TUI text, logos, CLI help and messages), not necessarily to where configuration lives on disk or how it is named in the environment.  Where the harness needs configuration that has no upstream equivalent, it is added within the existing `~/.omp` and `.omp/` roots rather than in a new location.

# Agents

# Herdr Compatibility

Herdr (`https://herdr.dev/`) is a terminal workspace manager for coding agents.  It keeps panes alive across disconnects, renders per-agent status (idle, working, blocked) in a sidebar, and exposes a socket API that lets an agent split panes, prompt other agents, and wait until a peer is genuinely blocked.  Upstream `omp` is one of the few agents Herdr grants full lifecycle authority, and Clyean must reach parity with that behavior.

Herdr has no built-in knowledge of Clyean, and acquiring it would require a new Herdr binary release, because foreground process detection, agent labels, bundled screen-detection manifests, and `herdr integration install <agent>` are all compiled into the Herdr executable.  Every requirement in this section must therefore be satisfied by Clyean alone, against Herdr's published socket API and the extension surface Clyean already inherits from the contained harness.  Clyean must not depend on any change to Herdr.

## Detection and inert behavior outside Herdr

- Clyean must behave identically to its non-Herdr behavior when it is not running inside a Herdr pane.  Every requirement below is inert in that case.
- Treat `HERDR_ENV=1`, or the presence of any of `HERDR_PANE_ID`, `HERDR_TAB_ID`, or `HERDR_WORKSPACE_ID`, as the signal that Clyean is running inside a Herdr pane.  The identity variables are the fallback because `HERDR_ENV` does not survive every environment-sanitizing launcher.
- Never treat the client-side variables (`HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_SESSION`, `HERDR_CONFIG_PATH`, `HERDR_CLIENT_SOCKET_PATH`) as a detection signal, since they can legitimately be set outside a Herdr pane.  This mirrors the contained harness's own `isInsideHerdr()` logic.
- Perform no reporting at all unless `HERDR_ENV=1` and both `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` are set.

## Preserved harness integration surface

The following pieces of the contained harness are the integration contract that Herdr compatibility depends on.  They are an explicit exception to the option-pruning allowance in the "Basic User Experience" section, must survive rebranding, and must be re-verified on every `git subtree pull` from upstream.

- Extension discovery under the user-level `~/.omp/agent/extensions` directory and the project-level `.omp/` directory.
- The extension event names `session_start`, `session_switch`, `agent_start`, `agent_end`, `tool_approval_requested`, `tool_approval_resolved`, `tool_execution_start`, and `tool_execution_end`, along with their payload fields.
- The custom event bus (`pi.events`) and its `herdr:blocked` event.
- The context accessors `ctx.hasUI`, `ctx.isIdle()`, `ctx.sessionManager.getSessionFile()`, and `ctx.sessionManager.getSessionId()`.
- The ability for extension code to open a Unix domain socket (`node:net`) from within the agent process.
- Upstream's Herdr-aware terminal multiplexer detection in the TUI layer, so synchronized output, resize handling, and graphics capability gating behave under Clyean exactly as they do under `omp`.

## Clyean ships its own state reporter

- Clyean must scaffold and maintain its own Herdr state-reporting extension.  It must not require `herdr integration install clyean` (which does not exist), must not fail or degrade because that command is unavailable, and must not depend on Herdr writing anything into the host's `~/.omp` directory.
- The extension is delivered through `.clyean-container-root` so that it is present at `~/.omp/agent/extensions/` inside the Chief of Staff agent's container.
- The extension is Clyean-managed rather than user-managed.  It must carry an integration version marker, be replaced when Clyean upgrades it, and state in a header comment that user customizations belong in sibling files rather than in edits to it.
- Reports must identify themselves with the source `custom:clyean` and the agent label `clyean`.
- Exactly one reporter may claim a pane.  Herdr's own `herdr:omp` integration file must never be shipped into or installed within a Clyean container.

## Reported agent state

- Report state with the `pane.report_agent` method, using only the `idle`, `working`, and `blocked` states.
- Report `working` from the start of an agent turn until the turn ends.
- Report `blocked` while any tool approval is outstanding and while the `ask` tool is awaiting an answer.  Overlapping blocks must be reference counted so that resolving one of several does not prematurely clear the state.  The reported message should be the approval reason or the first question text, so the sidebar explains what the agent is waiting on.
- Report `idle` otherwise, debounced (default 250 ms, overridable by environment variable) so that brief gaps between turns do not flicker the sidebar.
- Hold `working` through retryable provider failures (overload, rate limiting, 5xx responses, and transport resets) for a grace period (default 2500 ms, overridable by environment variable) before falling through to `blocked`, so that automatic retries are not misreported as idle.
- Stamp every report with a monotonically increasing sequence number, serialize reports through a single queue, and ensure a duplicate or late turn-end event cannot publish a false `idle`.
- Call `pane.release_agent` only when the user or the process genuinely quits.  Internal lifecycle actions that tear down and rebind the extension runtime (`/reload`, `/new`, `/resume`, and `/fork` in upstream terms) must not release Herdr authority, because the replacement runtime still owns the pane.
- Speak newline-delimited JSON, one request per line, over the Unix domain socket.  Use bounded connect and write timeouts with a single retry, and fail open.  Herdr being slow, stopped, or absent must never block, stall, or fail an agent turn.

## Only the Chief of Staff reports

- The Chief of Staff agent is the sole reporter of agent state, session identity, and pane metadata.  Every other Clyean agent must stay silent on the Herdr socket for these purposes.
- Silence must be enforced by two independent gates: the harness root-session check (`ctx.hasUI === true`) and Clyean's own agent-role identity.
- This is required because all of a project's agents share the single pane the user attaches to with `podman exec -it`.  Multiple reporters would contend for one pane's status and produce misleading sidebar state.
- Because the Chief of Staff coordinates the other agents, Clyean must surface orchestration-level waiting through the same channel.  When the Chief of Staff is awaiting the user for something that is neither a tool approval nor an `ask` question (a specification or plan review, for example), Clyean must emit `herdr:blocked` on the custom event bus with a human-readable label, and clear it when the wait is satisfied.

## Session identity

- Report the Chief of Staff's session reference with `pane.report_agent_session` on session start, on session switch or resume, and at turn start, preferring `agent_session_path` over `agent_session_id`.
- The reported path must be absolute and resolvable by Herdr, which runs on the host rather than in the container.  Clyean must translate the container-side session file path to its host equivalent using the project's mount mapping, and must omit the path (falling back to the session id) when no host-visible equivalent exists.
- Clyean must not depend on Herdr-driven session resume, since Herdr has no Clyean resume command to launch.  Session identity is reported for status rollups, pane history, and handoff.

## Environment propagation and socket access

- Propagate `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`, and `HERDR_SOCKET_PATH` into the Chief of Staff agent's container.
- Resolve the host socket from `HERDR_SOCKET_PATH` rather than assuming the default location, because named Herdr sessions place their socket under `~/.config/herdr/sessions/<name>/`.
- Bind-mount the resolved host socket into the container with read-write access, and rewrite `HERDR_SOCKET_PATH` in the container environment to the in-container mount path.
- **This mount is an explicit exception to the "Sandboxing, Workspaces & Mounts" section.**  The Chief of Staff agent is granted the full Herdr socket API, including workspace, tab, and pane mutation (`pane.split`, `pane.send_input`, `pane.run`, and `agent.start`) and control of panes that do not belong to the project.  Clyean interacts with Herdr in a manner identical to `omp`, so the socket must not be filtered, proxied, or otherwise reduced.  Alongside the Product Director agent's remote repository access, this is a sanctioned route out of the sandbox and must be documented as such for users.
- When the `herdr` executable is present on the host, mount it read-only into the container and propagate `HERDR_BIN_PATH`, so that Chief of Staff tooling which shells out to the Herdr CLI works as it does under `omp`.  Its absence must not be an error.

## Pane presentation

- Clyean must not rely on Herdr's foreground process detection or its screen-detection manifests for agent state.  Herdr sees `podman` as the pane's foreground process and has no Clyean manifest, and in any case a lifecycle report is the highest status authority and supersedes both.
- Set the pane's displayed identity explicitly with `pane.report_metadata`, supplying "Clyean" as the display agent and a pane title derived from the project, rather than expecting Herdr to label the pane correctly on its own.

## Verification and documentation

- Integration tests must run the reporter against a stub socket server and assert the exact JSON frames emitted for each lifecycle transition, covering blocked reference counting, the retry hold, sequence monotonicity, release on quit only, and silence from every agent other than the Chief of Staff.
- A test must assert that no socket traffic is attempted and no failure occurs when the Herdr environment variables are absent.
- The `docs` directory must contain a how-to guide for running Clyean inside Herdr, including the sandboxing exception above and its implications.

## Out of scope

Native Herdr support for Clyean is deferred and must not be a prerequisite for anything above.  This includes `herdr integration install clyean`, foreground process detection of the `clyean` binary, a bundled screen-detection manifest, and Herdr-driven session resume.  If Herdr later ships first-class Clyean support, Clyean must detect it and defer to it rather than reporting twice for the same pane.
