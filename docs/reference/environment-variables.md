# Environment variables

Variables read by the `clyean` host program.  The variables an agent sees inside its container are listed in the [sandbox contract](sandbox-contract.md).

| Variable | Effect |
| --- | --- |
| `CLYEAN_HARNESS_BINARY` | Path of the harness binary to install into the sandbox, taking precedence over `sandbox.harnessBinary`, a sibling of the executable, and the release download. |
| `CLYEAN_BRIDGE_BINARY` | Path of the static Linux bridge executable to mount into User Assistant containers, taking precedence over a `clyean-bridge-linux-<arch>` or `clyean-bridge` file beside the `clyean` executable and the release download (`clyean-bridge-linux-<arch>` of the running version, verified against `SHA256SUMS` and cached).  `cargo build-bridge` installs one beside `bin/clyean` for development builds. |
| `CLYEAN_LOG` | A `tracing` filter for diagnostics on standard error, for example `info` or `clyean::orchestrator=debug`.  `--verbose` sets `info` when the variable is unset; the default is `warn`. |
| `CLYEAN_PODMAN_TESTS` | Set to `1` to run the Podman-backed integration tests of the Rust workspace (used by CI).  They also need `CLYEAN_BRIDGE_BINARY`, and `CLYEAN_TEST_IMAGE` overrides the image they populate a test root filesystem from (default `docker.io/library/alpine:3.22`). |
| `XDG_CACHE_HOME` | When set, downloads (the harness binary, the bridge, and the PlantUML jar) are cached under `$XDG_CACHE_HOME/clyean/`; otherwise under `~/.cache/clyean/`. |
| `HOME` | Used for the default cache location. |
| `USER`, `USERNAME` | The host user name, sanitized into the sandbox user name (`/home/<name>`).  Falls back to `clyean`. |
| `TERM`, `COLORTERM`, `TERM_PROGRAM` | Copied into the User Assistant container so the terminal UI renders for your terminal. |
| `PATH` | Searched for `podman`, `git`, and (inside a Herdr pane) `herdr`. |

## Passed into every agent container

Host variables whose names match one of these patterns are copied into every agent container unchanged; `sandbox.passthroughEnv` in `project.json` adds exact names or patterns with a leading or trailing `*`.

| Pattern or name | Purpose |
| --- | --- |
| `*_API_KEY`, `*_API_TOKEN`, `*_BASE_URL` | Provider keys, tokens, and endpoints, for example `ANTHROPIC_API_KEY` or `OPENAI_BASE_URL`. |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_DEFAULT_REGION`, `AWS_PROFILE`, `AWS_BEARER_TOKEN_BEDROCK` | AWS and Bedrock credentials. |
| `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_API_VERSION` | Azure OpenAI. |
| `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION` | Google Cloud.  Note that a credentials file path must also be reachable inside the container, for example through a mount. |
| `OMP_AUTH_BROKER_URL`, `OMP_AUTH_BROKER_TOKEN` | The harness's auth broker. |

## Herdr

| Variable | Effect |
| --- | --- |
| `HERDR_ENV` | Must be `1` for Clyean to report to Herdr. |
| `HERDR_PANE_ID` | The pane to report for; required for reporting. |
| `HERDR_SOCKET_PATH` | The host socket to mount into the User Assistant container; required for reporting. |
| `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID` | Propagated into the container when present; also count as pane-detection signals. |
| `HERDR_BIN_PATH` | The `herdr` executable to mount read-only; when unset, `herdr` on `PATH` is used if it exists. |

Inside the container, `CLYEAN_HERDR_IDLE_DEBOUNCE_MS` (default 250) and `CLYEAN_HERDR_RETRY_GRACE_MS` (default 2500) tune the reporter; set them through `sandbox.podmanRunArgs` (`--env NAME=VALUE`).

## Install scripts

| Variable | Effect |
| --- | --- |
| `CLYEAN_INSTALL_DIR` | Directory that receives the executable: default `~/.local/bin` (`install.sh`) or `%LOCALAPPDATA%\clyean` (`install.ps1`). |
