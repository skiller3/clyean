# Limitations

What this version of Clyean does not do, stated so you can plan around it.

- Sandbox population works on Linux hosts only.  Populating `.clyean/container-root` uses `podman unshare` to extract the image inside Podman's user namespace, which needs rootless Podman on a Linux host.  `clyean` builds for macOS and Windows and its inspection commands work there, but a launch stops at sandbox population.  On Windows, the orchestrator socket additionally needs a Unix host, so the socket server refuses to start.
- Rootless Podman is assumed.  Running Podman as root makes the container's root the host's root, and files written into the workspace would be owned by root.
- Six agents are implemented: User Assistant, Scaffolder, Software Engineering Director, Specifier, Software Architect, Programmer.  The other ten in `AGENT_SPECS.md` are placeholders: they appear in `clyean agents` but have no instructions, profile, or behavior.  The Software Engineering Director's remote repository access is not implemented yet, so nothing pushes or opens pull requests.
- `clyean.com` does not exist.  The install one-liners in the scripts' headers point at it; until it exists, download the scripts from the GitHub repository.
- The harness binary for the sandbox is downloaded from the GitHub release of the running Clyean version.  A build from source, or a version without a published release, must supply the harness through `CLYEAN_HARNESS_BINARY` or `sandbox.harnessBinary`.  The download is not verified against `SHA256SUMS`.
- Credentials are your responsibility.  When neither the passed-through environment variables nor the inherited credential store give a sub-agent a usable provider, its harness exits with `No models available` and the unit of work fails with that message.  This is the expected failure mode, not a defect in the orchestration.
- The orchestrator lives inside the `clyean` process you are attached with.  Detaching keeps the User Assistant running, but closing the terminal that runs `clyean` ends any delegated work in flight.  Journals make it resumable; they do not keep it running.
- Print mode cannot answer information requests.  A workflow that asks a question in `clyean -p` stays resumable but needs an interactive session to continue.
- One unit of work per project at a time, unless the project uses Git worktrees, in which case the lock is a no-op and nothing else yet coordinates concurrent work.
- Diagrams are rendered to PDF only, with the fourteen diagram types scaffolded for every project type, including miscellaneous ones.
- Re-provisioning discards nothing but is only triggered by a provisioning schema bump or an image change; other changes to the sandbox contents need `clyean sandbox rebuild`.
- The Herdr reporter's `pane.release_agent` request, sent when the User Assistant quits, carries the pane id and Clyean's source identifier.  Herdr's own integrations never call that method and its parameter schema is not published, so this request has not been verified against a live Herdr; every other report mirrors Herdr's shipped `omp` integration.
