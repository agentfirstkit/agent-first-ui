use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use tokio::time::sleep;

#[cfg(any(target_os = "macos", test))]
use crate::APP_ICON_PATH;
use crate::{Error, Result};

const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[cfg(target_os = "macos")]
const MACOS_OSACOMPILE: &str = "/usr/bin/osacompile";

#[cfg(target_os = "macos")]
const MACOS_WINDOW_APP_NAME: &str = "AFUI Review.app";

#[cfg(target_os = "macos")]
const MACOS_WINDOW_SCRIPT: &str = r#"
ObjC.import("AppKit");
ObjC.import("WebKit");

let reviewWindow = null;
let reviewWebView = null;

function showReviewWindow() {
    if (reviewWindow === null) {
        return;
    }
    if (reviewWindow.miniaturized) {
        reviewWindow.deminiaturize(null);
    }
    reviewWindow.makeKeyAndOrderFront(null);
    $.NSApplication.sharedApplication.activateIgnoringOtherApps(true);
}

function run() {
    const app = $.NSApplication.sharedApplication;
    app.setActivationPolicy($.NSApplicationActivationPolicyRegular);

    const appIconUrl = $.NSURL.URLWithString(AFUI_APP_ICON_URL);
    const appIconData = $.NSData.dataWithContentsOfURL(appIconUrl);
    if (appIconData) {
        const appIcon = $.NSImage.alloc.initWithData(appIconData);
        if (appIcon) {
            const appBundlePath = ObjC.unwrap($.NSBundle.mainBundle.bundlePath);
            $.NSWorkspace.sharedWorkspace.setIconForFileOptions(
                appIcon,
                appBundlePath,
                0
            );
            app.setApplicationIconImage(appIcon);
        }
    }

    const frame = $.NSMakeRect(0, 0, 1120, 820);
    const styleMask =
        $.NSWindowStyleMaskTitled |
        $.NSWindowStyleMaskClosable |
        $.NSWindowStyleMaskMiniaturizable |
        $.NSWindowStyleMaskResizable;
    const configuration = $.WKWebViewConfiguration.alloc.init;
    configuration.websiteDataStore = $.WKWebsiteDataStore.nonPersistentDataStore;

    reviewWebView = $.WKWebView.alloc.initWithFrameConfiguration(frame, configuration);
    reviewWindow = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
        frame,
        styleMask,
        $.NSBackingStoreBuffered,
        false
    );
    reviewWindow.title = "AFUI";
    reviewWindow.releasedWhenClosed = false;
    reviewWindow.contentView = reviewWebView;

    const url = $.NSURL.URLWithString(AFUI_WINDOW_URL);
    reviewWebView.loadRequest($.NSURLRequest.requestWithURL(url));
    reviewWindow.center;
    showReviewWindow();
}

function reopen() {
    showReviewWindow();
}

function idle() {
    if (reviewWindow === null || !reviewWindow.visible) {
        $.NSApplication.sharedApplication.terminate(null);
        return 0;
    }

    // Keep the title stable instead of repeatedly bridging WKWebView.title
    // through JavaScript for Automation. The WebKit value is an Objective-C
    // object delivered over WebKit IPC; polling it from a stay-open applet can
    // race a frame-state update and crash the applet inside AtomString decode.
    // Window visibility is an AppKit scalar and is the only value this idle
    // handler needs in order to notice that the person closed the window.
    return 0.25;
}

function quit() {
    if (reviewWindow !== null) {
        reviewWindow.close;
    }
    return true;
}
"#;

/// Names the Chromium-family browser binary AFUI launches instead of the
/// platform's default window runtime.
pub const BROWSER_BINARY_ENV: &str = "AFUI_BROWSER_BINARY";

#[derive(Debug, Clone, Default)]
pub struct UiWindowConfig {
    browser_binary: Option<PathBuf>,
}

impl UiWindowConfig {
    #[must_use]
    pub fn with_browser_binary(mut self, browser_binary: impl Into<PathBuf>) -> Self {
        self.browser_binary = Some(browser_binary.into());
        self
    }
}

/// One isolated desktop window process and its disposable runtime resources.
pub struct UiWindow {
    child: Child,
    _resources_dir: TempDir,
}

impl UiWindow {
    pub fn launch(url: &str, config: &UiWindowConfig) -> Result<Self> {
        let browser_override = config
            .browser_binary
            .clone()
            .or_else(browser_binary_from_env);
        let resources_dir = tempfile::Builder::new()
            .prefix("afui-window-")
            .tempdir()
            .map_err(Error::Io)?;

        #[cfg(target_os = "macos")]
        let child = match browser_override {
            Some(binary) => launch_chromium_window(url, &binary, resources_dir.path())?,
            None => launch_macos_window(url, resources_dir.path())?,
        };

        #[cfg(not(target_os = "macos"))]
        let child = {
            let binary = browser_override
                .or_else(discover_browser)
                .ok_or(Error::WindowBinaryNotFound)?;
            launch_chromium_window(url, &binary, resources_dir.path())?
        };

        Ok(Self {
            child,
            _resources_dir: resources_dir,
        })
    }

