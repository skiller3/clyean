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

Clyean re-projects all of this into the agent's profile, `/home/<user>/.omp/profiles/<agent-id>/agent/` in the project's sandbox root filesystem, on every launch, so edits take effect the next time the agent starts.

## Choose an agent's model

Set the harness's default model role in the settings overlay.  The Programmer, for example:

```json
{
  "tools": {"approvalMode": "yolo"},
  "modelRoles": {"default": "anthropic/claude-opus-5"}
}
```

An agent whose overlay names no default model runs with the User Assistant's current model at the moment the agent starts, which Clyean passes to its harness with `--model`.  The overlay's other model roles and its `retry.fallbackChains` work as they do in the harness, and the agent receives credentials for their providers too (see below).

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

Every agent has its own login store: the credential store in its own profile, which no other agent's container can see.  You sign in only through the User Assistant, for example with `/login`, and Clyean gives each other agent copies of the credentials it needs.

- When a sub-agent starts, Clyean works out the providers of the models its overlay names (the default model or else the User Assistant's, the other model roles, and the fallback chains) and the servers in its `<NAME>.mcp.json` that the User Assistant holds a sign-in for.  It asks the User Assistant for copies of those credentials only, and the copies replace everything the sub-agent's store held, so a sign-out in the User Assistant reaches every agent by its next start.
- A copy lets an agent use a credential but not refresh it.  Before a copy expires, the agent asks Clyean for a new one, and the User Assistant refreshes its own sign-in when needed and hands over a fresh copy, even in the middle of a turn.
- Host environment variables reach only the agents that need them.  The User Assistant receives every host variable whose name matches `*_API_KEY`, `*_API_TOKEN`, or `*_BASE_URL`, the AWS variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_DEFAULT_REGION`, `AWS_PROFILE`, `AWS_BEARER_TOKEN_BEDROCK`), `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_API_VERSION`, `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_CLOUD_PROJECT`, and `GOOGLE_CLOUD_LOCATION`.  A sub-agent receives only the variables the harness reads for its models' providers, for example `OPENAI_API_KEY`, or the AWS variables for Amazon Bedrock.
- When neither the User Assistant nor the host environment can supply the model an agent runs with, Clyean does not start the agent, and the work fails with a message naming the sign-in or variable to add.

Other host variables pass through only when the project names them in `sandbox.passthroughEnv`.  An entry written as a string reaches the User Assistant only; an entry written as an object reaches the agents it lists by identifier (see [the agent identifiers](../reference/sandbox-contract.md#agent-identifiers)):

```json
{"sandbox": {"passthroughEnv": [
  "CORP_PROXY_TOKEN",
  {"name": "GH_TOKEN", "agents": ["software-engineering-director"]}
]}}
```

Names may be exact or `*` glob patterns with a leading or trailing `*`.  Put the setting in `project.local.json` when it should stay off the record.  The harness's auth broker variables (`OMP_AUTH_BROKER_URL` and `OMP_AUTH_BROKER_TOKEN`) never pass through on their own, because a broker hands every client every credential it holds.

`clyean scaffold --project-type` runs the Scaffolder without a User Assistant.  Nothing can then resolve the Scaffolder's needs or supply copies, so the Scaffolder receives the host variables the User Assistant would, and no copies.

## Sign in to an MCP server for another agent

An MCP server configured only for another agent still takes its sign-in from the User Assistant.  In the User Assistant, run:

```text
/clyean sign-in <agent> <server>
```

`<agent>` is the agent's identifier, for example `software-engineering-director`, and `<server>` a server name from that agent's `<NAME>.mcp.json` or its local companion.  Clyean runs the harness's MCP sign-in for the server, shows the address to open, and records the sign-in in the User Assistant's login store, from which the agent receives a copy the next time it starts.  The sign-in's callback listens inside the User Assistant's container, where your browser cannot reach it, so when the browser cannot return to Clyean, paste the address it ended on when Clyean asks for it.

## What the slash commands affect

Inside the User Assistant, `/model`, `/switch`, `/login`, `/logout`, and `/mcp` apply to the User Assistant's profile and say so in their descriptions (`(agent: user-assistant)`).  Sign-ins made there reach the other agents as copies; everything else about the other agents is configured only through the files above.  `clyean agents` lists them and shows which instruction files exist.
