# Relationship to Oh-My-Pi

Clyean contains a fork of [Oh-My-Pi](https://omp.sh/) and sits above it.  The fork is the harness every agent runs on; Clyean is the orchestration layer that coordinates several of those harness processes.  The welcome screen says it plainly: Clyean forks and wraps the wonderful Oh-My-Pi harness.

## The vendored fork

Upstream lives at `https://github.com/can1357/oh-my-pi` and is vendored at `vendor/omp` as a squashed `git subtree` tracked against the `upstream` remote (push disabled).  Upstream changes are merged with:

```sh
git subtree pull --prefix=vendor/omp upstream main --squash
```

The `--squash` flag is required: the subtree was added squashed, and a pull without it fails with "refusing to merge unrelated histories".  Upstream tags are not fetched so they cannot collide with Clyean's own version tags.

Modifications to the harness are made in place under `vendor/omp`, kept as narrow as practical because every divergence is a conflict to resolve on the next pull.  Every divergence is listed in `vendor/omp/CLYEAN-MODIFICATIONS.md`, with the reason for each pruned command and a grep-based checklist for the Herdr integration surface that must survive every pull.

## What is rebranded

What you read: the process name, `--version` (`clyean/<version>`), `--help` (`clyean v<version>`, `$ clyean [COMMAND]`), the welcome box title (`Clyean v<version>`, with the version of the `clyean` program, not the harness's) and its attribution band, terminal and desktop notification titles, the setup wizard, and the brand mark (a yellow rubber duck in place of π, drawn in half-block characters).  Commands and flags that make no sense inside a sandboxed agent (self-update, collaboration relays, the host browser relay, speech, telemetry to upstream servers, benchmarking tools, worktree and completion management that the host program owns) are unregistered; their source files stay so upstream merges apply cleanly.  With `CLYEAN_AGENT` set, `/model`, `/switch`, `/login`, `/logout`, and `/mcp` state which agent they apply to.

## What deliberately is not rebranded

Where configuration lives: the user-level config root stays `~/.omp` (with profiles at `~/.omp/profiles/<name>`), the project-level directory stays `.omp/`, environment variables keep their `OMP_*` and `PI_*` prefixes, and package identifiers stay `@oh-my-pi/*`.  Keeping these keeps the divergence small and preserves the harness's compatibility with the configuration files it already imports from other tools.  Log file names, export file names, protocol strings, and identifiers other software matches on are also left alone.

## How the harness reaches the sandbox

The harness is compiled by CI with bun into a Linux binary and published as the release assets `clyean-harness-linux-x64` and `clyean-harness-linux-arm64`.  `clyean` installs the asset matching its own version into the sandbox at `/usr/local/bin/clyean`, and replaces it at launch whenever the harness it would install changes, so inside the container the harness is simply `clyean`.  [Manage the sandbox](../how-to/manage-the-sandbox.md) describes the resolution order and how to substitute a locally built harness.
