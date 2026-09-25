# Orchestrator protocol

The User Assistant agent runs inside a Podman container while Clyean's orchestration logic runs on the host, inside the `clyean` process that started the container.  They talk over the Unix socket `/run/clyean/orchestrator.sock` inside the container, which the bridge serves: each connection to it travels over the bridge's `podman exec` session to the orchestrator (see [the sandbox contract](sandbox-contract.md)).  This page is the reference for that protocol.

## Transport

Newline-delimited JSON.  The client (the `clyean-orchestration` harness extension) opens one connection per request, writes exactly one request object followed by `\n`, and then reads response and event objects, one per line, until the connection is closed by the server or a terminal event arrives.  The server never handles more than one request per connection.

Every request carries a client-chosen `id` string and a `method`.  Every response echoes the `id`.

```json
{"id":"req-1","method":"ping","params":{}}
{"id":"req-1","result":{"type":"pong","version":"0.1.0"}}
```

Errors replace `result` with `error`:

```json
{"id":"req-1","error":{"code":"unknown_method","message":"unknown method: nope"}}
```

Error codes are `invalid_request`, `unknown_method`, `project_locked`, `not_scaffolded`, `work_not_found`, `request_not_found`, `work_failed`, and `internal`.

## Methods

### `session.lease`

Held once by the User Assistant for the life of its harness process.  The server answers `{"type":"lease"}` and then keeps the connection open, ignoring anything the client writes, until the client closes it.  The connection ends from the host side only when the bridge does, which happens exactly when the `clyean` process that started the container is gone, so the client shuts the harness down when it ends.

```json
{"id":"lease:42","method":"session.lease","params":{}}
{"id":"lease:42","result":{"type":"lease"}}
```

### `ping`

No parameters.  Result: `{"type":"pong","version":"<clyean version>"}`.

### `project.status`

No parameters.  Result:

```json
{
  "type": "project_status",
  "scaffolded": true,
  "project_type": "SOFTWARE_ENGINEERING_PROJECT",
  "locked": false,
  "incomplete_work": [
    {"work_id": "…", "prompt_type": "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION", "session_id": "…", "phase": "…", "started_at": "…"}
  ]
}
```

`project_type` is `null` until the project is scaffolded.

### `project.scaffold`

Parameters: `{"project_type": "SOFTWARE_ENGINEERING_PROJECT" | "MISCELLANEOUS_PROJECT", "session_id": "<User Assistant session id>"}`.

The server locks the project, scaffolds it, unlocks it, and streams progress events followed by a terminal event.  The immediate response is `{"type":"work_accepted","work_id":"…"}`.

### `work.start`

Parameters:

```json
{
  "prompt_type": "SOFTWARE_ENGINEERING_PROJECT_RESEARCH" | "SOFTWARE_ENGINEERING_PROJECT_PLANNING" | "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION",
  "prompt": "<the refined and enriched prompt>",
  "original_prompt": "<the user's own words>",
  "session_id": "<User Assistant session id>",
  "plan": "<optional change plan reference, e.g. 2026-09-21-add-login/v2>"
}
```

Immediate response: `{"type":"work_accepted","work_id":"…"}`.  The connection then streams events (see below) until a terminal event.

### `work.provide_information`

Parameters: `{"work_id": "…", "request_id": "…", "answers": ["…", "…"]}`.  One answer per question of the matching `information_requested` event, in order.  Immediate response: `{"type":"work_resumed","work_id":"…"}`.  The connection then streams events until the next terminal event.

### `work.resume`

Parameters: `{"work_id": "…"}`.  Resumes a unit of work whose journal is incomplete (for example after the host process was restarted).  Immediate response and streaming behavior are the same as `work.provide_information`.  If the work is waiting for information, the first streamed event repeats the pending `information_requested` event.

### `work.cancel`

Parameters: `{"work_id": "…"}`.  Result: `{"type":"work_cancelled","work_id":"…"}`.  Any connection streaming that work receives a `failed` event with code `cancelled`.

## Streamed events

Event objects have an `event` field instead of `id`.  Each carries `work_id` and a monotonically increasing `seq` so a client can discard duplicates after a reconnect.

| `event` | Fields | Meaning |
| --- | --- | --- |
| `progress` | `agent`, `phase`, `text` | Human-readable progress from the orchestrator or a sub-agent.  `agent` is an agent identifier or `orchestrator`; `phase` is a short machine label such as `planning.overview`. |
| `agent_output` | `agent`, `phase`, `text` | A chunk of sub-agent output suitable for streaming to the user. |
| `information_requested` | `request_id`, `questions` (array of strings), `context` | The workflow cannot proceed without answers from the user.  The connection closes after this event; the client answers with `work.provide_information`. |
| `completed` | `summary`, `artifacts` (array of repository-relative paths), `plan` (optional plan reference) | The work finished.  Terminal. |
| `failed` | `code`, `message` | The work failed or was cancelled.  Terminal. |

`information_requested` and the two terminal events are the only events after which the server closes the connection.

## Locking

`project.scaffold` and every `work.*` method that starts or resumes work take the project lock for their duration unless the project uses Git worktrees, in which case locking is a no-op.  A second request that needs the lock while it is held fails immediately with `project_locked`.

## Durability

The orchestrator journals every unit of work in `.clyean/plans/<plan>/journal.json` for planning and implementation, and in `.clyean/work/<work-id>.json` for research.  A journal records the current phase, the answers collected so far, the sub-agent sessions in use, and the Git commit made after each completed step.  `project.status` lists journals that have not reached a terminal phase so the User Assistant can offer to resume them.
