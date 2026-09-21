# Resume interrupted work

Every unit of orchestrated work (scaffolding, research, planning, implementation) is journaled after each step, so a workflow interrupted by a crash, a closed terminal, or a cancelled turn can be finished rather than restarted.

## Find unfinished work

From the host:

```sh
clyean work
```

lists every journal with its kind, status, phase, and plan.  Statuses `running` and `awaiting_information` are resumable; `completed`, `failed`, and `cancelled` are terminal.  A `running` status on a work that is not actually running means the process that ran it died.

Inside the User Assistant, `/clyean` shows the same status, and at every session start Clyean notifies you of unfinished work and gives the User Assistant a hidden note asking it to confirm with you before resuming.

## Resume

Ask the User Assistant to resume, or call the tool directly by name:

```text
Resume work 01a0c4eadbf876f1bddb45215067c861.
```

The User Assistant calls `clyean_resume`.  Clyean reloads the journal, re-opens the sub-agent sessions recorded in it (they are resumed from their session files, so the agents keep their context), and continues from the recorded phase.  A work that was waiting for information replays its questions first; answer them and the User Assistant calls `clyean_provide_information`.

Only one unit of work runs per project at a time unless the project uses Git worktrees; a second one fails with `project_locked` until the first finishes.

## Cancel

```text
Cancel work 01a0c4eadbf876f1bddb45215067c861.
```

`clyean_cancel` stops the workflow at the next step boundary, marks the journal `cancelled`, and shuts the sub-agent sessions down.  Commits already made stay in history; the plan files stay on disk.

## Where journals live

- Planning and implementation: `.clyean/plans/<plan>/journal-<work-id>.json`, tracked and committed with the plan.
- Research and scaffolding: `.clyean/work/<work-id>.json`, ignored by Git.

The [change plans reference](../reference/change-plans.md) describes the journal fields.
