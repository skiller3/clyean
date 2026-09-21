// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Extraction of the verdict object every agent instruction asks for: the last fenced
//! `json` code block of the reply.

use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{OrchestratorError, Result};

/// Finds the last fenced JSON block in `reply` and parses it as `T`.  Falls back to the
/// last top-level JSON object in the text when no fence is present.
pub fn extract_verdict<T: DeserializeOwned>(agent: &str, reply: &str) -> Result<T> {
    let candidate = last_fenced_json(reply)
        .or_else(|| last_bare_object(reply))
        .ok_or_else(|| OrchestratorError::Verdict {
            agent: agent.to_string(),
            reason: "no JSON verdict block found".to_string(),
        })?;
    let value: Value =
        serde_json::from_str(&candidate).map_err(|e| OrchestratorError::Verdict {
            agent: agent.to_string(),
            reason: format!("verdict is not valid JSON: {e}"),
        })?;
    serde_json::from_value(value).map_err(|e| OrchestratorError::Verdict {
        agent: agent.to_string(),
        reason: format!("verdict has the wrong shape: {e}"),
    })
}

fn last_fenced_json(reply: &str) -> Option<String> {
    let fence =
        Regex::new(r"(?s)```(?:json|JSON)?[ \t]*\r?\n(.*?)\r?\n[ \t]*```").expect("valid regex");
    fence
        .captures_iter(reply)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|block| block.starts_with('{'))
        .last()
}

fn last_bare_object(reply: &str) -> Option<String> {
    let start = reply.rfind("{\"decision\"")?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in reply[start..].char_indices() {
        match c {
            '\\' if in_string => {
                escaped = !escaped;
                continue;
            }
            '"' if !escaped => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(reply[start..start + offset + 1].to_string());
                }
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(tag = "decision", rename_all = "snake_case")]
    enum Decision {
        NeedsInformation { questions: Vec<String> },
        Proceed,
    }

    #[test]
    fn takes_the_last_fenced_block() {
        let reply = "Thinking...\n```json\n{\"decision\": \"proceed\"}\n```\nActually:\n```json\n{\"decision\": \"needs_information\", \"questions\": [\"Which DB?\"]}\n```\n";
        let verdict: Decision = extract_verdict("sed", reply).unwrap();
        assert_eq!(
            verdict,
            Decision::NeedsInformation {
                questions: vec!["Which DB?".into()]
            }
        );
    }

    #[test]
    fn falls_back_to_a_bare_object_and_reports_missing_verdicts() {
        let reply = "No fence here but {\"decision\": \"proceed\"} at the end.";
        let verdict: Decision = extract_verdict("sed", reply).unwrap();
        assert_eq!(verdict, Decision::Proceed);
        let error = extract_verdict::<Decision>("sed", "nothing").unwrap_err();
        assert!(error.to_string().contains("no JSON verdict"));
        let wrong = extract_verdict::<Decision>("sed", "```json\n{\"decision\": \"dance\"}\n```")
            .unwrap_err();
        assert!(wrong.to_string().contains("wrong shape"));
    }
}
