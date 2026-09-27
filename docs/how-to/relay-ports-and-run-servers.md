# Relay ports and run servers

Agents run inside Podman containers that nothing outside the sandbox can reach.  This guide shows how to open a relay from your computer into the sandbox, how to run a server that other agents reach and that can outlive your `clyean` process, and how to decide in advance which relays need no prompt.  The exact behavior is in the [orchestrator protocol](../reference/orchestrator-protocol.md#relayopen), the [relay allowlist reference](../reference/relay-allowlist.md), and the [sandbox contract](../reference/sandbox-contract.md#server-containers).

## Choose where the server runs

| You want | Run the server | Reach it through |
| --- | --- | --- |
| To open something the User Assistant runs, such as a development server, in your browser | In the User Assistant's own container | A relay into that container (`clyean_relay`) |
| A server that sub-agents use while they work, such as a database or a development server the Programmer tests against | In a server container (`clyean_server`) | Its name on the project network, for example `web:5173` |
| A server that keeps running after you quit Clyean | In a persistent server container | Its name inside the sandbox, and its relays from your computer |

Everything in the User Assistant's container, including its relays, ends when its `clyean` process ends.

## Relay a server in the User Assistant's container

Ask the User Assistant, for example: "Start the development server and let me open it in my browser."  It starts the server in its container and calls `clyean_relay`, and Clyean shows a prompt over the transcript:

```text
Relay request from a process in the User Assistant's container
  Process 812: /usr/local/bin/clyean --cwd /home/me/workspace/app ...   (as it reports itself)
  TCP  127.0.0.1:3000 and [::1]:3000 on this computer  ->  port 5173 in the User Assistant's container
  Reason given: "Preview the sign-up page"

> Allow permanently for this project
  Allow permanently for this session
  Allow once
  Deny
```

Arrow keys or a digit choose and Enter confirms; Escape denies.  An unanswered prompt denies the relay after five minutes.  "Allow permanently" records an entry for exactly this protocol, these ports, and these addresses, so the same request later opens without asking.  Once the relay is open, browse to `http://localhost:3000`.

A script in the User Assistant's container can open the same relay, and gets the result as JSON:

```sh
/usr/local/libexec/clyean/clyean-bridge relay open tcp 3000 5173 --reason "Preview the sign-up page"
```

The server must listen on the container's loopback or on all interfaces.  Close a relay with `clyean_relay`, `clyean-bridge relay close <relay id>`, or `/clyean relays`; all of them close when Clyean exits.

## Run a server container

Ask the User Assistant or any agent for a server, for example: "Run the web app's development server for the Programmer, and let me reach it on port 3000."  The agent calls its `clyean_server` tool, which Clyean answers after you approve any relay the launch asks for.  A server without relays starts without a prompt.

From a script in the User Assistant's container:

```sh
/usr/local/libexec/clyean/clyean-bridge server launch web \
  --port tcp/5173 --relay tcp:3000:5173 --cwd app --reason "Preview the sign-up page" \
  -- npm run dev -- --host 0.0.0.0
```

The launch waits until every declared port listens (60 seconds by default) and returns where the server listens:

```text
Server web is ready: from inside the sandbox at web:5173 (TCP); from this computer at 127.0.0.1:3000 and [::1]:3000.  It stops when this clyean process ends.
```

When a sub-agent launches a server during delegated work, the progress the User Assistant relays says so, and the User Assistant tells you where the server listens and whether it persists.

The server container shares the project's root filesystem and workspace, so it sees the packages and files the agents see.  It gets no sign-ins and none of your host variables.  Its output goes to `.clyean/logs/servers/<name>.log`.

## Reach a server from agents

Every agent container of the project is on the project network, so agents reach a server by its name and port:

```sh
curl http://web:5173/
psql -h db -p 5432 -U postgres
```

The launch result also gives the server's address on the network, which works even where names do not resolve.

## Make the server listen on all interfaces

Traffic on the project network arrives at the server container's network address, not at its loopback, so a server that listens only on `localhost` is reachable from nowhere else.  The launch reports such a port as `loopback_only` and tells the agent what to change.  Common fixes:

| Server | Listen on all interfaces with |
| --- | --- |
| Vite | `vite --host 0.0.0.0` (or `npm run dev -- --host 0.0.0.0`) |
| Next.js | `next dev -H 0.0.0.0` |
| Python's `http.server` | `python3 -m http.server 8000 --bind 0.0.0.0` |
| Flask | `flask run --host 0.0.0.0` |
| Rails | `rails server -b 0.0.0.0` |
| PostgreSQL | `listen_addresses = '*'` |

This does not apply to relays into the User Assistant's container, which reach its loopback.

## Reach a server from other devices

A relay listens on loopback unless it names host addresses.  To reach a development server from a phone on the same network, relay it on every address:

```sh
/usr/local/libexec/clyean/clyean-bridge server launch web --port tcp/5173 \
  --relay tcp:3000:5173@0.0.0.0 -- npm run dev -- --host 0.0.0.0
```

The prompt warns that listening on `0.0.0.0` accepts connections from every network this computer is on.  Your operating system's firewall may ask as well.  On WSL, other devices reach such a relay only in mirrored networking mode, through the Hyper-V firewall; in NAT mode they need a Windows port proxy.

## Keep a server running after Clyean exits

Launch it with `persistent: true` (`--persistent` for `clyean-bridge server launch`).  When `clyean` exits it names the persistent servers still running, and a new session tells you about them.  A persistent server keeps write access to the workspace until you stop it, and it stops with your computer or the Podman machine and does not restart.

List and stop servers from the host or the TUI:

```sh
clyean server list
clyean server stop web
clyean server stop --all
```

`/clyean servers` in the User Assistant lists them and offers to stop one.  Non-persistent servers stop on their own when their `clyean` process exits; if that process is killed, the next launch or `clyean sandbox prune` stops them.

## Allow relays in advance

Allowlist entries open matching relays without a prompt.  From the User Assistant:

```text
/clyean relays allow project tcp 3000-3010 any 127.0.0.1,::1
```

allows TCP relays from host ports 3000 to 3010 on loopback, to any port in the sandbox, for this project.  `/clyean relays` shows the lists and removes entries.  You can also edit `relays.json` in Clyean's configuration directory directly; see [the relay allowlist reference](../reference/relay-allowlist.md).  Set `"enabled": false` there to turn relaying off entirely.

## Troubleshoot

| Symptom | Cause and fix |
| --- | --- |
| `relay_port_unavailable` | The host port is in use (perhaps by another `clyean` process) or below 1024.  Choose another host port; the guest port can stay. |
| `relay_denied` with `no_user_interface` | Nobody could be asked: print mode, or `clyean scaffold --project-type`.  Add an allowlist entry, or run interactively. |
| `relay_protocol_unsupported` | SCTP was requested on macOS or native Windows, without the kernel's SCTP module, or for a server container.  SCTP relays go only into the User Assistant's container on Linux and WSL hosts. |
| A port is `loopback_only` | The server listens only on `localhost`; see "Make the server listen on all interfaces". |
| `ready: false` | The server did not listen before its timeout.  It keeps running: read its log and list it again later. |
| `server_failed` | Its command ended before its ports listened, or Podman could not start it.  The message carries the exit status and the log's last lines; a command that exits with status 0 is daemonizing, so run it in the foreground. |
| `server_exists` | A server of that name runs with other parameters.  Stop it, or use another name. |
| Agents cannot resolve a server's name | Podman lacks aardvark-dns (Clyean warns at launch); use the address the launch returned.  If `sandbox.podmanRunArgs` sets `--network`, it replaces the project network and agents cannot reach servers at all. |
| A server logs connections from an unexpected address | Relayed connections to a server container may arrive from an address inside Podman's rootless network rather than from the client. |
