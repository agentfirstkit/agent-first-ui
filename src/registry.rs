//! Where live sessions announce themselves, so something else can find them.
//!
//! A session lives inside whichever process created it — a review, a terminal
//! window, an inspection panel. Nothing could see across those
//! processes, so a person had no way to ask "what is waiting for me?" and the
//! `afui` CLI had nothing to schedule.
//!
//! Each active session writes one small file here and removes it when it ends.
//! That file holds the session's URL, and the URL holds its credential — so the
//! directory is created private to the user, every entry is written `0600`, and
//! the field is `_secret`-suffixed the way AFDATA requires. This is a private
//! handoff between one person's own processes, never a broadcast: it is not
//! emitted to stdout, and an agent reading a Provider's events still never sees
//! a credential.
//!
//! Entries outlive crashes, so a reader prunes any whose process is gone rather
//! than trusting the directory to be tidy.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, UiSessionMetadata, global_afui_root};

/// Set to a non-empty, non-`0`, non-`false` value to keep sessions out of the
/// registry entirely. Nothing else changes; the `afui` CLI simply cannot see
/// them.
pub const NO_REGISTRY_ENV: &str = "AFUI_NO_REGISTRY";

/// One live session, as another process can see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSessionEntry {
    pub session_id: String,
    pub provider_id: String,
    pub ui_kind: String,
    /// What this session is about, in the Provider's own words — a table name,
    /// a case reference. Identity, not presentation: AFUI never parses it and
    /// never decides anything from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The session URL, credential included. `_secret` because it is one.
    pub access_url_secret: String,
    /// The process the session belongs to.
    ///
    /// Load-bearing, not a label. For a session AFUI serves it is also the
    /// process holding the socket, so it says nothing the reachability probe
    /// does not — but a session that only *names* an upstream is announced by
    /// one process and served by another, and then this is the only thing that
    /// answers §4.6's third criterion.
    pub owner_pid: u32,
}

impl UiSessionEntry {
    /// The authority to probe for liveness, from the session's own URL.
    ///
    /// An announced upstream may carry its credential in a query, so the
    /// authority ends at whichever comes first rather than at the path.
    fn authority(&self) -> Option<&str> {
        let rest = self.access_url_secret.strip_prefix("http://")?;
        Some(&rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())])
    }
}

/// Removes this process's entry when the session ends.
pub(crate) struct RegistryHandle {
    path: Option<PathBuf>,
}

impl Drop for RegistryHandle {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ignored = fs::remove_file(path);
        }
    }
}

pub(crate) fn publish(
    metadata: &UiSessionMetadata,
    subject: Option<String>,
    access_url: &str,
) -> RegistryHandle {
    if opted_out() {
        return RegistryHandle { path: None };
    }
    // Announcing is a convenience, never a precondition. A session that cannot
    // write here still works; it is only invisible to `afui session list`.
    let path = write_entry(metadata, subject, access_url).ok();
    RegistryHandle { path }
}

/// Announce, or say why not.
///
/// The same write as [`publish`] under a different error policy, because the
/// two callers are not in the same position. A served session that cannot
/// announce itself still has its window and its URL, so failing there would
/// take away something that works. An announced upstream has nothing else —
/// the listing *is* the delivery — so a failure that went unmentioned would
/// leave a Provider waiting on a session nobody can find.
pub(crate) fn publish_strictly(
    metadata: &UiSessionMetadata,
    subject: Option<String>,
    access_url: &str,
) -> Result<RegistryHandle> {
    if opted_out() {
        return Ok(RegistryHandle { path: None });
    }
    Ok(RegistryHandle {
        path: Some(write_entry(metadata, subject, access_url)?),
    })
}

fn write_entry(
    metadata: &UiSessionMetadata,
    subject: Option<String>,
    access_url: &str,
) -> Result<PathBuf> {
    let directory = sessions_directory()?;
    fs::create_dir_all(&directory).map_err(Error::Io)?;
    restrict_to_owner(&directory, 0o700)?;
    let entry = UiSessionEntry {
        session_id: metadata.session_id.as_str().to_owned(),
        provider_id: metadata.provider_id.clone(),
        ui_kind: metadata.ui_kind.clone(),
        subject,
        access_url_secret: access_url.to_owned(),
        owner_pid: std::process::id(),
    };
    let path = directory.join(format!("{}.json", entry.session_id));
    let body = serde_json::to_vec_pretty(&entry)
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
    write_private(&path, &body)?;
    // Also after the fact, because the mode above only applies to a file this
    // call created: an entry left behind by an earlier run keeps its own.
    restrict_to_owner(&path, 0o600)?;
    Ok(path)
}

/// Write an entry that is never briefly readable by anyone else.
///
/// Creating it and then narrowing it leaves a window in which the file holds a
/// session credential at whatever the umask allowed. The window is small, and
/// so is the change that closes it.
#[cfg(unix)]
fn write_private(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(Error::Io)?;
    file.write_all(body).map_err(Error::Io)
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &[u8]) -> Result<()> {
    fs::write(path, body).map_err(Error::Io)
}

