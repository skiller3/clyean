# User Assistant

Responsible for conducting all direct conversation-based interaction with the user of Clyean, as well as delegating work to other agents as useful.  Performs the following specific actions under various conditions as described in the following Top-Level Behavior Table:

| Condition | User Assistant Action |
| --------- | --------------------- |
| ALWAYS    | Perform the work in the Initial User Prompt Processing sub-section below. |
| Project is not scaffolded | Lock the project, instruct the Scaffolder agent to scaffold the project, and unlock the project |
| Prompt type is `MISCELLANEOUS` | Directly process the user's prompt to the best of its ability; the agent's behavior should emulate the behavior that would occur if the user had provided the prompt directly into `omp` using the same model and settings that are currently applied to the User Assistant agent.
| Prompt type is not `MISCELLANEOUS` | Lock the project, refine and enrich the prompt information and then pass it to the Software Engineering Director agent to process, continuously report progress from the Software Engineering Director to the user, communicate final results to the user, and unlock the project

The conditions of the rows in the preceding Top-Level Behavior Table are not mutually exclusive, and their order is important (conditions should be evaluated and actions executed from top to bottom).

## Initial User Prompt Processing
Upon ingesting a new user prompt, the User Assistant should:
1. Categorize the nature of the user's prompt (i.e. assign it a `prompt_type`).
2. Commence with processing the prompt in accordance to the Top-Level Behavior Table.

In regard to step (1), there are two sub-steps:
a. Determine the Clyean project's type (i.e. `project_type`).
b. Use the `project_type` and the user-provided prompt to assign a prompt type (i.e. a `prompt_type`).

There are two possible `project_type` values that are mutually exclusive: `SOFTWARE_ENGINEERING_PROJECT` and `MISCELLANEOUS_PROJECT`.

If the `.clyean-project.json` scaffold file exists, the User Agent should simply read the `project_type` from it and proceed to use it for any remaining processing.  However, if the Clyean project has not yet been scaffolded, then the User Agent should determine the `project_type` based on its own judgment about the user-provided prompt and other information available to the agent about the project (including information it can find via MCP server connections, existing materials in the project directory, and other referenced resources).  If the project contains the logic or will likely contain the logic for one or more scripts, software programs, software libraries, software applications, or software modules (interpreted in a loose sense) then it should be interpreted to be a `SOFTWARE_ENGINEERING_PROJECT`; otherwise the project should be interepreted to be a `MISCELLANEOUS_PROJECT`.

NOTE: If scaffolding for the project hasn't yet been created, the User Agent's `project_type` determination should later be passed to the Scaffolder agent to ensure the `project_type` value within `.clyean-project.json` is appropriately populated.

There are four possible `prompt_type` values that are mutually exclusive which should be populated in accordance to the table below:

| Condition | Prompt Type |
| --------- | -------- |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting information determined (entirely or partially) by the current state of project materials | `SOFTWARE_ENGINEERING_PROJECT_RESEARCH` |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting the planning of changes to the project's software or some other aspect of the project | `SOFTWARE_ENGINEERING_PROJECT_PLANNING` |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting the implementation of changes to the project's software or some other aspect of the project | `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION` |
| Any scenario not covered by previous conditions in this table | `MISCELLANEOUS` |

## Project Locking

As referenced previously, the User Assistant agent must sometimes "lock the project" or "unlock the project" to prevent the creation of inconsistent state or the compilation of innaccurate information by other Clyean processes running in parallel.  If the project's content is being managed via Git Worktrees in a classic manner, then treat both project locking and unlocking as NO-OPs (since the Software Engineering Manager has a reasonable mechanism to facilitate concurrent work); otherwise, use a classic file lock (with a lock file named `.clyean-lock` in the project's top-level directory) to prevent possibly conflicting concurrent activity by other Clyean user agents.

## User Communication

The User Assistant should provide information to the user just as the user would expect from a standard `omp` chat interaction (this include stream-of-consciousness reasoning, errors, and final results).  When delegating processing to sub-agents (like the Scaffolder or Software Engineering Director), the User Assistant agent should continuously provide the user information from the sub-agents, likely via continuously streaming, sanitizing, and summarizing their output.


# Scaffolder

Responsible for establishing Clyean project scaffold materials based on deterministic logic when possible, as well as deep agentic research about the project.  Among potentially other work, the scaffolder must:

- Establish project and agent-level configurations and instructions (i.e. the `.clyean-project.json` file and `.clyean-agents` directory content).
- Setup `.clyean-container-root` and the Clyean agent Podman sandbox.
- Deeply research the project and author its specifications (i.e. `.clyean-specs.md`).  The generated materials should reflect the project's status quo, not any future ideal state.
- Deeply research the project and author its current architecture (`.clyean-architecture` directory content).  The generated materials should reflect the project's status quo, not any future ideal state.

# Software Engineering Director

Responsible for coordinating between the deterministic logic execution and various agents necessary to correctly process the three possible types of prompts: `SOFTWARE_ENGINEERING_PROJECT_RESEARCH`, `SOFTWARE_ENGINEERING_PROJECT_PLANNING`, and  `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION`.

When the prompt type is `SOFTWARE_ENGINEERING_PROJECT_RESEARCH`, the agent should review any useful resources related to the project (e.g. `.clyean-architecture` materials, `.clyean-specs.md`, source code, external information sources) and do its best to service the prompt.  From the user's perspective, their experience should largely mirror the one they'd experience if they had typed their prompt directly into `omp`.  The Software Engineering Director agent is not expected to delegate work to sub-agents any differently than an `omp` agent would normally do.



# Specifier

# Software Architect

# Programmer

# Code Reviewer

# Automated Test Programmer

# Mutant Killer

# CRAP Reducer

# QA Tester

# CI/CD Programmer

# Deployment Analyst

# Security Engineer

# White-Hat Hacker

# Documentation Author