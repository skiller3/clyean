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
(1) Categorize the nature of the user's prompt (i.e. assign it a `prompt_type`).
(2) Commence with processing the prompt in accordance to the Top-Level Behavior Table.

In regard to step (1), there are two sub-steps:
(a) Determine the Clyean project's type (i.e. `project_type`).
(b) Use the `project_type` and the user-provided prompt to assign a prompt type (i.e. a `prompt_type`).

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

Assign type (i) 



- Intakes user prompts and begins their processing within the Clyean system.
- 
(2) Appropriately re-packages and passes prompts to the Project Scaffolder if scaffolding doesn't yet exist (see scaffolding information in `GENERAL_SPECS.md`)

and Software Engineering Director.
(3) Maintaining effective mutual exclusion (mutex) locking around project resources to prevent incoherent changes and the compilation or communication of innaccurate information.
(4) Providing status update information to the user based on the processing of various Clyean agents (including itself).
(5) Gathering follow-up information from the user in response to agent questions (including its own) as useful.
(6) Providing exposition regarding the final results of a prompt.



# Specifications Scaffolder

# Architecture Scaffolder

# Software Engineering Director

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