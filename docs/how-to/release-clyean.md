# Release Clyean

Clyean follows trunk-based development: features land on branches cut from `main`, every push is built, and releases are cut from `main` onto `release/v<MAJOR>.<MINOR>` branches with semantic version tags.

## What runs on every push

`.github/workflows/ci.yml` runs on every push to any branch and on pull requests.  It calls the reusable `build.yml`, which runs the test jobs and, only when they pass, the release builds:

| Job | What it does |
| --- | --- |
| `test-rust` | On Ubuntu 24.04 (Podman 4.9.3) and Ubuntu 26.04 (Podman 5.7.0): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, a static build of the bridge, and `cargo test --workspace` with `CLYEAN_PODMAN_TESTS=1` and `CLYEAN_BRIDGE_BINARY` so the Podman-backed tests run on the runner. |
| `test-extensions` | `bun test extensions` for the two harness extensions against stub socket servers. |
| `test-harness` | Installs the vendored harness's dependencies, stages the prebuilt native addons for the vendored version from npm, runs `--version`, and runs the welcome screen tests. |
| `build-cli` | Builds `clyean` for the six host targets: `clyean-linux-x64`, `clyean-linux-arm64`, `clyean-darwin-x64`, `clyean-darwin-arm64`, `clyean-windows-x64.exe`, `clyean-windows-arm64.exe`. |
| `build-bridge` | Builds the static (musl) bridge for Linux x64 and arm64 as `clyean-bridge-linux-x64` and `clyean-bridge-linux-arm64`, and checks that they are statically linked. |
| `build-harness` | Compiles the contained harness with bun for Linux x64 and arm64 as `clyean-harness-linux-x64` and `clyean-harness-linux-arm64`. |
| `publish` | Only with a release tag: writes `SHA256SUMS` and attaches every asset to the GitHub release for that tag. |

Artifacts of non-release runs stay on the workflow run.

## Cut a release

Run the manual **Cut release** workflow (`cut-release.yml`) from `main` in the GitHub Actions tab, or:

```sh
gh workflow run cut-release.yml --ref main
```

It finds the highest existing `v<MAJOR>.<MINOR>.<PATCH>` tag (none counts as `v0.0.0`), creates `release/v<MAJOR>.<MINOR+1>` from `main`, tags its head `v<MAJOR>.<MINOR+1>.0`, pushes both, and dispatches the **Release** workflow for the tag.  It refuses to run when the branch or tag already exists.

## Patch releases

Every push to a `release/**` branch runs `release-patch-tag.yml`.  If the head already carries a release tag (the initial push, or a tag you made by hand), nothing happens.  Otherwise it tags the head with the next patch number of that branch's `MAJOR.MINOR` line and dispatches the Release workflow.  Only the head of a push is tagged, so one push with several commits yields one patch release.

## Major versions

Major bumps are manual by design: create and push `v<MAJOR+1>.0.0` by hand together with the matching `release/v<MAJOR+1>.0` branch.  Tags pushed by a person trigger the Release workflow directly through its `push` trigger; later runs of Cut release continue from that tag.

## Why releases are dispatched explicitly

Tags pushed by a workflow with `GITHUB_TOKEN` do not trigger `push` events.  The cut and patch workflows therefore run `gh workflow run release.yml --ref <tag> -f tag=<tag>`.  The Release workflow refuses to run unless its ref is the tag it was asked to publish.

## Install scripts

`install.sh` and `install.ps1` at the repository root download the asset for the host platform from the latest release (or `--ref <tag>` / `-Ref <tag>`), verify it against `SHA256SUMS`, smoke-test `clyean --version`, and install missing host dependencies (Git, curl, Podman) with the system package manager, printing each command.  `--no-deps` / `-NoDeps` prints instructions instead, and `--source` / `-Source` builds with `cargo install --locked --git https://github.com/skiller3/clyean --tag <tag> clyean`.  Until `clyean.com` serves them, reference the raw GitHub URLs.

Note that the harness binary inside the sandbox is downloaded separately by `clyean` for the version it runs (see [Manage the sandbox](manage-the-sandbox.md)), so a source install needs `CLYEAN_HARNESS_BINARY` or a published release of the same version.
