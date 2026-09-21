// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The instructions the orchestrator sends to sub-agents at each workflow step.  Every
//! prompt names the step, the inputs, and the verdict schema the agent must end with.

use crate::journal::WorkJournal;

const VERDICT_RULE: &str = "End your reply with exactly one fenced code block tagged json containing the verdict object and nothing after it.";

fn information_section(journal: &WorkJournal) -> String {
    let block = journal.answered_information_block();
    if block.is_empty() {
        String::new()
    } else {
        format!("\n## Information supplied by the user\n\n{block}")
    }
}

fn blocking_issues_section(journal: &WorkJournal) -> String {
    if journal.blocking_issues.is_empty() {
        return String::new();
    }
    let items: Vec<String> = journal
        .blocking_issues
        .iter()
        .map(|issue| format!("- {issue}"))
        .collect();
    format!(
        "\n## Blocking issues discovered during implementation (additional concerns of this change)\n\n{}\n",
        items.join("\n")
    )
}

pub fn director_overview_information(journal: &WorkJournal, plan_path: &str) -> String {
    format!(
        "# Step: decide whether more information is needed for the change plan overview\n\n\
         A change plan is being authored at `{plan_path}`.  The User Assistant relayed the following refined request:\n\n\
         {prompt}\n\n\
         Original words of the user:\n\n{original}\n{information}{blocking}\n\
         Determine whether you need more information from the user before you can write or update the plan's `## Overview` section faithfully.  Do not write the section yet.\n\n\
         Verdict schema: {{\"decision\": \"needs_information\", \"questions\": [\"...\"]}} or {{\"decision\": \"proceed\"}}.  {rule}",
        prompt = journal.prompt,
        original = journal.original_prompt.as_deref().unwrap_or("(same as above)"),
        information = information_section(journal),
        blocking = blocking_issues_section(journal),
        rule = VERDICT_RULE,
    )
}

pub fn director_write_overview(journal: &WorkJournal, plan_path: &str) -> String {
    format!(
        "# Step: write or update the change plan overview\n\n\
         Create or update the `## Overview` section of `{plan_path}` (at most 1,200 characters) so that it summarizes the change described below.  The file already contains the three section headings; edit only the Overview section.\n\n\
         {prompt}\n{information}{blocking}\n\
         Verdict schema: {{\"decision\": \"overview_written\"}}.  {rule}",
        prompt = journal.prompt,
        information = information_section(journal),
        blocking = blocking_issues_section(journal),
        rule = VERDICT_RULE,
    )
}

pub fn specifier_enrich(
    journal: &WorkJournal,
    plan_path: &str,
    review_issues: Option<&[String]>,
) -> String {
    let revision = match review_issues {
        Some(issues) => format!(
            "\n## Misalignment reported by the Software Engineering Director\n\nRevise the section so that every issue below is resolved:\n\n{}\n",
            issues.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")
        ),
        None => String::new(),
    };
    format!(
        "# Step: enrich the change plan with detailed specification changes\n\n\
         Read `{plan_path}`, the current `.clyean/SPECS.md`, and the relevant code, then write the plan's `## Specification Changes` section as your instructions describe.  Edit only that section.\n\n\
         The Software Engineering Director relays this request from the User Assistant:\n\n{prompt}\n{information}{blocking}{revision}\n\
         Verdict schema: {{\"decision\": \"needs_information\", \"questions\": [\"...\"]}} or {{\"decision\": \"completed\", \"summary\": \"...\"}}.  {rule}",
        prompt = journal.prompt,
        information = information_section(journal),
        blocking = blocking_issues_section(journal),
        rule = VERDICT_RULE,
    )
}

pub fn architect_enrich(
    journal: &WorkJournal,
    plan_path: &str,
    review_issues: Option<&[String]>,
) -> String {
    let revision = match review_issues {
        Some(issues) => format!(
            "\n## Misalignment reported by the Software Engineering Director\n\nRevise the section so that every issue below is resolved:\n\n{}\n",
            issues.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")
        ),
        None => String::new(),
    };
    format!(
        "# Step: enrich the change plan with the implementation architecture\n\n\
         Read `{plan_path}` (its Overview and Specification Changes sections), `.clyean/SPECS.md`, the sources under `.clyean/architecture`, and the relevant code, then write the plan's `## Implementation Architecture` section as your instructions describe.  Edit only that section.\n\n\
         The Software Engineering Director relays this request from the User Assistant:\n\n{prompt}\n{information}{blocking}{revision}\n\
         Verdict schema: {{\"decision\": \"needs_information\", \"questions\": [\"...\"]}} or {{\"decision\": \"completed\", \"summary\": \"...\"}}.  {rule}",
        prompt = journal.prompt,
        information = information_section(journal),
        blocking = blocking_issues_section(journal),
        rule = VERDICT_RULE,
    )
}

pub fn director_answer_questions(asking_agent: &str, questions: &[String]) -> String {
    format!(
        "# Step: answer questions from the {asking_agent}\n\n\
         The {asking_agent} needs the following information before continuing:\n\n{}\n\n\
         Answer from the information you already hold when you can.  Otherwise escalate so the User Assistant asks the user.\n\n\
         Verdict schema: {{\"decision\": \"answered\", \"answers\": [\"...\"]}} (one answer per question, in order) or {{\"decision\": \"escalate\", \"questions\": [\"...\"]}}.  {rule}",
        questions.iter().map(|q| format!("- {q}")).collect::<Vec<_>>().join("\n"),
        rule = VERDICT_RULE,
    )
}

