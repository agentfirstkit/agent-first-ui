use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_first_data::{
    ArgSpec, CliEmitter, CliValue, Combination, CommandSpec, OutputFormat, OutputTo,
    ResolvedInvocation,
};
use agent_first_ui::Outcome;
use agent_first_ui::ask::{AskBlock, AskOption, AskQuestion, AskRequest, AskResponse};
use agent_first_ui::cli::{UiDeliveryOffer, delivery_of, ready_event_revealing_link};
use serde_json::{Value, json};

/// Set on the background process `--detach` starts. It writes its terminal
/// result to the question's result file instead of stdout.
const DETACHED_ENV: &str = "AFUI_ASK_DETACHED";
/// The id the single-question shape gives its one question.
const SINGLE_QUESTION_ID: &str = "question";
const WAIT_POLL: Duration = Duration::from_millis(250);

pub(super) fn ask_command() -> CommandSpec {
    CommandSpec::new(["ask"])
        .about("Ask a person one or more questions with options and written answers; wait for the answer.")
        .arg(ArgSpec::option("--question", "TEXT")
            .about("The question, displayed as plain text; its answer has question_id `question`"))
        .arg(ArgSpec::option("--options", "JSON").about(
            "JSON array of 1–32 options: unique id, label, optional description, optional recommended",
        ))
        .arg(ArgSpec::flag("--multiple").about("Allow choosing several options"))
        .arg(
            ArgSpec::flag("--no-custom")
                .about("Offer only the supplied options; disable written answers and notes"),
        )
        .arg(ArgSpec::option("--questions", "JSON").about(
            "JSON array of 1–8 questions: unique id, question, options, optional multiple, optional allow_custom",
        ))
        .arg(ArgSpec::option("--context", "JSON").about(
            "JSON array of background blocks shown above the questions: paragraph {text}, list {items}, or code {text}",
        ))
        .arg(UiDeliveryOffer::WithLink.arg("--mode"))
        .arg(ArgSpec::flag("--detach").about(
            "Return once the question is ready; collect the answer later with `afui ask wait`",
        ))
        .combination(
            Combination::new("ask")
                .action("ask")
                .about("One question from --question and --options")
                .required(["question", "options"])
                .optional(["multiple", "no_custom", "context", "mode", "detach"])
                .output(super::output()),
        )
        .combination(
            Combination::new("ask-many")
                .action("ask")
                .about("Several questions on one page from --questions")
                .required(["questions"])
                .optional(["context", "mode", "detach"])
                .output(super::output()),
        )
}

pub(super) fn wait_command() -> CommandSpec {
    CommandSpec::new(["ask", "wait"])
        .about("Wait for the answer to a question started with `afui ask --detach`.")
        .arg(
            ArgSpec::positional("session_id", 0, "SESSION_ID")
                .about("Session identifier from the `ask_started` result"),
        )
        .arg(ArgSpec::option_i64("--timeout-s", "SECONDS").about(
            "Stop waiting after this long and report `ask_pending`; the question stays open. Absent or `0` waits until it ends",
        ))
        .combination(
            Combination::new("ask-wait")
                .action("ask_wait")
                .required(["session_id"])
                .optional(["timeout_s"])
                .output(super::output()),
        )
}

fn result(outcome: Outcome<AskResponse>) -> Value {
    match outcome {
        Outcome::Completed(AskResponse::Cancelled) => json!({"code": "ask_cancelled"}),
        Outcome::Completed(AskResponse::Answered { answers }) => {
            json!({"code": "ask_answered", "answers": answers})
        }
        Outcome::Closed => json!({"code": "ask_closed"}),
        Outcome::Expired => json!({"code": "ask_expired"}),
    }
}

