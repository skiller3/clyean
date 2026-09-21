# Your first change plan

This tutorial follows one planning prompt through Clyean and then implements the plan.  It assumes a scaffolded project (see [Getting started](getting-started.md)) and a User Assistant that can reach a model provider.

## 1. Ask for a plan

In the User Assistant, describe the change:

```text
Plan adding a --json flag to the count command so that scripts can parse the output.
```

The User Assistant classifies the prompt as `SOFTWARE_ENGINEERING_PROJECT_PLANNING`, writes a refined version of it (goal, acceptance criteria, constraints, the files you mentioned, your exact words where they matter), and calls `clyean_delegate`.  The tool call streams the workflow as it runs; each line is prefixed with the agent it came from.

## 2. Answer the information request

The Software Engineering Director reads the request and decides whether it can write the plan overview faithfully.  When it cannot, the workflow emits an information request and the User Assistant asks you the questions with its `ask` tool, for example:

```text
Should --json apply to every subcommand or only to count?
```

Answer in the dialog.  The User Assistant calls `clyean_provide_information` with your answers, and the workflow continues in the same sub-agent sessions it started; nothing is lost while it waits.  While the answer is outstanding, a Herdr sidebar (if you run inside Herdr) shows the pane as blocked.

## 3. Watch the sections being written

The workflow now runs the steps of `workflow-planning.mmd`:

1. The Director writes `## Overview` (at most 1,200 characters) and Clyean commits it.
2. The Specifier writes `## Specification Changes`, the exact externally legible behavior changes and the edits that will be made to `.clyean/SPECS.md`.  The Director reviews the section; when it is misaligned, the Specifier revises it, up to three rounds.
3. The Software Architect writes `## Implementation Architecture` and the Director reviews it the same way.

When the Director finds the architecture section aligned, the tool returns `completed` with the plan reference, for example `2026-09-21-plan-adding-a-json-flag-to-the-count-command/v1`.

## 4. Review the plan

Open the file:

```sh
cat .clyean/plans/2026-09-21-plan-adding-a-json-flag-to-the-count-command/v1.md
git log --oneline -6
```

You see one commit per authored section, each with a `Clyean-Agent:` trailer naming its author.  Beside the plan sits `journal-<work-id>.json`, the durable record of the workflow (phase, answers, sub-agent sessions, commits).  Edit the plan by hand if you want to steer the implementation; the implementation workflow reads the file, not the conversation.

## 5. Implement it

```text
Implement 2026-09-21-plan-adding-a-json-flag-to-the-count-command/v1.
```

Naming the plan makes the User Assistant pass it as the `plan` reference; without it, an implementation prompt first runs planning.  The workflow of `workflow-implementation.mmd` then:

1. Has the Specifier apply the Specification Changes section to `.clyean/SPECS.md` (commit).
2. Has the Software Architect update the `.puml` sources; Clyean renders them to PDF inside the sandbox and sends failures back, up to three attempts (commit).
3. Has the Programmer implement the plan, run the build and tests, and report `ready_for_review` (commit of everything in the working tree).
4. Has the Software Architect review the implementation.  Blocker or major issues send the work back to the Programmer for remediation, up to three rounds; minor issues are recorded in the summary.

If the Programmer hits a blocking issue it cannot resolve, the workflow records the issue, creates `v2.md` of the plan seeded from `v1.md`, and re-runs planning with the issue as an additional concern before implementing again.  This happens at most twice.

## 6. Where everything landed

- `.clyean/plans/<plan>/v1.md` (and `v2.md` after a re-plan): the plan versions, never edited in place by Clyean.
- `.clyean/plans/<plan>/journal-<work-id>.json`: the workflow journal, committed with the plan.
- `.clyean/SPECS.md` and `.clyean/architecture/*.puml` plus rendered `*.pdf`: the updated specification and architecture.
- Git history: one commit per step, authored as `Clyean <Agent> <agent-id@agents.clyean.com>`.

`clyean plans` lists plans and versions; `clyean work` lists journals and their status.
