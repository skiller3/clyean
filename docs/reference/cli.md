# Command line

```text
clyean [OPTIONS] [PROMPT]...
clyean [OPTIONS] <COMMAND>
```

Without a command, `clyean` launches the User Assistant in the project's sandbox.  Positional arguments are joined with spaces and sent as the initial prompt.

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
| `-c`, `--continue` | Continue the User Assistant's previous session. |
| `-r`, `--resume [SESSION]` | Resume a session by id prefix or path; without a value, open the session picker. |
| `--model <MODEL>` | Model or configured role for the User Assistant, for this launch only. |
| `-p`, `--print` | Non-interactive mode: send the prompt, print the result, exit.  Runs in a fresh container without a pseudo-terminal. |
| `--no-session` | Do not save the User Assistant session. |
| `--worktrees <yes|no>` | Answer the Git worktree question of a new project instead of being asked.  Without a terminal on standard input the answer defaults to `no`. |
| `--image <IMAGE>` | Image to populate a new project's sandbox from (default `docker.io/library/ubuntu:latest`).  Ignored for scaffolded projects. |
| `--mount <PATH>` | Host path mounted read-only at `/mnt/<base name>` in every agent container; repeatable.  Ignored for scaffolded projects. |

What a launch does, in order: resolve the project; ask the worktree question when the project is not scaffolded; initialize Git when needed and write the agent files and ignore rules; populate and provision the sandbox when missing or outdated; project every agent's profile; start the orchestrator on `$XDG_RUNTIME_DIR/clyean/<project-id>.sock` (or `/tmp/clyean-<uid>/<project-id>.sock`); then either attach to the User Assistant container that is still running from an earlier launch (`podman attach`) or create a new one and start it attached (`podman create`, `podman start --attach --interactive`).  The detach key sequence is Ctrl-p Ctrl-q.  Ctrl-C is passed to the User Assistant rather than terminating the launcher.  When the User Assistant exits, its container is removed; when you detach, it keeps running.

## Commands

### `clyean scaffold [--project-type <TYPE>] [--worktrees <yes|no>] [--image <IMAGE>] [--mount <PATH>]...`

Prepares the host scaffold (Git repository, agent files, ignore rules, sandbox, profiles) without launching the User Assistant.  With `--project-type software-engineering` or `--project-type miscellaneous` on an unscaffolded project it also writes `project.json`, the specification and diagram skeletons, commits them, and runs the Scaffolder agent, streaming its progress to standard output.  It fails when the project is already scaffolded and a project type is given.

### `clyean sandbox status`

Prints the image, digest, provisioning time and version, and harness version recorded for the sandbox root.

### `clyean sandbox build`

Populates and provisions the sandbox root when it is missing or outdated, then refreshes the agent profiles.

### `clyean sandbox rebuild`

Discards `.clyean/container-root` (inside Podman's user namespace, so files owned by container users are removed too) and provisions it again.

### `clyean sandbox shell`

Opens an interactive root shell in a maintenance container with the workspace mounted.

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