/// Describe a malformed JSON argument without echoing what it contained.
///
/// serde_json's own message quotes offending values, so only the messages
/// that name a field — never a value — pass through.
fn json_problem(flag: &str, error: &serde_json::Error) -> String {
    let message = error.to_string();
    let detail = match message.split(" at line ").next() {
        Some(detail)
            if ["unknown field", "missing field", "duplicate field"]
                .iter()
                .any(|prefix| detail.starts_with(prefix)) =>
        {
            detail.to_owned()
        }
        _ => match error.classify() {
            serde_json::error::Category::Syntax => "not valid JSON".to_owned(),
            serde_json::error::Category::Eof => "JSON that ends early".to_owned(),
            _ => "a value of the wrong type or an unknown block type".to_owned(),
        },
    };
    format!(
        "{flag} has {detail} (line {}, column {})",
        error.line(),
        error.column()
    )
}

fn parse<T: serde::de::DeserializeOwned>(
    invocation: &ResolvedInvocation,
    id: &str,
    flag: &str,
) -> Result<Option<T>, String> {
    super::optional(invocation, id)
        .map(|text| serde_json::from_str(&text).map_err(|error| json_problem(flag, &error)))
        .transpose()
}

fn request(invocation: &ResolvedInvocation) -> Result<AskRequest, String> {
    let context: Vec<AskBlock> = parse(invocation, "context", "--context")?.unwrap_or_default();
    let questions = match parse::<Vec<AskQuestion>>(invocation, "questions", "--questions")? {
        Some(questions) => questions,
        None => vec![AskQuestion {
            id: SINGLE_QUESTION_ID.to_owned(),
            question: super::required(invocation, "question"),
            options: parse::<Vec<AskOption>>(invocation, "options", "--options")?
                .unwrap_or_default(),
            multiple: super::flag(invocation, "multiple"),
            allow_custom: !super::flag(invocation, "no_custom"),
        }],
    };
    let request = AskRequest { context, questions };
    request.validate().map_err(|error| error.to_string())?;
    Ok(request)
}

pub(super) fn run(invocation: &ResolvedInvocation) -> ExitCode {
    let (format, output_to) = super::plan_output(invocation.output_plan());
    let request = match request(invocation) {
        Ok(request) => request,
        Err(message) => {
            return super::emit_error("ask_request_invalid", &message, format, output_to, 1);
        }
    };
    let plan = match delivery_of(invocation, "mode")
        .and_then(|mode| UiDeliveryOffer::WithLink.resolve(mode))
    {
        Ok(plan) => plan,
        Err(error) => {
            return super::emit_error(
                "ask_delivery_invalid",
                &error.to_string(),
                format,
                output_to,
                1,
            );
        }
    };
    if super::flag(invocation, "detach") {
        return detach(format, output_to);
    }
    let detached = std::env::var_os(DETACHED_ENV).is_some();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return super::emit_error(
                "ask_runtime_failed",
                "could not start the question runtime",
                format,
                output_to,
                1,
            );
        }
    };
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    let mut session_id = None;
    let outcome = runtime.block_on(async {
        let active = request
            .start(plan)
            .await
            .map_err(|error| error.to_string())?;
        session_id = Some(active.facts().session_id);
        emitter
            .emit(
                agent_first_data::json_progress(ready_event_revealing_link(
                    &active.facts(),
                    json!({"phase": "ask_ready"}),
                ))
                .build(),
            )
            .map_err(|error| error.to_string())?;
        active
            .wait()
            .await
            .map(result)
            .map_err(|error| error.to_string())
    });
    if detached {
        // Nobody reads this process's output any more; the result file is
        // the only place its ending can go.
        if let Some(id) = session_id {
            let record = match &outcome {
                Ok(result) => json!({"result": result}),
                Err(message) => json!({"error": {"code": "ask_failed", "message": message}}),
            };
            let _unrecorded_is_reported_by_wait = store_result(&id, &record);
        }
    }
    let (emitted, exit_code) = match outcome {
        Ok(result) => (emitter.emit_result(result), 0),
        Err(message) => (emitter.emit_error("ask_failed", &message), 1),
    };
    match emitted {
        Ok(()) => ExitCode::from(exit_code),
        Err(_) => ExitCode::from(4),
    }
}

