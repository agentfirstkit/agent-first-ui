//! Handing a URL to the person's own browser.
//!
//! A `window` delivery is a bare web view: no address bar, no tabs, nothing
//! the person can navigate away to. That is the point of it, and it is also
//! why a link inside one must never navigate *in* it — a page that replaced
//! the surface would leave a person on an unknown site with no way to see its
//! address. So the runtime accepts one request from the page: open this URL
//! outside, in the system default browser, where the address bar and the
//! browser's own protections apply. The host decides whether the URL is even
//! eligible; the page only asks.

use std::io;
use std::process::{Command, Stdio};

/// Longer than any URL a person could read, shorter than what a shell or an
/// `open` implementation might choke on.
pub(crate) const MAX_EXTERNAL_URL_BYTES: usize = 2048;

/// Why a URL is not handed to the system browser, as the caller-safe code the
/// page receives. `None` means it may be opened.
pub(crate) fn external_url_rejection(url: &str) -> Option<&'static str> {
    if url.is_empty() || url.len() > MAX_EXTERNAL_URL_BYTES {
        return Some("ui_open_external_url_invalid");
    }
    if url
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Some("ui_open_external_url_invalid");
    }
    let lower = url.to_ascii_lowercase();
    // `None` from this function means "eligible", so a missing scheme has to
    // be spelled out as a refusal rather than propagated with `?`.
    let Some(after_scheme) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return Some("ui_open_external_url_invalid");
    };
    // Something must follow the scheme, and it must not be a leading slash:
    // `http:///etc/passwd` is a file URL in disguise on some openers.
    if after_scheme.is_empty() || after_scheme.starts_with('/') {
        return Some("ui_open_external_url_invalid");
    }
    None
}

/// Open `url` in the system default browser. The URL is passed as one
/// argument to the platform opener, never through a shell, so its characters
/// cannot become options or commands.
pub(crate) fn open_in_system_browser(url: &str) -> io::Result<()> {
    if let Some(code) = external_url_rejection(url) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, code));
    }
    let mut command = opener_command(url);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // The opener hands the URL to the browser and exits within moments, but a
    // child nobody waits for stays a zombie for the life of this process, and
    // a long-lived Provider's window may send thousands of links its way. The
    // wait happens off the caller's thread, so a slow opener never stalls the
    // runtime that asked for it.
    std::thread::Builder::new()
        .name("afui-open-external".to_string())
        .spawn(move || {
            let _exit_status_is_not_ours_to_judge = child.wait();
        })
        .map(|_reaper| ())
}

#[cfg(target_os = "macos")]
fn opener_command(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(target_os = "windows")]
fn opener_command(url: &str) -> Command {
    // `cmd /C start` would interpret `&` and `^` inside the URL; the shell
    // API entry point takes the URL as a plain argument.
    let mut command = Command::new("rundll32");
    command.arg("url.dll,FileProtocolHandler").arg(url);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn opener_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(test)]
mod tests {
    use super::external_url_rejection;

    #[test]
    fn only_plain_http_and_https_urls_are_eligible() {
        assert_eq!(
            external_url_rejection("https://example.com/a?b=1&c=2"),
            None
        );
        assert_eq!(external_url_rejection("HTTP://Example.com"), None);
        for rejected in [
            "",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "mailto:a@b.c",
            "http:///etc/passwd",
            "https://",
            "https://example.com/a b",
            "https://example.com/\u{7}",
            "example.com",
        ] {
            assert_eq!(
                external_url_rejection(rejected),
                Some("ui_open_external_url_invalid"),
                "{rejected:?}"
            );
        }
        let long = format!("https://example.com/{}", "a".repeat(2100));
        assert!(external_url_rejection(&long).is_some());
    }
}