    pub async fn wait_closed(&mut self) -> Result<()> {
        loop {
            match self.child.try_wait().map_err(Error::WindowWait)? {
                Some(_) => return Ok(()),
                None => sleep(WINDOW_POLL_INTERVAL).await,
            }
        }
    }
}

fn launch_chromium_window(url: &str, binary: &Path, resources_dir: &Path) -> Result<Child> {
    Command::new(binary)
        // `--app` is what makes this a window rather than a browser: no
        // address bar, no tabs, and nothing the person can navigate away
        // to. `--new-window` would hand them a whole browser instead.
        .arg(format!("--app={url}"))
        .arg(format!(
            "--user-data-dir={}",
            resources_dir.to_string_lossy()
        ))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-features=Translate",
            "--disable-sync",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| Error::WindowLaunch {
            binary: binary.display().to_string(),
            source,
        })
}

#[cfg(target_os = "macos")]
fn launch_macos_window(url: &str, resources_dir: &Path) -> Result<Child> {
    let source_path = resources_dir.join("window.js");
    let app_bundle = resources_dir.join(MACOS_WINDOW_APP_NAME);
    std::fs::write(&source_path, macos_window_script(url)).map_err(Error::Io)?;

    let output = Command::new(MACOS_OSACOMPILE)
        .args(["-l", "JavaScript", "-s", "-o"])
        .arg(&app_bundle)
        .arg(&source_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| Error::WindowLaunch {
            binary: MACOS_OSACOMPILE.to_owned(),
            source,
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::WindowLaunch {
            binary: MACOS_OSACOMPILE.to_owned(),
            source: std::io::Error::other(if detail.is_empty() {
                format!("exited with {}", output.status)
            } else {
                detail
            }),
        });
    }

    let executable = app_bundle.join("Contents/MacOS/applet");
    Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| Error::WindowLaunch {
            binary: executable.display().to_string(),
            source,
        })
}

#[cfg(target_os = "macos")]
fn macos_window_script(url: &str) -> String {
    let url_literal = serde_json::Value::String(url.to_owned()).to_string();
    let app_icon_url_literal = serde_json::Value::String(app_icon_url(url)).to_string();
    format!(
        "const AFUI_WINDOW_URL = {url_literal};\n\
         const AFUI_APP_ICON_URL = {app_icon_url_literal};\n\
         {MACOS_WINDOW_SCRIPT}"
    )
}

/// Resolve the Provider-owned icon next to a UI's own entry point while
/// preserving a credential carried in the query string.
///
/// Hosted AFUI sessions end in `/`, while an announced third-party page may
/// not. Both expose the same reserved suffix under their own page base.
#[cfg(any(target_os = "macos", test))]
fn app_icon_url(url: &str) -> String {
    let without_fragment = url.split_once('#').map_or(url, |(base, _)| base);
    let (base, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, None), |(base, query)| {
            (base, Some(query))
        });
    let separator = if base.ends_with('/') { "" } else { "/" };
    let mut resolved = format!("{base}{separator}{APP_ICON_PATH}");
    if let Some(query) = query {
        resolved.push('?');
        resolved.push_str(query);
    }
    resolved
}

impl Drop for UiWindow {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ignored = self.child.kill();
            let _ignored = self.child.wait();
        }
    }
}

/// Override discovery with an explicit browser binary.
///
/// This exists so a Provider's window can be exercised on a machine with no
/// display — a CI run, or a headless check of a page a person would normally
/// look at. A Provider that sets the binary in its own config still wins.
fn browser_binary_from_env() -> Option<PathBuf> {
    std::env::var_os(BROWSER_BINARY_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn discover_browser() -> Option<PathBuf> {
    [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "microsoft-edge",
    ]
    .into_iter()
    .find_map(find_on_path)
}

#[cfg(windows)]
fn discover_browser() -> Option<PathBuf> {
    windows_fixed_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| {
            windows_path_candidate_names()
                .iter()
                .find_map(|name| find_on_path(name))
        })
}

/// Binary names AFUI looks for on `PATH` when discovering a Windows browser.
///
/// Not `#[cfg(windows)]` — only ever *called* from [`discover_browser`] on
/// Windows — so a single test can assert its shape (non-empty, every entry
/// ending `.exe`) on whatever platform this crate happens to be built on.
#[cfg(any(windows, test))]
fn windows_path_candidate_names() -> &'static [&'static str] {
    &["chrome.exe", "msedge.exe", "chromium.exe"]
}

