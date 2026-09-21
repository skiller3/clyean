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

If the `.clyean/project.json` scaffold file exists, the User Agent should simply read the `project_type` from it and proceed to use it for any remaining processing.  However, if the Clyean project has not yet been scaffolded, then the User Agent should determine the `project_type` based on its own judgment about the user-provided prompt and other information available to the agent about the project (including information it can find via MCP server connections, existing materials in the project directory, and other referenced resources).  If the project contains the logic or will likely contain the logic for one or more scripts, software programs, software libraries, software applications, or software modules (interpreted in a loose sense) then it should be interpreted to be a `SOFTWARE_ENGINEERING_PROJECT`; otherwise the project should be interepreted to be a `MISCELLANEOUS_PROJECT`.

NOTE: If scaffolding for the project hasn't yet been created, the User Agent's `project_type` determination should later be passed to the Scaffolder agent to ensure the `project_type` value within `.clyean/project.json` is appropriately populated.

There are four possible `prompt_type` values that are mutually exclusive which should be populated in accordance to the table below:

| Condition | Prompt Type |
| --------- | -------- |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting information determined (entirely or partially) by the current state of project materials | `SOFTWARE_ENGINEERING_PROJECT_RESEARCH` |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting the planning of changes to the project's software or some other aspect of the project | `SOFTWARE_ENGINEERING_PROJECT_PLANNING` |
| Project type is `SOFTWARE_ENGINEERING_PROJECT` and user prompt is requesting the implementation of changes to the project's software or some other aspect of the project | `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION` |
| Any scenario not covered by previous conditions in this table | `MISCELLANEOUS` |

## Project Locking

As referenced previously, the User Assistant agent must sometimes "lock the project" or "unlock the project" to prevent the creation of inconsistent state or the compilation of innaccurate information by other Clyean processes running in parallel.  If the project's content is being managed via Git Worktrees in a classic manner, then treat both project locking and unlocking as NO-OPs (since the Software Engineering Director has a reasonable mechanism to facilitate concurrent work); otherwise, use a classic file lock (the project's `.clyean/lock` file) to prevent possibly conflicting concurrent activity by other Clyean user agents.

## User Communication

The User Assistant should provide information to the user just as the user would expect from a standard `omp` chat interaction (this include stream-of-consciousness reasoning, errors, and final results).  When delegating processing to sub-agents (like the Scaffolder or Software Engineering Director), the User Assistant agent should continuously provide the user information from the sub-agents, likely via continuously streaming, sanitizing, and summarizing their activity and output.


# Scaffolder

Responsible for establishing Clyean project scaffold materials based on deterministic logic when possible, as well as deep agentic research about the project.  Among potentially other work, the scaffolder must:

- Initialize Git repo (`.git` directory) if it doesn't exist.
- Ensure Git ignores the scaffold content that must stay out of version control, namely the `.clyean/container-root` directory and the local-only `*.local.<ext>` enhancements and overrides.  Determine whether a path is already ignored by consulting Git's effective ignore rules (e.g. `git check-ignore`) rather than by text-matching `.gitignore` files, so that a rule the user has already placed anywhere in the repository is honored rather than duplicated.  Write any missing rule to `.clyean/.gitignore`, keeping Clyean's exclusions out of a `.gitignore` the user maintains.
- Establish project and agent-level configurations and instructions (i.e. the `.clyean/project.json` file and `.clyean/agents` directory content).
- Setup `.clyean/container-root` and the Clyean agent Podman sandbox.
- Deeply research the project and author its specifications (i.e. `.clyean/SPECS.md`).  The generated materials should reflect the project's status quo, not any future ideal state.
- Deeply research the project and author its current architecture (`.clyean/architecture` directory content).  The generated materials should reflect the project's status quo, not any future ideal state.

# Software Engineering Director

