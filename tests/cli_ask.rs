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
    for text in [" \n".to_owned(), "字".repeat(6000), " ".repeat(16385)] {
        let error = client
            .call::<_, ()>(&submit(&[], Some(&text)))
            .await
            .unwrap_err();
        let expected = if text.len() > 16384 {
            "ask_text_invalid"
        } else {
            "ask_answer_missing"
        };
        assert_eq!(error.code(), Some(expected));
    }
    let error = client
        .call::<_, ()>(&submit(&[("retry", Some(&" ".repeat(16385)))], None))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ask_text_invalid"));
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
    let mut background = BackgroundOwner(entry.owner_pid);
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
            assert!(output.status.success(), "{output:?}");
            let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
            if value["result"]["code"] != "ask_pending" {
                return value;
            }
        }
    })
    .await
    .unwrap();
    background.0 = 0;
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

#[test]
fn a_session_delivery_without_a_registry_is_refused_rather_than_unreachable() {
    let config = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["ask", "--question", "Question", "--options", OPTIONS])
        .args(["--mode", "session"])
        .env("AFUI_CONFIG_DIR", config.path())
        .env("AFUI_NO_REGISTRY", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    let event: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(event["error"]["code"], "ask_delivery_invalid");
    assert!(!config.path().join("sessions").exists());
}

