# Run Clyean inside Herdr

[Herdr](https://herdr.dev/) is a terminal workspace manager for coding agents.  Clyean reports the User Assistant's state and session to Herdr on its own, without any Herdr-side support for Clyean, and it does so through a deliberate hole in the sandbox that you should understand before relying on it.

## Start Clyean in a Herdr pane

Nothing changes on the command line: run `clyean` in a Herdr pane as you would anywhere else.  Clyean treats `HERDR_ENV=1`, or any of `HERDR_PANE_ID`, `HERDR_TAB_ID`, and `HERDR_WORKSPACE_ID`, as the sign that it runs inside a pane.  It reports only when `HERDR_ENV=1` and both `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` are present.  The client-side variables (`HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_SESSION`, `HERDR_CONFIG_PATH`, `HERDR_CLIENT_SOCKET_PATH`) are never treated as detection signals on their own.  Outside a pane every Herdr feature is inert.

You do not run `herdr integration install`; that command has no Clyean integration and Clyean does not need one.  Do not install Herdr's `omp` integration into a Clyean project either: only one reporter may claim a pane, and Clyean's reporter identifies itself as source `custom:clyean` with the agent label `clyean`.

## What Clyean mounts and propagates

For the User Assistant container only, Clyean:

- Bind-mounts the host socket named by `HERDR_SOCKET_PATH` read-write at `/run/herdr/herdr.sock` and sets `HERDR_SOCKET_PATH` to that path inside the container.  Named Herdr sessions keep their socket under `~/.config/herdr/sessions/<name>/`, which is why the path is resolved from the variable rather than assumed.
- Propagates `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, and `HERDR_WORKSPACE_ID`.
- Bind-mounts the `herdr` executable read-only at `/usr/local/bin/herdr` and sets `HERDR_BIN_PATH` when the executable is found through `HERDR_BIN_PATH` or on `PATH`.  Its absence is not an error.

Every other agent container gets none of this, and the reporter extension is installed only in the User Assistant's profile.

## The sandboxing exception

Clyean's sandbox promise is that agents cannot modify anything outside the workspace mount.  The Herdr socket is an explicit exception: it grants the User Assistant the full Herdr socket API, unfiltered, including workspace, tab, and pane mutation (`pane.split`, `pane.send_input`, `pane.run`, `agent.start`) and control of panes that do not belong to the project.  An instruction that convinces the User Assistant to use the `herdr` executable or the socket can therefore affect your other panes.  Treat the User Assistant inside Herdr with the same trust you give any agent you run directly in a Herdr pane.  The other route out of the sandbox is remote repository access by the Product Director agent, which is not implemented yet.

## What the sidebar shows

The reporter sets the pane's displayed agent to `Clyean` and its title to the project directory name (`pane.report_metadata`), and reports the session (`pane.report_agent_session`) on session start, on `/new`, `/resume`, and `/fork`, and at the start of every turn.  The session path is translated to its host path (the workspace mount maps to your workspace directory, everything else to `.clyean/container-root`); paths under `/run` and `/mnt` have no host equivalent and fall back to the session id.

States (`pane.report_agent`):

- `working` from the start of a turn until it ends.  Turns that the harness will automatically continue do not count as ended.
- `blocked` while a tool approval is outstanding, while the `ask` tool waits for you, and while the User Assistant waits on the orchestrator (the label reads `Awaiting answers for the orchestrator`) or has a change plan ready for you to review (`Change plan ready for review`).  Overlapping waits are reference counted, so resolving one keeps the pane blocked until all are resolved.
- `idle` otherwise, debounced by 250 ms so brief gaps between turns do not flicker.

Retryable provider failures (overload, rate limiting, 5xx responses, transport resets) hold `working` for 2,500 ms before falling through to `blocked`, so automatic retries are not shown as idle.  Every report carries an increasing sequence number and reports are sent one at a time.  `pane.release_agent` is sent only when the User Assistant genuinely quits (`/exit`, Ctrl-D, a signal); `/new`, `/resume`, `/fork`, `/reload`, and `/restart` keep the pane claimed.

Herdr being slow or absent never blocks a turn: each report has a bounded connect and write timeout with one retry, and failures are dropped.

## Tuning

Two timings can be overridden with environment variables inside the container.  Add them to the project's Podman arguments, for example in `.clyean/project.local.json`:

```json
{"sandbox": {"podmanRunArgs": ["--env", "CLYEAN_HERDR_IDLE_DEBOUNCE_MS=500", "--env", "CLYEAN_HERDR_RETRY_GRACE_MS=4000"]}}
```

Values are milliseconds; invalid or negative values fall back to the defaults.

## Troubleshooting

- Nothing in the sidebar: confirm the three variables are set in the pane (`env | grep HERDR`), then confirm the socket is mounted (`clyean sandbox shell`, then `ls -la /run/herdr`).  Clyean requires `HERDR_ENV=1`; a launcher that strips it also strips reporting.
- The pane is claimed by `omp`: a Herdr `omp` integration file exists under the User Assistant's profile.  Clyean never ships it; remove `~/.omp/profiles/user-assistant/agent/extensions/herdr-omp-agent-state.ts` from `.clyean/container-root/home/<user>/`.
- Herdr ships first-class Clyean support later: the reporter stays silent when `HERDR_CLYEAN_INTEGRATION=1` is set or when `herdr-clyean-agent-state.ts` exists in the profile's extensions directory, so the two never report twice for one pane.
