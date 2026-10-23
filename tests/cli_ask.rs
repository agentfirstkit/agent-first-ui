//! The real CLI, its private registry and typed runtime, without opening a window.
#![cfg(feature = "test-support")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Stdio;
use std::time::Duration;

use agent_first_ui::UiSessionEntry;
use agent_first_ui::ask::{AskAction, AskChoice, AskReply, AskRequest};
use agent_first_ui::test_support::RuntimeClient;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

const OPTIONS: &str =
    r#"[{"id":"retry","label":"Retry","description":"Run again"},{"id":"stop","label":"Stop"}]"#;
const BUDGET: Duration = Duration::from_secs(10);

struct Question {
    child: Child,
    config: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    entry: UiSessionEntry,
}

impl Question {
    async fn start(extra: &[&str]) -> Self {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_afui"))
            .args([
                "ask",
                "--question",
                "接下来？ <script>ignored</script>",
                "--options",
                OPTIONS,
            ])
            .args(extra)
            .env("AFUI_CONFIG_DIR", config.path())
            .env("AFUI_DELIVERY", "session")
            .env("AFUI_NO_REGISTRY", "0")
            .current_dir(workspace.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut line = String::new();
        timeout(
            BUDGET,
            BufReader::new(child.stderr.take().unwrap()).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let ready: Value = serde_json::from_str(&line).expect("ready progress event");
        assert_eq!(ready["kind"], "progress", "{ready}");
        assert_eq!(ready["progress"]["phase"], "ask_ready");
        assert_eq!(ready["progress"]["mode"], "session");
        assert!(ready["progress"].get("link_url").is_none());
        assert!(!line.contains("<script>"));
        let id = ready["progress"]["session_id"].as_str().unwrap();
        let entry: UiSessionEntry = serde_json::from_slice(
            &std::fs::read(config.path().join("sessions").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert!(entry.waits_for_decision);
        Self {
            child,
            config,
            _workspace: workspace,
            entry,
        }
    }

    async fn client(&self) -> RuntimeClient {
        let mut client = RuntimeClient::connect(&self.entry.access_url_secret, "ask-test")
            .await
            .unwrap();
        let state: AskRequest = client.opening_state().await.unwrap().unwrap();
        assert_eq!(state.questions[0].options.len(), 2);
        assert!(state.questions[0].question.contains("<script>"));
        client
    }

    async fn result(self) -> Value {
        let output = timeout(BUDGET, self.child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let result: Value = serde_json::from_slice(&output.stdout).expect("one result on stdout");
        assert_eq!(result["kind"], "result");
        assert_eq!(result["trace"], serde_json::json!({}));
        assert_eq!(
            std::fs::read_dir(self.config.path().join("sessions"))
                .unwrap()
                .count(),
            0
        );
        result["result"].clone()
    }
}

fn choices(choices: &[(&str, Option<&str>)]) -> Vec<AskChoice> {
    choices
        .iter()
        .map(|(option, note)| AskChoice {
            option_id: (*option).into(),
            note: note.map(str::to_owned),
        })
        .collect()
}

fn submit(chosen: &[(&str, Option<&str>)], text: Option<&str>) -> AskAction {
    AskAction::Submit {
        answers: vec![AskReply {
            question_id: "question".into(),
            choices: choices(chosen),
            text: text.map(str::to_owned),
        }],
    }
}

#[tokio::test]
async fn invalid_answer_can_be_corrected_and_option_is_resolved_on_server() {
    let question = Question::start(&[]).await;
    let mut client = question.client().await;
    let rejected = client
        .call::<_, ()>(&submit(&[("forged", None)], None))
        .await
        .unwrap_err();
    assert_eq!(rejected.code(), Some("ask_option_invalid"));
    client
        .call::<_, ()>(&submit(&[("retry", Some("先看一下日志"))], None))
        .await
        .unwrap();
    let result = question.result().await;
    assert_eq!(result["code"], "ask_answered");
    assert_eq!(
        result["answers"],
        serde_json::json!([{
            "question_id": "question",
            "selected": [{"option_id": "retry", "label": "Retry", "note": "先看一下日志"}]
        }])
    );
}

#[tokio::test]
async fn custom_answer_is_exact_and_empty_or_oversized_text_does_not_complete() {
    let question = Question::start(&[]).await;
    let mut client = question.client().await;
    for text in [" \n".to_owned(), "字".repeat(6000)] {
        let error = client
            .call::<_, ()>(&submit(&[], Some(&text)))
            .await
            .unwrap_err();
        let expected = if text.trim().is_empty() {
            "ask_answer_missing"
        } else {
            "ask_text_invalid"
        };
        assert_eq!(error.code(), Some(expected));
    }
    let text = "  我想试试另一种办法\n ";
    client
        .call::<_, ()>(&submit(&[], Some(text)))
        .await
        .unwrap();
    let result = question.result().await;
    assert_eq!(result["answers"][0]["selected"], serde_json::json!([]));
    assert_eq!(result["answers"][0]["text"], text);
}

#[tokio::test]
async fn disabled_custom_input_is_enforced_on_server_and_cancel_has_no_answer() {
    let question = Question::start(&["--no-custom"]).await;
    let mut client = question.client().await;
    let error = client
        .call::<_, ()>(&submit(&[("retry", Some("a note"))], None))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ask_custom_disabled"));
    client.call::<_, ()>(&AskAction::Cancel).await.unwrap();
    let result = question.result().await;
    assert_eq!(result["code"], "ask_cancelled");
    assert!(result.get("answers").is_none());
}

#[tokio::test]
async fn externally_ending_the_session_returns_closed_without_an_answer() {
    let question = Question::start(&[]).await;
    let output = Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["session", "close", &question.entry.session_id])
        .env("AFUI_CONFIG_DIR", question.config.path())
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result = question.result().await;
    assert_eq!(result["code"], "ask_closed");
    assert!(result.get("answers").is_none());
}

#[test]
fn malformed_requests_fail_before_creating_a_session_or_echoing_input() {
    for options in [
        "private-canary",
        "[]",
        r#"[{"id":"x","label":"X","extra":"private-canary"}]"#,
        r#"[{"id":"x","label":"X"},{"id":"x","label":"Duplicate"}]"#,
    ] {
        let config = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(["ask", "--question", "Question", "--options", options])
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let event: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(event["error"]["code"], "ask_request_invalid");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-canary"));
        assert!(!config.path().join("sessions").exists());
    }
}

#[tokio::test]
async fn several_questions_answer_together_with_choices_and_notes() {
    let questions = r#"[
        {"id":"scope","question":"修哪些？","multiple":true,"options":[{"id":"readme","label":"README","recommended":true},{"id":"subject","label":"Subject"}]},
        {"id":"when","question":"什么时候发布？","allow_custom":false,"options":[{"id":"now","label":"Now"},{"id":"later","label":"Later"}]}
    ]"#;
    let context = r#"[{"type":"paragraph","text":"审查结果"},{"type":"list","items":["一","二"]},{"type":"code","text":"afui ask"}]"#;
    let config = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["ask", "--questions", questions, "--context", context])
        .env("AFUI_CONFIG_DIR", config.path())
        .env("AFUI_DELIVERY", "session")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    timeout(
        BUDGET,
        BufReader::new(child.stderr.take().unwrap()).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    let ready: Value = serde_json::from_str(&line).unwrap();
    let id = ready["progress"]["session_id"].as_str().unwrap();
    let entry: UiSessionEntry = serde_json::from_slice(
        &std::fs::read(config.path().join("sessions").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(entry.subject.as_deref(), Some("修哪些？"));
    let mut client = RuntimeClient::connect(&entry.access_url_secret, "ask-test")
        .await
        .unwrap();
    let state: AskRequest = client.opening_state().await.unwrap().unwrap();
    assert_eq!(state.questions.len(), 2);
    assert_eq!(state.context.len(), 3);
    let reply = |question_id: &str, chosen: &[(&str, Option<&str>)], text: Option<&str>| AskReply {
        question_id: question_id.into(),
        choices: choices(chosen),
        text: text.map(str::to_owned),
    };
    let error = client
        .call::<_, ()>(&AskAction::Submit {
            answers: vec![reply("scope", &[("readme", None)], None)],
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ask_answer_missing"));
    client
        .call::<_, ()>(&AskAction::Submit {
            answers: vec![
                reply("when", &[("now", None)], None),
                reply(
                    "scope",
                    &[("subject", Some("顺手")), ("readme", None)],
                    Some("还有 skill"),
                ),
            ],
        })
        .await
        .unwrap();
    let output = timeout(BUDGET, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["result"]["answers"],
        serde_json::json!([
            {"question_id": "scope", "selected": [
                {"option_id": "subject", "label": "Subject", "note": "顺手"},
                {"option_id": "readme", "label": "README"}
            ], "text": "还有 skill"},
            {"question_id": "when", "selected": [{"option_id": "now", "label": "Now"}]}
        ])
    );
}

#[tokio::test]
async fn a_detached_question_is_collected_later_exactly_once() {
    let config = tempfile::tempdir().unwrap();
    let afui = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(args)
            .env("AFUI_CONFIG_DIR", config.path())
            .env("AFUI_DELIVERY", "session")
            .output()
            .unwrap()
    };
    let started = afui(&[
        "ask",
        "--question",
        "Next?",
        "--options",
        OPTIONS,
        "--detach",
    ]);
    assert!(started.status.success(), "{started:?}");
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    assert_eq!(started["result"]["code"], "ask_started");
    let id = started["result"]["session_id"].as_str().unwrap().to_owned();

    let pending = afui(&["ask", "wait", &id, "--timeout-s", "1"]);
    let pending: Value = serde_json::from_slice(&pending.stdout).unwrap();
    assert_eq!(pending["result"]["code"], "ask_pending");
    assert!(pending["result"]["waiting_s"].as_u64().is_some());

    let entry = agent_first_ui_entry(config.path(), &id);
    let mut client = RuntimeClient::connect(&entry.access_url_secret, "ask-test")
        .await
        .unwrap();
    let _state: Option<AskRequest> = client.opening_state().await.unwrap();
    client
        .call::<_, ()>(&submit(&[("stop", None)], None))
        .await
        .unwrap();

    let answered = timeout(BUDGET, async {
        loop {
            let output = afui(&["ask", "wait", &id, "--timeout-s", "1"]);
            let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
            if value["result"]["code"] != "ask_pending" {
                return value;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(answered["result"]["code"], "ask_answered");
    assert_eq!(
        answered["result"]["answers"][0]["selected"][0]["option_id"],
        "stop"
    );
    let again = afui(&["ask", "wait", &id]);
    assert_eq!(again.status.code(), Some(1));
    let again: Value = serde_json::from_slice(&again.stderr).unwrap();
    assert_eq!(again["error"]["code"], "ask_not_found");
}

fn agent_first_ui_entry(config: &std::path::Path, id: &str) -> UiSessionEntry {
    serde_json::from_slice(
        &std::fs::read(config.join("sessions").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap()
}
