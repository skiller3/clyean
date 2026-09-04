# Purpose
Clyean is an agentic programming harness designed to minimize the accumulation of technical debt (i.e. architectural and code slop) during iterative development of software projects.  It achieves its goal via coordination of multiple specialized agents that ensure:

- Up-to-date system interface (UI/UX and API) specifications
- Correct and complete implementation of system interfaces
- System architecture that supports project goals by:
  - Providing abstractions that reduce the reasoning and work required to (i) enhance the system, (ii) support/troubleshoot the system, and (iii) deploy/host the system
  - Enabling sufficient system performance
  - Enabling sufficient system availability and resiliency
  - Enabling sufficient system security posture
- Up-to-date system architecture specifications
- Architectural design compliance
- Broad, high-quality, and maximally automated testing that includes:
  - Deterministic unit tests
  - Deterministic integration tests
  - Deterministic system interface (UI/UX and API) tests
  - Non-deterministic QA tests
- High code quality via:
  - Clear and concise naming of variables, functions, modules, macros, files, etc.
  - Adherence to the Principle of Least Astonishment (POLA) to the extent reasonable
  - Adherence to SOLID (https://en.wikipedia.org/wiki/SOLID) to the extent reasonable
  - Adherence to Don't Repeat Yourself (DRY) to the extent reasonable
  - Enforcing low cyclomatic complexity of functions and methods
- Accurate and sufficient documentation
- Strong security posture

This project was born from frustration with Slop Debt (see `https://arpitbhayani.me/blogs/slop-debt/`).

# Specifications
`SPECS.md` contains Clyean's specifications — **read them before doing any work!**

# Key Resources
- `git` CLI - Use this for any Git operations
- `gh` CLI - Use this for any Github-specific operations
- `playwright` MCP server connection - Use this for browser-based interaction with web resources

# Development Process & Constraints
- Adhere to a Trunk-Based Development (`https://trunkbaseddevelopment.com/`) SDLC to the extent practical.  Most importantly, ensure that you:
  - Develop all features, bugfixes, and tests within appropriately named branches cut from the head of `main`, push these branches and associated changes upstream to remote (Github), and create pull requests from the relevant branches into the trunk branch (i.e. `main`).
  - Do NOT merge pull requests into the `main` branch — I will manually review and merge them.
  - Author and maintain a Github CI/CD workflow that executes upon every commit to any branch.  The workflow should:
    - Execute all unit and integration tests
    - Conditioned upon the preceding automated tests passing, build a release of the `clyean` CLI from the branch for all supported operating systems (Linux, MacOS, Windows) and CPU architectures (x86 / x64, ARM)
  - Author and maintain a Github workflow to cut an appropriately named release branch from trunk and tag its head appropriately.  Releases should comply with standard semantic versioning (`https://semver.org/`), and tags should reflect the semver version of the software. Major version updates should only be achievable via manual tagging by me, but the "cut release" Github workflow should apply a tag in which the minor version is incremented on the head of the branch it creates.  The "cut release" Github workflow should be designed to be manually invoked by me.
  - Author and maintain a Github workflow that automatically tags each new commit to release branches with an incremented semver patch version.
- As we work together, continuously update `SPECS.md` to reflect the latest specified requirements.  Review and refactor the file as useful upon each edit to ensure requirements coherency and consistency. 
- Author unit and integration tests that thoroughly test `clyean` and its abstractions.
- Author and maintain thorough documentation of `clyean` within a `docs` directory found at the top level of the project.  Documentation should be written in markdown (in "*.md" files) and adhere to Diátaxis principles (see `https://diataxis.fr/`).
- Do NOT immediately begin work or mutate the state of this project upon ingesting this file!  Instead, wait for further instruction from me.