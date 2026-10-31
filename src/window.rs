use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
const MACOS_WINDOW_APP_NAME: &str = "AFUI.app";

#[cfg(target_os = "macos")]
const MACOS_WINDOW_SCRIPT: &str = r#"
ObjC.import("AppKit");
ObjC.import("WebKit");

let uiWindow = null;
let uiWebView = null;

function installWindowMenu(app) {
    let mainMenu = app.mainMenu;
    if (!mainMenu) {
        mainMenu = $.NSMenu.alloc.initWithTitle("AFUI");
        app.mainMenu = mainMenu;
    }
    const fileIndex = mainMenu.indexOfItemWithTitle("File");
    let fileMenu;
    if (fileIndex >= 0) {
        fileMenu = mainMenu.itemAtIndex(fileIndex).submenu;
    } else {
        fileMenu = $.NSMenu.alloc.initWithTitle("File");
        const fileItem = $.NSMenuItem.alloc.initWithTitleActionKeyEquivalent("File", null, "");
        fileItem.submenu = fileMenu;
        mainMenu.insertItemAtIndex(fileItem, Math.min(1, mainMenu.numberOfItems));
    }
    const closeItem = fileMenu.addItemWithTitleActionKeyEquivalent(
        "Close Window", $.NSSelectorFromString("performClose:"), "w"
    );
    closeItem.keyEquivalentModifierMask = $.NSEventModifierFlagCommand;
    // Each applet owns one window. Target it directly so web-view focus cannot
    // divert the close action through the applet's responder chain.
    closeItem.target = uiWindow;
}

function showUiWindow() {
    if (uiWindow === null) {
        return;
    }
    if (uiWindow.miniaturized) {
        uiWindow.deminiaturize(null);
    }
    uiWindow.makeKeyAndOrderFront(null);
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

    uiWebView = $.WKWebView.alloc.initWithFrameConfiguration(frame, configuration);
    uiWindow = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
        frame,
        styleMask,
        $.NSBackingStoreBuffered,
        false
    );
    uiWindow.title = "AFUI";
    uiWindow.releasedWhenClosed = false;
    uiWindow.contentView = uiWebView;
    installWindowMenu(app);

    const url = $.NSURL.URLWithString(AFUI_WINDOW_URL);
    uiWebView.loadRequest($.NSURLRequest.requestWithURL(url));
    uiWindow.center;
    showUiWindow();
}

function reopen() {
    showUiWindow();
}

function idle() {
    if (uiWindow === null || !uiWindow.visible) {
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
    if (uiWindow !== null) {
        uiWindow.close;
    }
    return true;
}
"#;

/// Names the Chromium-family browser binary AFUI launches instead of the
/// platform's default window runtime.
///
/// The binary is not handed the session URL. It gets `--app=file://…/launch.html`,
/// a private page in the window's temporary directory that moves on to the
/// session URL with `location.replace`, so the credential never appears in the
/// browser's process arguments. A stand-in binary that needs the session URL
/// reads it out of that page, as `test_support::stub_browser` does. The one
/// exception is a browser confined to a snap or Flatpak, which cannot see this
/// machine's temporary directory and is given the URL itself.
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
    arrived: Option<Arc<AtomicBool>>,
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
            arrived: None,
            _resources_dir: resources_dir,
        })
    }

    pub(crate) fn track_arrival(mut self, arrived: Arc<AtomicBool>) -> Self {
        self.arrived = Some(arrived);
        self
    }

    pub async fn wait_closed(&mut self) -> Result<()> {
        loop {
            match self.child.try_wait().map_err(Error::WindowWait)? {
                Some(status) => {
                    if !status.success() {
                        return Err(Error::WindowWait(std::io::Error::other(format!(
                            "UI window exited with {status}"
                        ))));
                    }
                    if self
                        .arrived
                        .as_ref()
                        .is_some_and(|arrived| !arrived.load(Ordering::Acquire))
                    {
                        return Err(Error::WindowPageNotReached);
                    }
                    return Ok(());
                }
                None => sleep(WINDOW_POLL_INTERVAL).await,
            }
        }
    }
}

/// The page a Chromium window is launched on, written beside the window's own
/// profile.
pub(crate) const LAUNCH_PAGE: &str = "launch.html";