/// Re-run this same question in a background process and return once it is
/// ready, so a caller whose commands time out never has to hold one open.
fn detach(format: OutputFormat, output_to: OutputTo) -> ExitCode {
    let fail = |code: &str, message: &str| super::emit_error(code, message, format, output_to, 1);
    if agent_first_ui::registry_opted_out() {
        return fail(
            "ask_detach_unavailable",
            "--detach needs the session registry; unset AFUI_NO_REGISTRY",
        );
    }
    let Ok(program) = std::env::current_exe() else {
        return fail("ask_detach_failed", "could not locate the afui executable");
    };
    let mut command = Command::new(program);
    command
        .args(background_args(std::env::args_os().skip(1)))
        .args(["--output", "json", "--output-to", "split"])
        .env(DETACHED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Out of the caller's process group, so a caller that kills its command's
    // group on timeout or interrupt does not take the question with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let Ok(mut child) = command.spawn() else {
        return fail(
            "ask_detach_failed",
            "could not start the background question",
        );
    };
    let mut line = String::new();
    let read = child
        .stderr
        .take()
        .map(|stderr| BufReader::new(stderr).read_line(&mut line));
    let event: Option<Value> = match read {
        Some(Ok(n)) if n > 0 => serde_json::from_str(&line).ok(),
        _ => None,
    };
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    let emitted = match event {
        Some(event) if event["kind"] == "progress" => {
            let mut started = event["progress"].clone();
            if let Some(fields) = started.as_object_mut() {
                fields.remove("phase");
                fields.insert("code".into(), json!("ask_started"));
            }
            emitter.emit_result(started).map(|()| 0)
        }
        Some(event) if event["kind"] == "error" => emitter
            .emit_error(
                event["error"]["code"].as_str().unwrap_or("ask_failed"),
                event["error"]["message"].as_str().unwrap_or_default(),
            )
            .map(|()| 1),
        _ => {
            let _already_gone = child.kill();
            emitter
                .emit_error(
                    "ask_detach_failed",
                    "the background question ended before it was ready",
                )
                .map(|()| 1)
        }
    };
    match emitted {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::from(4),
    }
}

/// The caller's own arguments, minus the ones the background run must not see.
fn background_args(args: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut kept = Vec::new();
    let mut skip_value = false;
    for arg in args {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        let text = arg.to_string_lossy();
        if text == "--detach" {
            continue;
        }
        if text == "--output" || text == "--output-to" {
            skip_value = true;
            continue;
        }
        if text.starts_with("--output=") || text.starts_with("--output-to=") {
            continue;
        }
        kept.push(arg);
    }
    kept
}

fn results_directory() -> Result<PathBuf, String> {
    let directory = agent_first_ui::global_afui_root()
        .map_err(|error| error.to_string())?
        .join("asks");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(directory)
}

