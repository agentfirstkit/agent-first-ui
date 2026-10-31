//! End-to-end window delivery, driven by a stub browser.
//!
//! The real Chromium launch is one `Command::spawn`, but everything around it
//! is load-bearing: the page must actually be reachable at the advertised URL,
//! relative asset references must resolve, and the person closing the window
//! must end the session. A stub browser lets all of that run in CI, on a
//! machine with no display.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_first_ui::{Outcome, UiSession, UiWindowConfig};
use axum::Router;
use axum::response::Html;
use axum::routing::get;

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

/// A "browser" that probes the URL it is given, saves the findings, then exits.
///
/// Exiting is what a person closing the window looks like to AFUI, so this
/// exercises the `Outcome::Closed` path as well as the serving path.
fn stub_browser(script_dir: &Path, capture: &Path) -> io::Result<PathBuf> {
    // AFUI launches `--app=file://…` on a private page that moves on to the
    // session URL, so the credential never appears in the browser's arguments.
    let body = format!(
        r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    --app=file://*) url="$(sed -n 's/.*location\.replace("\(.*\)");.*/\1/p' "${{arg#--app=file://}}")" ;;
    --app=*) printf 'credential in arguments\n' > "{capture}"; exit 1 ;;
  esac
done
base="${{url%%\?*}}"
noslash="${{base%/}}"
origin="${{noslash%/*}}"
{{
  printf 'URL %s\n' "$url"
  printf 'advertised %s\n' "$(curl -sS -o /dev/null -w '%{{http_code}}' "$url")"
  printf 'asset %s\n' "$(curl -sS -o /dev/null -w '%{{http_code}}' "${{base}}style.css")"
  printf 'wrong_credential %s\n' "$(curl -sS -o /dev/null -w '%{{http_code}}' \
    "$origin/0000000000000000000000000000000000000000000000000000000000000000/")"
  printf 'body %s\n' "$(curl -sS "$url")"
}} > "{capture}"
"#,
        capture = capture.display()
    );
    write_script(&script_dir.join("stub-browser.sh"), &body)
}

fn panel_router() -> Router {
    Router::new()
        .route(
            "/",
            get(|| async {
                // A relative reference, the way a real panel links its stylesheet.
                Html("<!doctype html><link rel=\"stylesheet\" href=\"style.css\"><h1>orders</h1>")
            }),
        )
        .route("/style.css", get(|| async { "h1 { color: red }" }))
}

#[tokio::test]
async fn a_window_serves_its_page_and_closing_it_ends_the_session() {
    let workdir = tempfile::tempdir().unwrap();
    let capture = workdir.path().join("capture.txt");
    let browser = stub_browser(workdir.path(), &capture).unwrap();

    let session = UiSession::<()>::new("inspector", "table_inspect").unwrap();
    let config = UiWindowConfig::default().with_browser_binary(&browser);
    let outcome = session.window(panel_router(), &config).await.unwrap();

    // The stub exits after probing, which is a person closing the window.
    assert_eq!(outcome, Outcome::Closed);

    let captured = std::fs::read_to_string(&capture).unwrap();
    assert!(captured.contains("URL http://127.0.0.1:"), "{captured}");
    // Regression: the advertised URL ends in a slash, and `nest` served every
    // path under the credential *except* that one, so the page itself 404'd
    // while its assets resolved.
    assert!(
        captured.contains("advertised 200"),
        "the advertised URL did not serve the page: {captured}"
    );
    assert!(
        captured.contains("<h1>orders</h1>"),
        "page body missing: {captured}"
    );
    // The trailing slash is what makes `href="style.css"` resolve inside the
    // session rather than at the server root.
    assert!(
        captured.contains("asset 200"),
        "relative asset did not resolve: {captured}"
    );
    assert!(
        captured.contains("wrong_credential 404"),
        "a wrong credential was not refused: {captured}"
    );
}

#[tokio::test]
async fn a_window_session_survives_longer_than_the_old_fixed_ttl() {
    // Regression: expiry used to be a session-level default that applied to
    // window delivery too, so a panel someone left open died on its own. A
    // window is bounded by the window, and by nothing else.
    let workdir = tempfile::tempdir().unwrap();
    let script = write_script(
        &workdir.path().join("idle-browser.sh"),
        "#!/bin/sh\nsleep 30\n",
    )
    .unwrap();

    let session = UiSession::<()>::new("console", "terminal").unwrap();
    let config = UiWindowConfig::default().with_browser_binary(&script);
    let window = session.window(panel_router(), &config);
    // Nothing should resolve while the stub browser just sits there.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), window)
            .await
            .is_err(),
        "a window session ended on its own while the window was still open"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn window_exit_failure_and_unreached_page_are_distinct_errors() {
    for (binary, kind) in [
        (
            "/usr/bin/false",
            agent_first_ui::UiErrorKind::WindowWaitFailed,
        ),
        (
            "/usr/bin/true",
            agent_first_ui::UiErrorKind::WindowUnavailable,
        ),
    ] {
        let session = UiSession::<()>::new("reviewer", "panel").unwrap();
        let error = session
            .window(
                panel_router(),
                &UiWindowConfig::default().with_browser_binary(binary),
            )
            .await
            .expect_err("unusable window must be an error");
        assert_eq!(error.kind(), kind);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_unmarked_page_request_is_not_window_arrival() {
    let directory = tempfile::tempdir().unwrap();
    let browser = write_script(&directory.path().join("unmarked.sh"), r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in --app=file://*) url="$(sed -n 's/.*location\.replace("\(.*\)");.*/\1/p' "${arg#--app=file://}")"; url="${url%%\?*}"; curl -fsS "$url" >/dev/null || exit 1 ;; esac
done
"#).unwrap();
    let error = UiSession::<()>::new("reviewer", "panel")
        .unwrap()
        .window(
            panel_router(),
            &UiWindowConfig::default().with_browser_binary(browser),
        )
        .await
        .expect_err("only the launch-marked request counts");
    assert_eq!(error.kind(), agent_first_ui::UiErrorKind::WindowUnavailable);
}
