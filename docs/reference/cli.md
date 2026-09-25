# Command line

```text
clyean [OPTIONS] [PROMPT]...
clyean [OPTIONS] <COMMAND>
```

Without a command, `clyean` launches a User Assistant of its own in the project's sandbox.  Positional arguments are joined with spaces and sent as the initial prompt.

## Global options

| Option | Effect |
| --- | --- |
| `--cwd <DIR>` | Project directory (default: the current directory).  Resolved to a canonical absolute path. |
| `--workspace <DIR>` | Workspace directory: the project directory or one of its ancestors.  It is mounted read-write into the sandbox.  Defaults to the project directory.  Any other directory is rejected. |
| `-v`, `--verbose` | Detailed diagnostics on standard error (equivalent to `CLYEAN_LOG=info`). |
| `-h`, `--help` | Help for `clyean` or a subcommand. |
| `-V`, `--version` | Print the version and exit. |

## Launch options

These apply only when no command is given.

| Option | Effect |
| --- | --- |
| `-c`, `--continue` | Continue the project's most recent User Assistant session.  When another invocation of the project is running, `clyean` warns that the session may be in use. |
| `-r`, `--resume [SESSION]` | Resume a session by id prefix or path; without a value, open the session picker. |
| `--model <MODEL>` | Model or configured role for the User Assistant, for this launch only. |
| `-p`, `--print` | Non-interactive mode: send the prompt, print the result, exit.  The container gets no pseudo-terminal. |
| `--no-session` | Do not save the User Assistant session. |
| `--worktrees <yes|no>` | Answer the Git worktree question of a new project instead of being asked.  Without a terminal on standard input the answer defaults to `no`. |
| `--image <IMAGE>` | Image to populate a new project's sandbox from (default `docker.io/library/ubuntu:latest`).  Ignored for scaffolded projects. |
| `--mount <PATH>` | Host path mounted read-only at `/mnt/<base name>` in every agent container; repeatable.  Ignored for scaffolded projects. |

What a launch does, in order: resolve the project; ask the worktree question when the project is not scaffolded; initialize Git when needed and write the agent files and ignore rules; ask Podman about itself, stopping when it is older than the floor for this operating system (4.9 on Linux and Linux on WSL, 5.0 on native Windows and macOS) and warning about a Podman machine's provider or size; record a sandbox identifier in `.clyean/sandbox.local.json` when the project has none; populate and provision the sandbox root filesystem when missing or outdated, or else replace its harness when it differs from the one this `clyean` installs; project every agent's profile and record this use in the sandbox marker; find the static bridge executable (see [environment variables](environment-variables.md)); on a Podman machine, check that it sees every bind-mount source; start the orchestrator inside the `clyean` process; run this invocation's own User Assistant container in the foreground (`podman run --rm`); and, once the container runs, open its bridge with `podman exec --interactive`.  The User Assistant's harness starts only once the bridge is serving.

Every invocation gets its own container and session, so several invocations of one project can run at once; delegated work stays serialized by the project lock unless the project uses Git worktrees.  Nothing can attach to a running User Assistant, and detaching is disabled.  Ctrl-C is passed to the User Assistant rather than terminating the launcher.  When the User Assistant exits, its container is removed.  When the `clyean` process ends for any other reason, its bridge ends with it, the User Assistant shuts itself down, and Podman removes the container.

## Commands

### `clyean scaffold [--project-type <TYPE>] [--worktrees <yes|no>] [--image <IMAGE>] [--mount <PATH>]...`

Prepares the host scaffold (Git repository, agent files, ignore rules, sandbox, profiles) without launching the User Assistant.  With `--project-type software-engineering` or `--project-type miscellaneous` on an unscaffolded project it also writes `project.json`, the specification and diagram skeletons, commits them, and runs the Scaffolder agent, streaming its progress to standard output.  It fails when the project is already scaffolded and a project type is given.

### `clyean sandbox status`

Prints the sandbox identifier and the root filesystem's location on the Podman host, the image, digest, provisioning time and version, and harness version and SHA-256 prefix recorded in its marker, the project that used it last and when, and the User Assistants running on it.  Exits 1 when the root is not provisioned.

### `clyean sandbox build`

Populates and provisions the sandbox root when it is missing or outdated, or else replaces its harness when it differs from the one this `clyean` installs, then refreshes the agent profiles.

### `clyean sandbox rebuild`

Discards the sandbox root filesystem from a helper container, so files owned by container users are removed too, and provisions it again.  Refuses while any container of the sandbox runs.

### `clyean sandbox shell`

Opens an interactive root shell in a maintenance container with the workspace mounted.

### `clyean sandbox prune [--remove]`

Removes User Assistant containers, in every project, whose `clyean` process is gone: containers more than a minute old that are stopped or have no running bridge process.  They are left behind only when a User Assistant's harness stops responding and cannot shut itself down.  Containers whose processes cannot be listed are kept.

It then lists every sandbox root filesystem in Clyean's roots directory with its identifier, the project that used it last, the time of that use, its size, and whether it is orphaned: not used by a running container, and either its recorded project directory is gone or holds another sandbox identifier, or it has no marker and is more than a day old.  `--remove` deletes the orphaned root filesystems too.

### `clyean agents`

Lists every agent in the roster with its identifier, display name, status (`implemented` or `placeholder`), and instruction file when present.

### `clyean plans`

Lists change plans with their version count and latest version file.

### `clyean work`

Lists every journal with its kind, status, phase, and plan.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success.  For a launch, the User Assistant (or `podman`) exited with 0. |
| `1` | An error printed as `error: ...`, a failed launch, a failed workflow in `clyean scaffold`, or `clyean sandbox status` without a provisioned root. |
| `2` | `clyean scaffold --project-type` stopped because the workflow needs information that only the User Assistant can collect. |
| other | The exit code of the User Assistant process, passed through. |
