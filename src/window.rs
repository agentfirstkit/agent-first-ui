use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use tokio::time::sleep;

use crate::{Error, Result};

const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Names the browser binary AFUI launches, overriding discovery.
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

/// One isolated Chromium-family browser process and its disposable profile.
pub struct UiWindow {
    child: Child,
    _profile_dir: TempDir,
}

impl UiWindow {
    pub fn launch(url: &str, config: &UiWindowConfig) -> Result<Self> {
        let binary = match &config.browser_binary {
            Some(binary) => binary.clone(),
            None => browser_binary_from_env()
                .or_else(discover_browser)
                .ok_or(Error::WindowBinaryNotFound)?,
        };
        let profile_dir = tempfile::Builder::new()
            .prefix("afui-profile-")
            .tempdir()
            .map_err(Error::Io)?;
        let child = Command::new(&binary)
            // `--app` is what makes this a window rather than a browser: no
            // address bar, no tabs, and nothing the person can navigate away
            // to. `--new-window` would hand them a whole browser instead.
            .arg(format!("--app={url}"))
            .arg(format!(
                "--user-data-dir={}",
                profile_dir.path().to_string_lossy()
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
            })?;
        Ok(Self {
            child,
            _profile_dir: profile_dir,
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

fn discover_browser() -> Option<PathBuf> {
    const MACOS_CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ];
    MACOS_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .or_else(|| {
            [
                "chromium",
                "chromium-browser",
                "google-chrome",
                "microsoft-edge",
            ]
            .into_iter()
            .find_map(find_on_path)
        })
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(binary))
        .find(|path| is_executable(path))
}

#[cfg(unix)]
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
