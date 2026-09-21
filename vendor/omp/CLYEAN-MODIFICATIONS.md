# Clyean modifications to the vendored Oh-My-Pi harness

This directory is a `git subtree` of Oh-My-Pi (upstream `https://github.com/can1357/oh-my-pi`, merged with `git subtree pull --prefix=vendor/omp upstream main --squash`).  Every divergence from upstream is listed here so that each pull can re-apply and re-verify it.  The governing requirements are the "Basic User Experience" and "Oh-My-Pi (omp) Architectural Relationship & Usage" sections of `GENERAL_SPECS.md`, plus the "Preserved harness integration surface" list of its "Herdr Compatibility" section.

Upstream snapshot at the time of writing: 18.2.7 (upstream commit `97f945c130`).

## Rebranding rules

What the user reads is rebranded; where configuration lives is not.

- `packages/utils/src/dirs.ts` gains `PRODUCT_NAME = "Clyean"`, `CLI_NAME = "clyean"`, and `HARNESS_ATTRIBUTION`.  `APP_NAME = "omp"` and `CONFIG_DIR_NAME = ".omp"` stay untouched because they name on-disk paths, log files, XDG directories, `OMP_*`/`PI_*` environment variables, and the HTTP user agent.
- `packages/utils/src/clyean.ts` (new, exported from the package index) provides `getClyeanAgent()` and `agentScopeSuffix()`, which read `CLYEAN_AGENT` from the environment.
- `@oh-my-pi/*` package names, upstream documentation links, code comments, protocol strings, and identifiers that other software matches on are left alone.

## Inventory of changed files

### `packages/utils`

| File | Change |
| --- | --- |
| `src/dirs.ts` | Added `PRODUCT_NAME`, `CLI_NAME`, `HARNESS_ATTRIBUTION` beside `APP_NAME`. |
| `src/clyean.ts` | New: `getClyeanAgent`, `agentScopeSuffix`. |
| `src/index.ts` | Re-exports `./clyean`. |

### `packages/coding-agent/src`