fn store_result(session_id: &str, record: &Value) -> Result<(), String> {
    let directory = results_directory()?;
    // Owner-only and complete-or-absent: a reader never sees half an answer.
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    file.write_all(record.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    file.persist(directory.join(format!("{session_id}.json")))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read a stored ending once. Collecting it removes it.
fn take_result(path: &std::path::Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let _removed = std::fs::remove_file(path);
    serde_json::from_slice(&bytes).ok()
}

fn emit_record(record: &Value, format: OutputFormat, output_to: OutputTo) -> ExitCode {
    match (record.get("result"), record.get("error")) {
        (Some(result), _) => super::emit_result(result.clone(), format, output_to),
        (None, Some(error)) => super::emit_error(
            error["code"].as_str().unwrap_or("ask_failed"),
            error["message"].as_str().unwrap_or_default(),
            format,
            output_to,
            1,
        ),
        (None, None) => super::emit_error(
            "ask_failed",
            "the stored question result is unreadable",
            format,
            output_to,
            1,
        ),
    }
}

pub(super) fn run_wait(invocation: &ResolvedInvocation) -> ExitCode {
    let (format, output_to) = super::plan_output(invocation.output_plan());
    let session_id = super::required(invocation, "session_id");
    let fail =
        |code: &str, message: String| super::emit_error(code, &message, format, output_to, 1);
    if session_id.is_empty()
        || session_id.len() > 64
        || !session_id.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return fail(
            "ask_not_found",
            "a session ID is the hexadecimal `session_id` from `ask_started`".to_owned(),
        );
    }
    let directory = match results_directory() {
        Ok(directory) => directory,
        Err(message) => return fail("ask_wait_failed", message),
    };
    let path = directory.join(format!("{session_id}.json"));
    let timeout = invocation
        .optional("timeout_s")
        .and_then(CliValue::as_i64)
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(Duration::from_secs);
    let started = Instant::now();
    loop {
        if let Some(record) = take_result(&path) {
            return emit_record(&record, format, output_to);
        }
        let entry = match agent_first_ui::find_session(&session_id) {
            Ok(entry) => entry.filter(|e| e.provider_id == "afui" && e.ui_kind == "ask"),
            Err(error) => return fail("ask_wait_failed", error.to_string()),
        };
        let Some(entry) = entry else {
            // The question may have ended between the two reads.
            if let Some(record) = take_result(&path) {
                return emit_record(&record, format, output_to);
            }
            return fail(
                "ask_not_found",
                format!(
                    "no open question `{session_id}` and no stored answer; it was already collected, or its process ended"
                ),
            );
        };
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            let waiting_s = now_ms.saturating_sub(u128::from(entry.announced_at_epoch_ms)) / 1000;
            return super::emit_result(
                json!({"code": "ask_pending", "session_id": session_id, "waiting_s": waiting_s}),
                format,
                output_to,
            );
        }
        std::thread::sleep(WAIT_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_first_ui::ask::{AskAnswer, AskSelection};

    #[test]
    fn only_an_answered_question_carries_answers() {
        for outcome in [
            Outcome::Completed(AskResponse::Cancelled),
            Outcome::Closed,
            Outcome::Expired,
        ] {
            assert!(result(outcome).get("answers").is_none());
        }
        let answers = vec![AskAnswer {
            question_id: "question".into(),
            selected: vec![AskSelection {
                option_id: "retry".into(),
                label: "Retry".into(),
                note: Some("note".into()),
            }],
            text: None,
        }];
        let result = result(Outcome::Completed(AskResponse::Answered { answers }));
        assert_eq!(result["code"], "ask_answered");
        assert_eq!(result["answers"][0]["selected"][0]["option_id"], "retry");
        assert_eq!(result["answers"][0]["selected"][0]["note"], "note");
    }

    #[test]
    fn the_background_run_drops_detach_and_the_callers_output_choice() {
        let args = [
            "ask",
            "--detach",
            "--output",
            "yaml",
            "--question",
            "--output",
            "--output-to=stdout",
            "--options",
            "[]",
        ]
        .map(OsString::from);
        let kept = background_args(args.into_iter());
        assert_eq!(
            kept,
            ["ask", "--question", "--options", "[]"].map(OsString::from)
        );
    }

    #[test]
    fn json_problems_name_fields_but_never_values() {
        let unknown = serde_json::from_str::<Vec<AskOption>>(
            r#"[{"id":"x","label":"X","extra":"private-canary"}]"#,
        )
        .unwrap_err();
        let message = json_problem("--options", &unknown);
        assert!(message.contains("unknown field `extra`"), "{message}");
        assert!(!message.contains("private-canary"));
        let wrong = serde_json::from_str::<Vec<AskOption>>(r#""private-canary""#).unwrap_err();
        let message = json_problem("--options", &wrong);
        assert!(!message.contains("private-canary"), "{message}");
        assert!(message.contains("line 1"));
    }
}
