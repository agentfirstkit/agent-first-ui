use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::AgentFirstUiError;
use crate::profile::{ProfileValidationReport, ValidationProfile, validate_space_profile};
use crate::store::SpaceStore;
use crate::types::{AgentFirstUiMessage, MessagePayload, PROTOCOL_VERSION, Space};
use crate::validation::validate_space;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
}

/// A summary report of a space's structure and validation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceReport {
    pub protocol_version: String,
    pub owner_count: usize,
    pub capability_count: usize,
    pub unit_count: usize,
    pub unit_kinds: BTreeMap<String, usize>,
    pub capability_risks: BTreeMap<String, usize>,
    pub validation: ValidationReport,
    pub profile_validation: ProfileValidationReport,
}

/// Inspect a space and produce a report.
pub fn inspect_space(space: &Space) -> SpaceReport {
    let validation = match validate_space(space) {
        Ok(()) => ValidationReport {
            valid: true,
            issues: Vec::new(),
        },
        Err(AgentFirstUiError::WellFormedness { issues }) => ValidationReport {
            valid: false,
            issues,
        },
        Err(e) => ValidationReport {
            valid: false,
            issues: vec![e.to_string()],
        },
    };
    let profile_validation = validate_space_profile(space, ValidationProfile::ReferenceHost);

    let mut owner_count = 0;
    let mut capability_count = 0;
    let mut unit_count = 0;
    let mut unit_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut capability_risks: BTreeMap<String, usize> = BTreeMap::new();

    if let Some(obj) = space.as_value().as_object() {
        for section in obj.values() {
            owner_count += 1;
            if let Some(caps) = section.get("capabilities").and_then(|v| v.as_array()) {
                capability_count += caps.len();
                for cap in caps {
                    let risk = cap
                        .get("risk")
                        .and_then(|r| r.as_str())
                        .unwrap_or("destructive");
                    *capability_risks.entry(risk.to_string()).or_insert(0) += 1;
                }
            }
            if let Some(units) = section.get("units").and_then(|v| v.as_array()) {
                for unit in units {
                    unit_count += 1;
                    if let Some(kind) = unit.get("kind").and_then(|k| k.as_str()) {
                        *unit_kinds.entry(kind.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    SpaceReport {
        protocol_version: PROTOCOL_VERSION.to_string(),
        owner_count,
        capability_count,
        unit_count,
        unit_kinds,
        capability_risks,
        validation,
        profile_validation,
    }
}

/// A record of a failed replay step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFailure {
    pub line: usize,
    pub message: String,
}

/// A summary of replaying a stream of messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub message_count: usize,
    pub applied_count: usize,
    pub snapshot_count: usize,
    pub emit_count: usize,
    pub request_count: usize,
    pub failure_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<ReplayFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_report: Option<SpaceReport>,
}

/// Replay a JSONL stream of messages. Each non-blank line must be an
/// `AgentFirstUiMessage`. Snapshots and emits advance the store; requests are
/// recorded without changing space state.
pub fn replay_jsonl(input: &str) -> ReplayReport {
    let mut store = SpaceStore::new();
    let mut report = ReplayReport {
        message_count: 0,
        applied_count: 0,
        snapshot_count: 0,
        emit_count: 0,
        request_count: 0,
        failure_count: 0,
        failures: Vec::new(),
        final_report: None,
    };

    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        report.message_count += 1;

        let message: AgentFirstUiMessage = match serde_json::from_str(trimmed) {
            Ok(msg) => msg,
            Err(e) => {
                push_failure(&mut report, line_number, e);
                continue;
            }
        };

        match &message.payload {
            MessagePayload::Snapshot { .. } => report.snapshot_count += 1,
            MessagePayload::Emit { .. } => report.emit_count += 1,
            MessagePayload::Request { .. } => report.request_count += 1,
        }

        let result = match &message.payload {
            MessagePayload::Request { .. } => store.record_request(message).map(|_| ()),
            _ => store.apply(&message).map(|_| ()),
        };

        match result {
            Ok(()) => report.applied_count += 1,
            Err(e) => push_failure(&mut report, line_number, e),
        }
    }

    report.failure_count = report.failures.len();
    report.final_report = store.space().map(inspect_space);
    report
}

/// Replay a slice of messages, returning the final space state (or None if no
/// snapshot was seen).
pub fn replay_messages(
    messages: &[AgentFirstUiMessage],
) -> Result<Option<Space>, AgentFirstUiError> {
    let mut store = SpaceStore::new();
    for message in messages {
        match &message.payload {
            MessagePayload::Request { .. } => {
                store.record_request(message.clone())?;
            }
            _ => {
                store.apply(message)?;
            }
        }
    }
    Ok(store.space().cloned())
}

fn push_failure(report: &mut ReplayReport, line: usize, error: impl ToString) {
    report.failures.push(ReplayFailure {
        line,
        message: error.to_string(),
    });
}