#[test]
fn negative_wait_timeout_is_rejected_before_reading_or_collecting_an_answer() {
    let config = tempfile::tempdir().unwrap();
    let directory = config.path().join("asks");
    std::fs::create_dir(&directory).unwrap();
    let id = "abcdef0123456789";
    let result = directory.join(format!("{id}.json"));
    std::fs::write(&result, r#"{"result":{"code":"ask_cancelled"}}"#).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["ask", "wait", id, "--timeout-s", "-1"])
        .env("AFUI_CONFIG_DIR", config.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let event: Value = serde_json::from_slice(&output.stderr).unwrap();
    agent_first_data::assert_strict_event(&event).unwrap();
    assert!(event["error"]["code"].as_str().unwrap().starts_with("cli_"));
    assert!(
        result.exists(),
        "invalid invocations must not collect an answer"
    );
}

#[test]
fn a_detached_result_handoff_survives_the_session_leaving_the_registry() {
    let config = tempfile::tempdir().unwrap();
    let directory = config.path().join("asks");
    std::fs::create_dir(&directory).unwrap();
    let id = "abcdef0123456789";
    let marker = std::fs::File::create_new(directory.join(format!("{id}.pending"))).unwrap();
    marker.lock().unwrap();
    let wait = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(["ask", "wait", id, "--timeout-s", "1"])
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap()
    };
    let pending = wait();
    assert!(pending.status.success(), "{pending:?}");
    let pending: Value = serde_json::from_slice(&pending.stdout).unwrap();
    assert_eq!(pending["result"]["code"], "ask_pending");
    std::fs::write(
        directory.join(format!("{id}.json")),
        r#"{"result":{"code":"ask_cancelled"}}"#,
    )
    .unwrap();
    drop(marker);
    let completed = wait();
    assert!(completed.status.success(), "{completed:?}");
    let completed: Value = serde_json::from_slice(&completed.stdout).unwrap();
    assert_eq!(completed["result"]["code"], "ask_cancelled");
    let collected = wait();
    assert_eq!(collected.status.code(), Some(1));
    let collected: Value = serde_json::from_slice(&collected.stderr).unwrap();
    assert_eq!(collected["error"]["code"], "ask_not_found");
}

fn agent_first_ui_entry(config: &std::path::Path, id: &str) -> UiSessionEntry {
    serde_json::from_slice(
        &std::fs::read(config.join("sessions").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap()
}

/// Stop a detached fixture if an assertion fails before it submits an answer.
struct BackgroundOwner(u32);

impl Drop for BackgroundOwner {
    fn drop(&mut self) {
        if self.0 == 0 {
            return;
        }
        #[cfg(unix)]
        // SAFETY: the pid comes from the worker this fixture started.
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            let _stopped = std::process::Command::new("taskkill")
                .args(["/PID", &self.0.to_string(), "/F"])
                .output();
        }
    }
}

async fn ask_resource(url: &str, relative: &str) -> (String, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (authority, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut socket = tokio::net::TcpStream::connect(authority).await.unwrap();
    socket
        .write_all(
            format!(
                "GET /{path}{relative} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(BUDGET, socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let boundary = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    (
        String::from_utf8_lossy(&response[..boundary]).into_owned(),
        response[boundary + 4..].to_vec(),
    )
}

/// Both source kinds cross the same parent/worker boundary. Readiness means
/// request and image files are no longer needed by either delivery path.
#[tokio::test]
async fn full_request_sources_survive_detach_and_deleted_inputs() {
    use tokio::io::AsyncWriteExt;
    for source_kind in ["stdin", "file"] {
        for detached in [false, true] {
            for large in [false, true] {
                let config = tempfile::tempdir().unwrap();
                let workspace = tempfile::tempdir().unwrap();
                let image = workspace.path().join("a.png");
                let bytes = hex::decode("89504e470d0a1a0a0000000d4948445200000001000000010804000000b51c0c020000000b4944415478da6364f80f00010501012718e3660000000049454e44ae426082").unwrap();
                std::fs::write(&image, &bytes).unwrap();
                let mut request = serde_json::json!({
                    "context": [
                        {"type": "image", "path": image, "title": "Image A"},
                        {"type": "table", "columns": ["Name", "Value"], "rows": [["A", "<b>plain</b>"]]},
                        {"type": "diff", "text": "@@ -1 +1 @@\n-before\n+after"}
                    ],
                    "questions": [{"id": "answer", "question": "Your assessment?", "options": []}]
                });
                if large {
                    request["questions"] = serde_json::json!((0..8).map(|question| serde_json::json!({
                    "id": format!("answer{question}"), "question": "Your assessment?",
                    "options": (0..32).map(|option| serde_json::json!({"id": format!("option{option}"), "label": "Option", "description": "x".repeat(4096)})).collect::<Vec<_>>()
                })).collect::<Vec<_>>());
                    assert!(
                        request.to_string().len() > agent_first_ui::MAX_UI_RUNTIME_MESSAGE_BYTES
                    );
                }
                let source = workspace.path().join("request.json");
                let text = request.to_string();
                std::fs::write(&source, &text).unwrap();
                let source_arg = if source_kind == "stdin" {
                    "stdin".to_owned()
                } else {
                    format!("file:{}", source.display())
                };
                let mut command = Command::new(env!("CARGO_BIN_EXE_afui"));
                command
                    .args(["ask", "--request", &source_arg, "--mode", "session"])
                    .env("AFUI_CONFIG_DIR", config.path())
                    .env("AFUI_NO_REGISTRY", "0")
                    .current_dir(workspace.path())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                if detached {
                    command.arg("--detach");
                }
                let mut child = command.spawn().unwrap();
                if source_kind == "stdin" {
                    child
                        .stdin
                        .take()
                        .unwrap()
                        .write_all(text.as_bytes())
                        .await
                        .unwrap();
                } else {
                    drop(child.stdin.take());
                }
                let mut child = Some(child);
                let ready = if detached {
                    let output = timeout(BUDGET, child.take().unwrap().wait_with_output())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone()
                } else {
                    let mut line = String::new();
                    timeout(
                        BUDGET,
                        BufReader::new(child.as_mut().unwrap().stderr.take().unwrap())
                            .read_line(&mut line),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    serde_json::from_str::<Value>(&line).unwrap()["progress"].clone()
                };
                let id = ready["session_id"].as_str().expect("ready session");
                let entry = agent_first_ui_entry(config.path(), id);
                let mut background = BackgroundOwner(if detached { entry.owner_pid } else { 0 });
                std::fs::remove_file(source).unwrap();
                std::fs::remove_file(image).unwrap();
                let mut client = RuntimeClient::connect(&entry.access_url_secret, "request-test")
                    .await
                    .unwrap();
                let mut state: Value = client.opening_state().await.unwrap().unwrap();
                if large {
                    let reference = state["request_url"]
                        .as_str()
                        .expect("bounded state reference");
                    let (headers, body) = ask_resource(&entry.access_url_secret, reference).await;
                    assert!(headers.starts_with("HTTP/1.1 200"));
                    assert!(body.len() > agent_first_ui::MAX_UI_RUNTIME_MESSAGE_BYTES);
                    state = serde_json::from_slice(&body).unwrap();
                } else {
                    assert_eq!(state["questions"][0]["options"], serde_json::json!([]));
                }
                assert_eq!(state["context"][0]["title"], "Image A");
                assert!(state["context"][0].get("path").is_none());
                let blob = state["context"][0]["blob"]["blob_url"].as_str().unwrap();
                let (headers, body) = ask_resource(&entry.access_url_secret, blob).await;
                assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
                assert!(headers.contains("content-type: image/png"));
                assert_eq!(body, bytes);
                assert_eq!(state["context"][1]["rows"][0][1], "<b>plain</b>");
                assert_eq!(state["context"][2]["text"], "@@ -1 +1 @@\n-before\n+after");
                client
                    .call::<_, ()>(&AskAction::Submit {
                        answers: state["questions"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|question| AskReply {
                                question_id: question["id"].as_str().unwrap().to_owned(),
                                choices: vec![],
                                text: Some("Looks good".into()),
                            })
                            .collect(),
                    })
                    .await
                    .unwrap();
                let result = if detached {
                    let output = Command::new(env!("CARGO_BIN_EXE_afui"))
                        .args(["ask", "wait", id])
                        .env("AFUI_CONFIG_DIR", config.path())
                        .output()
                        .await
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    serde_json::from_slice::<Value>(&output.stdout).unwrap()
                } else {
                    let output = timeout(BUDGET, child.take().unwrap().wait_with_output())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    serde_json::from_slice::<Value>(&output.stdout).unwrap()
                };
                background.0 = 0;
                agent_first_data::assert_strict_event(&result).unwrap();
                agent_first_data::assert_no_lint_findings(&result).unwrap();
                assert_eq!(result["result"]["code"], "ask_answered");
                assert_eq!(result["result"]["answers"][0]["text"], "Looks good");
            }
        }
    }
}

#[test]
fn request_shape_and_text_only_validation_reject_invalid_mixtures() {
    for args in [
        vec![
            "ask",
            "--request",
            "stdin",
            "--question",
            "No",
            "--options",
            "[]",
        ],
        vec!["ask", "--request", "stdin", "--questions", "[]"],
        vec!["ask", "--request", "stdin", "--context", "[]"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let event: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(event["error"]["code"], "cli_unregistered_combination");
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
        .args([
            "ask",
            "--question",
            "Write",
            "--options",
            "[]",
            "--no-custom",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let event: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(event["error"]["code"], "ask_request_invalid");
}

#[cfg(unix)]
#[test]
fn window_process_failures_are_errors_with_recoverable_codes() {
    for (binary, code) in [
        ("/usr/bin/false", "ask_window_wait_failed"),
        ("/usr/bin/true", "ask_window_unavailable"),
    ] {
        let config = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args([
                "ask",
                "--question",
                "Next?",
                "--options",
                OPTIONS,
                "--mode",
                "window",
            ])
            .env("AFUI_BROWSER_BINARY", binary)
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty());
        let event: Value = serde_json::from_str(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        agent_first_data::assert_strict_event(&event).unwrap();
        agent_first_data::assert_no_lint_findings(&event).unwrap();
        assert_eq!(event["kind"], "error");
        assert_eq!(event["error"]["code"], code);
    }
}

#[cfg(unix)]
#[test]
fn a_window_that_loaded_the_question_returns_ask_closed() {
    use std::os::unix::fs::PermissionsExt;
    let config = tempfile::tempdir().unwrap();
    let browser = config.path().join("browser.sh");
    std::fs::write(
        &browser,
        r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in --app=file://*) curl -fsS "$(sed -n 's/.*location\.replace("\(.*\)");.*/\1/p' "${arg#--app=file://}")" >/dev/null || exit 1 ;; esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
        .args([
            "ask",
            "--question",
            "Next?",
            "--options",
            OPTIONS,
            "--mode",
            "window",
        ])
        .env("AFUI_BROWSER_BINARY", browser)
        .env("AFUI_CONFIG_DIR", config.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let event: Value = serde_json::from_slice(&output.stdout).unwrap();
    agent_first_data::assert_strict_event(&event).unwrap();
    assert_eq!(event["result"]["code"], "ask_closed");
    assert!(event["result"].get("answers").is_none());
}