pub fn director_review_section(section: &str, plan_path: &str, summary: &str) -> String {
    format!(
        "# Step: review the {section} section of the change plan\n\n\
         Read `{plan_path}` and judge whether its `## {section}` section is aligned with the plan Overview, with the earlier sections, and with the instructions and information supplied by the User Assistant.  The author summarized the section as: {summary}\n\n\
         Verdict schema: {{\"decision\": \"aligned\"}} or {{\"decision\": \"misaligned\", \"issues\": [\"...\"]}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

pub fn specifier_apply_specs(plan_path: &str) -> String {
    format!(
        "# Step: update .clyean/SPECS.md to reflect the change plan\n\n\
         Apply the `## Specification Changes` section of `{plan_path}` to `.clyean/SPECS.md` as your instructions describe.\n\n\
         Verdict schema: {{\"decision\": \"completed\", \"summary\": \"...\"}} or {{\"decision\": \"blocked\", \"issue\": \"...\"}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

pub fn architect_apply_architecture(plan_path: &str, render_failures: Option<&str>) -> String {
    let failures = match render_failures {
        Some(text) => format!("\n## Rendering failures from the previous attempt\n\n{text}\nFix the sources so that every diagram renders.\n"),
        None => String::new(),
    };
    format!(
        "# Step: update .clyean/architecture to reflect the change plan and SPECS.md\n\n\
         Apply the `## Implementation Architecture` section of `{plan_path}` and the latest `.clyean/SPECS.md` to the PlantUML sources under `.clyean/architecture` as your instructions describe.{failures}\n\
         Verdict schema: {{\"decision\": \"completed\", \"summary\": \"...\"}} or {{\"decision\": \"blocked\", \"issue\": \"...\"}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

pub fn programmer_implement(plan_path: &str, review_issues: Option<&str>) -> String {
    let remediation = match review_issues {
        Some(issues) => format!("\n## Issues from the implementation review\n\n{issues}\nResolve every blocker and major issue.\n"),
        None => String::new(),
    };
    format!(
        "# Step: implement the change plan\n\n\
         Implement `{plan_path}` in adherence to `.clyean/SPECS.md` and the materials under `.clyean/architecture`, as your instructions describe.{remediation}\n\
         Verdict schema: {{\"decision\": \"ready_for_review\", \"summary\": \"...\"}} or {{\"decision\": \"blocked\", \"issue\": \"...\", \"suggested_resolution\": \"...\"}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

pub fn architect_review_implementation(plan_path: &str, programmer_summary: &str) -> String {
    format!(
        "# Step: review the implemented changes\n\n\
         Review the implementation of `{plan_path}` as your instructions describe.  The Programmer summarized the work as: {programmer_summary}\n\n\
         Verdict schema: {{\"decision\": \"approved\", \"summary\": \"...\"}} or {{\"decision\": \"issues\", \"issues\": [{{\"severity\": \"blocker|major|minor\", \"location\": \"...\", \"description\": \"...\"}}]}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

pub fn director_research(journal: &WorkJournal) -> String {
    format!(
        "# Research request\n\n\
         The User Assistant relays this research request from the user:\n\n{prompt}\n\n\
         Original words of the user:\n\n{original}\n\n\
         Answer it as your instructions describe.  No verdict block is needed.",
        prompt = journal.prompt,
        original = journal
            .original_prompt
            .as_deref()
            .unwrap_or("(same as above)"),
    )
}

pub fn scaffolder_research(project_type: &str) -> String {
    format!(
        "# Step: author the project's specifications and architecture\n\n\
         Clyean has created the deterministic parts of the scaffold.  The project type is `{project_type}`.  Author `.clyean/SPECS.md` and replace the diagram skeletons under `.clyean/architecture` as your instructions describe.\n\n\
         Verdict schema: {{\"decision\": \"completed\", \"summary\": \"...\"}} or {{\"decision\": \"blocked\", \"issue\": \"...\"}}.  {rule}",
        rule = VERDICT_RULE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{InformationExchange, Phase, WorkKind};
    use clyean_agents::AgentId;

    #[test]
    fn prompts_carry_the_request_information_and_verdict_schema() {
        let mut journal = WorkJournal::new("w".into(), WorkKind::Planning, "Add login".into());
        journal.information.push(InformationExchange {
            request_id: "r".into(),
            asked_by: AgentId::SoftwareEngineeringDirector,
            phase: Phase::PlanningOverviewInformation,
            questions: vec!["Which provider?".into()],
            answers: Some(vec!["OAuth".into()]),
        });
        journal
            .blocking_issues
            .push("The database has no migrations".into());
        let prompt = director_overview_information(&journal, ".clyean/plans/x/v2.md");
        assert!(prompt.contains("Add login"));
        assert!(prompt.contains("A: OAuth"));
        assert!(prompt.contains("no migrations"));
        assert!(prompt.contains("needs_information"));
        let review = director_review_section("Specification Changes", "p", "did things");
        assert!(review.contains("misaligned"));
        let implement = programmer_implement("p", Some("- fix it"));
        assert!(implement.contains("Resolve every blocker"));
    }
}
