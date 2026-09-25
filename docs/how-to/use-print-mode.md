# Use print mode

`clyean -p` runs the User Assistant without the terminal UI: it sends one prompt, writes the result to standard output, and exits.  Use it from scripts and other tools.

```sh
clyean -p "Summarize the change plans in this project."
clyean -p --model opus "What does the count command do?"
echo "Which tests cover the parser?" | clyean -p
```

Print mode still prepares the host scaffold and the sandbox, starts the orchestrator, and runs the invocation's own User Assistant container, without a pseudo-terminal, through the same bridge as an interactive launch, so delegated workflows work the same way they do interactively.  When standard input is a pipe, the harness reads it until it closes, as `omp -p` does; redirect it from `/dev/null` in scripts that keep it open.  What differs:

- Questions the workflow needs answered cannot be asked interactively.  The tool result tells the User Assistant what was asked; the User Assistant reports it and the work stays resumable (`clyean work`, then resume it in an interactive session).
- Approval prompts cannot be shown; configure `tools.approvalMode` in `USER_ASSISTANT.omp.json` or `USER_ASSISTANT.omp.local.json` accordingly.
- The exit code is the harness's exit code.

Combine with `--continue` to keep the conversation, or `--no-session` to leave nothing behind:

```sh
clyean -p --continue "And which of those tests fail today?"
clyean -p --no-session "List the fourteen diagrams under .clyean/architecture."
```

For a scripted scaffold without any model interaction beyond the Scaffolder agent, use `clyean scaffold --project-type software-engineering`, which streams the same events to standard output and exits 0 on completion, 2 when the workflow asked for information, and 1 on failure.
