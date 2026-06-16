use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::AfuiError;
use crate::host::HostStore;
use crate::runtime::apply_afui_message;
use crate::types::{Action, AfuiMessage, AfuiPayload, Snapshot, View, AFUI_VERSION};
use crate::validation::validate_document;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfuiReport {
    pub afui_version: String,
    pub surface_id: String,
    pub surface_title: String,
    pub screen_count: usize,
    pub view_count: usize,
    pub action_count: usize,
    pub has_theme: bool,
    pub view_kinds: BTreeMap<String, usize>,
    pub action_risks: BTreeMap<String, usize>,
    pub validation: ValidationReport,
}

pub fn inspect_snapshot(snapshot: &Snapshot) -> AfuiReport {
    let validation = validation_report(snapshot);
    let mut view_kinds = BTreeMap::new();
    for screen in &snapshot.document.screens {
        for view in &screen.views {
            collect_view_kind(view, &mut view_kinds);
        }
    }

    let mut action_risks = BTreeMap::new();
    for action in &snapshot.document.actions {
        collect_action_risk(action, &mut action_risks);
    }

    AfuiReport {
        afui_version: AFUI_VERSION.to_string(),
        surface_id: snapshot.document.surface.id.clone(),
        surface_title: snapshot.document.surface.title.clone(),
        screen_count: snapshot.document.screens.len(),
        view_count: view_kinds.values().sum(),
        action_count: snapshot.document.actions.len(),
        has_theme: snapshot.theme.is_some(),
        view_kinds,
        action_risks,
        validation,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFailure {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub message_count: usize,
    pub applied_count: usize,
    pub snapshot_count: usize,
    pub state_patch_count: usize,
    pub document_patch_count: usize,
    pub theme_patch_count: usize,
    pub user_action_count: usize,
    pub failure_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<ReplayFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_report: Option<AfuiReport>,
}

pub fn replay_jsonl(input: &str) -> ReplayReport {
    let mut store = HostStore::new();
    let mut report = ReplayReport {
        message_count: 0,
        applied_count: 0,
        snapshot_count: 0,
        state_patch_count: 0,
        document_patch_count: 0,
        theme_patch_count: 0,
        user_action_count: 0,
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
        let message: AfuiMessage = match serde_json::from_str(trimmed) {
            Ok(message) => message,
            Err(error) => {
                push_failure(&mut report, line_number, error);
                continue;
            }
        };

        match &message.payload {
            AfuiPayload::UiSnapshot { .. } => report.snapshot_count += 1,
            AfuiPayload::StatePatch { .. } => report.state_patch_count += 1,
            AfuiPayload::DocumentPatch { .. } => report.document_patch_count += 1,
            AfuiPayload::ThemePatch { .. } => report.theme_patch_count += 1,
            AfuiPayload::UserAction(_) => report.user_action_count += 1,
        }

        let applied = match &message.payload {
            AfuiPayload::UserAction(_) => store.emit_user_action(message).map(|_| ()),
            _ => apply_host_message(&mut store, &message),
        };
        if let Err(error) = applied {
            push_failure(&mut report, line_number, error);
        } else {
            report.applied_count += 1;
        }
    }

    report.failure_count = report.failures.len();
    report.final_report = store.snapshot().map(inspect_snapshot);
    report
}

pub fn replay_messages(messages: &[AfuiMessage]) -> Result<Option<Snapshot>, AfuiError> {
    let mut snapshot = None;
    for message in messages {
        if matches!(&message.payload, AfuiPayload::UserAction(_)) {
            continue;
        }
        snapshot = Some(apply_afui_message(snapshot.as_ref(), message)?);
    }
    Ok(snapshot)
}

fn validation_report(snapshot: &Snapshot) -> ValidationReport {
    match validate_document(&snapshot.document) {
        Ok(()) => ValidationReport {
            valid: true,
            issues: Vec::new(),
        },
        Err(AfuiError::WellFormedness { issues }) => ValidationReport {
            valid: false,
            issues,
        },
        Err(error) => ValidationReport {
            valid: false,
            issues: vec![error.to_string()],
        },
    }
}

fn collect_view_kind(view: &View, view_kinds: &mut BTreeMap<String, usize>) {
    *view_kinds.entry(view.kind.clone()).or_insert(0) += 1;
    for child in view.child_views() {
        collect_view_kind(child, view_kinds);
    }
}

fn collect_action_risk(action: &Action, action_risks: &mut BTreeMap<String, usize>) {
    *action_risks
        .entry(action.risk.as_str().to_string())
        .or_insert(0) += 1;
}

fn push_failure(report: &mut ReplayReport, line: usize, error: impl ToString) {
    report.failures.push(ReplayFailure {
        line,
        message: error.to_string(),
    });
}

fn apply_host_message(store: &mut HostStore, message: &AfuiMessage) -> Result<(), AfuiError> {
    store.apply_message(message).map(|_| ())
}