/// Fixed install locations AFUI checks directly, without touching `PATH`,
/// when discovering a Windows browser.
///
/// Reads `%ProgramFiles%`, `%ProgramFiles(x86)%`, and `%LocalAppData%`, each
/// joined with the vendor's own install subdirectory; a variable this
/// platform never set is skipped rather than guessed at. Deliberately no
/// registry lookup (the `App Paths` key) — these paths are tried first, and
/// only `PATH` search is the fallback.
#[cfg(any(windows, test))]
fn windows_fixed_candidates() -> Vec<PathBuf> {
    const INSTALL_DIRS: &[(&str, &str, &str)] = &[
        ("ProgramFiles", "Google\\Chrome\\Application", "chrome.exe"),
        (
            "ProgramFiles(x86)",
            "Google\\Chrome\\Application",
            "chrome.exe",
        ),
        ("LocalAppData", "Google\\Chrome\\Application", "chrome.exe"),
        ("ProgramFiles", "Microsoft\\Edge\\Application", "msedge.exe"),
        (
            "ProgramFiles(x86)",
            "Microsoft\\Edge\\Application",
            "msedge.exe",
        ),
        ("LocalAppData", "Microsoft\\Edge\\Application", "msedge.exe"),
    ];
    INSTALL_DIRS
        .iter()
        .filter_map(|(env_var, vendor_dir, exe)| {
            let base = std::env::var_os(env_var)?;
            Some(Path::new(&base).join(vendor_dir).join(exe))
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn find_on_path(binary: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(binary))
        .find(|path| is_executable(path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows discovery is exercised on every platform this crate is built
    /// on, not only on Windows itself: the candidate lists are ordinary data,
    /// and a test gated to `#[cfg(windows)]` would never run in this crate's
    /// own CI unless a Windows leg happened to execute it.
    #[test]
    fn windows_candidate_list_is_non_empty_and_every_entry_ends_in_exe() {
        let mut candidates = windows_fixed_candidates();
        candidates.extend(windows_path_candidate_names().iter().map(PathBuf::from));

        assert!(
            !candidates.is_empty(),
            "Windows discovery must have at least the bare PATH names to try"
        );
        for candidate in &candidates {
            let name = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            assert!(name.ends_with(".exe"), "{candidate:?} does not end in .exe");
        }
    }

    #[test]
    fn window_binary_not_found_names_the_env_override() {
        let message = Error::WindowBinaryNotFound.to_string();
        assert!(
            message.contains(BROWSER_BINARY_ENV),
            "{message} does not mention {BROWSER_BINARY_ENV}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_window_source_uses_an_isolated_store_and_escapes_its_url() {
        let source = macos_window_script("http://127.0.0.1/a\"b\n/");

        assert!(
            source.starts_with("const AFUI_WINDOW_URL = \"http://127.0.0.1/a\\\"b\\n/\";"),
            "{source}"
        );
        assert!(source.contains("nonPersistentDataStore"), "{source}");
        assert!(source.contains("function reopen()"), "{source}");
        assert!(source.contains("dataWithContentsOfURL"), "{source}");
        assert!(source.contains("setIconForFileOptions"), "{source}");
        assert!(source.contains("setApplicationIconImage"), "{source}");
        assert!(
            !source.contains("reviewWebView.title"),
            "the stay-open applet must not poll a WebKit object property: {source}"
        );
    }

    #[test]
    fn the_app_icon_url_keeps_path_and_query_credentials() {
        assert_eq!(
            app_icon_url("http://127.0.0.1:8080/credential/"),
            "http://127.0.0.1:8080/credential/__afui/app-icon.svg"
        );
        assert_eq!(
            app_icon_url("http://127.0.0.1:8080/panel?handoff_secret=secret"),
            "http://127.0.0.1:8080/panel/__afui/app-icon.svg?handoff_secret=secret"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_window_source_compiles_to_a_stay_open_application() {
        let resources = tempfile::tempdir().unwrap();
        let source_path = resources.path().join("window.js");
        let app_bundle = resources.path().join(MACOS_WINDOW_APP_NAME);
        std::fs::write(
            &source_path,
            macos_window_script("http://127.0.0.1:8080/review/"),
        )
        .unwrap();

        let output = Command::new(MACOS_OSACOMPILE)
            .args(["-l", "JavaScript", "-s", "-o"])
            .arg(&app_bundle)
            .arg(&source_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(app_bundle.join("Contents/MacOS/applet").is_file());
    }
}
