# Manage the sandbox

Every agent of a project runs in a Podman container whose root filesystem is the project's sandbox root filesystem, a directory that Clyean keeps beside Podman's own data.  This guide covers inspecting, rebuilding, and customizing it.  The exact paths and environment inside are in the [sandbox contract](../reference/sandbox-contract.md).

## Where the root filesystem lives

At the first launch in a project, Clyean records a random sandbox identifier in `.clyean/sandbox.local.json`, a local-only file Git ignores.  The root filesystem is the directory named by that identifier in Clyean's roots directory, which sits beside Podman's `containers` data directory: `~/.local/share/clyean/roots/<identifier>` on Linux and Linux on WSL, and the same place inside the Podman machine on native Windows and macOS.  Keeping it there puts it on a Linux filesystem that Podman already uses, outside the project, where no Podman prune command removes it.

- **Moving the project directory** keeps its sandbox: the identifier moves with it, and the next launch records the new location.
- **Cloning the repository** gives the clone its own sandbox, because the identifier file is not committed.
- **A Git worktree** is a separate checkout and gets its own sandbox.  To share one, copy `.clyean/sandbox.local.json` into it.
- **Resetting or removing the Podman machine** on native Windows or macOS removes every sandbox stored in it, including the agents' sessions and login stores.

## Inspect

```sh
clyean sandbox status
```

prints the sandbox identifier and location, the image and digest the root was populated from, when and by which Clyean version it was provisioned, the harness version inside, which project used it last and when, and the User Assistants running on it.  It exits 1 when the root is not provisioned.

```sh
clyean sandbox shell
```

opens a root shell in a maintenance container with the workspace mounted, the same way agents see it.  Packages you install with `apt-get` persist, because the root filesystem is a directory that every container of the project shares.

## Build and rebuild

`clyean` provisions the sandbox on launch when it is missing or outdated, so you rarely need these:

```sh
clyean sandbox build     # populate and provision when missing or outdated, then refresh profiles
clyean sandbox rebuild   # discard the root filesystem and provision it again
```

Provisioning is considered outdated when the marker file `/.clyean-sandbox.json` in the root records an older provisioning schema than the running `clyean`, or an image other than the configured one.  A rebuild discards everything agents installed, and refuses while any container of the sandbox runs, so end the project's `clyean` processes first.

Population pulls the image, exports a container created from it, and streams the export into a helper container that extracts it into the root, so ownership inside the root is right under rootless Podman on every platform.  Provisioning then runs, inside the root, a package installation with whichever of `apt-get`, `apk`, or `dnf` the image provides (certificates, curl, git, an SSH client, `procps`, Python 3, and a headless Java runtime), installs the pinned PlantUML jar at `/opt/plantuml/`, installs the harness at `/usr/local/bin/clyean`, and verifies all three by running them.

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

Each mount appears read-only at `/mnt/<base name>` in every agent container.  `podmanRunArgs` are appended verbatim to every `podman run` Clyean issues for an agent container, which is the way to add resource limits, network options, or arbitrary environment variables.  Provider credentials do not need it: each agent receives the host variables of its providers automatically, as [Configure agents](configure-agents.md#credentials-for-the-agents) describes, and `sandbox.passthroughEnv` passes further variables to the agents it names.  `--mount` and `--image` on the command line set the same values when a project is scaffolded.

## Where the harness comes from

The harness inside the sandbox is the Linux build of Clyean's fork of Oh-My-Pi.  Clyean looks for it in this order:

1. `CLYEAN_HARNESS_BINARY` in the environment.
2. `sandbox.harnessBinary` in `project.json` or `project.local.json`.
3. A file named `clyean-harness-linux-<x64|arm64>` or `clyean-harness` next to the `clyean` executable.
4. The release asset `clyean-harness-linux-<arch>` of the running Clyean version, downloaded from GitHub into `~/.cache/clyean/harness/<version>/` (or `$XDG_CACHE_HOME/clyean`).

The architecture is the one Podman reports for the host.  Development builds and source installs have no matching release, so point option 1 or 2 at a harness you built.

## What is shared and what is not

Shared by every agent of a project: the root filesystem, packages installed into it, and the workspace mount.  Separate per agent: the harness profile under `/home/<user>/.omp/profiles/<agent-id>/agent/` (settings, MCP servers, sessions, and the login store), which only that agent's containers can see, and the host variables it receives.  Sub-agents hold copies of the User Assistant's sign-ins, which Clyean replaces every time a sub-agent starts.  Separate per unit of work: the sub-agent containers, which are created for one unit of work and removed when it ends.  Separate per `clyean` invocation: the User Assistant container, which is removed when the invocation ends.

## Remove what is left behind

```sh
clyean sandbox prune            # remove orphaned User Assistant containers, list orphaned root filesystems
clyean sandbox prune --remove   # also remove the orphaned root filesystems
```

A User Assistant shuts itself down when its `clyean` process ends, and Podman then removes its container.  A harness that has stopped responding cannot do that, and its container stays up without a bridge.  `clyean sandbox prune` removes, in every project, User Assistant containers more than a minute old that are stopped or have no running bridge, and leaves running ones alone.  For containers stranded by a host crash, enable Podman's `podman-clean-transient.service`, which removes them at the next boot.

It then lists every root filesystem in the roots directory with its identifier, the project that used it last, when, and its size.  A root filesystem is orphaned when no running container uses it and the project directory recorded in its marker is gone or holds another sandbox identifier, or when it has no marker and is more than a day old, which an interrupted population leaves.  Listing is the default because a moved project looks orphaned until its next launch records the new location; `--remove` deletes the orphans.