Responsible for coordinating between the deterministic logic execution and various agents necessary to correctly process the three possible types of prompts: `SOFTWARE_ENGINEERING_PROJECT_RESEARCH`, `SOFTWARE_ENGINEERING_PROJECT_PLANNING`, and `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION`.

## Research Prompt Handling

When the prompt type is `SOFTWARE_ENGINEERING_PROJECT_RESEARCH`, the agent should review any useful resources related to the project (e.g. `.clyean/architecture` materials, `.clyean/SPECS.md`, source code, external information sources) and do its best to service the prompt.  From the user's perspective, their experience should largely mirror the one they'd experience if they had typed their prompt directly into `omp`.  The Software Engineering Director agent is not expected to delegate work to sub-agents any differently than an `omp` agent would normally do.

## Planning Prompt Handling

When the prompt type is `SOFTWARE_ENGINEERING_PROJECT_PLANNING`, the Software Engineering Director should create an implementation plan in `.clyean/plans` that adheres to a reasonable naming convention aligned with plan names as composed by `omp` or Claude Code.  The plan it creates should always have 3 high-level sections (each of which may contain as many sub-sections as useful) that are built as follows:

| Section | Content | Clyean Sub-Agent Author |
| ------- | ------- | ----------------------- |
| Overview | Summary of the change that is 1,200 characters in maximum length | Software Engineering Director |
| Specification Changes | Detailed description of the exact changes (if any) to behavior that will be externally legible to human users, agent users, API consumers, and other stakeholders of the software. Much of the content will fit under the description of "system interface" changes, and the content of this section should accurately and comprehensively describe the changes that will be made to `.clyean/SPECS.md` | Specifier |
| Implementation Architecture | Detailed description of the exact changes (if any) to the software system's architecture and the manner in which the requested changes will be incorporated into the architecture. The content of this section should accurately and comprehensively describe the changes that will be made to content within the `.clyean/architecture` directory | Software Architect |

To generate a change plan, Clyean's sub-agents should adhere to the workflow described in `workflow-planning.mmd`.

## Implementation Prompt Handling

When the prompt type is `SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION`, the Software Engineering Director should:
1. Create a change plan in accordance to the preceding "Planning Prompt Handling" section if it doesn't already exist.
2. Implement the relevant change plan in concert with other Clyean sub-agents in adherence to the workflow described in `workflow-implementation.mmd`.

For avoidance of doubt, the `sed4["Software Engineering Director: Re-run the planning workflow that generated the change plan with the additional concern of resolving the blocking issue"]` node in `workflow-implementation.mmd` represents re-execution of the preceding sub-section ("Planning Prompt Handling") with the intent of producing a new version of the change plan.  New versions of change plans should not clobber old versions via in-place plan file edits; instead, Clyean's change plan naming and tracking conventions should gracefully support incremental "versions" of a change plan.

# Specifier

Responsible for ensuring (1) change plans comprehensively describe the updates to `.clyean/SPECS.md` necessary for the software system's interface (e.g. UI, API, CLI) behavior to match the Clyean user's intended changes and (2) `./clyean/SPECS.md` is correctly updated to describe the software system's interface behavior as part of software change implementation.

# Software Architect

Responsible for ensuring the software is:

1. **Correct** – the software comprehensively fulfills its purpose to various stakeholders and the software porject's user interface requirements.
2. **Maintainable** – the software can easily be modified to adapt to changing requirements, fix problems, or improve performance.
3. **Resilient** – the software is highly available, horizontally scales to meet demand, has minimal RPO, has minimal RTO, fulfills disaster recovery obligations, and generally fulfills any other resiliency requirements important to the Clyean user.
4. **Observable** – the software is instrumented to capture and publish the information necessary for any potentially necessary monitoring/alerting, performance analysis, debugging/troubleshooting, and usage analysis.
5. **Efficient** – the software is designed to minimize compute (e.g. CPU, GPU) consumption, memory consumption, network bandwidth, network latency, operational cost, and any other performance characteristics important to the Clyean user.

