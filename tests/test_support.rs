//! What `test-support` promises a consumer's test suite.
//!
//! The promise worth testing is not "these functions run" — it is that a
//! consumer can arrange the world AFUI reads from *without knowing how AFUI
//! reads it*. So every case here goes through the real resolver: a frontend
//! this module trusted actually serves, one it did not does not, and one
//! edited after trusting stops.

#![cfg(feature = "test-support")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_first_ui::test_support::{InstalledFrontend, stub_browser, write_attention_config};
use agent_first_ui::{CONFIG_DIR_ENV, UiFrontend};

/// `AFUI_CONFIG_DIR` is process-wide, so these run one at a time.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!("afui-test-support-{name}-{stamp}"));
    std::fs::create_dir_all(&root).unwrap();
    root
}

const PAGE: &str = "<main id=\"status\">mine</main>";

/// The whole point: a consumer arranges a trusted frontend and it serves,
/// through the same resolver a real run uses — no fingerprint reimplemented,
/// no algorithm restated.
#[test]
fn a_frontend_this_module_trusted_actually_serves() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = temp_root("serves");
    let workspace = root.join("workspace");
    // SAFETY: serialised by `ENV_LOCK`.
    unsafe { std::env::set_var(CONFIG_DIR_ENV, root.join("config")) };

    let installed = InstalledFrontend::new(&workspace, "reviewer", "console", "1")
        .with_frontend_id("custom")
        .with_file("templates/page.html.j2", PAGE)
        .install()
        .unwrap();

    // Installed but not yet trusted: skipped in silence, which is the whole
    // reason trust exists.
    let untrusted = UiFrontend::resolve(&workspace, "reviewer", "console", "1").unwrap();
    assert_eq!(untrusted.frontend_id(), None);

    installed.trust().unwrap();
    let trusted = UiFrontend::resolve(&workspace, "reviewer", "console", "1").unwrap();
    assert_eq!(trusted.frontend_id(), Some("custom"));
    assert_eq!(
        trusted.text("templates/page.html.j2").unwrap().as_deref(),
        Some(PAGE)
    );

    // A byte changed after trusting revokes it. A consumer's test asserts this
    // without owning a hash function.
    installed
        .edit(
            "templates/page.html.j2",
            "<main id=\"status\">edited</main>",
        )
        .unwrap();
    let edited = UiFrontend::resolve(&workspace, "reviewer", "console", "1").unwrap();
    assert_eq!(edited.frontend_id(), None);

    // SAFETY: still under the lock.
    unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    let _ = std::fs::remove_dir_all(root);
}

/// Two Providers' frontends coexist: trusting one must not overwrite the
/// other's entry, which a naive "write the file" helper does.
#[test]
fn trusting_one_frontend_leaves_another_alone() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = temp_root("two");
    let workspace = root.join("workspace");
    // SAFETY: serialised by `ENV_LOCK`.
    unsafe { std::env::set_var(CONFIG_DIR_ENV, root.join("config")) };

    for ui_kind in ["console", "monitor"] {
        InstalledFrontend::new(&workspace, "reviewer", ui_kind, "1")
            .with_frontend_id("custom")
            .with_file("templates/page.html.j2", PAGE)
            .install()
            .unwrap()
            .trust()
            .unwrap();
    }
    for ui_kind in ["console", "monitor"] {
        let resolved = UiFrontend::resolve(&workspace, "reviewer", ui_kind, "1").unwrap();
        assert_eq!(resolved.frontend_id(), Some("custom"), "{ui_kind}");
    }

    // SAFETY: still under the lock.
    unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    let _ = std::fs::remove_dir_all(root);
}

/// A test that wants a Link to lapse in a second writes the policy rather than
/// waiting fifteen minutes for the default.
#[test]
fn the_attention_policy_lands_where_afui_reads_it() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = temp_root("attention");
    // SAFETY: serialised by `ENV_LOCK`.
    unsafe { std::env::set_var(CONFIG_DIR_ENV, &root) };

    let path = write_attention_config(Duration::from_secs(1), Duration::from_secs(2)).unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["attention"]["idle_timeout_s"], 1);
    assert_eq!(written["attention"]["grace_period_s"], 2);

    // SAFETY: still under the lock.
    unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    let _ = std::fs::remove_dir_all(root);
}

/// A browser that records rather than opens, so window delivery is testable on
/// a machine with no screen — and so a test suite does not leave windows behind.
#[test]
fn the_stub_browser_records_the_url_instead_of_opening_it() {
    let root = temp_root("browser");
    let record = root.join("opened.txt");
    let binary = stub_browser(&root, &record).unwrap();
    assert!(binary.is_file());

    let status = std::process::Command::new(&binary)
        .arg("http://127.0.0.1:9/abc/")
        .status()
        .expect("the stub browser runs");
    assert!(status.success());
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(recorded.contains("http://127.0.0.1:9/abc/"), "{recorded}");

    let _ = std::fs::remove_dir_all(root);
}