| File | Change |
| --- | --- |
| `cli.ts` | `CLI_NAME` for `process.title`, the OS process name, and the CLI `bin` (drives `--version` = `clyean/<ver>` and `--help` = `clyean v<ver>`, `$ clyean [COMMAND]`).  Removed the `--alias` dispatch branch. |
| `cli-commands.ts` | Removed the pruned subcommand entries (list below).  Reserved-word hints (`clyean extensions is not a management command ...`) use `CLI_NAME`. |
| `cli/args.ts` | Removed the parse arms for `--alias`, `--from-claude`, `--from-codex` (they now surface as unknown flags).  `CLI_NAME` in usage messages.  The `Args` fields stay so `commands/launch.ts` and `main.ts` compile unchanged; they are never set. |
| `cli/flag-tables.ts` | Removed `--from-claude` and `--from-codex` from `VALUELESS_FLAGS` and `SESSION_SOURCE_FLAGS`. |
| `cli/profile-bootstrap.ts` | Removed `--alias` extraction and the `aliasName` result field. |
| `cli/help-extra.ts` | `CLI_NAME` in tool and command examples; dropped the `--alias` tip. |
| `cli/command-help.ts` | `PRODUCT_NAME` in the `acp` description, `CLI_NAME` in the `auth-broker` description. |
| `cli/license.ts` | Header line names the Clyean harness (Oh-My-Pi fork). |
| `cli/auth-broker-cli.ts`, `cli/config-cli.ts`, `cli/grep-cli.ts`, `cli/plugin-cli.ts`, `cli/setup-cli.ts`, `cli/shell-cli.ts`, `cli/web-search-cli.ts`, `commands/models.ts`, `commands/launch-help.ts`, `utils/resume-command.ts` | `APP_NAME` replaced by `CLI_NAME` in usage text, examples, prompts, and the resume hint.  `launch-help.ts` also drops the `--alias`, `--from-claude`, `--from-codex` flag definitions and the alias example. |
| `config/settings-schema.ts` | `startup.checkUpdate` defaults to `false` (the `clyean` host program manages harness upgrades; the check would compare against upstream releases). |
| `slash-commands/builtin-registry.ts` | `CLYEAN_PRUNED_SLASH_COMMANDS` filters the builtin registry (list below). |
| `slash-commands/builtin-modes.ts` | `/model` and `/switch` descriptions, autocomplete descriptions, and status messages carry `agentScopeSuffix()`.  `/security` copy says Clyean-native. |
| `slash-commands/builtin-session.ts` | `/login`, `/logout`, `/mcp` descriptions carry `agentScopeSuffix()`; `/mcp add|remove` usage says `user = this agent` under Clyean. |
| `modes/controllers/mcp-command-controller.ts` | `/mcp help` gains a Scopes section under Clyean (project = shared by every agent, user = this agent's profile).  `PRODUCT_NAME` in OAuth hints. |
| `modes/controllers/command-controller.ts`, `session/exit-diagnostics.ts`, `tools/security-scan.ts`, `blob-broker/uploaders-cloud-drives.ts` | User-facing "OMP" / "Oh My Pi" text uses `PRODUCT_NAME`. |
| `tools/ask.ts`, `modes/controllers/event-controller.ts`, `debug/index.ts` | Terminal notification titles use `PRODUCT_NAME`. |
| `modes/acp/acp-agent.ts` | ACP terminal auth method label and `agentInfo.title` use `PRODUCT_NAME`/`CLI_NAME` (`agentInfo.name` stays `oh-my-pi`, it is an identifier). |
| `session/agent-session.ts` | Power assertion reason uses `PRODUCT_NAME`. |
| `tiny/worker.ts`, `launch/broker.ts`, `lsp/mux/server.ts` | OS process names use `CLI_NAME` (`clyean tiny ...`, `clyean daemon broker`, `clyean lsp mux`).  The "listening on" protocol strings are unchanged. |
| `utils/title-generator.ts` | Terminal title brand mark is the soap emoji instead of π. |

### `packages/tui/src`

| File | Change |
| --- | --- |
| `prompt/welcome.ts` | Box title `clyean v<version>`; full-width attribution band (`renderAttributionLines`, wraps on narrow terminals) between the columns and the bottom border; `PI_LOGO` renamed to `BRAND_LOGO` with the soap-bar art. |
| `theme/symbols.ts` | `icon.omp` is the brand mark: nerd `\u{f157f}` (nf-md-hand_wash, private-use as the Glyph Protocol requires), unicode `🧼`, ascii `(o)`. |
| `glyph-protocol.ts` | Comment only: the confirmation codepoint stays upstream's pi mark because the checked-in bundle carries its outline. |
| `terminal-capabilities.ts` | cmux notification title and OSC 99 app name use `PRODUCT_NAME`. |
| `desktop-notify.ts` | Notification app name uses `PRODUCT_NAME`. |
| `setup/wizard-overlay.ts`, `setup/scenes/outro.ts`, `setup/scenes/splash.ts` | `BRAND_LOGO`, `PRODUCT_NAME` wizard title, splash wordmark `C l y e a n`. |

### Tests

`packages/coding-agent/test`: `acp-builtins.test.ts` (dropped the four `/wt` cases), `acp-initialize-conformance.test.ts`, `blob-uploaders-cloud-drives.test.ts`, `cli-argv-routing.test.ts` (`update` example replaced by `gc`), `flag-tables.test.ts`, `install-command.test.ts`, `power-assertion-options.test.ts`, `profile-bootstrap.test.ts`, `profile-cli.test.ts`, `startup-composer.test.ts`, `terminal-title-state.test.ts`, `collab/registry-smoke.test.ts` (real-CLI collab case skipped), `join-command.test.ts` (skipped), `share.test.ts` (real-CLI share case skipped), `slash-commands/collab-list.test.ts` (skipped), `slash-commands/collab-qrcode.test.ts` (skipped).

`packages/tui/test`: `desktop-notify.test.ts`, `hook-selector-overflow.test.ts`, `notifications.test.ts`.

Skipped tests are kept verbatim under `describe.skip` / `test.skip` with a one-line Clyean note so upstream edits to them still merge.

## Pruned surface

Registration is filtered; the implementation files stay in place so subtree pulls merge cleanly.

### CLI subcommands (`cli-commands.ts`)

| Command | Reason |
| --- | --- |
| `update` | Clyean manages harness upgrades; the check targets upstream releases. |
| `join`, `share`, `collab`, `stream` | Publish session content to relays, share servers, or a public live channel from inside the sandbox. |
| `browser-relay` | Drives the host's Chrome through a relay the container cannot reach. |
| `say` | Host speakers. |
| `grievances` | Reports tool issues to qa.omp.sh. |
| `dry-balance`, `bench`, `if-bench`, `gallery`, `render` | Developer benchmarking and rendering tooling, incoherent for an agent sandbox. |
| `worktree` (`wt`) | The `clyean` host program owns worktrees. |
| `completions`, `__complete` | The `clyean` host CLI owns shell completions. |

### Launch flags

| Flag | Reason |
| --- | --- |
| `--alias` | Shell shortcuts on the host are meaningless inside a container. |
| `--from-claude`, `--from-codex` | Foreign session imports read host-only directories. |

### Slash commands (`builtin-registry.ts`)

| Command | Reason |
| --- | --- |
| `/share`, `/collab`, `/join`, `/leave` | Same as the `share`/`collab`/`join` subcommands. |
| `/live` | Codex realtime voice mode needs the host microphone. |
| `/wt` | The `clyean` host program owns worktrees. |

### Deliberately kept as upstream

`omp-session-<id>.html` export file names, `omp.<date>.<pid>.log` log names, the `Native OMP configuration from ~/.omp and .omp/` discovery source labels, settings descriptions that mention OMP, the DAP `clientName`, ACP `agentInfo.name`, MCP OAuth `client_name`, the `omp/<version>` user agent, the telemetry service name, the `omp lsp mux listening on` and `omp tiny worker listening on` protocol lines, developer-facing errors in the legacy Pi shims, and every string inside pruned modules.

## Per-agent scoping of `/mcp`, `/model`, `/switch`, `/login`, `/logout`

Each Clyean agent runs the harness under its own profile (`OMP_PROFILE=<agent-id>`), so model selection, OAuth credentials, and user-scope MCP servers are already separate per agent.  When `CLYEAN_AGENT` is set the commands say which agent they apply to, for example `Switch model for this session (agent: user-assistant)` and `Model set to anthropic/claude-opus-5 (agent: user-assistant).`  Outside Clyean the suffix is empty and the text is identical to upstream.

## Herdr integration surface checklist

Run from `vendor/omp` after every subtree pull.  Every count must stay above zero and every path must still exist.

```sh
grep -rl --include=*.ts -F 'export function isInsideHerdr' packages/tui/src            # 1 file
grep -rl --include=*.ts -F 'events: EventBus' packages/coding-agent/src               # ExtensionAPI bus (pi.events)
grep -rl --include=*.ts -F 'hasUI: boolean' packages/coding-agent/src                 # ctx.hasUI
grep -rl --include=*.ts -F 'isIdle(): boolean' packages/coding-agent/src              # ctx.isIdle()
grep -rl --include=*.ts -F 'getSessionFile' packages/coding-agent/src                 # ctx.sessionManager.getSessionFile()
grep -rl --include=*.ts -F 'getSessionId' packages/coding-agent/src                   # ctx.sessionManager.getSessionId()
for e in session_start session_switch agent_start agent_end tool_approval_requested tool_approval_resolved tool_execution_start tool_execution_end; do
  grep -rl --include=*.ts -F "\"$e\"" packages/coding-agent/src | wc -l               # each > 0
done
grep -n 'path.join(dir, "extensions")' packages/coding-agent/src/discovery/builtin.ts # extension discovery roots
grep -c 'node:net' packages/coding-agent/src/extensibility/extensions/loader.ts       # 0: the loader does not block Node builtins
```

Notes: `herdr:blocked` never appears in upstream source because the custom event bus is generic (`pi.events.emit(channel, data)`); Clyean's orchestration extension emits it and the reporter subscribes to it.  Extension discovery roots resolve through `getAgentDir()`, so under a profile they are `~/.omp/profiles/<agent>/agent/extensions` and the project `.omp/extensions`.  The Herdr-aware multiplexer detection lives in `packages/tui/src/terminal-multiplexer.ts` and is untouched.

Results on 18.2.7: `isInsideHerdr` 4 files, `events: EventBus` 2, `hasUI: boolean` 16, `isIdle(): boolean` 3, `getSessionFile` 42, `getSessionId` 60, each event name between 4 and 21 files, discovery roots present, `node:net` unmentioned by the loader.

## Verification recipe

```sh
export PATH="$HOME/.bun/bin:$PATH"           # bun 1.4.x
bun install --frozen-lockfile
# Tests need the prebuilt native addons; the npm leaf for the workspace version provides them.
leaf=node_modules/@oh-my-pi/pi-natives-linux-x64
[ -d "$leaf" ] || { mkdir -p "$leaf" && curl -fsSL "https://registry.npmjs.org/@oh-my-pi/pi-natives-linux-x64/-/pi-natives-linux-x64-$(jq -r .version packages/natives/package.json).tgz" | tar -xz -C "$leaf" --strip-components=1; }
cp "$leaf"/pi_natives.linux-x64-*.node packages/natives/native/
bun packages/coding-agent/src/cli.ts --version      # clyean/<version>
bun packages/coding-agent/src/cli.ts --help         # clyean v<version>, $ clyean [COMMAND]
bun packages/coding-agent/src/cli.ts --smoke-test   # smoke-test: ok
bun test packages/tui/test packages/utils/test
bun test packages/coding-agent/test/{profile-bootstrap,profile-cli,flag-tables,cli-argv-routing,cli-command-metadata,install-command,acp-builtins,startup-composer,welcome-history-resize,setup-wizard,terminal-title-state,status-line-brand-fade,acp-initialize-conformance,power-assertion-options,blob-uploaders-cloud-drives,available-commands}.test.ts packages/coding-agent/test/slash-commands
```

Known failures that predate these modifications in this environment (they fail identically on pristine upstream sources with bun 1.4.2 on Linux): the six `glyph protocol probe` cases in `packages/tui/test/glyph-protocol.test.ts`, the five `disposeTerminalTitleState` spinner cases in `packages/coding-agent/test/terminal-title-state.test.ts`, the ConPTY case in `packages/tui/test/resize-conpty-warp.test.ts`, and a `tsgo` type error in `packages/coding-agent/test/judgment-chain.test.ts` (`fetch.preconnect` missing from a mock).
