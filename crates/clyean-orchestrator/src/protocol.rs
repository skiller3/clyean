// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Wire types of the orchestrator protocol (`docs/reference/orchestrator-protocol.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use clyean_project::ProjectType;

/// The three prompt types the Software Engineering Director handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PromptType {
    SoftwareEngineeringProjectResearch,
    SoftwareEngineeringProjectPlanning,
    SoftwareEngineeringProjectImplementation,
}

impl PromptType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SoftwareEngineeringProjectResearch => "SOFTWARE_ENGINEERING_PROJECT_RESEARCH",
            Self::SoftwareEngineeringProjectPlanning => "SOFTWARE_ENGINEERING_PROJECT_PLANNING",
            Self::SoftwareEngineeringProjectImplementation => {
                "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION"
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScaffoldParams {
    pub project_type: ProjectType,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkStartParams {
    pub prompt_type: PromptType,
    pub prompt: String,
    #[serde(default)]
    pub original_prompt: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProvideInformationParams {
    pub work_id: String,
    pub request_id: String,
    pub answers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkIdParams {
    pub work_id: String,
}

/// The immediate, id-correlated reply to a request.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Response {
    Result { id: String, result: Value },
    Error { id: String, error: ProtocolError },
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

impl Response {
    pub fn result(id: &str, result: Value) -> Self {
        Self::Result {
            id: id.to_string(),
            result,
        }
    }

    pub fn error(id: &str, code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            id: id.to_string(),
            error: ProtocolError {
                code: code.to_string(),
                message: message.into(),
            },
        }
    }
}

/// An event streamed after the immediate response of a streaming method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamedEvent {
    Progress {
        work_id: String,
        seq: u64,
        agent: String,
        phase: String,
        text: String,
    },
    AgentOutput {
        work_id: String,
        seq: u64,
        agent: String,
        phase: String,
        text: String,
    },
    InformationRequested {
        work_id: String,
        seq: u64,
        request_id: String,
        questions: Vec<String>,
        context: String,
    },
    Completed {
        work_id: String,
        seq: u64,
        summary: String,
        artifacts: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Failed {
        work_id: String,
        seq: u64,
        code: String,
        message: String,
    },
}

impl StreamedEvent {
    /// Whether the server closes the connection after sending this event.
    pub fn closes_connection(&self) -> bool {
        matches!(
            self,
            Self::InformationRequested { .. } | Self::Completed { .. } | Self::Failed { .. }
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }

    pub fn work_id(&self) -> &str {
        match self {
            Self::Progress { work_id, .. }
            | Self::AgentOutput { work_id, .. }
            | Self::InformationRequested { work_id, .. }
            | Self::Completed { work_id, .. }
            | Self::Failed { work_id, .. } => work_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_the_documented_tag() {
        let event = StreamedEvent::Progress {
            work_id: "w".into(),
            seq: 3,
            agent: "orchestrator".into(),
            phase: "planning.overview".into(),
            text: "hello".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "progress");
        assert_eq!(json["seq"], 3);
        let completed = StreamedEvent::Completed {
            work_id: "w".into(),
            seq: 4,
            summary: "s".into(),
            artifacts: vec![],
            plan: None,
        };
        assert!(completed.closes_connection());
        assert!(!serde_json::to_string(&completed).unwrap().contains("plan"));
    }

    #[test]
    fn requests_and_prompt_types_parse_from_the_wire_form() {
        let request: Request = serde_json::from_str(
            r#"{"id":"r1","method":"work.start","params":{"prompt_type":"SOFTWARE_ENGINEERING_PROJECT_PLANNING","prompt":"p"}}"#,
        )
        .unwrap();
        let params: WorkStartParams = serde_json::from_value(request.params).unwrap();
        assert_eq!(
            params.prompt_type,
            PromptType::SoftwareEngineeringProjectPlanning
        );
        assert_eq!(params.plan, None);
        let response = Response::error("r1", "unknown_method", "nope");
        assert_eq!(
            serde_json::to_value(&response).unwrap()["error"]["code"],
            "unknown_method"
        );
    }
}