/// Write the page that sends the window on to `url`, and return its address.
///
/// The window URL carries the session's credential, and a process's arguments
/// are readable by every user on the machine (`ps`, `/proc/<pid>/cmdline`).
/// So the browser is given a file in the window's private directory instead,
/// and only that file holds the URL. `location.replace` keeps the launch page
/// out of the window's history, and the fragment — the window key — arrives
/// with it.
fn write_launch_page(url: &str, resources_dir: &Path) -> Result<String> {
    // JSON is a JavaScript string literal; `<` is escaped so the URL cannot
    // close the script element it sits in.
    let target = serde_json::to_string(url)
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?
        .replace('<', "\\u003c");
    let page = resources_dir.join(LAUNCH_PAGE);
    let source = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<script>location.replace({target});</script>\n"
    );
    write_private(&page, source.as_bytes())?;
    Ok(file_url(&page))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(Error::Io)?;
    std::io::Write::write_all(&mut file, bytes).map_err(Error::Io)
}

/// `path` as a `file://` URL, percent-encoding everything a path may hold
/// that a URL would read as something else.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from(if text.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Whether `binary` is a browser confined to a sandbox of its own (snap,
/// Flatpak), which has a private `/tmp` and cannot read a launch page written
/// to this machine's.
///
/// Such a browser is still given its URL as an argument, as before the launch
/// page existed: a window that never reaches its page would cost the person
/// the session outright. Recognised by where the binary resolves to, or by a
/// wrapper script that hands off to one — Ubuntu's `chromium-browser` is such
/// a script.
fn is_confined_browser(binary: &Path) -> bool {
    let confined = |text: &str| text.contains("/snap/") || text.contains("flatpak");
    let resolved = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    if confined(&binary.to_string_lossy())
        || confined(&resolved.to_string_lossy())
        || resolved.file_name().is_some_and(|name| name == "snap")
    {
        return true;
    }
    // A wrapper script: read only its head, which is where it names what it
    // runs, and never a binary's bytes as text.
    let mut head = [0_u8; 4096];
    let read = std::fs::File::open(&resolved)
        .and_then(|mut file| std::io::Read::read(&mut file, &mut head))
        .unwrap_or(0);
    head.starts_with(b"#!") && confined(&String::from_utf8_lossy(&head[..read]))
}

fn launch_chromium_window(url: &str, binary: &Path, resources_dir: &Path) -> Result<Child> {
    let launch = if is_confined_browser(binary) {
        url.to_owned()
    } else {
        write_launch_page(url, resources_dir)?
    };
    Command::new(binary)
        // `--app` is what makes this a window rather than a browser: no
        // address bar, no tabs, and nothing the person can navigate away
        // to. `--new-window` would hand them a whole browser instead.
        .arg(format!("--app={launch}"))
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
    launch_macos_window_script(&macos_window_script(url), resources_dir)
}

