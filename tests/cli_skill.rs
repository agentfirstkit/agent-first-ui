//! Installed skills must remain usable without a source checkout.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn skill(verb: &str, directory: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_afui"))
        .args(["skill", verb, "--agent", "codex", "--skills-dir"])
        .arg(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let event: Value = serde_json::from_slice(&output.stdout).unwrap();
    agent_first_data::assert_strict_event(&event).unwrap();
    event["result"].clone()
}

#[test]
fn skill_install_bundles_every_reference_and_detects_reference_edits() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("agent-first-ui");
    let report = skill("install", directory.path());
    assert_eq!(report["targets"][0]["current"], true);
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/agent-first-ui");
    for entry in walkdir::WalkDir::new(&source) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() || entry.file_name() == "SKILL.md" {
            continue;
        }
        let relative = entry.path().strip_prefix(&source).unwrap();
        assert!(
            std::fs::read(installed.join(relative)).unwrap()
                == std::fs::read(entry.path()).unwrap(),
            "missing or stale bundled asset: {}",
            relative.display()
        );
    }
    let reference = installed.join("references/ask.md");
    std::fs::write(&reference, "changed locally").unwrap();
    assert_eq!(
        skill("status", directory.path())["targets"][0]["current"],
        false
    );
    skill("install", directory.path());
    assert_eq!(
        skill("status", directory.path())["targets"][0]["current"],
        true
    );
    skill("uninstall", directory.path());
    assert!(!installed.exists());
}