/// Every live session this user owns, with dead owners' entries removed.
pub fn list_sessions() -> Result<Vec<UiSessionEntry>> {
    let directory = sessions_directory()?;
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(&directory).map_err(Error::Io)? {
        let path = item.map_err(Error::Io)?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        match read_entry(&path) {
            // A Provider that crashed never removed its file, so the entry is
            // not evidence of anything. What makes a session real is that it is
            // still being served *and* that whoever announced it still exists.
            Some(entry) if entry.is_live() => entries.push(entry),
            _ => {
                let _ignored = fs::remove_file(&path);
            }
        }
    }
    entries.sort_by(|left, right| {
        (&left.provider_id, &left.ui_kind, &left.session_id).cmp(&(
            &right.provider_id,
            &right.ui_kind,
            &right.session_id,
        ))
    });
    Ok(entries)
}

/// One live session by id, or `None` when it is not running.
pub fn find_session(session_id: &str) -> Result<Option<UiSessionEntry>> {
    Ok(list_sessions()?
        .into_iter()
        .find(|entry| entry.session_id == session_id))
}

fn read_entry(path: &Path) -> Option<UiSessionEntry> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn sessions_directory() -> Result<PathBuf> {
    Ok(global_afui_root()?.join("sessions"))
}

fn opted_out() -> bool {
    std::env::var_os(NO_REGISTRY_ENV).is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(Error::Io)
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path, _mode: u32) -> Result<()> {
    // Windows inherits the user profile's ACL, which is already owner-only.
    Ok(())
}

/// How long to wait for a session's host to accept a connection.
///
/// Every session is on this machine, so a host that has not accepted in this
/// long is not one a person is about to use.
const REACHABILITY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

impl UiSessionEntry {
    /// Whether this entry describes something a person can still use.
    ///
    /// Two questions, because a session has two ways of being over and only one
    /// of them used to be asked. Reachability answers "is this still being
    /// served?", which for a session AFUI serves is the whole story: the host
    /// lives in the owning process and stops accepting the moment that process
    /// does. An announced upstream separates the two — its page is served by a
    /// long-running host while the session belongs to the shorter-lived command
    /// that minted its credential — so a crashed
    /// announcer would otherwise leave a listing whose host is very much still
    /// answering.
    ///
    /// Asking both costs a served session nothing (its host cannot answer once
    /// its process is gone) and is exactly what §4.6's third criterion asks
    /// for: no session outlives the process that supplies it.
    fn is_live(&self) -> bool {
        self.owner_is_alive() && self.is_reachable()
    }

    /// Whether the process that announced this session still exists.
    ///
    /// Signal 0 asks without sending anything. `EPERM` means the process exists
    /// and belongs to someone else, which is still "exists" — entries are the
    /// user's own, so this is a same-user question in practice, and answering
    /// "gone" on a permission error would prune a live session.
    #[cfg(unix)]
    fn owner_is_alive(&self) -> bool {
        let Ok(pid) = i32::try_from(self.owner_pid) else {
            return false;
        };
        // 0 and negatives address process *groups*, so they are never an owner.
        if pid <= 0 {
            return false;
        }
        // SAFETY: `kill` with signal 0 performs the permission and existence
        // checks and delivers nothing.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// Windows has no equivalent question this crate can ask without a platform
    /// dependency it has no other use for, so it keeps the older, weaker rule:
    /// an entry lives as long as its host answers. A served session is
    /// unaffected — its host and its owner are the same process — and an
    /// announced upstream may linger until its host stops.
    #[cfg(not(unix))]
    fn owner_is_alive(&self) -> bool {
        true
    }

    /// Whether this session's host is still accepting connections.
    ///
    /// Deliberately a plain TCP connect rather than an HTTP request: it answers
    /// the liveness question without spending the session's credential, and
    /// without sliding an idle expiry forward just because someone ran `list`.
    fn is_reachable(&self) -> bool {
        use std::net::ToSocketAddrs;
        let Some(authority) = self.authority() else {
            return false;
        };
        let Ok(addresses) = authority.to_socket_addrs() else {
            return false;
        };
        addresses.into_iter().any(|address| {
            std::net::TcpStream::connect_timeout(&address, REACHABILITY_TIMEOUT).is_ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UiSessionId;

    fn metadata(provider: &str, kind: &str) -> UiSessionMetadata {
        UiSessionMetadata {
            provider_id: provider.to_owned(),
            ui_kind: kind.to_owned(),
            session_id: UiSessionId::generate().unwrap(),
        }
    }

    /// Each test gets its own registry root so they cannot see each other.
    fn isolated_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module are serialised by `TEST_LOCK`.
        unsafe { std::env::set_var(crate::CONFIG_DIR_ENV, root.path()) };
        root
    }

    use crate::ENV_LOCK as TEST_LOCK;

    /// A stand-in for a Provider's session host: something actually listening.
    fn listening() -> (std::net::TcpListener, String) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/deadbeefdeadbeef/",
            listener.local_addr().unwrap()
        );
        (listener, url)
    }

    #[test]
    fn a_published_session_is_visible_then_disappears_when_it_ends() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let metadata = metadata("inspector", "table_inspect");
        let (host, url) = listening();

        let handle = publish(&metadata, Some("public.orders".to_owned()), &url);
        // Assert on this session rather than on the directory as a whole:
        // every session in this binary announces itself, so the registry is
        // shared and a total count would be a race, not a fact.
        let mine = find_session(metadata.session_id.as_str()).unwrap().unwrap();
        assert_eq!(mine.provider_id, "inspector");
        assert_eq!(mine.ui_kind, "table_inspect");
        assert_eq!(mine.subject.as_deref(), Some("public.orders"));
        assert_eq!(mine.access_url_secret, url);
        assert_eq!(mine.owner_pid, std::process::id());

        drop(handle);
        assert!(
            find_session(metadata.session_id.as_str())
                .unwrap()
                .is_none(),
            "an ended session must stop being listed"
        );
        drop(host);
    }

