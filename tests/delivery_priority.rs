//! `UiDeliveryPlan::resolve` priority: an explicit mode beats `AFUI_DELIVERY`.
//!
//! In its own test binary because it sets `AFUI_DELIVERY` and
//! `AFUI_BROWSER_BINARY`, which are process global — sharing a binary with
//! other tests would let them race over it.

#![cfg(feature = "session")]

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use agent_first_ui::{
    BROWSER_BINARY_ENV, DELIVERY_ENV, Outcome, UiDeliveryMode, UiDeliveryPlan, UiSession,
};
use axum::Router;

fn write_script(path: &Path, body: &str) -> io::Result<PathBuf> {
    let mut file = std::fs::File::create(path)?;
    write!(file, "{body}")?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path.to_path_buf())
}

/// A "browser" that exits the instant it is launched — a person closing the
/// window before doing anything else.
fn stub_browser(script_dir: &Path) -> io::Result<PathBuf> {
    write_script(&script_dir.join("stub-browser.sh"), "#!/bin/sh\nexit 0\n")
}

#[tokio::test]
async fn an_explicit_mode_wins_over_a_conflicting_environment_default() {
    let workdir = tempfile::tempdir().unwrap();
    let browser = stub_browser(workdir.path()).unwrap();

    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe {
        std::env::set_var(DELIVERY_ENV, "session");
        std::env::set_var(BROWSER_BINARY_ENV, &browser);
    }

    let session = UiSession::<()>::new("console", "terminal").unwrap();
    // `AFUI_DELIVERY=session` alone would only register the session and wait
    // — nothing would ever launch the stub browser, and this call would hang
    // until the test times out. Only the explicit `Window` mode reaches
    // `UiSession::window`, which is what proves the priority actually held.
    let outcome = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Window))
        .unwrap()
        .deliver(session, Router::new())
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Closed);
}
