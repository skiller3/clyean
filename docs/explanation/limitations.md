# Limitations

What this version of Clyean does not do, stated so you can plan around it.

- Sandbox population works on Linux hosts only.  Populating `.clyean/container-root` uses `podman unshare` to extract the image inside Podman's user namespace, which needs rootless Podman on a Linux host.  `clyean` builds for macOS and Windows and its inspection commands work there, but a launch stops at sandbox population.
- Rootless Podman is assumed.  Running Podman as root makes the container's root the host's root, and files written into the workspace would be owned by root.
- Six agents are implemented: User Assistant, Scaffolder, Software Engineering Director, Specifier, Software Architect, Programmer.  The other ten in `AGENT_SPECS.md` are placeholders: they appear in `clyean agents` but have no instructions, profile, or behavior.  The Software Engineering Director's remote repository access is not implemented yet, so nothing pushes or opens pull requests.
- `clyean.com` does not exist.  The install one-liners in the scripts' headers point at it; until it exists, download the scripts from the GitHub repository.
- The harness binary and the static bridge are downloaded from the GitHub release of the running Clyean version and verified against its `SHA256SUMS`.  A build from source, or a version without a published release, must supply the harness through `CLYEAN_HARNESS_BINARY` or `sandbox.harnessBinary`, and the bridge through `CLYEAN_BRIDGE_BINARY` or `cargo build-bridge`.
- Credentials are your responsibility.  Sign in through the User Assistant or export the provider's variable on the host.  When neither can supply the model a sub-agent runs with, Clyean does not start the agent, and the unit of work fails with a message naming the sign-in to add.
- An agent's credential needs come from its files under `.clyean/agents`.  Model roles set only in the project's `.omp/config.yml` or in a profile's own `config.yml` still apply inside the harness, but Clyean gives the agent credentials for their providers only when the agent's own files name those providers too.
- Sub-agents receive copies of sign-ins, never the sign-ins themselves, so the User Assistant that started them must be running for their copies to be renewed.  Stdio MCP servers receive their token once, when they start, so a long unit of work can outlive it; the server keeps the old token until it reconnects.
- `/clyean sign-in` runs the MCP sign-in inside the User Assistant's container, where the sign-in's callback is not reachable from your browser.  It works for servers that let you paste the address the browser ends on.
- Files that every profile reads, such as `~/.env` in the sandbox home, are shared by all agents of a project.  Keep secrets out of them.
- `clyean scaffold --project-type` has no User Assistant to resolve the Scaffolder's needs, so the Scaffolder then receives every provider variable the User Assistant would.
- The orchestrator lives inside the `clyean` process that started your User Assistant.  Closing that terminal, or stopping the process in any other way, ends the User Assistant and any delegated work in flight.  Journals make the work resumable; they do not keep it running.
- Two invocations that both pass `--continue` open the project's most recent session from two processes, which the harness does not support; Clyean only warns.
- A User Assistant whose harness stops responding cannot shut itself down when its `clyean` process ends; `clyean sandbox prune` removes its container.
- Print mode cannot answer information requests.  A workflow that asks a question in `clyean -p` stays resumable but needs an interactive session to continue.
- One unit of work per project at a time, unless the project uses Git worktrees, in which case the lock is a no-op and nothing else yet coordinates concurrent work.
- Diagrams are rendered to PDF only, with the fourteen diagram types scaffolded for every project type, including miscellaneous ones.
- Re-provisioning discards nothing but is only triggered by a provisioning schema bump or an image change; other changes to the sandbox contents need `clyean sandbox rebuild`.
- The Herdr reporter's `pane.release_agent` request, sent when the User Assistant quits, carries the pane id and Clyean's source identifier.  Herdr's own integrations never call that method and its parameter schema is not published, so this request has not been verified against a live Herdr; every other report mirrors Herdr's shipped `omp` integration.
