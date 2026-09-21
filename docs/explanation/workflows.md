# Workflows

The orchestrator implements the two workflow diagrams at the repository root, `workflow-planning.mmd` and `workflow-implementation.mmd`, as resumable state machines.  This page describes them in prose, with the bounds the implementation adds.

## Prompt types

The User Assistant classifies every prompt.  `MISCELLANEOUS` prompts are handled by the User Assistant itself, like an ordinary harness session.  `SOFTWARE_ENGINEERING_PROJECT_RESEARCH` prompts go to one Software Engineering Director session, which answers directly.  `SOFTWARE_ENGINEERING_PROJECT_PLANNING` and `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION` prompts run the workflows below.  Scaffolding is a fourth workflow that runs when a project has no `project.json` yet.

## Planning

1. Clyean creates the plan directory and `v1.md` with the three section headings and commits it.
2. The Software Engineering Director decides whether it needs more information to write the overview.  If so, the questions go to the User Assistant, which asks you; the answers are recorded in the journal and the Director decides again with them in hand.  This repeats until the Director proceeds.
3. The Director writes `## Overview`.  Clyean checks that the section is no longer the placeholder and asks for a shorter version once when it exceeds 1,200 characters.  Commit.
4. The Specifier writes `## Specification Changes`.  When the Specifier needs information, the Director answers from what it knows or escalates to you through the User Assistant.  Commit.
5. The Director reviews the section against the overview and your instructions.  A misaligned verdict sends the issues back to the Specifier for a revision; after three misaligned reviews the work fails.
6. The Software Architect writes `## Implementation Architecture` the same way, and the Director reviews it the same way.
7. The plan is complete.  For a planning prompt, Clyean commits and reports the plan reference; for an implementation prompt, the workflow continues below.

## Implementation

An implementation prompt that names a plan starts here; one that does not runs planning first.

1. The Specifier applies the Specification Changes section to `.clyean/SPECS.md`.  Commit.
2. The Software Architect updates the `.puml` sources under `.clyean/architecture`.  Clyean renders them to PDF inside the sandbox; sources that fail to render are sent back with PlantUML's output, up to three attempts.  Commit.
3. The Programmer implements the plan against the specification and the architecture, runs the build and tests, and reports `ready_for_review` or `blocked`.  On `ready_for_review`, Clyean commits everything in the working tree.
4. The Software Architect reviews the implementation against the plan, all of `SPECS.md`, the architecture materials, and the code quality characteristics of `AGENT_SPECS.md`.  `approved` completes the work.  Issues with blocker or major severity send the work back to the Programmer for remediation, then to review again, up to three remediation rounds; issues that are all minor complete the work with the notes recorded in the summary.

## The re-planning rule

When the Programmer reports a blocking issue it cannot or should not resolve itself, Clyean records the issue as an additional concern of the change, creates the next plan version seeded from the current one, commits it, and re-runs planning from the overview information step.  The Director, Specifier, and Architect see the blocking issue in their prompts and revise the plan; implementation then restarts at the specification step.  At most two such cycles run per unit of work; a third blocking issue fails the work with the issue in its message.

## Scaffolding

Clyean writes `project.json`, the specification skeleton, the fourteen diagram skeletons, and the plans directory and commits them.  The Scaffolder then researches the project and authors `.clyean/SPECS.md` and the diagram sources, describing the project as it is.  Clyean renders the diagrams, sending failures back up to three times, and commits.

## Why the loops are bounded

Every review and remediation loop has a fixed bound so that a disagreement between agents cannot consume budget without end.  A failed work is left in its journal with the reason, its commits stay in history, and you decide what to do next with the full record in front of you.
