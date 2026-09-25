# Getting started

This tutorial takes you from nothing to a project that Clyean has scaffolded, planned, and changed.  It assumes a Linux workstation; see [Limitations](../explanation/limitations.md) for other hosts.

## 1. Install Clyean

Clyean needs Git and Podman on the host.  The installer installs both when they are missing, printing every command it runs, and never prompts.

```sh
curl -fsSL https://raw.githubusercontent.com/skiller3/clyean/main/install.sh | sh
```

Until `clyean.com` exists, use the raw GitHub URL above.  The executable lands in `~/.local/bin/clyean` (override with `CLYEAN_INSTALL_DIR`).  Check it:

```sh
clyean --version
podman info --format '{{.Host.Security.Rootless}}'
```

Rootless Podman is what Clyean is designed for: inside a container the agent is root, and rootless Podman maps that root to your own user, so every file an agent writes into your project is owned by you.

## 2. Launch in a project

Change into a directory that holds, or will hold, a project and run `clyean`:

```sh
mkdir -p ~/workspace/hello && cd ~/workspace/hello
clyean
```

On the first launch in a directory without a `.clyean/project.json`, Clyean asks one question on the host terminal: whether it may use Git worktrees.  Worktrees are preferable because they let agents work concurrently; answer `Y` unless you have a reason not to.  Pass `--worktrees yes` or `--worktrees no` to skip the question.

Clyean then:

1. Initializes a Git repository when the directory is not already inside one.
2. Writes `.clyean/agents/` (baseline instructions and per-agent settings), and `.clyean/.gitignore` with the rules Git does not already honor.
3. Checks Podman's version (4.9 or later on Linux and Linux on WSL, 5.0 or later on native Windows and macOS), records a sandbox identifier in `.clyean/sandbox.local.json`, and populates the project's sandbox root filesystem from `ubuntu:latest`, beside Podman's own data, then provisions it: a Java runtime, the pinned PlantUML jar, and the contained harness.  This takes a few minutes the first time and downloads a few hundred megabytes; later launches skip it.
4. Projects each agent's profile into the sandbox and starts the orchestrator inside the `clyean` process.
5. Starts this invocation's own User Assistant container, connects your terminal to it, and opens the bridge that links the container to the orchestrator.  You are now talking to the User Assistant inside the sandbox; the welcome box reads `Clyean v<version>` (the version of the `clyean` you ran), shows the rubber duck logo, and credits the Oh-My-Pi harness.

The harness needs a model provider.  Either export the provider's variable (for example `ANTHROPIC_API_KEY`) before running `clyean`, or use `/login` in the User Assistant.  Either way, the other agents receive what they need for their models when they start.  [Configure agents](../how-to/configure-agents.md) has the details.

## 3. Let the User Assistant scaffold

Type your first prompt, for example:

```text
This will be a small Rust command line tool that counts words in files. Plan the initial implementation.
```

The User Assistant first calls `clyean_status`, learns that the project is not scaffolded, decides the project type (`SOFTWARE_ENGINEERING_PROJECT` here), and calls `clyean_scaffold`.  You see its progress stream in the tool call: the deterministic scaffold (`project.json`, `SPECS.md`, fourteen diagram skeletons, the plans directory, one commit), then the Scaffolder agent researching the project and writing `.clyean/SPECS.md` and the PlantUML sources under `.clyean/architecture`, then a render and a second commit.

When scaffolding finishes, the User Assistant classifies the prompt as `SOFTWARE_ENGINEERING_PROJECT_PLANNING`, refines it, and hands it to the Software Engineering Director with `clyean_delegate`.  The planning workflow may come back with questions; the User Assistant asks you with its `ask` tool and sends your answers on.  The next tutorial, [Your first change plan](your-first-change-plan.md), follows that exchange in detail.

## 4. Implement the plan

Once the User Assistant reports that the change plan is authored, read it (the path is printed, for example `.clyean/plans/2026-09-21-plan-the-initial-implementation/v1.md`), then ask:

```text
Implement that plan.
```

The implementation workflow updates `.clyean/SPECS.md`, updates and renders the architecture diagrams, has the Programmer implement the change, and has the Software Architect review it.  Every step ends in a commit whose message carries a `Clyean-Agent:` trailer, so `git log` tells you which agent did what.

## 5. Leave and come back

Quit the User Assistant with `/exit` (or Ctrl-D); its container stops and is removed.  The User Assistant also ends when you close the terminal or the `clyean` process stops for any other reason, because every invocation owns its container.  Your sessions live in the sandbox, so nothing is lost: `clyean --continue` resumes the most recent session, `clyean --resume` opens the session picker, and `clyean work` lists units of orchestrated work, including any that were interrupted and can be resumed.

You can also run `clyean` in a second terminal of the same project while the first is open.  Each invocation gets its own User Assistant and session; if both delegate work at once, the second is told the project is busy unless the project uses Git worktrees.