To fulfill the preceding requirements, the agent must ensure (i) change plans comprehensively describe UML content updates to `.clyean/architecture` materials, (ii) `.clyean/architecture` UML materials are correctly updated to reflect the software's architecture as part of software change implementation, and (iii) implemented changes fulfill the preceding requirements as well as the following important software architecture and coding quality characteristics!

## Important Characteristics of High Quality Software Architecture

Each characteristic below is tagged in italics with the qualities from the preceding list that it contributes to most directly.

### Boundaries and Decomposition

1. **Component boundaries aligned to business capabilities rather than technical layers.**  A change to one capability lands in one component.  Layer-aligned decomposition guarantees the opposite: every meaningful change touches every layer.  _Maintainable_

2. **Explicit bounded contexts with a ubiquitous language (domain-driven design, https://en.wikipedia.org/wiki/Domain-driven_design).**  Within a context each term means exactly one thing, in the specification and in the code.  Relationships between contexts are mapped, and an anti-corruption layer sits wherever a foreign model would otherwise leak in.  _Maintainable_

3. **Low dynamic coupling between components.**  What a component must call at runtime to service a request is minimized, as distinct from what it needs in order to start.  Runtime coupling is what determines composite availability, tail latency, and blast radius.  _Resilient, Efficient_

4. **Explicitly directed, acyclic dependencies between components.**  The dependency direction is a stated design property rather than an emergent one, and no cycles exist.  Components in a cycle must be built, tested, and released together regardless of how they are packaged.  _Maintainable_

5. **Component granularity justified by a named force.**  Each split is attributable to independent scaling, fault isolation, differing data sensitivity, divergent rates of change, or separate ownership.  Splits made without such a force pay distribution cost and return nothing.  _Maintainable, Efficient_

6. **Stateless, shared-nothing (https://en.wikipedia.org/wiki/Shared-nothing_architecture) request handling, with state held in purpose-built stores.**  Any instance can serve any request, which is the precondition for horizontal scaling, rolling deployment, and instance-level failure being a non-event.  _Resilient, Efficient_

7. **Structure that matches ownership boundaries (Conway's law, https://en.wikipedia.org/wiki/Conway%27s_law).**  Component boundaries coincide with the boundaries of the groups that build and operate them.  Where the two disagree, the mismatch shows up permanently as coordination cost.  _Maintainable_

### Contracts, Evolution, and Deployability

8. **Versioned, backward-compatible published contracts at every boundary.**  APIs, event and message schemas, exposed views, file formats, and command-line surfaces are treated as contracts, changed compatibly or versioned with both live until consumers migrate.  _Maintainable, Resilient_

9. **Schemas designed for forward and backward compatibility.**  Encodings tolerate readers and writers of different vintages in both directions, which is what makes rolling deployment possible at all.  _Maintainable, Resilient_

10. **Expand-and-contract capability for every stateful migration.**  New shape added, backfilled, dual-written, readers moved, old shape removed in a later release.  The absence of this capability is what turns a schema change into scheduled downtime.  _Maintainable, Resilient_

11. **Independent deployability of each component.**  Any component can be released without coordinating a simultaneous release of others.  Components that must deploy together are one component with extra network hops between its parts.  _Maintainable, Resilient_

12. **Deployment decoupled from release (feature toggles, https://en.wikipedia.org/wiki/Feature_toggle).**  Exposing behavior is a runtime decision made separately from shipping the artifact, so reverting behavior does not require redeploying.  _Resilient, Maintainable_

13. **Architecturally significant decisions recorded (https://en.wikipedia.org/wiki/Architectural_decision) with rationale and consequences.**  Context, alternatives rejected, and consequences are written down.  A constraint whose rationale is unrecorded is a constraint that will be violated by someone who never knew it existed.  _Maintainable_

14. **Automated fitness functions guarding the characteristics that must not regress.**  Dependency direction, layer access rules, latency budgets, and artifact size are asserted by tests that run continuously, so architectural drift is caught mechanically rather than during review.  _Maintainable_

15. **Reversibility, with irreversible decisions deferred and isolated.**  Where options are close, the cheaper-to-undo one is taken, and the decisions that genuinely cannot be unwound are both delayed and confined behind an interface.  _Maintainable_

### Data Architecture

16. **Exactly one owning component per data store.**  No component writes to another's storage, and no schema is shared for write access.  Single ownership is what leaves the owner free to change its internals at all.  _Maintainable, Resilient_

17. **A designated system of record, with every other copy explicitly derived and rebuildable.**  Caches, search indexes, materialized views, read models, and extracts are labeled as derived and can be reconstructed from the system of record after loss or corruption.  _Resilient, Maintainable_

18. **Consistency guarantees (https://en.wikipedia.org/wiki/Consistency_model) stated per read path.**  Each path specifies whether it offers read-your-writes, monotonic reads, consistent prefix reads, snapshot isolation, or linearizability, and the behavior built on it is designed to be acceptable under that guarantee.  _Resilient, Maintainable_

19. **Transaction boundaries contained within a single ownership boundary.**  Atomicity is required only within one owner, and a long-running transaction (https://en.wikipedia.org/wiki/Long-running_transaction) with explicit compensating actions is used only where a business process genuinely spans owners.  _Resilient, Maintainable_

20. **A data model selected for the actual access patterns.**  The store type matches the reads and writes the system performs, and the queries the choice makes expensive are known and accepted rather than discovered in production.  _Efficient_

21. **Explicit replication (https://en.wikipedia.org/wiki/Replication_%28computing%29) and partitioning (https://en.wikipedia.org/wiki/Partition_%28database%29) strategy, including skew and hot-spot handling.**  The leader model, failover behavior, partition key choice, rebalancing trigger, and hot-spot mitigation are stated design properties rather than configuration discovered after an incident.  _Resilient, Efficient_

### Availability and Fault Tolerance

22. **Design assumptions free of the fallacies of distributed computing (https://en.wikipedia.org/wiki/Fallacies_of_distributed_computing).**  The design does not assume a reliable network, zero latency, infinite bandwidth, a fixed topology, zero transport cost, or agreeing clocks.  Designs that assume any of these fail in ways that are expensive to reproduce.  _Resilient_

23. **A finite timeout on every cross-process interaction.**  Network calls, pool checkouts, lock acquisitions, and queue reads are all bounded.  An unbounded wait is the exact mechanism by which a downstream slowdown becomes an upstream outage.  _Resilient_

24. **Circuit breaking (https://en.wikipedia.org/wiki/Circuit_breaker_design_pattern) at every integration point.**  A failing dependency stops being called and the dependent feature degrades deliberately.  Continuing to retry into a failing dependency is what converts its outage into a cascading one.  _Resilient_

25. **Bulkheaded (https://en.wikipedia.org/wiki/Bulkhead_pattern) resource pools that contain failure domains.**  Threads, connections, and capacity are partitioned per dependency, tenant, or traffic class, so one saturated path cannot consume what every other path needs.  _Resilient_

26. **Defined graceful degradation for each dependency failure.**  For every dependency, the reduced-function behavior on its loss is a designed and tested outcome rather than whatever happens to occur.  _Resilient_

27. **Load shedding and back pressure in place of unbounded queueing.**  Excess demand is rejected early with a clear signal to the caller.  An unbounded queue does not absorb overload; it converts it into a state where everything times out and nothing completes.  _Resilient, Efficient_

28. **Retry with exponential backoff (https://en.wikipedia.org/wiki/Exponential_backoff) and jitter, confined to idempotent operations.**  Retries are bounded, spread in time to avoid synchronized surges, and applied only where repetition is safe.  _Resilient_

29. **Redundancy sufficient to eliminate single points of failure (https://en.wikipedia.org/wiki/Single_point_of_failure), and horizontal scalability (https://en.wikipedia.org/wiki/Scalability).**  No individual instance, zone, or dependency is uniquely required, and capacity is added by adding instances rather than by enlarging one.  _Resilient, Efficient_

### Recovery and Continuity

30. **Steady-state operation: automated reclamation for everything that accumulates.**  Logs, temporary files, session data, audit rows, cache entries, and orphaned records all have defined retention and automated purge.  A system needing periodic human intervention to stay healthy has a design defect, not an operational one.  _Resilient, Efficient_

31. **Crash-tolerant startup: any component restartable from an arbitrary state.**  Restart is the primary recovery mechanism and requires no manual reconciliation, which makes restarting a safe first response during an incident.  _Resilient_

32. **Zero-downtime deployment with a tested rollback path.**  Rollout and rollback both occur without interrupting service, and rollback has been exercised rather than assumed.  _Resilient, Maintainable_

33. **Recovery point and recovery time objectives defined, and disaster recovery (https://en.wikipedia.org/wiki/IT_disaster_recovery) proven by exercise.**  Acceptable data loss and restoration time are stated numbers, and the procedure that meets them has actually been run.  An untested recovery plan is an estimate.  _Resilient_

### Observability

34. **Correlated telemetry: metrics, structured logs, and distributed traces (https://en.wikipedia.org/wiki/Tracing_%28software%29) sharing one identifier.**  The three signals can be pivoted between for a single unit of work, which is what turns three data sources into one diagnostic capability.  _Observable_

35. **Self-describing instances.**  Every instance reports, on demand, its build version, effective configuration, dependency health, and readiness.  A fleet that cannot be interrogated can only be debugged by inference.  _Observable_

36. **Service level objectives with instrumented error budgets, and alerting on symptoms.**  Targets are expressed as measured user-facing indicators, and alerts fire on degraded user experience rather than on every internal cause.  _Observable, Resilient_

37. **Usage and business-event instrumentation distinct from operational telemetry.**  Product and adoption questions are answerable from purpose-built events with their own retention, rather than by mining operational logs that were never designed to answer them.  _Observable_

### Efficiency and Cost

38. **Efficiency budgets attached to an identified critical path.**  Latency, throughput, memory, and operating cost targets are stated, the critical path is known, and the system is measured against the budget at that path.  _Efficient_

39. **A caching strategy with explicit invalidation rules and staleness bounds.**  Each cache has a stated purpose, a defined maximum staleness, and a known invalidation mechanism, at the tier where it actually pays.  Caching added without those three trades correctness for a speedup nobody measured.  _Efficient_

40. **Elasticity: provisioned capacity that tracks demand.**  Resources scale with load rather than being fixed at peak, so cost follows utilization.  This is also what makes absorbing an unexpected surge a scaling event instead of an outage.  _Efficient, Resilient_

## Important Characteristics of High Quality Software Code

Each characteristic below is tagged in italics with the qualities from the preceding list that it contributes to most directly.

### Structure and Modularity

1. **Adherence to SOLID (https://en.wikipedia.org/wiki/SOLID) to the extent reasonable.**  Single responsibility, open/closed (https://en.wikipedia.org/wiki/Open%E2%80%93closed_principle), Liskov substitution (https://en.wikipedia.org/wiki/Liskov_substitution_principle), interface segregation (https://en.wikipedia.org/wiki/Interface_segregation_principle), and dependency inversion (https://en.wikipedia.org/wiki/Dependency_inversion_principle), applied where each reduces the cost of change.  Applied mechanically, the same five produce interface proliferation that costs more than it returns, which is what "to the extent reasonable" is doing.  _Maintainable_

2. **High cohesion (https://en.wikipedia.org/wiki/Cohesion_%28computer_science%29) within each unit.**  The elements of a function, class, or module serve one purpose and change for one reason.  Functional cohesion is the target; coincidental, logical, and temporal cohesion are the degraded forms.  _Maintainable_

3. **Low coupling (https://en.wikipedia.org/wiki/Coupling_%28computer_programming%29) between units.**  A unit depends on as few others as possible, and on their interfaces rather than their internals.  Coupling is what converts a local change into a distributed one and a local failure into a distributed one.  _Maintainable, Resilient_

4. **Information hiding (https://en.wikipedia.org/wiki/Information_hiding) behind narrow interfaces.**  Each module encapsulates a design decision that its callers cannot observe and do not depend on.  The inverse condition, where one design decision is reflected in several modules, means the decision can no longer be changed in one place.  _Maintainable_

5. **Depth: substantial functionality behind a small interface.**  The ratio of functionality provided to interface complexity imposed is high.  A class whose interface is nearly as complex as its implementation transfers cost to its callers without hiding anything from them.  _Maintainable_

6. **Separation of concerns (https://en.wikipedia.org/wiki/Separation_of_concerns).**  Distinct concerns (business rules, persistence, presentation, transport, configuration, logging) are addressable and testable independently.  _Maintainable_

7. **Dependencies injected rather than constructed in place (dependency injection, https://en.wikipedia.org/wiki/Dependency_injection).**  A unit receives its collaborators instead of naming and building them, which is what permits substitution for testing, fault injection, and instrumentation.  _Maintainable, Observable_

8. **An acyclic module dependency graph (acyclic dependencies principle, https://en.wikipedia.org/wiki/Acyclic_dependencies_principle).**  Dependencies run in one direction with no cycles.  A cycle makes the participating modules one indivisible unit for purposes of building, testing, reasoning, and release.  _Maintainable_

9. **Compliance with the Law of Demeter (https://en.wikipedia.org/wiki/Law_of_Demeter) to the extent reasonable.**  A unit talks to its immediate collaborators rather than navigating through them into a third party's internals.  Applied absolutely it generates wrapper methods that hide nothing, so the useful form is a bias rather than a rule.  _Maintainable_

10. **Absence of duplicated knowledge (DRY, https://en.wikipedia.org/wiki/Don%27t_repeat_yourself).**  Each piece of knowledge has one authoritative representation.  The target is duplicated knowledge, not duplicated text: two identical lines that encode different decisions are not a violation, and two dissimilar implementations of one rule are.  _Maintainable_

11. **Absence of speculative generality (YAGNI, https://en.wikipedia.org/wiki/You_aren%27t_gonna_need_it).**  Abstractions, extension points, configuration surfaces, and compatibility shims exist only for requirements that exist.  Unused generality is complexity paid now against a benefit that usually never arrives.  _Maintainable_

### Clarity and Predictability

12. **Adherence to the Principle of Least Astonishment (https://en.wikipedia.org/wiki/Principle_of_least_astonishment) (POLA) to the extent reasonable.**  Names, signatures, return values, defaults, and side effects behave the way a competent reader of the surrounding code would predict.  Every surprise is a defect waiting for the reader who does not happen to investigate.  To help minimize surprise, adhere to programming language community and framework community norms and idioms to the extent reasonable.  _Maintainable_

13. **Simplicity proportionate to the problem (KISS, https://en.wikipedia.org/wiki/KISS_principle).**  The implementation is no more elaborate than the problem requires, and cleverness that buys nothing measurable is absent.  _Maintainable_

14. **Low cyclomatic complexity (https://en.wikipedia.org/wiki/Cyclomatic_complexity) per unit.**  Few independent paths through any single function, which bounds both the reasoning a reader must do and the number of tests required to cover it.  _Maintainable_

15. **Self-documenting (https://en.wikipedia.org/wiki/Self-documenting_code) naming.**  Names state what an entity is and, for a variable, what invariant it holds.  Names too vague to be wrong (`data`, `info`, `process`, `manager`) signal that the author lacked a clear model of the thing named.  _Maintainable_

16. **Internal consistency of convention, vocabulary, and layout.**  One concept keeps one name throughout, and one convention is applied uniformly.  A consistent codebase is cheaper to work in than one with a better convention applied unevenly.  _Maintainable_

17. **Comments confined to non-obvious rationale.**  Comments carry why, invariants, units, hidden constraints, and references for specific workarounds.  Comments that restate adjacent code impose maintenance cost and supply no information, and they go stale silently.  _Maintainable_

18. **Command-query separation (https://en.wikipedia.org/wiki/Command%E2%80%93query_separation) to the extent reasonable.**  A function either changes observable state or returns a value, not both.  The exceptions that earn their keep (atomic pop, compare-and-swap) are the ones where the combination is the point.  _Maintainable_

19. **Absence of recognized code smells (https://en.wikipedia.org/wiki/Code_smell).**  Long function, long parameter list, feature envy, shotgun surgery, divergent change, data clumps, primitive obsession, repeated switches, message chains, and large class are named, recognizable conditions with known remedies.  Their absence is measurable in a way that "good code" is not.  _Maintainable_

### State, Data, and Types

20. **Immutability (https://en.wikipedia.org/wiki/Immutable_object) by default.**  Values do not change after construction unless mutation is required and justified.  Immutable data is safe to share across threads and cannot be invalidated by a distant caller.  _Maintainable, Resilient_

21. **Referential transparency (https://en.wikipedia.org/wiki/Referential_transparency) in the computational core.**  The logic that computes is separated from the logic that performs input and output, so the computational part is a function of its arguments alone.  Such code is trivially testable, safely memoizable, and safely parallelizable.  _Maintainable, Efficient_

22. **Narrow variable scope and absence of global mutable state.**  Data is declared in the smallest scope that works and initialized where declared.  Global mutable state couples every unit that touches it and makes behavior depend on execution history, which is what makes the resulting defects unreproducible.  _Maintainable, Resilient_

23. **Type safety strong enough to make invalid states unrepresentable.**  Types and constructors are constrained so that an object cannot exist in an inconsistent state, which moves whole categories of error from runtime to compile time.  _Maintainable_

24. **Domain-specific types in place of bare primitives.**  A monetary amount, identifier, duration, or percentage carries a type that encodes its rules, rather than travelling as an untyped integer or string that can be silently transposed with its neighbor.  _Maintainable_

25. **Thread safety (https://en.wikipedia.org/wiki/Thread_safety) and reentrancy (https://en.wikipedia.org/wiki/Reentrancy_%28computing%29) explicitly stated and enforced.**  Each unit documents its concurrency contract and the code upholds it.  Concurrency assumptions that are implicit are assumptions that will eventually be violated by a caller who never saw them.  _Resilient, Efficient_

### Failure Behavior

26. **Explicit contracts: preconditions, postconditions, and invariants (design by contract, https://en.wikipedia.org/wiki/Design_by_contract).**  Obligations are stated and checked at the boundary where a violation is still attributable to a specific caller.  _Maintainable, Resilient_

27. **Fail-fast (https://en.wikipedia.org/wiki/Fail-fast_system) behavior on violated invariants.**  Detection of an impossible state halts at the point of detection with a precise message rather than continuing on corrupt data.  A crash with an accurate cause is far cheaper than corruption surfacing three systems downstream.  _Resilient, Observable_

28. **Errors designed out of existence where the interface permits.**  The interface is shaped so that common cases are not errors at all: deleting a missing item succeeds, an out-of-range slice clamps, an empty result is an ordinary value.  Every error a caller must handle is complexity multiplied across every caller.  _Maintainable, Resilient_

29. **Idempotence (https://en.wikipedia.org/wiki/Idempotence) of any operation that may be retried.**  Repeating an operation produces the same result as performing it once.  This is the property that makes retry, replay, and at-least-once delivery safe, and without it those mechanisms cause duplicate effects.  _Resilient_

30. **Deterministic, scope-bound resource release (RAII, https://en.wikipedia.org/wiki/Resource_acquisition_is_initialization, or the language's equivalent).**  Memory, file handles, connections, locks, subscriptions, and timers are released on every path including every error path, through a scoped construct rather than manual cleanup at each exit.  _Resilient, Efficient_

31. **Validation at trust boundaries and least-privilege (https://en.wikipedia.org/wiki/Principle_of_least_privilege) execution.**  Untrusted input is validated where it enters and encoded where it is used, and each component runs with the minimum authority it needs.  Both bound the blast radius of a defect or a compromise.  _Resilient_

### Diagnosability

32. **Structured, machine-parseable log events.**  Events carry named fields with stable types rather than interpolated prose, so they can be filtered, aggregated, and correlated without regular expressions over free text.  _Observable_

33. **Correlation identifiers propagated through every call path.**  A request identifier travels through synchronous calls, asynchronous handoffs, and background work, so that one unit of work can be reconstructed end to end.  _Observable_

34. **Errors that carry actionable context.**  An exception or error value names what was attempted, on what input, and why it failed, and preserves the underlying cause rather than discarding it.  _Observable_

35. **Instrumentation at unit boundaries.**  Significant operations emit counts, latencies, and outcomes as a matter of course rather than being retrofitted after the first incident that needed them.  _Observable_

36. **Testability: units exercisable in isolation and deterministically.**  A unit can be constructed and driven without standing up its whole environment, and repeated runs produce identical results.  Untestable code is also undiagnosable code, because both depend on the ability to isolate.  _Maintainable, Observable_

### Performance and Resource Behavior

37. **Algorithmic complexity (https://en.wikipedia.org/wiki/Time_complexity) appropriate to expected input sizes.**  The complexity class of each hot operation is known and suits the realistic range of n, rather than suiting the range that happened to appear in development data.  _Efficient_

38. **Bounded growth of every collection, buffer, cache, and retry count.**  Nothing accumulates without a limit.  Anything unbounded is an out-of-memory failure waiting for an input nobody anticipated.  _Efficient, Resilient_

39. **Access patterns that respect locality of reference (https://en.wikipedia.org/wiki/Locality_of_reference) and avoid repeated round trips.**  Data is accessed in batches and in contiguous order where that matters, and the N+1 query and per-item remote call patterns are absent.  _Efficient_

40. **Optimization grounded in measurement of an identified critical path (Amdahl's law, https://en.wikipedia.org/wiki/Amdahl%27s_law).**  Performance work is directed by profiling and verified by before-and-after measurement.  Optimization applied off the critical path trades clarity away for no measurable return.  _Efficient, Maintainable_

# Programmer

Responsible for implementing (i.e. authoring) the project's software such that it's:

1. **Correct** – the software comprehensively fulfills its purpose to various stakeholders and the software project's user interface requirements.
2. **Adherent to Code Best Practices** – the software's code adheres to the best practices mentioned in the "Important Characteristics of High Quality Software Code" sub-section of the "Software Architect" section of this document.
2. **Architecturally Aligned** – the software complies with the architectural specifications in `.clyean/architecture`.

# Code Reviewer

Placeholder – do not implement agent yet!

# Automated Test Programmer

Placeholder – do not implement agent yet!

# Mutant Killer

Placeholder – do not implement agent yet!

# CRAP Reducer

Placeholder – do not implement agent yet!

# QA Tester

Placeholder – do not implement agent yet!

# CI/CD Programmer

Placeholder – do not implement agent yet!

# Deployment Analyst

Placeholder – do not implement agent yet!

# Security Engineer

Placeholder – do not implement agent yet!

# White-Hat Hacker

Placeholder – do not implement agent yet!

# Documentation Author

Placeholder – do not implement agent yet!