    #[test]
    fn an_entry_whose_host_stopped_is_pruned_rather_than_offered() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let metadata = metadata("reviewer", "case_review");
        let (host, url) = listening();
        // Leak the handle so nothing cleans up on drop: this is a Provider
        // that died, leaving its file behind with nothing on its port.
        std::mem::forget(publish(&metadata, None, &url));
        assert!(
            find_session(metadata.session_id.as_str())
                .unwrap()
                .is_some()
        );
        drop(host);

        assert!(
            find_session(metadata.session_id.as_str())
                .unwrap()
                .is_none(),
            "a session whose host stopped answering must not be offered"
        );
        let path = sessions_directory()
            .unwrap()
            .join(format!("{}.json", metadata.session_id.as_str()));
        assert!(
            !path.exists(),
            "a stale entry must be removed, not merely hidden"
        );
    }

    #[test]
    fn opting_out_keeps_a_session_out_of_the_registry() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::set_var(NO_REGISTRY_ENV, "1") };
        let (host, url) = listening();
        let metadata = metadata("reviewer", "case_review");
        let handle = publish(&metadata, None, &url);
        let found = find_session(metadata.session_id.as_str()).unwrap();
        drop(host);
        unsafe { std::env::remove_var(NO_REGISTRY_ENV) };
        drop(handle);
        assert!(found.is_none(), "an opted-out session must not be listed");
    }

    #[cfg(unix)]
    #[test]
    fn an_upstream_whose_announcer_died_stops_being_listed_though_its_host_answers() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        // The shape only an announced upstream has: the process that owns the
        // session is not the process that serves it. Here the server is still
        // up — it is this test's own listener — and the announcer is a pid that
        // no longer exists, which is what a crashed announcer leaves behind
        // while the server it announced keeps running.
        let (host, url) = listening();
        let session_id = UiSessionId::generate().unwrap();
        let directory = sessions_directory().unwrap();
        fs::create_dir_all(&directory).unwrap();
        let entry = UiSessionEntry {
            session_id: session_id.as_str().to_owned(),
            provider_id: "remote".to_owned(),
            ui_kind: "takeover".to_owned(),
            subject: None,
            access_url_secret: format!("{url}?handoff_secret=deadbeef"),
            owner_pid: departed_pid(),
        };
        let path = directory.join(format!("{}.json", entry.session_id));
        fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();

        assert!(
            find_session(entry.session_id.as_str()).unwrap().is_none(),
            "a session whose announcer is gone must not be offered while its \
             upstream happens to still be answering"
        );
        assert!(!path.exists(), "the stale entry must be removed too");
        drop(host);
    }

    /// A pid that has certainly gone: this process's own child, reaped.
    #[cfg(unix)]
    fn departed_pid() -> u32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn a process that exits immediately");
        let pid = child.id();
        child.wait().expect("reap it");
        pid
    }

    #[cfg(unix)]
    #[test]
    fn entries_are_readable_only_by_their_owner() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let (host, url) = listening();
        let published = metadata("console", "terminal");
        let handle = publish(&published, None, &url);
        let directory = sessions_directory().unwrap();
        // The URL carries a credential, so neither the file nor the directory
        // that lists it may be readable by anyone else.
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // This session's own entry, not every file in the directory: every
        // session in this binary announces itself here, so a sweep of the
        // directory would be reading half-written neighbours rather than
        // checking a fact.
        let path = directory.join(format!("{}.json", published.session_id.as_str()));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "{path:?}"
        );
        drop(handle);
        drop(host);
    }
}
