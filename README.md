# Clyean

Zero-slop agentic coding harness.

Clyean is an orchestration layer for software development by AI agents.  It contains a fork of the [Oh-My-Pi](https://omp.sh/) harness and coordinates several instances of it, one per specialized agent (a User Assistant you talk to, a Scaffolder, a Software Engineering Director, a Specifier, a Software Architect, and a Programmer), so that every change to a project starts from its written specification and architecture and ends in reviewed, committed code.  Every agent runs in a Podman container whose root filesystem belongs to the project, and every step of every workflow is journaled and committed, so work survives interruption and history says which agent did what.

## Install

Clyean needs Git and Podman on the host; the installers add them when they are missing.

```sh
# Linux and macOS
curl -fsSL https://raw.githubusercontent.com/skiller3/clyean/main/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/skiller3/clyean/main/install.ps1 | iex
```

The scripts accept `--ref <tag>` (`-Ref <tag>`) for a specific release, `--source` (`-Source`) to build with cargo, and `--no-deps` (`-NoDeps`) to skip dependency installation.  This version populates the sandbox on Linux hosts only; see [Limitations](docs/explanation/limitations.md).

## Quick start

```sh
cd ~/workspace/my-project     # any directory, with or without a Git repository
export ANTHROPIC_API_KEY=...  # or log in with /login once inside
clyean                        # scaffolds the project and starts your own User Assistant
```

Then type what you want, for example `Plan adding a --json flag to the count command`.  The User Assistant scaffolds the project if needed, hands engineering prompts to the Software Engineering Director, relays questions back to you, and reports the change plan under `.clyean/plans`.  Ask it to implement the plan when you have read it.

## Documentation

The [documentation map](docs/README.md) is organized as tutorials, how-to guides, reference, and explanation.  Good entry points: [Getting started](docs/tutorials/getting-started.md), [Architecture](docs/explanation/architecture.md), and the [command line reference](docs/reference/cli.md).  `GENERAL_SPECS.md` and `AGENT_SPECS.md` are the specifications Clyean is built to.

## License

Copyright (C) 2026 Skye Isard.

Clyean is licensed under the [GNU Affero General Public License v3](LICENSE),
**with the [Clyean Generated Output Exception](LICENSE-EXCEPTION-OUTPUT.md)**.

```
SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
```

### What you build with Clyean is yours

The AGPL's copyleft covers Clyean itself.  It does not reach the work Clyean
helps you produce. Explicitly:

- **You own your output.**  Code, docs, configuration, or anything else Clyean
  generates for you is yours, to license however you like — including
  closed-source and commercial.
- **Your repository stays your own.**  Pointing Clyean at your codebase does not
  make that codebase a derivative work of Clyean, and edits Clyean writes into
  your files carry no obligation.
- **AGPL section 13 does not reach your users.**  Shipping something you built
  with Clyean as a network service gives your users no claim to Clyean's
  source.
- **No conditions.**  No attribution, no notice, no source disclosure is
  required for the parts of Clyean that end up embedded in your output.

The one thing the exception does not do is let you redistribute Clyean *itself*
outside the AGPL. See [LICENSE-EXCEPTION-OUTPUT.md](LICENSE-EXCEPTION-OUTPUT.md)
for the operative text.

Other licensing terms, including commercial licences, are available from the
copyright owner: skye.isard@gmail.com.

## Contributing

Contributions are welcome.  Clyean uses a copyright-assignment
[CLA](CLA.md), signed with one comment on your pull request and enforced
automatically. See [CONTRIBUTING.md](CONTRIBUTING.md).
