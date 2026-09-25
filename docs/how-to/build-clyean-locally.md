# Build Clyean locally

Build every executable a local `clyean` needs into the repository's `bin/`:

```sh
cargo build-bin      # or its alias: cargo buildlocal
```

It builds, in order:

| File | How | Needs |
| --- | --- | --- |
| `bin/clyean` | `cargo install --path crates/clyean` | A Rust toolchain. |
| `bin/clyean-bridge` | `cargo install --path crates/clyean-bridge` for the static musl target of `<arch>` | The target, once: `rustup target add x86_64-unknown-linux-musl` (or `aarch64-unknown-linux-musl` on ARM64). |
| `bin/clyean-harness-linux-<arch>` | The steps of CI's `build-harness` job for one architecture: `bun install` in `vendor/omp`, the prebuilt native addons of the vendored harness version (downloaded once from npm into `target/xtask/`), and `bun run ci:release:build-binaries` | `bun` on `PATH`, and network access the first time. |

`<arch>` is the architecture Podman runs containers on (`x64` or `arm64`).  The bridge and the harness are Linux executables, so build on Linux, including a distribution under WSL.  The harness takes a few minutes and is about 250 MB.

Like other Cargo commands, it takes `-v` (or `-vv`) or `-q` after its name, and passes the matching flag to every command it runs: `cargo install`, `bun install`, `curl`, and `tar`.  With `-q`, a command's output appears only if it fails.  Cargo's own lines about building and starting the build program take the flag before the name, so `cargo -q build-bin -q` prints nothing at all.

Put `bin/` on your `PATH` to run the result.  Because the bridge and the harness sit beside `bin/clyean`, that `clyean` uses them instead of downloading release assets, and at its next launch in each project it replaces the sandbox's harness with the one you built (see [Manage the sandbox](manage-the-sandbox.md#build-and-rebuild)).

## Rebuild one part

```sh
cargo build-cli      # bin/clyean only, after a change to the Rust code
cargo build-bridge   # bin/clyean-bridge only
```

They take the same verbosity flags, and both leave the other files in `bin/` as they are.  After a change under `vendor/omp`, run `cargo build-bin` again: until you do, `clyean` keeps using the harness from the last full build.

`cargo build` compiles the workspace into `target/debug/` and installs nothing into `bin/`.  The commands behind the aliases live in `crates/xtask`.
