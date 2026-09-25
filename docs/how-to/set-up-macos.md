# Set up macOS

On macOS, Clyean's agent containers run in a Linux virtual machine that Podman manages, a Podman machine.  This guide covers what Clyean needs from that machine and how to fix the common problems.

## Install

```sh
curl -fsSL https://clyean.com/install.sh | sh
```

The installer installs Podman with Homebrew when it is missing, stops when the installed Podman is older than 5.0 (upgrade with `brew upgrade podman`), and, when no Podman machine exists, creates and starts one:

```sh
podman machine init --cpus <4, or all of the host's CPUs if it has fewer> \
  --memory <8192, or half of the host's memory in MiB if that is less> \
  --disk-size 100
podman machine start
```

The disk grows as it fills, up to 100 GiB; it does not take that space up front.  Run the installer with `--dry-run` to see the commands and sizes without changing anything.

An existing machine is never changed; the installer only starts it when it is stopped.

## The machine's size

Clyean recommends at least 4 CPUs and 8 GiB of memory, and warns at every launch when the machine has less.  A macOS machine's CPUs and memory are fixed when it is created.  To change them, recreate the machine:

```sh
podman machine rm
podman machine init --cpus 4 --memory 8192 --disk-size 100
podman machine start
```

Removing the machine removes every sandbox stored in it, including the agents' sessions and login stores, so do it between pieces of work.

## Providers

Clyean supports the provider Podman uses by default on macOS, `applehv`.  It warns, without stopping, when the machine uses any other provider.  `podman machine info` shows the provider as `VMType`.

## Where projects can live

The machine sees your Mac's files through shared directories, which by default include your home directory.  Clyean mounts the project's workspace into the containers from the same path, and checks before each launch that the machine sees it.  For a project outside the shared directories, the check fails and names the path; recreate the machine with an extra volume for it:

```sh
podman machine init --volume /Volumes/Work:/Volumes/Work ...
```

## Where sandboxes live

Each project's sandbox root filesystem lives inside the machine, in `~/.local/share/clyean/roots/<identifier>` of the machine's user.  `clyean sandbox status` prints its location, and `podman machine ssh` opens a shell in the machine to reach it.  Resetting or removing the machine removes every sandbox; `clyean` then provisions a new one at the next launch.

## Containers left by a crash

When the machine stops abruptly, containers that were running can stay behind as stopped.  `clyean sandbox prune` removes stopped User Assistant containers.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `Clyean needs Podman 5.0 or later` | `brew upgrade podman`, then `podman machine stop` and `podman machine start`. |
| `the Podman machine cannot see <path>` | Move the project under your home directory, or recreate the machine with a volume for its location. |
| A launch warns about CPUs or memory | Recreate the machine with the recommended size (see above). |
| `Cannot connect to Podman` | `podman machine start`. |
