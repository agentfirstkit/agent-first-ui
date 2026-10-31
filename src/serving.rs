//! The routes every Provider page serves, and the one response shape a page's
//! own bytes travel back in.
//!
//! Neither is a decision about what a page contains. A page is at `/`, its
//! stylesheet is a route rather than an inline block so a frontend can replace
//! presentation without touching structure, and the override's `assets/` tree
//! is nested under a fixed name so a template can refer to it relatively.
//! Nothing about that shape varies between Providers, the `text/css` header
//! included, so a hand-written copy of it can only differ by mistake.
//!
//! The blob responder is the same observation about a page serving bytes of
//! its own — a preview, a generated code, an attachment. It does not know what
//! the bytes are: the caller names the media type and it has to be one the
//! caller said was acceptable. What it does know is what a browser must be
//! told about anything a session hands it, and how to read a file out of a
//! directory without leaving it.

use std::path::{Component, Path};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};

use crate::frontend::UiFrontend;

/// Where a page's stylesheet answers, relative to the session root.
///
/// Fixed rather than configurable: a template refers to it as
/// `href="style.css"`, and a name per Provider would be one more thing for a
/// replacement frontend to get wrong for no gain.
pub const PAGE_STYLESHEET_PATH: &str = "style.css";

/// Where an override's static files answer, relative to the session root.
pub const PAGE_ASSETS_PREFIX: &str = "assets";

/// The three routes a rendered page serves: itself, its stylesheet, and the
/// frontend's assets.
///
/// Merge a Provider's own domain routes onto the result; nothing here claims a
/// name outside these three.
///
/// ```no_run
/// use agent_first_ui::{UiFrontend, page_routes};
///
/// # fn example(frontend: &UiFrontend, page: String, stylesheet: Vec<u8>, domain: axum::Router)
/// # -> axum::Router {
/// page_routes(page, stylesheet, frontend).merge(domain)
/// # }
/// ```
///
/// For a page that is rebuilt per request rather than rendered once — a live
/// panel, or one reread from a workspace that can change under it — route `/`
/// yourself and merge [`page_asset_routes`] instead.
pub fn page_routes(
    page: impl Into<String>,
    stylesheet: impl Into<Vec<u8>>,
    frontend: &UiFrontend,
) -> Router {
    let page = page.into();
    Router::new()
        .route(
            "/",
            get(move || {
                let page = page.clone();
                async move { Html(page).into_response() }
            }),
        )
        .merge(page_asset_routes(stylesheet, frontend))
}

/// Everything [`page_routes`] serves except the page itself.
///
/// For a Provider whose `/` is not one rendered string — a live panel pushing
/// snapshots, or a page reread per request because the workspace behind it can
/// change while a person is looking at it.
pub fn page_asset_routes(stylesheet: impl Into<Vec<u8>>, frontend: &UiFrontend) -> Router {
    let stylesheet = stylesheet.into();
    Router::new()
        .route(
            &format!("/{PAGE_STYLESHEET_PATH}"),
            get(move || {
                let stylesheet = stylesheet.clone();
                async move {
                    (
                        [(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("text/css; charset=utf-8"),
                        )],
                        stylesheet,
                    )
                        .into_response()
                }
            }),
        )
        .nest(&format!("/{PAGE_ASSETS_PREFIX}"), frontend.assets_router())
}

/// What a page may serve as bytes of its own, and how much of it.
///
/// Deliberately narrow. There is no caching here, no `Range`, and no upload:
/// a page's own bytes are small and generated per session, and each of those
/// would be a second thing to get right in a response that a session hands to
/// a browser under a bearer credential.
///
/// Every refusal is a `404`, whatever the reason — too large, wrong media
/// type, a symlink, a path that tried to leave. A page asking for something it
/// may not have learns only that it is not there, which is also all a person
/// looking at that page can act on.
#[derive(Debug, Clone)]
pub struct UiBlobPolicy {
    max_bytes: u64,
    media_types: Vec<String>,
}

impl UiBlobPolicy {
    /// A policy admitting exactly these media types, up to this many bytes.
    ///
    /// The allowlist is the caller's, because only the caller knows what its
    /// page is for. AFUI never looks at the bytes and never guesses a type
    /// from them: an allowlist is what stops a browser from sniffing past a
    /// label into something it will execute.
    pub fn new(max_bytes: u64, media_types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            max_bytes,
            media_types: media_types.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether this media type is one the caller said its page may serve.
    #[must_use]
    pub fn admits(&self, media_type: &str) -> bool {
        self.media_types.iter().any(|allowed| allowed == media_type)
    }

    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Bytes the caller already has, as a response a page can load.
    ///
    /// `no-store` because a session's bytes outliving the session in a browser
    /// cache is not something a bearer credential can take back; `nosniff` so
    /// the media type below is the only thing that decides what this is; and
    /// `inline` because a page displays this rather than downloading it.
    #[must_use]
    pub fn respond(&self, media_type: &str, bytes: Vec<u8>) -> Response {
        if !self.admits(media_type) || bytes.len() as u64 > self.max_bytes {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Ok(media_type) = HeaderValue::from_str(media_type) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut response = Response::new(Body::from(bytes));
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, media_type);
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("inline"),
        );
        response
    }

