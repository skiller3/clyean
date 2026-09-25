# Basic User Experience
From a UX perspective, `clyean` is primarily a CLI program that launches a terminal UI (TUI) in which the user performs agentic programming (a.k.a vibe coding) and other LLM-based tasks (e.g. research, modeling, authoring).  The CLI/TUI should behave identically to Oh-My-Pi (`https://omp.sh/`; `https://github.com/can1357/oh-my-pi`) with the following modifications:
  - Titles, headers, and text should be rebranded from "omp" and "Oh-My-Pi" to "clyean" and "Clyean".  However, the TUI welcome page should prominently display the text "Clyean uses the fabulous Oh-My-Pi (https://omp.sh/) harness!"
  - The TUI welcome page's title shows the version of the `clyean` program that launched the harness, not the contained harness's own version.  The page shows no placeholder text (such as "Unknown") for details it does not have yet, such as the model before one is selected.
  - CLI help text and messages should be rebranded from "omp" and "Oh-My-Pi" to "clyean" and "Clyean".  `clyean -h` introduces the program as "Clyean: the slop-scrubbing agentic coding harness".
  - The "Pi" symbol logo should be replaced with Clyean's logo, `assets/clyean-logo.svg`: a bar of soap with a glossy highlight band and rising bubbles, with no smile or other face.  The TUI's welcome and setup views show a rendition of it in terminal characters that new users readily recognize as a bar of soap.
  - `omp` commandline arguments/options that are non-coherent given Clyean's goals or complicated to implement due to its architecture should be eliminated.
  - `omp` TUI configuration, options, commands (including slash commands), and overall functionality that is non-coherent given the Clyean's goals or complicated to implement given its architecture should be eliminated.
  - MCP (`/mcp`) commands and server connections should be supported with the enhancement of generally being scoped to particular agents (it shouldn't be assumed that all agents that coordinate to perform a unit of work should have access to the same MCP servers).
  - Model (`/model`) and switch (`/switch`) commands need to be adjusted to ensure model connections, authentication mechanisms, and usage parameters are scoped to specific agents (see "Credentials & Secrets").

IMPORTANT: When the user passes prompts to the Clyean CLI/TUI, they should be directly interacting with the "User Assistant" agent, which is itself one of several agents specified in this file's "Agents" section.


# Supported Platforms
Clyean supports the following operating systems, each on both the x86-64 and ARM64 (AArch64) processor architectures.  No other architectures are supported.

| Operating system | Installer | Where Podman runs agent containers |
| --- | --- | --- |
| Linux | `install.sh` | On the host's kernel, rootless |
| Linux on WSL (a Linux distribution running under WSL 2) | `install.sh`, run inside the distribution | On the distribution's kernel, rootless |
| Native Windows | `install.ps1` | In a Linux virtual machine managed by `podman machine` |
| macOS | `install.sh` | In a Linux virtual machine managed by `podman machine` |

Supported means that every requirement in this document and in `AGENT_SPECS.md` holds on each of the eight combinations of operating system and architecture.

Clyean requires Podman 5.0 or later on native Windows and macOS, and Podman 4.9 or later on Linux and Linux on WSL.  On native Windows and macOS, Clyean supports the Podman machine provider that the installed Podman uses by default (on native Windows, WSL).  The Hyper-V provider on native Windows is supported on a best-effort basis, and other providers are unsupported.  Before starting any container, Clyean verifies the Podman version and stops with an actionable message when it is older than the floor for its operating system.  It warns, without stopping, when a Podman machine uses any provider other than the default, or has fewer CPUs or less memory than the "Installation" section recommends.

Some requirements in this document are written in terms of a mechanism that works only when Podman runs agent containers on the same kernel as the `clyean` process, such as bind-mounting a host executable into a container.  On Linux and Linux on WSL, those requirements apply as written.  On native Windows and macOS, where agent containers run inside a Podman machine and such mechanisms may be unavailable, each of those requirements is met by any means that produces the same result for agents and the user, with the same security properties.  For example, the requirement to mount the host's `herdr` executable into the User Assistant's container is met on native Windows and macOS, where that executable cannot run in a Linux container, by installing a Linux build of the Herdr CLI at the same path, where the agent cannot modify it.

The contained harness always runs inside a Linux container, so the `clyean-harness-linux-x64` and `clyean-harness-linux-arm64` builds serve every supported platform, selected by the architecture of the kernel that runs the containers.


# Installation
Users are expected to install the latest release (or any specific release via a passed release version argument) via one of two scripts:
- `install.sh` - Bourne script expected to be used for installation within Linux (including Linux distributions running under WSL) and MacOS environments, often via a command like `curl -fsSL https://clyean.com/install.sh | sh`.
- `install.ps1` - PowerShell script expected to be used within Windows environments (outside of WSL), often via a command like `irm https://clyean.com/install.ps1 | iex`.

The preceding scripts are expected to install any necessary required dependencies (e.g. Podman), and broadly comply with software installation norms for their respective OS environments (for example, the `clyean` executable should be installed by default within `/home/<user>/.local/bin/` within Ubuntu Linux environments).  On native Windows and macOS, the required dependencies include a running Podman machine.  When no Podman machine exists, the installers create one sized for Clyean: 4 CPUs, or all of the host's CPUs if it has fewer; 8 GiB of memory, or half of the host's memory if that is less; and a disk that can grow to 100 GiB.  Where the machine provider takes its limits from settings shared beyond the machine, as WSL does on native Windows, the installers leave those settings unchanged and report how to raise them when they fall below this size.  The installers never change the settings of an existing Podman machine; they only start it when it is stopped.  The installers verify that the installed Podman meets the floor for their operating system (see "Supported Platforms") and, when it does not, stop with instructions for installing a newer release.  When in doubt about correct design and behavior of these scripts, please mirror the design and behavior of `omp`'s installation scripts at `https://omp.sh/install` and `https://omp.sh/install.ps1`.

Both scripts install the prebuilt `clyean` executable published as a GitHub release asset named `clyean-<platform>-<arch>` (`linux`, `darwin`, or `windows`; `x64` or `arm64`; `.exe` on Windows), verify it against the release's `SHA256SUMS`, and smoke-test it with `clyean --version` before reporting success.  Both accept a release tag (`--ref`/`-Ref`), a source build via `cargo install` (`--source`/`-Source`), and an opt-out of dependency installation (`--no-deps`/`-NoDeps`).  They never prompt, because their standard input is the download pipe.

No `clyean.com` website is currently launched; please just author the two install scripts at the top level of the `clyean` repository for now.


# Projects & Scaffolding
Unless otherwise specified via arguments in a fashion that mirrors `omp`'s behavior, the `clyean` executable should assume the current working directory of its process represents a "project" for which it is going to be used to perform work.  If a `.clyean/project.json` file exists within the project directory, Clyean should consider the project to be scaffolded; if the file doesn't exist, then `clyean` should immediately scaffold the project by creating the following resources in the project's top-level directory:

- `.git` directory - All Clyean projects must be version controlled via Git.  The harness should simply use the existing Git repository for the project if it already exists, but if there isn't one then a Git repository should be appropriately initialized.  When initializing a new repository for a project, the user should be asked (unless the info was already supplied via args or config) if Git worktrees may be used.  When posing the question, the user should also be informed that worktree usage is preferrable to enable better performance via concurrent work.
- `.clyean` directory - Contains all Clyean configuration and state for the project.  Every scaffold resource other than the Git repository resides within it.
- `.clyean/project.json` JSON file - Contains the overall top-level configuration of the Clyean project.  Among other information, the file must contain (i) the version of `clyean` that initially generated the scaffolding, (ii) the UTC timestamp at which the scaffolding was initially generated, (iii) the initial Podman/Docker image used for the project's agent sandboxes, (iv) the workspace path to be used in conjunction with the project, (v) any additional mounts and podman run arguments that should be used for running agents on behalf of the project.  Unless specified by the user, the initial image used for a project's agent sandboxes should be `ubuntu:latest`.
- `.clyean/agents` directory - Contains base instructions (per typical `AGENTS.md` files) and omp-compatible configurations of the various agents comprising `clyean` as a system.  The base instruction files are expected to adhere to the pattern `AGENTS__<AGENT_NAME>.md` (e.g. `AGENTS__USER_ASSISTANT.md`).  Each agent's harness configuration lives beside its instructions as `<AGENT_NAME>.omp.json` (a settings overlay in the harness's `config.yml` schema, applied last through the harness's `--config` flag) and `<AGENT_NAME>.mcp.json` (the agent's MCP servers, merged into its profile's `mcp.json`).  Before an agent starts, Clyean projects these files into the agent's harness profile inside the sandbox, so model, authentication, usage, and MCP settings are scoped to the agent that owns them.  Credentials and other secrets reach each agent as the "Credentials & Secrets" section describes.
- `.clyean/architecture` directory - Contains up-to-date UML architecture diagrams spanning all 14 official UML diagram types for the project.  Each diagram is represented both as (i) a PlantUML source file (`*.puml`) and (ii) a PDF file (`*.pdf`) rendered directly from that source.  The PlantUML source is the artifact of record; the PDF is a generated rendering of it, must never be hand-edited, and must never be allowed to drift from its source.  See the "Architecture Diagram Rendering" sub-section below.
- `.clyean/SPECS.md` markdown file - Contains up-to-date specifications (i.e. requirements) for the project.
- `.clyean/plans` directory - Contains plans for changes to the project; these "change plans" are analogous to the ones developed and indexed/catalogued by Claude Code or `omp` but will adhere to a content organization outline that streamlines human user review and processing by the sub-agents comprising the Clyean system.  Each plan is a directory named `<YYYY-MM-DD>-<slug>` holding one file per version (`v1.md`, `v2.md`, ...) and one journal per unit of work that touched it (`journal-<work-id>.json`).  Versions are never edited in place.
- `.clyean/work` directory - Contains the journals of units of work that do not belong to a change plan (research and scaffolding).  It is excluded from version control.
- `.clyean/.gitignore` file - Holds the ignore rules Clyean needs (`*.local.*`, `lock`, `logs/`, `work/`), written only for rules Git does not already honor.
- `.clyean/sandbox.local.json` JSON file - Holds the random identifier of the project's sandbox root filesystem (see "Sandboxing, Workspaces & Mounts").  Being local-only, it is never committed, it moves with the project directory, and it differs between clones of one repository.  Copying it into another checkout of the same repository makes both checkouts share one sandbox.

All of the preceding `.clyean` scaffold materials are intended to be modifiable by the user and Clyean itself and, with the exception of `.clyean/sandbox.local.json` and the other files that `.clyean/.gitignore` excludes, subjected to version control locally and within remote project repositories.  However, Clyean should also support user-provided local-only (i.e. not pushed to remote repository) (i) enhancements of agent baseline instructions, (ii) overrides of agent configurations, and (iii) overrides of Clyean project configuration.  These local-only enhancements and overrides should be implemented by mirorring Claude Code semantics for `*.local.<ext>` files.  


## Architecture Diagraming & Rendering

Clyean renders the `.clyean/architecture` diagrams with PlantUML (`https://plantuml.com/`), which supports all 14 official UML diagram types with first-class notation.

- Clyean must use PlantUML's MIT-licensed distribution (`plantuml-mit-<version>.jar`), not the default GPL distribution.  The MIT distribution omits only the optional `ditaa`, `jcckit`, and sudoku integrations, none of which Clyean uses, and it generates every UML diagram type.
- Pin an explicit PlantUML version rather than tracking the latest release, so that an upstream layout change cannot silently alter a project's rendered diagrams.
- Render with the Smetana layout engine (`-Playout=smetana`) so that no external Graphviz installation is required inside the agent sandbox image.
- Render locally and only locally.  Clyean must never submit diagram sources to the public PlantUML server or to any other third-party rendering service, because those sources describe the user's private architecture.
- The agent sandbox image must provide a Java runtime and the pinned PlantUML jar.  Scaffolding must fail with an actionable message if either is missing rather than silently producing a project whose diagrams cannot be rendered.
- Treat a non-zero PlantUML exit status as a failed render and discard its output.  PlantUML can emit an image carrying a sponsored message in place of a diagram when a render fails, and such an image must never be written into `.clyean/architecture`.


# Sandboxing, Workspaces & Mounts
Clyean must sandbox all agent activity via Podman (https://podman.io/) containers, and users are expected to be aware that Podman-based sandboxing is occurring (do not try to abstract them aware from this fact).  To be architecturally specific, any Clyean agent should always run as the sole direct child process of an `init` root process (PID 1) within a Podman container.

All agents used within a "project" should share the same container image and file system.  The entire agent container file system is the project's sandbox root filesystem: a directory that Clyean owns on the Podman host's own Linux filesystem, outside the project directory and outside Podman's volume management, in a `clyean/roots` directory beside Podman's `containers` data directory.  For rootless Podman that is `~/.local/share/clyean/roots/<sandbox-id>` on Linux and Linux on WSL, and the same location inside the Podman machine on native Windows and macOS.  The sandbox identifier is the random value in the project's `.clyean/sandbox.local.json`.  Clyean populates the root filesystem by streaming an export of the configured image into a helper container that extracts it, provisions it (base packages, a Java runtime, the pinned PlantUML jar, and the harness binary at `/usr/local/bin/clyean`), records a marker file at its root, and starts every agent container with `podman run --init --rootfs <that directory>`.  Every operation on a root filesystem that needs container privileges runs in a helper container; Clyean does not use `podman unshare`.

A sandbox root filesystem persists across Clyean sessions, Clyean upgrades, and restarts of the host or the Podman machine, and no Podman prune command removes it.  It is removed only by Clyean at the user's request, or together with the Podman machine that holds it when that machine is reset or removed.  A marker whose provisioning schema is behind the running `clyean` triggers re-provisioning in place.  `clyean sandbox rebuild` discards and re-creates the project's root filesystem.  `clyean sandbox prune` lists root filesystems whose recorded project directory no longer exists or no longer holds the matching sandbox identifier, and removes them when invoked with `--remove`.

Each invocation of the `clyean` command starts exactly one User Assistant agent, in a container of its own, and connects the user's terminal to it.  The container runs in the foreground with `podman run --rm --init`, detaching from it is disabled, and it ends when the invocation ends, for whatever reason.  A second invocation in the same project starts a second User Assistant, in a second container, with its own session; no container ever hosts more than one User Assistant.  Maintenance shells run in separate containers on the same root filesystem, opened with `clyean sandbox shell`.  The User Assistant's container communicates with its `clyean` process only through the bridge: a single `podman exec -i` session that `clyean` opens into the container and multiplexes.  Inside the container, the bridge serves `/run/clyean/orchestrator.sock`, which carries the orchestrator protocol and nothing else, and, when Clyean runs inside a Herdr pane, the Herdr socket (see "Herdr Compatibility"), both on private in-memory filesystems that no other container can reach.  The User Assistant's harness starts only once the bridge is serving.  When the `clyean` process ends, the bridge ends with it, the User Assistant shuts down, and its container is removed.  `docs/reference/sandbox-contract.md` is the normative description of the container layout and environment.

Clyean should also contain the formal user-exposed notion of a "workspace" directory, and the workspace directory must either be the project directory itself or be an ancestor directory of the project directory.  By default, the workspace directory should be mounted into the agent container file tree with read-write access at `/home/<user>/workspace/<host-workspace-directory-name>`.

The user should also be able to configure any number (up to practical limits imposed by Linux and Podman) of mounts of local OS file system content into the Podman guest OS file system.  The mount points inside the container should reside in `/mnt/`, follow the naming convention `/mnt/<host-directory-or-file-name>`, and be read-only.

Clyean's sandboxing behavior should be enforced as follows:
- The Podman-based architecture should deterministaclly ensure agents NEVER modify file content outside of the specified workspace.
- By default, all agents should be instructed to never modify file system content outside of the project directory unless directly and explicitly instructed to do so.
- With the exception of the "Software Engineering Director" agent being capable of pushing changes to remote repositories and issuing pull requests, all agents should be instructed by default to never modify the state of any connected system (via MCP, API, UI, or otherwise) unless directly and explicitly instructed to do so. 
- As a further exception, the "User Assistant" agent is granted the full Herdr socket API when Clyean runs inside a Herdr pane, which lets it mutate the user's terminal workspace outside the project.  See the "Herdr Compatibility" section for how the socket reaches the container and the capabilities it grants.


# Credentials & Secrets
Every agent has its own login store: the harness credential store in that agent's own harness profile (`~/.omp/profiles/<agent-id>`), which holds the provider sign-ins, API keys, and MCP server credentials the agent authenticates with.  Agents authenticate only from their own login stores and from the secrets Clyean passes to them.  Clyean never connects agents to a shared credential store or credential service, such as the harness's auth broker.

- The user signs in through the User Assistant (for example with `/login`), and every sign-in is recorded in the User Assistant's login store.  That includes sign-ins that only other agents need, such as one for an MCP server configured only for another agent.
- When any other agent starts, and before it sends any request, Clyean copies into that agent's login store the credentials it needs, and only those: the credentials for the providers of the models it is configured to use and for the MCP servers configured for it.  An agent whose configuration names no default model runs with the User Assistant's current model and receives the credentials for that model's provider.  The copies replace the ones the agent held before, so a sign-out through the User Assistant reaches every agent no later than its next start.
- A copy lets an agent use a credential but not refresh it.  Sign-ins are refreshed only in the User Assistant's login store, and Clyean renews each copy from that store before the copy expires, including while the agent is working, so that no agent can invalidate a credential another agent holds.
- Clyean copies credentials through the harness's own credential interfaces, never by copying a store's files, so that every copy is a consistent snapshot.
- Each agent's containers expose that agent's own login store and no other agent's.  This is an explicit exception to the shared file system described in the "Sandboxing, Workspaces & Mounts" section.
- Secrets from the host reach only the agents that need them.  Provider API keys and cloud credentials in the host environment pass to the User Assistant, whose model the user chooses while working, and to each other agent only for the providers of the models it is configured to use.  Further variables that the project passes through reach only the agents the project configuration assigns them to, such as a repository token for the Software Engineering Director alone.


# Oh-My-Pi (omp) Architectural Relationship & Usage

Clyean *contains* a fork of `omp` and sits above it: Clyean's own code is an orchestration layer that coordinates communication between multiple agents, each of which runs independently on the contained harness.  The rebranding, option-pruning, and per-agent scoping requirements in the "Basic User Experience" section describe changes to the contained harness; the user interacts with a "User Assistant" agent through the terminal of the `clyean` invocation that started it, in a container of its own (see "Sandboxing, Workspaces & Mounts").

The `omp` fork is vendored into this repository at `vendor/omp` as a squashed `git subtree`, tracked against the `upstream` remote (`https://github.com/can1357/oh-my-pi`, push disabled).  Upstream changes are taken with `git subtree pull --prefix=vendor/omp upstream main --squash`.  Upstream tags are deliberately not fetched so they cannot collide with Clyean's own semantic version tags.  Modifications to the harness are made in place under `vendor/omp` and should be kept as narrow and as well isolated as practical, since every additional point of divergence is a conflict to resolve on each upstream pull.  Every divergence is recorded in `vendor/omp/CLYEAN-MODIFICATIONS.md`, which each upstream pull re-applies and re-verifies.

The harness that runs inside the sandbox is a Linux binary built from `vendor/omp` by the CI/CD workflow and published as the release assets `clyean-harness-linux-x64` and `clyean-harness-linux-arm64`.  When provisioning a sandbox, `clyean` takes the harness from `CLYEAN_HARNESS_BINARY`, then from `sandbox.harnessBinary` in `project.json`, then from a `clyean-harness-linux-<arch>` file beside its own executable, and otherwise downloads the asset of its own version into the user's cache directory.

The contained harness deliberately keeps `omp`'s configuration discovery model rather than rebranding it.  Retaining the upstream paths and environment variables keeps the divergence from upstream small and should help preserve compatibility with the config files `omp` already imports from other tools to the maximum extent practical.  Specifically:
  - The user-level config root remains `~/.omp` (with the agent directory at `~/.omp/agent` and named profiles at `~/.omp/profiles/<name>`).
  - Project-level configuration remains the `.omp/` directory discovered by walking up from the working directory.
  - Environment variables retain their `OMP_*` and `PI_*` prefixes (for example `OMP_PROFILE`, `PI_CONFIG_DIR`, `PI_CODING_AGENT_DIR`).
  - Package and module identifiers inherited from upstream (for example `@oh-my-pi/*` imports) are not renamed.

These paths and variables are an explicit exception to the rebranding requirements in the "Basic User Experience" section.  Rebranding applies to what the user reads (TUI text, logos, CLI help and messages), not necessarily to where configuration lives on disk or how it is named in the environment.  Where the harness needs configuration that has no upstream equivalent, it is added within the existing `~/.omp` and `.omp/` roots rather than in a new location.


# Agents

Clyean is a system of several specialized agents that coordinate to perform work, and the specification of those agents lives in a companion file, `AGENT_SPECS.md`.  Ingest `AGENT_SPECS.md` in full alongside this file before doing any work; it carries the same authority as this file, and every agent named here (the User Assistant and the Software Engineering Director, for example) is defined there.

Keep the division between the two files intact as they evolve.  Requirements that describe Clyean as a whole, or that hold regardless of which agent is acting, belong in this file.  The roster of agents and each agent's responsibilities, baseline instructions, and interactions with the other agents belong in `AGENT_SPECS.md`.

**IMPORTANT**: From a user perspective, Clyean should have the notion of a session that is analogous to an `omp` or Claude Code session, and the semantics for a "session" and its ID should be nearly the same (e.g. user should be able to resume a previous existing session; all interaction with a `clyean` User Assistant agent via the TUI should occur within the same session unless explicit action is taken by the user).  From an implementation perspective, a Clyean session should simply be the `omp` session of a specific User Assistant agent conversation.  Each `clyean` invocation runs one User Assistant, which starts a new session unless the user asks to continue or resume an earlier one; a continued or resumed session runs in the new invocation's container.  All other Clyean agents besides the User Assistant should be invoked with new sessions for each new user-provided prompt being processed.  All `omp` sessions should be re-used within the processing of a given Clyean prompt (for avoidance of doubt, usage of these sub-agent sessions should survive the gathering of follow-up information from the user in order to fulfill an initial user-supplied prompt).

The resetting of sub-agent context windows for each new user-supplied prompt is critical to Clyean's goal of preventing the accumulation of technical debt within a project.

Coordination between agents is performed by Clyean's host-side orchestrator, which runs inside the `clyean` process on the host.  The User Assistant reaches it through the tools `clyean_status`, `clyean_scaffold`, `clyean_delegate`, `clyean_provide_information`, `clyean_resume`, and `clyean_cancel`, provided by a Clyean-managed harness extension over the orchestrator socket; `docs/reference/orchestrator-protocol.md` is the normative description of that protocol.  The orchestrator runs each workflow step by step, opening one harness session per sub-agent per unit of work in its own container, and relays sub-agent output and information requests back to the User Assistant.  When an instruction asks a sub-agent for a decision, the sub-agent ends its reply with one fenced `json` block holding the verdict object the instruction describes.


# Git Usage & Durable Execution

Within a Clyean session, all activities should be subject to durable execution as commonly defined (see `https://restate.dev/what-is-durable-execution`) to the extent practical; among the typical benefits of workflow systems with durable execution, a Clyean user should expect a resumed session to finish the work that was already started within it.  The preferred method to keep track of the state of work is Git.  Other than respecting `.gitignore` and other explicit Git-related instructions from the user, Clyean's deterministic logic and sub-agents should stage and commit their changes to project state (including changes to change plans themselves).  Consequently, the responsible agents should stage and commit materials after many of the steps in both the `workflow-planning.mmd` and `workflow-implementation.mmd` files.

Unless explicitly instructed otherwise, Clyean sub-agents should always identify themselves as the author of specific changes in their Git commit messages; the mechanism and format of all such identification should be standardized across all Clyean agents.  The standard is a commit author of `Clyean <Agent Display Name> <agent-id@agents.clyean.com>` (for example `Clyean Programmer <programmer@agents.clyean.com>`) and a final trailer line `Clyean-Agent: <agent-id>` in the message.  Agents receive this identity through the `GIT_AUTHOR_*` and `GIT_COMMITTER_*` variables of their container, and commits made by Clyean's deterministic logic on an agent's behalf use the same identity.

Every unit of work is journaled: the journal records the current workflow phase, the questions asked of the user and their answers, the harness sessions of the participating sub-agents, and the commits made.  Plan journals are committed with their plan, so a resumed session finds the work exactly where it stopped and resumes the sub-agents' own sessions.


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
- The extension is delivered through the project's sandbox root filesystem so that it is present at `~/.omp/agent/extensions/` inside the User Assistant agent's container (under the User Assistant's harness profile, as `clyean-herdr-reporter.ts`).
- The extension is Clyean-managed rather than user-managed.  It must carry an integration version marker, be replaced when Clyean upgrades it, and state in a header comment that user customizations belong in sibling files rather than in edits to it.
- Reports must identify themselves with the source `custom:clyean` and the agent label `clyean`.
- Exactly one reporter may claim a pane.  Herdr's own `herdr:omp` integration file must never be shipped into or installed within a Clyean container.

## Reported agent state

- Report state with the `pane.report_agent` method, using only the `idle`, `working`, and `blocked` states.
- Report `working` from the start of an agent turn until the turn ends.
- Report `blocked` while any tool approval is outstanding and while the `ask` tool is awaiting an answer.  Overlapping blocks must be reference counted so that resolving one of several does not prematurely clear the state.  The reported message should be the approval reason or the first question text, so the sidebar explains what the agent is waiting on.
- Report `idle` otherwise, debounced (default 250 ms, overridable with `CLYEAN_HERDR_IDLE_DEBOUNCE_MS`) so that brief gaps between turns do not flicker the sidebar.
- Hold `working` through retryable provider failures (overload, rate limiting, 5xx responses, and transport resets) for a grace period (default 2500 ms, overridable with `CLYEAN_HERDR_RETRY_GRACE_MS`) before falling through to `blocked`, so that automatic retries are not misreported as idle.
- Stamp every report with a monotonically increasing sequence number, serialize reports through a single queue, and ensure a duplicate or late turn-end event cannot publish a false `idle`.
- Call `pane.release_agent` only when the user or the process genuinely quits.  Internal lifecycle actions that tear down and rebind the extension runtime (`/reload`, `/new`, `/resume`, and `/fork` in upstream terms) must not release Herdr authority, because the replacement runtime still owns the pane.
- Speak newline-delimited JSON, one request per line, over the Unix domain socket.  Use bounded connect and write timeouts with a single retry, and fail open.  Herdr being slow, stopped, or absent must never block, stall, or fail an agent turn.

## Only the User Assistant reports

- The User Assistant agent is the sole reporter of agent state, session identity, and pane metadata.  Every other Clyean agent must stay silent on the Herdr socket for these purposes.
- Silence must be enforced by two independent gates: the harness root-session check (`ctx.hasUI === true`) and Clyean's own agent-role identity.
- This is required because a User Assistant and the sub-agents it coordinates all belong to the single pane of the `clyean` invocation that started the User Assistant.  Multiple reporters would contend for that pane's status and produce misleading sidebar state.  User Assistants started by different invocations of one project report for their own panes, independently.
- Because the User Assistant coordinates the other agents, Clyean must surface orchestration-level waiting through the same channel.  When the User Assistant is awaiting the user for something that is neither a tool approval nor an `ask` question (a specification or plan review, for example), Clyean must emit `herdr:blocked` on the custom event bus with a human-readable label, and clear it when the wait is satisfied.

## Session identity

- Report the User Assistant's session reference with `pane.report_agent_session` on session start, on session switch or resume, and at turn start, preferring `agent_session_path` over `agent_session_id`.
- The reported path must be absolute and resolvable by Herdr, which runs on the host rather than in the container.  Clyean must translate the container-side session file path to its host equivalent using the location of the project's sandbox root filesystem and the project's mount mapping, and must omit the path (falling back to the session id) when no host-visible equivalent exists.
- Clyean must not depend on Herdr-driven session resume, since Herdr has no Clyean resume command to launch.  Session identity is reported for status rollups, pane history, and handoff.

## Environment propagation and socket access

- Propagate `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`, and `HERDR_SOCKET_PATH` into the User Assistant agent's container.
- Resolve the host socket from `HERDR_SOCKET_PATH` rather than assuming the default location, because named Herdr sessions place their socket under `~/.config/herdr/sessions/<name>/`.
- Relay the resolved host socket into the User Assistant's container through the bridge, at a fixed in-container path, and rewrite `HERDR_SOCKET_PATH` in the container environment to that path.
- **This channel is an explicit exception to the "Sandboxing, Workspaces & Mounts" section.**  The User Assistant agent is granted the full Herdr socket API, including workspace, tab, and pane mutation (`pane.split`, `pane.send_input`, `pane.run`, and `agent.start`) and control of panes that do not belong to the project.  Clyean interacts with Herdr in a manner identical to `omp`, so the relay must pass every request and response through unmodified and must not filter or otherwise reduce the socket API.  Alongside the Software Engineering Director agent's remote repository access, this is a sanctioned route out of the sandbox and must be documented as such for users.
- When the `herdr` executable is present on the host, mount it read-only into the container and propagate `HERDR_BIN_PATH`, so that User Assistant tooling which shells out to the Herdr CLI works as it does under `omp`.  Its absence must not be an error.

## Pane presentation

- Clyean must not rely on Herdr's foreground process detection or its screen-detection manifests for agent state.  Herdr sees `podman` as the pane's foreground process and has no Clyean manifest, and in any case a lifecycle report is the highest status authority and supersedes both.
- Set the pane's displayed identity explicitly with `pane.report_metadata`, supplying "Clyean" as the display agent and a pane title derived from the project, rather than expecting Herdr to label the pane correctly on its own.

## Verification and documentation

- Integration tests must run the reporter against a stub socket server and assert the exact JSON frames emitted for each lifecycle transition, covering blocked reference counting, the retry hold, sequence monotonicity, release on quit only, and silence from every agent other than the User Assistant.
- A test must assert that no socket traffic is attempted and no failure occurs when the Herdr environment variables are absent.
- The `docs` directory must contain a how-to guide for running Clyean inside Herdr, including the sandboxing exception above and its implications.

## Out of scope

Native Herdr support for Clyean is deferred and must not be a prerequisite for anything above.  This includes `herdr integration install clyean`, foreground process detection of the `clyean` binary, a bundled screen-detection manifest, and Herdr-driven session resume.  If Herdr later ships first-class Clyean support, Clyean must detect it and defer to it rather than reporting twice for the same pane.


# Code

Clyean's software logic (other than the built-in Podman functionality, built-in `omp` functionality, and the installation scripts) should be implemented via the Rust programming language (https://rust-lang.org/) and commonly used Rust crates (i.e. packages).  The Rust code is a Cargo workspace under `crates/` whose members separate the concerns of project state (`clyean-project`), Git (`clyean-git`), the agent roster and profiles (`clyean-agents`), PlantUML (`clyean-plantuml`), the Podman sandbox (`clyean-sandbox`), the harness RPC client (`clyean-harness`), orchestration (`clyean-orchestrator`), and the command line (`clyean`), with dependencies flowing in that direction only.  The two harness extensions shipped into the sandbox are TypeScript files under `extensions/`, embedded into the `clyean` executable at build time and tested with `bun test`.  Ensure you adhere to the following software development best practices ordered from most to least important:

- Accurate, descriptive, and concise naming of variables, functions, modules, macros, files, etc.
- Upholding the Principle of Least Astonishment (POLA)
- Rigorous factoring and refactoring of code to separate concerns
- SOLID (https://en.wikipedia.org/wiki/SOLID)
- Don't Repeat Yourself (DRY)
- Rust developer community norms and idioms