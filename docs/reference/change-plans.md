# Change plans

A change plan describes one change to a project before it is implemented.  Plans live under `.clyean/plans` and are versioned files rather than documents edited in place.

## Naming

A plan directory is named `<YYYY-MM-DD>-<slug>`, where the date is the UTC day the plan was created and the slug is derived from the first line of the prompt: lower-cased, ASCII letters and digits kept, everything else collapsed into single hyphens, at most 48 characters.  A name that already exists gets a numeric suffix (`-2`, `-3`).

A plan reference names a plan and optionally a version: `2026-09-21-add-oauth-login` (the latest version) or `2026-09-21-add-oauth-login/v2`.  References are what `clyean_delegate` accepts in its `plan` parameter and what completion events report.

## Versions

Each version is one file, `v1.md`, `v2.md`, and so on.  A new version is created when an implementation is blocked and planning is re-run; it starts as a copy of the previous version with the title suffix updated, so the agents edit incrementally.  Versions are never deleted or rewritten by Clyean.

## Sections

Every version has exactly three top-level sections in this order:

| Section | Author | Content |
| --- | --- | --- |
| `## Overview` | Software Engineering Director | Summary of the change, at most 1,200 characters (the Director is asked to shorten a longer one). |
| `## Specification Changes` | Specifier | The exact externally legible behavior changes, comprehensively describing the edits that will be made to `.clyean/SPECS.md`. |
| `## Implementation Architecture` | Software Architect | The exact architecture changes and how the change fits the architecture, comprehensively describing the edits that will be made under `.clyean/architecture`. |

The skeleton Clyean writes carries the title `# <first prompt line> (v<N>)` and a placeholder line in each section.  Clyean verifies after each step that the section is no longer empty.

## Journals

Each unit of work that touches a plan writes `journal-<work-id>.json` in the plan directory (research and scaffolding journals go to `.clyean/work/` instead).  The journal is saved after every phase transition and committed with the plan, so it survives crashes and is visible in history.  Fields:

| Field | Meaning |
| --- | --- |
| `workId`, `kind`, `promptType` | The identifier, kind (`scaffold`, `research`, `planning`, `implementation`), and prompt type of the work. |
| `prompt`, `originalPrompt`, `sessionId` | The refined prompt, the user's words, and the User Assistant session that started the work. |
| `createdAt`, `updatedAt` | UTC timestamps. |
| `phase` | The current workflow phase, for example `planning.specification_review` or `implementation.programming`. |
| `status` | `running`, `awaiting_information`, `completed`, `failed`, or `cancelled`. |
| `plan` | The plan reference being authored or implemented. |
| `information` | Every round of questions, who asked them, and the answers once given. |
| `sessions` | Session id and file of each sub-agent, used to resume them. |
| `commits` | SHAs of the commits made after each step. |
| `iterations` | Counters of the bounded loops (review rounds, remediation rounds, re-planning cycles). |
| `blockingIssues` | Concerns added by blocking implementation issues, carried into the re-planned version. |
| `summary`, `error`, `artifacts` | The outcome once terminal. |
| `scratch` | Step-local data such as section summaries and review issues. |

## Commits

Clyean commits after each completed step with a subject that names it: `Create change plan skeleton`, `Write change plan overview`, `Write change plan specification changes`, `Write change plan implementation architecture`, `Complete change plan`, `Update specifications for change plan`, `Update architecture diagrams for change plan`, `Implement change plan`, `Start new change plan version after blocking issue`, and `Complete change plan implementation`.  The author is the agent responsible for the step and the message ends with `Clyean-Agent: <identifier>`.  Agents may also commit their own work inside the sandbox with the same trailer; Clyean commits whatever they leave uncommitted.