    /// One file under `root`, read only if it is genuinely there and genuinely
    /// under it.
    ///
    /// `relative` is refused outright if it is absolute or carries anything but
    /// plain names — no `..`, no root, no prefix — and every component along
    /// the way is opened without following symlinks, relative to the previous
    /// directory handle. A rename cannot redirect a later path lookup through
    /// an unchecked replacement directory.
    ///
    /// The bound is applied twice: once against the file's own length, and
    /// again against what was actually read, because a file can grow between
    /// the two.
    #[must_use]
    pub fn respond_with_file(&self, root: &Path, relative: &Path, media_type: &str) -> Response {
        match self.read(root, relative) {
            Some(bytes) => self.respond(media_type, bytes),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// The bounded read [`Self::respond_with_file`] is built from, for a caller
    /// whose bytes are not the file's own.
    ///
    /// A page that serves one part of a container it read off disk — an
    /// attachment inside a message, an entry inside an archive — still wants
    /// the same bound and the same refusal to follow a link out of the tree,
    /// but has its own work to do between reading and responding. `None` for
    /// every reason the response form would have been a `404`.
    #[must_use]
    pub fn read(&self, root: &Path, relative: &Path) -> Option<Vec<u8>> {
        use std::io::Read;

        if relative.as_os_str().is_empty() || relative.is_absolute() {
            return None;
        }
        let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority()).ok()?;
        let file = open_file_beneath(directory, relative)?;
        let leaf = file.metadata().ok()?;
        if !leaf.is_file() || leaf.len() > self.max_bytes {
            return None;
        }
        let mut bytes = Vec::new();
        // One byte past the bound, so a file that grew between the check above
        // and this read is caught rather than truncated into a valid-looking
        // answer.
        file.take(self.max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= self.max_bytes).then_some(bytes)
    }
}

/// Hold each ancestor open while resolving the next single path component.
fn open_file_beneath(mut directory: Dir, relative: &Path) -> Option<File> {
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return None;
        };
        if components.peek().is_some() {
            directory = directory.open_dir_nofollow(name).ok()?;
        } else {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            // Inspect the opened handle before reading; a FIFO or device must
            // not block the open before that type check can reject it.
            #[cfg(unix)]
            options.nonblock(true);
            return directory.open_with(name, &options).ok();
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn policy() -> UiBlobPolicy {
        UiBlobPolicy::new(16, ["image/png", "image/jpeg"])
    }

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("afui-blob-{name}-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn bytes_a_page_may_serve_arrive_inert_and_uncached() {
        let response = policy().respond("image/png", b"tiny".to_vec());
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(headers[header::CONTENT_TYPE], "image/png");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::CONTENT_DISPOSITION], "inline");
    }

    /// Every refusal looks the same from outside, so asking for something a
    /// page may not have tells nobody whether it exists.
    #[test]
    fn a_type_the_page_never_admitted_and_a_body_over_the_bound_are_both_absent() {
        assert_eq!(
            policy().respond("text/html", b"<p>".to_vec()).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            policy().respond("image/png", vec![0; 17]).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            policy().respond("image/png", vec![0; 16]).status(),
            StatusCode::OK,
            "the bound itself is admitted"
        );
    }

    #[test]
    fn a_file_under_the_root_is_served_and_one_that_is_not_is_absent() {
        let root = temp_root("read");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/ok.png"), b"tiny").unwrap();
        fs::write(root.join("big.png"), vec![0_u8; 32]).unwrap();

        let policy = policy();
        assert_eq!(
            policy
                .respond_with_file(&root, Path::new("nested/ok.png"), "image/png")
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            policy
                .respond_with_file(&root, Path::new("big.png"), "image/png")
                .status(),
            StatusCode::NOT_FOUND
        );
        for escape in [
            "../outside.png",
            "/etc/hosts",
            "nested/../../outside.png",
            "",
        ] {
            assert_eq!(
                policy
                    .respond_with_file(&root, Path::new(escape), "image/png")
                    .status(),
                StatusCode::NOT_FOUND,
                "{escape}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    /// A link is refused wherever it appears on the way, not only at the end:
    /// a linked directory is just as much a way out of the root as a linked
    /// file.
    #[cfg(unix)]
    #[test]
    fn a_symlink_anywhere_along_the_path_is_refused() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-target");
        fs::write(outside.join("secret.png"), b"tiny").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.png"), root.join("linked.png")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();

        let policy = policy();
        for path in ["linked.png", "linked/secret.png"] {
            assert_eq!(
                policy
                    .respond_with_file(&root, Path::new(path), "image/png")
                    .status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn an_open_directory_is_not_redirected_by_replacing_its_path() {
        use std::io::Read;
        let root = temp_root("pinned-root");
        let moved = root.with_extension("moved");
        let outside = temp_root("pinned-outside");
        fs::write(root.join("preview.png"), b"intended").unwrap();
        fs::write(outside.join("preview.png"), b"outside-canary").unwrap();
        let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        let mut file = open_file_beneath(directory, Path::new("preview.png")).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"intended");
        fs::remove_file(&root).unwrap();
        fs::remove_dir_all(moved).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_special_file_is_rejected_without_waiting_for_a_writer() {
        let root = temp_root("fifo");
        let path = root.join("preview.png");
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the NUL-terminated path belongs to this test's private root.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        let reading_root = root.clone();
        let reader = std::thread::spawn(move || {
            sender
                .send(policy().read(&reading_root, Path::new("preview.png")))
                .unwrap();
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_none()
        );
        reader.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
