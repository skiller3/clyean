# Set up Windows

On native Windows, Clyean's agent containers run in a Linux virtual machine that Podman manages, a Podman machine, which by default runs under WSL 2.  This guide covers what Clyean needs from that machine and how to fix the common problems.  To use Clyean inside a Linux distribution under WSL instead, follow the Linux instructions in that distribution; Podman then runs on the distribution's own kernel, with no Podman machine.

## Install

```powershell
irm https://clyean.com/install.ps1 | iex
```

The installer installs Git and Podman with winget when they are missing, stops when the installed Podman is older than 5.0 (upgrade with `winget upgrade RedHat.Podman`), and, when no Podman machine exists, creates and starts one with `podman machine init` and `podman machine start`.  Run it with `-DryRun` to see the commands without changing anything.  An existing machine is never changed; the installer only starts it when it is stopped.

WSL 2 must be enabled first.  If `podman machine init` fails, run `wsl --install`, restart, and run the installer again.

## The machine's size

Clyean recommends at least 4 CPUs and 8 GiB of memory, and warns at every launch when the machine has less.  With the default WSL provider, the machine runs in WSL 2's shared virtual machine, whose limits are WSL's global settings: by default every logical processor and half of the host's memory, which meets the recommendation on a host with at least 4 logical processors and 16 GB of memory.  To raise them, set `processors` and `memory` in the `[wsl2]` section of `%UserProfile%\.wslconfig`:

```ini
[wsl2]
processors=4
memory=8GB
```

then run `wsl --shutdown` and `podman machine start`.  These settings apply to every WSL distribution, so the installer reports them instead of editing the file.

## Providers

Clyean supports WSL, the provider Podman uses by default on Windows, and supports Hyper-V on a best-effort basis.  To create a Hyper-V machine, set `CONTAINERS_MACHINE_PROVIDER=hyperv` before running the installer, which then sizes the machine like on macOS (4 CPUs or fewer, 8 GiB or half the host's memory, a disk that can grow to 100 GiB).  Clyean warns, without stopping, when the machine uses any other provider.

## Where projects can live

The machine sees your Windows drives under `/mnt/<drive letter>`, and Clyean translates the project's path accordingly (`C:\Users\me\app` is mounted from `/mnt/c/Users/me/app`).  Before each launch, Clyean checks that the machine sees the workspace and every configured mount, and names any path it cannot see.

File access across the Windows drive boundary is slow, particularly for Git.  For large repositories, clone them inside a WSL distribution and run Clyean there under Linux on WSL, which uses the distribution's own filesystem and kernel.

## Where sandboxes live

Each project's sandbox root filesystem lives inside the machine, in `~/.local/share/clyean/roots/<identifier>` of the machine's user.  `clyean sandbox status` prints its location, and `podman machine ssh` opens a shell in the machine to reach it.  Resetting or removing the machine removes every sandbox, including the agents' sessions and login stores; `clyean` provisions a new one at the next launch.

## Line endings

Clyean passes your global `core.autocrlf` and `core.eol` Git settings to Git inside the containers, so a repository checked out with CRLF line endings does not look modified to the agents.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `Clyean needs Podman 5.0 or later` | `winget upgrade RedHat.Podman`, then `podman machine stop` and `podman machine start`. |
| `the Podman machine cannot see <path>` | Keep the project on a drive the machine mounts, such as `C:`. |
| A launch warns about CPUs or memory | Raise `processors` and `memory` in `%UserProfile%\.wslconfig`, then `wsl --shutdown` and `podman machine start`. |
| `podman machine init` fails | Enable WSL 2 with `wsl --install` and restart. |
