# Clyean documentation

The documentation follows [Diátaxis](https://diataxis.fr/): tutorials teach, how-to guides solve a task, reference pages state facts, and explanation pages give the reasoning.  Start with the tutorial if Clyean is new to you.

## Tutorials

- [Getting started](tutorials/getting-started.md): install Clyean, launch it in a project, and watch the User Assistant scaffold and plan.
- [Your first change plan](tutorials/your-first-change-plan.md): a planning prompt end to end, from the information request to the implemented change.

## How-to guides

- [Run Clyean inside Herdr](how-to/run-inside-herdr.md): what Clyean reports to Herdr, what it relays, and the sandboxing exception that implies.
- [Configure agents](how-to/configure-agents.md): models, MCP servers, and instruction enhancements per agent, with local-only overrides.
- [Set up macOS](how-to/set-up-macos.md): the Podman machine Clyean needs on macOS, its size and provider, and where projects can live.
- [Set up Windows](how-to/set-up-windows.md): the Podman machine Clyean needs on Windows, WSL's limits, Hyper-V, and where projects can live.
- [Manage the sandbox](how-to/manage-the-sandbox.md): where each project's root filesystem lives; inspect, build, rebuild, prune, and enter the Podman sandbox; change the image, mounts, and harness binary.
- [Resume interrupted work](how-to/resume-interrupted-work.md): find unfinished work in journals and resume or cancel it.
- [Use print mode](how-to/use-print-mode.md): drive the User Assistant from scripts with `clyean -p`.
- [Build Clyean locally](how-to/build-clyean-locally.md): build `clyean`, the static bridge, and the harness into `bin/` with `cargo build-bin`, or one of them alone.
- [Release Clyean](how-to/release-clyean.md): what CI runs, how to cut a release, and how patch tags and assets are produced.

## Reference

- [Command line](reference/cli.md): every command, flag, and exit code of `clyean`.
- [Project layout](reference/project-layout.md): every path under `.clyean` and the `project.json` schema.
- [Agents](reference/agents.md): the roster, its files, and the verdicts each agent answers with.
- [Change plans](reference/change-plans.md): plan naming, versions, sections, and journals.
- [Environment variables](reference/environment-variables.md): host-side variables that change Clyean's behavior.
- [Sandbox contract](reference/sandbox-contract.md): the filesystem, mounts, and environment every agent can rely on inside its container.
- [Orchestrator protocol](reference/orchestrator-protocol.md): the socket protocol between the User Assistant and the host orchestrator.

## Explanation

- [Architecture](explanation/architecture.md): the host program, the orchestrator, the contained harness, and how they fit.
- [Workflows](explanation/workflows.md): the planning and implementation workflows in prose, with their bounded loops.
- [Relationship to Oh-My-Pi](explanation/relationship-to-oh-my-pi.md): what the vendored fork is, what is rebranded, and how upstream is merged.
- [Limitations](explanation/limitations.md): what this version does not do yet.
