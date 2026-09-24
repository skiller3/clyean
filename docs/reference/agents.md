# Agents

Clyean's agents are specified in `AGENT_SPECS.md`; this page states what the implementation provides.  Each implemented agent has baseline instructions embedded in `clyean` and written to `.clyean/agents/AGENTS__<NAME>.md` at scaffold time, a settings overlay `<NAME>.omp.json`, and an MCP seed `<NAME>.mcp.json`.

## Roster

| Identifier (`CLYEAN_AGENT`, profile) | Name | Status | Instruction file |
| --- | --- | --- | --- |
| `user-assistant` | User Assistant | implemented | `AGENTS__USER_ASSISTANT.md` |
| `scaffolder` | Scaffolder | implemented | `AGENTS__SCAFFOLDER.md` |
| `software-engineering-director` | Software Engineering Director | implemented | `AGENTS__SOFTWARE_ENGINEERING_DIRECTOR.md` |
| `specifier` | Specifier | implemented | `AGENTS__SPECIFIER.md` |
| `software-architect` | Software Architect | implemented | `AGENTS__SOFTWARE_ARCHITECT.md` |
| `programmer` | Programmer | implemented | `AGENTS__PROGRAMMER.md` |
| `code-reviewer` | Code Reviewer | placeholder | none |
| `automated-test-programmer` | Automated Test Programmer | placeholder | none |
| `mutant-killer` | Mutant Killer | placeholder | none |
| `crap-reducer` | CRAP Reducer | placeholder | none |
| `qa-tester` | QA Tester | placeholder | none |
| `ci-cd-programmer` | CI/CD Programmer | placeholder | none |
| `deployment-analyst` | Deployment Analyst | placeholder | none |
| `security-engineer` | Security Engineer | placeholder | none |
| `white-hat-hacker` | White-Hat Hacker | placeholder | none |
| `documentation-author` | Documentation Author | placeholder | none |

Placeholders are known to the roster (they appear in `clyean agents`) but have no instructions, profile, or container.

Every commit an agent makes, or that Clyean makes on its behalf, is authored as `Clyean <Name> <identifier@agents.clyean.com>` and ends with the trailer `Clyean-Agent: <identifier>`.

## Sessions

The User Assistant's harness session is the Clyean session: `--continue` and `--resume` act on it.  Every other agent gets a new session per unit of work (one user prompt), reused for every step of that work, including while the work waits for answers from the user, and shut down when the work ends.  Session files are recorded in the work's journal so a resumed work re-opens them.

## What each agent does and how it answers

The orchestrator sends one instruction per step and reads a verdict from the last fenced `json` block of the reply (a reply without one earns a single retry asking for the block alone).

### User Assistant

Talks to you, classifies each prompt (`project_type`, then `prompt_type`), handles `MISCELLANEOUS` prompts itself, and delegates the three software engineering prompt types through the tools `clyean_status`, `clyean_scaffold`, `clyean_delegate`, `clyean_provide_information`, `clyean_resume`, and `clyean_cancel`.  The `/clyean` slash command shows project status.  It does not answer with verdicts; it relays.

### Scaffolder

Researches the project after the deterministic scaffold and authors `.clyean/SPECS.md` and the fourteen `.puml` sources, describing the status quo.  Verdict: `{"decision": "completed", "summary": "..."}` or `{"decision": "blocked", "issue": "..."}`.  Render failures are sent back up to three times.

### Software Engineering Director

Answers research prompts directly (no verdict).  For plans it writes only the `## Overview` section and reviews the other two.  Verdicts by step:

- Information check: `{"decision": "needs_information", "questions": [...]}` or `{"decision": "proceed"}`.
- Overview written: `{"decision": "overview_written"}`.
- Answering a sub-agent's questions: `{"decision": "answered", "answers": [...]}` or `{"decision": "escalate", "questions": [...]}`.
- Section review: `{"decision": "aligned"}` or `{"decision": "misaligned", "issues": [...]}`.

### Specifier

Writes `## Specification Changes` during planning and applies it to `.clyean/SPECS.md` during implementation.  Verdicts: `{"decision": "needs_information", "questions": [...]}`, `{"decision": "completed", "summary": "..."}`, or `{"decision": "blocked", "issue": "..."}` (implementation only).

### Software Architect

Writes `## Implementation Architecture` during planning, updates the `.puml` sources during implementation, and reviews the implemented change.  Verdicts: the same as the Specifier for authoring, and for review `{"decision": "approved", "summary": "..."}` or `{"decision": "issues", "issues": [{"severity": "blocker|major|minor", "location": "...", "description": "..."}]}`.  Blocker and major issues send the work back to the Programmer; minor issues are recorded.

### Programmer

Implements the plan against `SPECS.md` and the architecture, runs the build and tests, and either reports `{"decision": "ready_for_review", "summary": "..."}` or `{"decision": "blocked", "issue": "...", "suggested_resolution": "..."}`, which triggers a new plan version.

## Files projected into a profile

Before an agent starts, Clyean writes into `/home/<user>/.omp/profiles/<identifier>/agent/` in the project's sandbox root filesystem:

- `AGENTS.md`: the baseline plus the local enhancement.
- `clyean-overlay.json`: the merged settings overlay, passed with `--config`.
- `mcp.json`: the existing file deep-merged with the merged MCP seed.
- `extensions/clyean-herdr-reporter.ts` and `extensions/clyean-orchestration.ts`: for the User Assistant only, refreshed when their `CLYEAN_EXTENSION_VERSION` marker is behind the embedded copy.

Sub-agents are started with `--mode rpc --approval-mode yolo --no-title` and, when resuming, `--resume <session file>`.
