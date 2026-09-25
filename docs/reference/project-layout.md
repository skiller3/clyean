# Project layout

Everything Clyean keeps in a project lives under `.clyean/`, beside the project's own `.git`.

## Paths

| Path | Tracked by Git | Who writes it | Contents |
| --- | --- | --- | --- |
| `.clyean/project.json` | yes | Clyean at scaffold time; you afterwards | Top-level configuration (schema below).  Its existence means the project is scaffolded. |
| `.clyean/project.local.json` | no (`*.local.json`) | you | Local overrides deep-merged over `project.json`. |
| `.clyean/.gitignore` | yes | Clyean | The ignore rules Git did not already honor, from `/container-root/`, `*.local.json`, `*.local.md`, `/lock`, `/logs/`, `/work/`.  Clyean checks each rule with `git check-ignore` and never edits a `.gitignore` you maintain. |
| `.clyean/agents/AGENTS__<NAME>.md` | yes | Clyean once, then you | Baseline instructions of one agent, projected into the agent's profile as `AGENTS.md`. |
| `.clyean/agents/AGENTS__<NAME>.local.md` | no | you | Appended to the baseline when the profile is projected. |
| `.clyean/agents/<NAME>.omp.json` | yes | Clyean once, then you | Harness settings overlay (`config.yml` schema), applied last through `--config`. |
| `.clyean/agents/<NAME>.omp.local.json` | no | you | Deep-merged over the tracked overlay. |
| `.clyean/agents/<NAME>.mcp.json` | yes | Clyean once, then you | MCP servers, deep-merged into the profile's `mcp.json`. |
| `.clyean/agents/<NAME>.mcp.local.json` | no | you | Deep-merged over the tracked seed. |
| `.clyean/SPECS.md` | yes | Scaffolder, Specifier | Current requirements of the project. |
| `.clyean/architecture/<type>.puml` | yes | Scaffolder, Software Architect | One PlantUML source per UML diagram type: `class`, `object`, `package`, `component`, `composite-structure`, `deployment`, `profile`, `use-case`, `activity`, `state-machine`, `sequence`, `communication`, `interaction-overview`, `timing`. |
| `.clyean/architecture/<type>.pdf` | yes | Clyean (rendered) | Rendered from the source of the same name; never edited by hand and never written when a render fails. |
| `.clyean/plans/<YYYY-MM-DD>-<slug>/v<N>.md` | yes | Director, Specifier, Architect | Change plan versions; see [Change plans](change-plans.md). |
| `.clyean/plans/<plan>/journal-<work-id>.json` | yes | Clyean | Journal of a planning or implementation unit of work. |
| `.clyean/plans/.gitkeep` | yes | Clyean | Keeps the directory in Git. |
| `.clyean/work/<work-id>.json` | no | Clyean | Journals of research and scaffolding work. |
| `.clyean/lock` | no | Clyean | Advisory file lock held while a unit of work runs (unused when worktrees are enabled). |
| `.clyean/logs/` | no | reserved | Reserved for logs. |
| `.clyean/container-root/` | no | Clyean and the agents | The sandbox root filesystem; see the [sandbox contract](sandbox-contract.md).  Masked with an empty `tmpfs` inside the containers. |
| `.clyean/container-root/.clyean-sandbox.json` | no | Clyean | Provisioning marker: image, digest, provisioning schema version, Clyean version, time, harness version. |

`<NAME>` is the agent identifier in upper snake case, for example `SOFTWARE_ENGINEERING_DIRECTOR`.

## `project.json`

Keys are camelCase.

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `schemaVersion` | integer | `1` | Version of this file's shape. |
| `clyeanVersion` | string | the scaffolding version | Clyean version that generated the scaffold. |
| `scaffoldedAt` | string | scaffold time | UTC RFC 3339 timestamp of the scaffold. |
| `projectType` | `SOFTWARE_ENGINEERING_PROJECT` or `MISCELLANEOUS_PROJECT` | decided by the User Assistant | The project type; never re-derived once set. |
| `workspace.path` | string | the project directory | Absolute host path of the workspace directory, mounted read-write into the sandbox. |
| `git.useWorktrees` | boolean | the answer to the worktree question | When true, project locking is a no-op. |
| `sandbox.image` | string | `docker.io/library/ubuntu:latest` | Image the root filesystem is populated from.  Changing it makes the sandbox outdated. |
| `sandbox.mounts` | array of strings | `[]` | Host paths mounted read-only at `/mnt/<base name>`. |
| `sandbox.podmanRunArgs` | array of strings | `[]` | Extra arguments appended to every `podman run`. |
| `sandbox.harnessBinary` | string, optional | absent | Host path of the harness binary to install into the sandbox instead of the release download. |
| `sandbox.passthroughEnv` | array of strings | `[]` | Extra host environment variable names, or `*` glob patterns, passed into every agent container in addition to the built-in credential patterns. |
| `sandbox.inheritCredentials` | boolean | `true` | Copy the User Assistant's credential store into each sub-agent's profile when the sub-agent starts. |

Example with a local override that adds a mount and a memory limit:

```json
{"sandbox": {"mounts": ["/home/me/reference-data"], "podmanRunArgs": ["--memory", "8g"]}}
```

Deep-merge semantics: objects merge key by key, arrays and scalars in the local file replace the tracked value, and `null` removes a key.
