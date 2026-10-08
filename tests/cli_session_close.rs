//! `afui session close`, as a process: what it exits with when the session it
//! was asked to end takes the request and never answers.
//!
//! In its own test binary because it sets `AFUI_CONFIG_DIR`, which is process
//! global — sharing a binary with other tests would let them race over it.

// The `afui` binary and the registry it reads are both behind `session`.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Read;
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

use agent_first_ui::UiUpstream;
use serde_json::Value;

#[test]
fn closing_a_session_that_never_answers_fails_within_its_budget() {
    let config = tempfile::tempdir().unwrap();
    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe { std::env::set_var("AFUI_CONFIG_DIR", config.path()) };

    // Accepts every connection and answers none of them. Each is read to its
    // end on a thread of its own, so the socket stays open for as long as the
    // other side keeps it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut received = Vec::new();
                let _ignored = stream.read_to_end(&mut received);
            });
        }
    });
    let announced = UiUpstream::new("reviewer", "panel", format!("http://{address}/"))
        .unwrap()
        .announce()
        .unwrap();
    let session_id = announced.metadata().session_id.as_str().to_owned();

    let workspace = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["session", "close", &session_id])
        .current_dir(workspace.path())
        .env("AFUI_CONFIG_DIR", config.path())
        .output()
        .expect("afui should run");
    let elapsed = started.elapsed();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    // Not an early failure dressed as a timeout, and not an unbounded wait.
    assert!(
        elapsed >= Duration::from_millis(4500),
        "gave up before its budget: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "gave up long after its budget: {elapsed:?}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    let event: Value = serde_json::from_slice(&output.stderr).expect("one JSON event");
    assert_eq!(event["kind"], "error");
    assert_eq!(event["error"]["code"], "frontend_error");
    assert_eq!(
        event["error"]["message"],
        "session close request timed out after 5 seconds"
    );

    drop(announced);
}
