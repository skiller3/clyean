# Orchestrator protocol

The User Assistant agent runs inside a Podman container while Clyean's orchestration logic runs on the host, inside the `clyean` process that started the container.  They talk over the Unix socket `/run/clyean/orchestrator.sock` inside the container, which the bridge serves: each connection to it travels over the bridge's `podman exec` session to the orchestrator (see [the sandbox contract](sandbox-contract.md)).  This page is the reference for that protocol.

## Transport

Newline-delimited JSON.  A client (the `clyean-orchestration` harness extension, `clyean-bridge relay` and `clyean-bridge server`, or any other process in the User Assistant's container) opens one connection per request, writes exactly one request object followed by `\n`, and then reads response and event objects, one per line, until the connection is closed by the server or a terminal event arrives.  The server never handles more than one request per connection.  The lease is the one exception: after it is granted, requests travel on it in both directions (see "Requests on the lease").

Every request carries a client-chosen `id` string and a `method`.  Every response echoes the `id`.

```json
{"id":"req-1","method":"ping","params":{}}
{"id":"req-1","result":{"type":"pong","version":"0.1.0"}}
```

Errors replace `result` with `error`, which may add a `reason`:

```json
{"id":"req-1","error":{"code":"unknown_method","message":"unknown method: nope"}}
{"id":"a1","error":{"code":"relay_denied","message":"the user denied the relay; do not ask for it again unless the user asks","reason":"denied"}}
```

| Code | Meaning |
| --- | --- |
| `invalid_request` | The request or its parameters are malformed. |
| `unknown_method` | No such method. |
| `project_locked` | Another Clyean process holds the project lock. |
| `not_scaffolded` | The method needs a scaffolded project. |
| `work_not_found`, `request_not_found` | No such unit of work, or no such pending information request. |
| `work_failed`, `internal` | The work, or the orchestrator, failed. |
| `credentials_unavailable` | A sub-agent cannot be given the credentials its model needs; it reaches a client as the code of a `failed` event. |
| `relay_disabled` | Relaying is turned off in the relay file. |
| `relay_invalid` | A relay parameter is malformed or out of range. |
| `relay_protocol_unsupported` | The protocol cannot be relayed here: SCTP on macOS or native Windows, without the kernel's SCTP module, or to a server container. |
| `relay_port_unavailable` | The host port is in use, or needs privileges `clyean` lacks. |
| `relay_address_unavailable` | A host address is not an address of this host. |
| `relay_denied` | The user denied the relay (`reason` `denied`), the prompt timed out (`timed_out`), or no user interface could ask (`no_user_interface`). |
| `relay_limit` | A relay, connection, flow, or prompt limit is reached. |
| `relay_unavailable` | The User Assistant's bridge is not open, or this process cannot relay. |
| `server_invalid` | A server parameter is malformed. |
| `server_exists` | A running server has the name, with different parameters. |
| `server_limit` | Eight server containers of the project already run. |
| `server_failed` | The server could not start, or its command ended before its ports listened; the message says why. |
| `lease_held` | This bridge session already granted its lease. |
| `lease_only` | The method is accepted only on the lease. |

Methods that close or stop something succeed when it is already gone.

## Methods

### `session.lease`

Held once by the User Assistant for the life of its harness process.  The first `session.lease` of a bridge session is the only one granted: the extension takes it before any agent command runs, and every later request is refused with `lease_held`.  The server answers `{"type":"lease"}` and then keeps the connection open until the client closes it.  The connection ends from the host side only when the bridge does, which happens exactly when the `clyean` process that started the container is gone, so the client shuts the harness down when it ends.

```json
{"id":"lease:42","method":"session.lease","params":{}}
{"id":"lease:42","result":{"type":"lease"}}
```

#### Requests on the lease

Once the lease is granted, either side writes requests on the lease connection, and the other answers each with the request's `id`, in any order, with a `result` or an `error`.

The orchestrator asks the User Assistant:

| Method | Parameters | Result |
| --- | --- | --- |
| `credentials.resolve`, `credentials.variables`, `credentials.copies` | See "Credential requests on the lease". | |
| `relay.approve` | `requester` (`{"kind": "agent", "agent", "display"}`, `{"kind": "sign_in", "provider"}`, or `{"kind": "process", "pid", "command"}`), `relay` (`protocol`, `host_port`, `guest_port`, `host_addresses`), `server` (`name`, `command`, `cwd`, `persistent`, or `null`), `reason`, `session_available`, `permanent_available`, `file_problem`, `warning` | `decision`: `project`, `session`, `once`, or `deny`, with `reason` (`denied`, `timed_out`, or `no_user_interface`) for a denial |
| `session.current` | none | `session_id`, or `null` before the session has one |

The extension asks the orchestrator for sign-in relays and allowlist edits, which the orchestrator accepts nowhere else (`lease_only`), and closes the relays it opened:

| Method | Parameters | Result |
| --- | --- | --- |
| `relay.open` | As for `relay.open` below, plus `purpose: "sign-in"` and `provider` | As below.  The host opens `http://localhost:<host port>/launch` in the user's browser once the relay opens. |
| `relay.close` | `relay_id` | `{"type": "relay_closed", "relay_id"}` |
| `relay.allowlist.list` | `session_id` | `{"type": "relay_allowlist", "project_dir", "enabled", "project", "session", "file", "file_problem", "relays"}`, where each entry carries its `text` |
| `relay.allowlist.add` | `scope` (`project` or `session`), `entry` (see [the relay allowlist](relay-allowlist.md)), `session_id` | The listing, as for `relay.allowlist.list` |
| `relay.allowlist.remove` | `scope`, `index` (from zero), `session_id` | The listing |

#### Credential requests on the lease

The User Assistant answers these with an `error` of code `not_ready` before its session starts, otherwise `credentials`.

| Method | Parameters | Result |
| --- | --- | --- |
| `credentials.resolve` | `patterns`: model patterns from a sub-agent's configuration | `models`: each pattern mapped to `{"provider", "model"}` or `null` when it names nothing known, where `model` is a `--model` selector, or `null` for a pattern that names a provider rather than a model; `current`: the User Assistant's current model in the same form, or `null` |
| `credentials.variables` | `providers` | `variables`: each provider mapped to the environment variable names the harness reads for it |
| `credentials.copies` | `agent`, `providers`, `mcp_servers` | `providers`: copies of the stored credentials per provider; `mcp`: copies of MCP server credentials keyed by the receiving agent's credential identifier; `unavailable`: the requested providers the User Assistant has no credential for |

```json
{"id":"credentials-3","method":"credentials.copies","params":{"agent":"programmer","providers":["anthropic"],"mcp_servers":[]}}
{"id":"credentials-3","result":{"type":"credential_copies","providers":{"anthropic":[{"type":"oauth","access":"…","refresh":"","expires":1790000000000}]},"mcp":{},"unavailable":[]}}
```

Copies never carry refresh tokens or client secrets; see [the sandbox contract](sandbox-contract.md#credentials) for how sub-agents receive and renew them.

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

### `relay.open`

Opens a relay from host addresses to a port on the User Assistant's container's loopback, once the user's allowlist covers it or the user allows it.  Parameters:

| Parameter | Meaning |
| --- | --- |
| `protocol` | `tcp`, `udp`, or `sctp` |
| `host_port` | 1 to 65535 |
| `guest_port` | 1 to 65535; it may differ from `host_port` |
| `host_addresses` | Optional, one to four addresses; `0.0.0.0` means every IPv4 address and `::` every IPv6 address.  Default: `127.0.0.1`, plus `::1` when the host has IPv6 loopback |
| `reason` | Optional text the prompt shows as the requester's own words |

```json
{"id":"a1","method":"relay.open","params":{"protocol":"tcp","host_port":3000,"guest_port":5173,"reason":"Preview the sign-up page"}}
{"id":"a1","result":{"type":"relay_open","relay_id":"r-3","protocol":"tcp","host_port":3000,"guest_port":5173,"host_addresses":["127.0.0.1","::1"],"approval":"project","endpoints":["127.0.0.1:3000","[::1]:3000"]}}
```

`approval` is `project` or `session` when an allowlist entry, or a permanent answer, allowed the relay, and `once` otherwise.  Repeating a request for an open relay returns that relay.  Relays close with `relay.close`, or when the container ends with its `clyean` process.

### `relay.close`

Parameters: `{"relay_id": "…"}`.  Result: `{"type":"relay_closed","relay_id":"…"}`.

### `relay.list`

No parameters.  Result: `{"type":"relay_list","relays":[…]}`, each relay as `relay.open` reports it plus its `requester`, `opened_at`, and open `connections`.

### `server.launch`

Launches a server container, once every relay it asks for is approved or allowlisted, and waits until it listens.  Parameters:

| Parameter | Meaning |
| --- | --- |
| `name` | Lower-case letters, digits, and inner hyphens, up to 32 characters, other than `localhost`; unique among the project's running servers and the server's name on the project network |
| `command` | Shell command, run with `sh -c`, that runs the server in the foreground |
| `cwd` | Optional directory, relative to the project directory or absolute, inside the workspace; default the project directory |
| `persistent` | Optional; `false` (default) stops the server when the launching `clyean` process ends, `true` keeps it running until stopped |
| `ports` | One to eight `{"protocol", "port"}` objects: every port the server listens on in its container |
| `relays` | Optional, up to four `{"protocol", "host_port", "guest_port", "host_addresses"}` objects, as for `relay.open`, where `guest_port` is one of `ports` with the same protocol and `protocol` is `tcp` or `udp` |
| `ready_timeout_seconds` | Optional, 1 to 300, default 60: how long to wait for the ports to listen |
| `reason` | Optional text the relay prompts show |

```json
{"id":"a2","method":"server.launch","params":{"name":"web","command":"npm run dev -- --host 0.0.0.0","cwd":"app","persistent":true,"ports":[{"protocol":"tcp","port":5173}],"relays":[{"protocol":"tcp","host_port":3000,"guest_port":5173}]}}
{"id":"a2","result":{"type":"server_started","name":"web","ready":true,"persistent":true,"container":"clyean-3f2a9c1b7d4e-server-web","log":".clyean/logs/servers/web.log","sandbox_endpoints":[{"protocol":"tcp","host":"web","address":"10.89.0.7","port":5173,"state":"listening"}],"relays":[{"protocol":"tcp","host_port":3000,"guest_port":5173,"host_addresses":["127.0.0.1","::1"],"approval":"once","endpoints":["127.0.0.1:3000","[::1]:3000"]}],"summary":"Server web is ready: from inside the sandbox at web:5173 (TCP); from this computer at 127.0.0.1:3000 and [::1]:3000.  It keeps running after Clyean exits, until stopped."}}
{"id":"a3","method":"server.launch","params":{"name":"db","command":"postgres -D /var/lib/postgresql/data","ports":[{"protocol":"tcp","port":5432}]}}
```

The second launch has no relays, so it starts without asking anyone.

- `sandbox_endpoints` tell any member of the project network where to send traffic: the server's name (`host`), its address on the network, and each declared port.  Each endpoint's `state` is `listening`; `loopback_only`, with a `message` saying to listen on all interfaces; or `not_listening`.  `ready` is true when every endpoint is `listening`.
- `relays` tell anything outside the sandbox which host addresses reach the server, and how each relay was allowed.
- A server not ready when the timeout passes still answers `server_started`, with `ready: false`, and keeps running.  A command that ends first answers `server_failed`, whose message carries the exit status and the last lines of the log.
- A launch whose relays are not all approved starts nothing and answers the relay error.  A repeated launch with the same name and parameters returns the running server; other parameters answer `server_exists`.

### `server.stop`

Parameters: `{"name": "…"}`.  Result: `{"type":"server_stopped","name":"…"}`.

### `server.list`

No parameters.  Result: `{"type":"server_list","servers":[…]}`, each server with the fields of `server_started` (its endpoints probed now) plus `command`, `cwd`, `requested_by` (an agent identifier), `started_at`, and `launched_by_this_process`.

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
