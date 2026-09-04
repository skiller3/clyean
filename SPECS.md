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

# Agent Multiplexer Compatibility