#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::process::Command;

use serde_json::{Value, json};

fn assert_serve_failure(args: &[&str]) {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let config = tempfile::tempdir().expect("temporary config");
    let output = Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["session", "serve"])
        .args(args)
        .current_dir(workspace.path())
        .env("AFUI_CONFIG_DIR", config.path())
        .env("AFUI_SAFE_MODE", "1")
        .output()
        .expect("afui should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    // One terminal event also proves the server never announced readiness.
    let event: Value = serde_json::from_slice(&output.stdout).expect("one JSON event");
    assert_eq!(event["kind"], "error");
    assert_eq!(event["error"]["code"], "session_serve_failed");
    assert_eq!(event["trace"], json!({}));
}

#[test]
fn negative_attention_overrides_are_cli_errors_before_server_setup() {
    for flag in ["--idle-minutes", "--grace-minutes"] {
        let output = Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(["session", "serve", flag, "-1", "--listen", "not-an-address"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{flag}: {output:?}");
        assert!(output.stdout.is_empty());
        let event: Value = serde_json::from_slice(&output.stderr).unwrap();
        agent_first_data::assert_strict_event(&event).unwrap();
        assert!(event["error"]["code"].as_str().unwrap().starts_with("cli_"));
    }
}

#[test]
fn invalid_origin_host_exits_with_domain_failure() {
    assert_serve_failure(&[
        "--listen",
        "127.0.0.1:0",
        "--session-origin-host",
        "127.0.0.1",
    ]);
}

#[test]
fn occupied_listen_port_exits_with_domain_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a local port");
    let address = listener.local_addr().expect("reserved address").to_string();
    assert_serve_failure(&["--listen", &address]);
}
