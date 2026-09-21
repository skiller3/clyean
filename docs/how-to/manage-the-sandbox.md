# Manage the sandbox

Every agent of a project runs in a Podman container whose root filesystem is the directory `.clyean/container-root`.  This guide covers inspecting, rebuilding, and customizing it.  The exact paths and environment inside are in the [sandbox contract](../reference/sandbox-contract.md).

## Inspect

```sh
clyean sandbox status
```

prints the image and digest the root was populated from, when and by which Clyean version it was provisioned, and the harness version inside.  It exits 1 when the root is missing or populated but not provisioned.

```sh
clyean sandbox shell
```

opens a root shell in a maintenance container with the workspace mounted, the same way agents see it.  Packages you install with `apt-get` persist, because the root filesystem is a directory that every container of the project shares.

## Build and rebuild

`clyean` provisions the sandbox on launch when it is missing or outdated, so you rarely need these:

```sh
clyean sandbox build     # populate and provision when missing or outdated, then refresh profiles
clyean sandbox rebuild   # discard .clyean/container-root and provision again
```

Provisioning is considered outdated when the marker file `.clyean/container-root/.clyean-sandbox.json` records an older provisioning schema than the running `clyean`, or an image other than the configured one.  A rebuild discards everything agents installed.

Provisioning runs, inside the root, a package installation with whichever of `apt-get`, `apk`, or `dnf` the image provides (certificates, curl, git, an SSH client, `procps`, Python 3, and a headless Java runtime), installs the pinned PlantUML jar at `/opt/plantuml/`, installs the harness at `/usr/local/bin/clyean`, and verifies all three by running them.  Population itself uses `podman pull`, `podman export`, and `podman unshare tar` so that ownership inside the root is right under rootless Podman; it works on Linux hosts only in this version.

## Change the image

Set `sandbox.image` in `.clyean/project.json` (tracked) or `.clyean/project.local.json` (local only), then run `clyean sandbox rebuild`:

```json
{"sandbox": {"image": "docker.io/library/debian:12"}}
```

Images must provide `apt-get`, `apk`, or `dnf`.  The default is `docker.io/library/ubuntu:latest`.

## Add read-only mounts and Podman arguments

```json
{
  "sandbox": {
    "mounts": ["/home/me/reference-data", "/home/me/specs.pdf"],
    "podmanRunArgs": ["--memory", "8g", "--env-file", "/home/me/.config/clyean/agent-env"]
  }
}
```

Each mount appears read-only at `/mnt/<base name>` in every agent container.  `podmanRunArgs` are appended verbatim to every `podman run` and `podman create` Clyean issues, which is the way to add resource limits, network options, or arbitrary environment variables.  Provider credentials do not need it: host variables matching the credential patterns listed in [Configure agents](configure-agents.md) are passed through automatically, and `sandbox.passthroughEnv` extends the list.  `--mount` and `--image` on the command line set the same values when a project is scaffolded.

## Where the harness comes from

The harness inside the sandbox is the Linux build of Clyean's fork of Oh-My-Pi.  Clyean looks for it in this order:

1. `CLYEAN_HARNESS_BINARY` in the environment.
2. `sandbox.harnessBinary` in `project.json` or `project.local.json`.
3. A file named `clyean-harness-linux-<x64|arm64>` or `clyean-harness` next to the `clyean` executable.
4. The release asset `clyean-harness-linux-<arch>` of the running Clyean version, downloaded from GitHub into `~/.cache/clyean/harness/<version>/` (or `$XDG_CACHE_HOME/clyean`).

The architecture is the one Podman reports for the host.  Development builds and source installs have no matching release, so point option 1 or 2 at a harness you built.

## What is shared and what is not

Shared by every agent of a project: the root filesystem, packages installed into it, the workspace mount, and the passed-through credential variables.  Separate per agent: the harness profile under `/home/<user>/.omp/profiles/<agent-id>/agent/` (settings, MCP servers, sessions, and the credential store, which by default is copied from the User Assistant's profile when a sub-agent starts; see `sandbox.inheritCredentials`).  Separate per unit of work: the sub-agent containers, which are created for one unit of work and removed when it ends.  The User Assistant container persists while it runs and is removed when the User Assistant exits.