#[cfg(target_os = "macos")]
fn launch_macos_window_script(source: &str, resources_dir: &Path) -> Result<Child> {
    let source_path = resources_dir.join("window.js");
    let app_bundle = resources_dir.join(MACOS_WINDOW_APP_NAME);
    std::fs::write(&source_path, source).map_err(Error::Io)?;

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

    /// The way out of a machine with no browser on it is a recovery action,
    /// not part of what went wrong — so it is on `hint()`, beside the message
    /// rather than inside it, where a caller with a hint field of its own can
    /// print it exactly once.
    #[test]
    fn window_binary_not_found_names_the_env_override_in_its_hint() {
        let error = Error::WindowBinaryNotFound;
        let hint = error.hint().unwrap_or_default();
        assert!(
            hint.contains(BROWSER_BINARY_ENV),
            "{hint} does not mention {BROWSER_BINARY_ENV}"
        );
        assert!(
            !error.to_string().contains(BROWSER_BINARY_ENV),
            "the recovery action must not also be in the message: {error}"
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
            !source.contains("uiWebView.title"),
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

    #[test]
    fn a_confined_browser_is_recognised_by_where_it_runs_from() {
        let scripts = tempfile::tempdir().unwrap();
        let wrapper = scripts.path().join("chromium-browser");
        std::fs::write(&wrapper, "#!/bin/sh\nexec /snap/bin/chromium \"$@\"\n").unwrap();
        let plain = scripts.path().join("chrome");
        std::fs::write(&plain, "#!/bin/sh\nexec /opt/google/chrome/chrome \"$@\"\n").unwrap();
        assert!(is_confined_browser(Path::new("/snap/bin/chromium")));
        assert!(is_confined_browser(Path::new(
            "/var/lib/flatpak/exports/bin/org.chromium.Chromium"
        )));
        assert!(is_confined_browser(&wrapper));
        assert!(!is_confined_browser(&plain));
        assert!(!is_confined_browser(Path::new(
            "/nonexistent/google-chrome"
        )));
    }

    #[test]
    fn a_window_is_launched_on_a_private_page_rather_than_its_credentialed_url() {
        let resources = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:8080/secret/?afui_launch_id=x#afui-window=key</script>";
        let launch = write_launch_page(url, resources.path()).unwrap();
        assert!(launch.starts_with("file://"), "{launch}");
        assert!(!launch.contains("secret"), "{launch}");
        let page = std::fs::read_to_string(resources.path().join(LAUNCH_PAGE)).unwrap();
        assert!(
            page.contains(r#"location.replace("http://127.0.0.1:8080/secret/?afui_launch_id=x#afui-window=key\u003c/script>")"#),
            "{page}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(resources.path().join(LAUNCH_PAGE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            file_url(Path::new("/tmp/a b/launch.html")),
            "file:///tmp/a%20b/launch.html"
        );
        assert_eq!(
            file_url(Path::new(r"C:\Users\x\launch.html")),
            "file:///C:/Users/x/launch.html"
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
            macos_window_script("http://127.0.0.1:8080/panel/"),
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

    /// Exercise AppKit's actual menu dispatch while the web view is the first
    /// responder. Compiling the applet alone cannot catch a missing shortcut.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires a desktop; run scripts/test.sh window-shortcuts"]
    async fn macos_window_close_shortcut() {
        let resources = tempfile::tempdir().unwrap();
        let result_path = resources.path().join("shortcut-result.txt");
        let result_literal = serde_json::to_string(&result_path).unwrap();
        let source = format!(
            r#"
const AFUI_WINDOW_URL = "data:text/html,<textarea autofocus></textarea>";
const AFUI_APP_ICON_URL = "file:///dev/null";
{MACOS_WINDOW_SCRIPT}
let probeStep = 0;
function recordProbe(result) {{
    $.NSString.alloc.initWithUTF8String(result).writeToFileAtomicallyEncodingError(
        {result_literal}, true, $.NSUTF8StringEncoding, null
    );
}}
ObjC.registerSubclass({{
    name: "AFUIShortcutProbe",
    superclass: "NSObject",
    methods: {{
        "probe:": {{
            types: ["void", ["id"]],
            implementation: function(timer) {{
                try {{
                    if (!uiWindow.visible) throw new Error("window closed before Command+W");
                    showUiWindow();
                    uiWindow.makeKeyWindow;
                    if (probeStep === 0) uiWindow.makeFirstResponder(uiWebView);
                    const modifiers = [0, $.NSEventModifierFlagControl, $.NSEventModifierFlagCommand][probeStep];
                    const event = $.NSEvent.keyEventWithTypeLocationModifierFlagsTimestampWindowNumberContextCharactersCharactersIgnoringModifiersIsARepeatKeyCode(
                        $.NSEventTypeKeyDown, $.NSMakePoint(0, 0), modifiers, 0,
                        uiWindow.windowNumber, null, "w", "w", false, 13
                    );
                    // Queue input for AppKit to dispatch after this callback
                    // returns, when the applet is no longer executing script.
                    $.NSApplication.sharedApplication.postEventAtStart(event, false);
                    probeStep++;
                    if (probeStep === 3) {{
                        recordProbe("Command+W posted");
                        timer.invalidate;
                    }}
                }} catch (error) {{
                    recordProbe(String(error));
                    timer.invalidate;
                    uiWindow.close;
                }}
            }}
        }}
    }}
}});
const originalRun = run;
let shortcutProbe = null;
run = function() {{
    originalRun();
    shortcutProbe = $.AFUIShortcutProbe.alloc.init;
    $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(
        0.5, shortcutProbe, $.NSSelectorFromString("probe:"), null, true
    );
}};
"#
        );
        let child = launch_macos_window_script(&source, resources.path()).unwrap();
        let mut window = UiWindow {
            arrived: None,
            child,
            _resources_dir: resources,
        };
        tokio::time::timeout(Duration::from_secs(10), window.wait_closed())
            .await
            .expect("applet exits after Command+W")
            .unwrap();
        assert!(window.child.try_wait().unwrap().unwrap().success());
        assert_eq!(
            std::fs::read_to_string(result_path).unwrap(),
            "Command+W posted"
        );
    }

    /// Use desktop input rather than a page's synthetic KeyboardEvent: browser
    /// accelerators and the window manager own these shortcuts.
    #[cfg(any(target_os = "linux", windows))]
    #[tokio::test]
    #[ignore = "requires a desktop; run scripts/test.sh window-shortcuts"]
    async fn chromium_window_close_shortcuts() {
        let binary = std::env::var_os("AFUI_TEST_BROWSER_BINARY")
            .expect("fixed script selects Chrome or Chromium");
        let config = UiWindowConfig::default().with_browser_binary(binary);
        // A file rather than a `data:` URL: the window is launched on a page
        // that moves on to its target, and a browser refuses a page-initiated
        // top-level navigation to `data:`.
        let fixture_dir = tempfile::tempdir().unwrap();
        let fixture = fixture_dir.path().join("fixture.html");
        std::fs::write(
            &fixture,
            "<title>AFUI shortcut fixture</title><textarea autofocus></textarea>",
        )
        .unwrap();
        let fixture_url = file_url(&fixture);
        for shortcut in ["ctrl+w", "alt+F4"] {
            let mut window = UiWindow::launch(&fixture_url, &config).unwrap();
            // Also catches a launcher that exits before its window closes.
            sleep(Duration::from_secs(2)).await;
            assert!(window.child.try_wait().unwrap().is_none());
            let driver = desktop_shortcut_driver(window.child.id(), shortcut);
            let output = tokio::time::timeout(Duration::from_secs(15), driver)
                .await
                .expect("desktop input driver finishes")
                .unwrap();
            assert!(
                output.status.success(),
                "{shortcut}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            tokio::time::timeout(Duration::from_secs(10), window.wait_closed())
                .await
                .expect("shortcut closes the browser process")
                .unwrap();
            assert!(window.child.try_wait().unwrap().unwrap().success());
        }
    }

    #[cfg(target_os = "linux")]
    async fn desktop_shortcut_driver(
        process_id: u32,
        shortcut: &str,
    ) -> std::io::Result<std::process::Output> {
        let mut command = tokio::process::Command::new("xdotool");
        let activated = command
            .args(["search", "--sync", "--onlyvisible", "--pid"])
            .arg(process_id.to_string())
            .args([
                "--name",
                "AFUI shortcut fixture",
                "windowactivate",
                "--sync",
            ])
            .kill_on_drop(true)
            .output()
            .await?;
        if !activated.status.success() {
            return Ok(activated);
        }
        // Send through XTest to the focused window. Sending to the saved X11
        // window ID races the key-up against a window closed by the key-down.
        tokio::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", shortcut])
            .kill_on_drop(true)
            .output()
            .await
    }

    #[cfg(windows)]
    async fn desktop_shortcut_driver(
        process_id: u32,
        shortcut: &str,
    ) -> std::io::Result<std::process::Output> {
        let keys = if shortcut == "ctrl+w" { "^w" } else { "%{F4}" };
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
if (-not [Environment]::UserInteractive) {{
    throw 'window shortcut tests require an interactive Windows desktop'
}}
$shell = New-Object -ComObject WScript.Shell
$deadline = [DateTime]::UtcNow.AddSeconds(10)
while (-not $shell.AppActivate({process_id})) {{
    if ([DateTime]::UtcNow -ge $deadline) {{ throw 'fixture window did not become active' }}
    Start-Sleep -Milliseconds 100
}}
Start-Sleep -Milliseconds 300
$shell.SendKeys('{keys}')
"#
        );
        tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .kill_on_drop(true)
            .output()
            .await
    }
}
