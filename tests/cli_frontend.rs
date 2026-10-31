//! Scope declarations must match the frontend trust gate they control.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used)]

#[test]
fn global_frontends_do_not_offer_a_workspace_enable_or_disable_gate() {
    let config = tempfile::tempdir().unwrap();
    for verb in ["enable", "disable"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args([
                "frontend",
                verb,
                "reviewer",
                "case_review",
                "--scope",
                "global",
            ])
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        let event: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        agent_first_data::assert_strict_event(&event).unwrap();
        assert_eq!(event["error"]["code"], "cli_invalid_argument_value");
    }
    assert!(!config.path().join("trust.json").exists());
}

#[test]
fn global_frontend_check_reports_the_trust_used_by_resolution() {
    let config = tempfile::tempdir().unwrap();
    for verb in ["init", "check"] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_afui"));
        command.args([
            "frontend",
            verb,
            "reviewer",
            "case_review",
            "--scope",
            "global",
        ]);
        if verb == "init" {
            command.args(["--frontend-id", "custom"]);
        }
        let output = command
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        if verb == "check" {
            let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(event["result"]["trusted"], true);
            assert_eq!(event["result"]["status"], "ready");
        }
    }
    assert!(!config.path().join("trust.json").exists());
}

/// `check` and `enable` read a frontend the same way a render does, so a
/// symlink the loader refuses is refused here too rather than trusted.
#[cfg(unix)]
#[test]
fn a_frontend_with_a_symlink_is_never_reported_ready_or_trusted() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let afui = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_afui"))
            .args(args)
            .current_dir(workspace.path())
            .env("AFUI_CONFIG_DIR", config.path())
            .output()
            .unwrap()
    };
    let init = afui(&[
        "frontend",
        "init",
        "reviewer",
        "case_review",
        "--frontend-id",
        "custom",
    ]);
    assert!(init.status.success(), "{init:?}");
    let root = workspace
        .path()
        .join(".afui/frontends/reviewer/case_review");
    std::fs::write(workspace.path().join("shared.svg"), "<svg/>").unwrap();
    std::os::unix::fs::symlink(workspace.path().join("shared.svg"), root.join("logo.svg")).unwrap();
    for verb in ["enable", "check"] {
        let output = afui(&["frontend", verb, "reviewer", "case_review"]);
        assert_eq!(output.status.code(), Some(1), "{verb}: {output:?}");
        let event: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(event["error"]["code"], "frontend_error", "{verb}");
        assert!(
            event["error"]["message"]
                .as_str()
                .unwrap()
                .contains("`logo.svg`: it is a symlink"),
            "{verb}: {event}"
        );
    }
    assert!(!config.path().join("trust.json").exists());
}
