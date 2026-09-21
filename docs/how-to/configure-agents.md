# Configure agents

Every Clyean agent runs the contained harness under its own profile, so models, credentials, MCP servers, and instructions are scoped to one agent at a time.  This guide covers the files under `.clyean/agents` that control them.

## The files

For each implemented agent, `<NAME>` being the upper-case identifier (`USER_ASSISTANT`, `SCAFFOLDER`, `SOFTWARE_ENGINEERING_DIRECTOR`, `SPECIFIER`, `SOFTWARE_ARCHITECT`, `PROGRAMMER`):

| File | Purpose | Local-only companion |
| --- | --- | --- |
| `AGENTS__<NAME>.md` | Baseline instructions, written at scaffold time and never overwritten by Clyean afterwards.  Becomes the profile's `AGENTS.md`. | `AGENTS__<NAME>.local.md`, appended after a blank line |
| `<NAME>.omp.json` | Harness settings overlay in the harness's `config.yml` schema (JSON is valid YAML).  Passed to the harness with `--config`, so it merges last and wins over the profile's own persisted settings. | `<NAME>.omp.local.json`, deep-merged over the tracked file |
| `<NAME>.mcp.json` | MCP servers in the harness's `mcp.json` shape (`{"mcpServers": {...}}`).  Deep-merged into the profile's `mcp.json` at every launch, so servers added from inside the harness survive. | `<NAME>.mcp.local.json`, deep-merged over the tracked file |

Deep merge means objects merge key by key, other values are replaced, and a JSON `null` removes a key.  The `.local.*` companions are ignored by Git through `.clyean/.gitignore`; use them for machine-specific or private settings.

Clyean re-projects all of this into `.clyean/container-root/home/<user>/.omp/profiles/<agent-id>/agent/` on every launch, so edits take effect the next time the agent starts.

## Pin an agent's model

Set the harness's default model role in the settings overlay.  The Programmer, for example:

```json
{
  "tools": {"approvalMode": "yolo"},
  "modelRoles": {"default": "anthropic/claude-opus-5"}
}
```

Keep `tools.approvalMode` at `yolo` for the sub-agents: nobody is there to approve their tool calls, and Clyean also passes `--approval-mode yolo` on their command line.  The User Assistant's overlay starts empty so that the harness default applies and `/model` switches persist in its profile.

## Give an agent MCP servers

```json
{
  "mcpServers": {
    "jira": {"type": "stdio", "command": "npx", "args": ["-y", "@example/jira-mcp"]}
  }
}
```

Put it in `<NAME>.mcp.json` to track it with the project, or in `<NAME>.mcp.local.json` when it carries a credential.  Inside the User Assistant, `/mcp add ... --scope user` writes to that agent's own profile, which the merge keeps; `--scope project` writes `.omp/mcp.json` in the project directory, which every agent of the project sees.

## Enhance instructions

Write `AGENTS__<NAME>.local.md` next to the tracked file.  Its content is appended verbatim after the baseline, so the agent reads both.  To change the tracked baseline for everyone, edit `AGENTS__<NAME>.md` itself and commit it.

## Credentials for the agents

Provider credentials reach the agents two ways, and both are on by default.

1. Environment passthrough.  Host variables whose names match `*_API_KEY`, `*_API_TOKEN`, or `*_BASE_URL`, the AWS variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_DEFAULT_REGION`, `AWS_PROFILE`, `AWS_BEARER_TOKEN_BEDROCK`), `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_API_VERSION`, `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, `OMP_AUTH_BROKER_URL`, and `OMP_AUTH_BROKER_TOKEN` are copied into every agent container.  Add names or `*` glob patterns (a leading or trailing `*`) with `sandbox.passthroughEnv`:

   ```json
   {"sandbox": {"passthroughEnv": ["MY_PROVIDER_SECRET", "CORP_*"]}}
   ```

   Export `ANTHROPIC_API_KEY` (or the variable your provider reads) before running `clyean` and every agent can use it.

2. Credential inheritance.  Unless `sandbox.inheritCredentials` is `false`, each sub-agent starts with a copy of the User Assistant's credential store (`agent.db` and its journal files) taken from the User Assistant's profile.  A `/login` performed in the User Assistant therefore applies to the other agents from their next start.  Set `inheritCredentials` to `false` when an agent must hold different credentials; that agent then needs its own login, for example through a passed-through variable or by editing its profile.

Put either setting in `project.local.json` when it should stay off the record.  When neither mechanism supplies a credential, a sub-agent's harness exits with `No models available` and the work fails with that message.

## What the slash commands affect

Inside the User Assistant, `/model`, `/switch`, `/login`, `/logout`, and `/mcp` apply to the User Assistant's profile and say so in their descriptions (`(agent: user-assistant)`).  Other agents are configured only through the files above; `clyean agents` lists them and shows which instruction files exist.
