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
    /// Whether a person is being waited on, as opposed to a session someone
    /// merely watches.
    ///
    /// True automatically for every session whose completion type is not
    /// `()` — handing a value back is, by definition, waiting on whoever
    /// supplies it — and true as well for a `T = ()` session that called
    /// `UiSession::awaiting_person` because it is unmistakably waited on even
    /// though it has nothing to hand back. A Provider that forgets to declare
    /// the second case only under-reports; the automatic half can never claim
    /// a watched session is waiting on someone, so the direction this can get
    /// wrong is the safe one.
    pub waits_for_decision: bool,
    /// The session URL, credential included. `_secret` because it is one.
    pub access_url_secret: String,
    /// The URL a local window opens: [`Self::access_url_secret`] plus the
    /// session runtime's window key, which is what lets that window ask this
    /// machine to open a link. Only `afui session open` reads it; a remote
    /// view is always reached through [`Self::access_url_secret`]. Absent for
    /// a session with no runtime, and for an upstream AFUI does not serve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_url_secret: Option<String>,
    /// When this session announced itself.
    ///
    /// Load-bearing: it is what puts a session where a person expects to find
    /// it. Somebody opens a terminal, works in it, and asks for a second
    /// interface — that one belongs *after* the terminal, because that is when
    /// it happened. Sorting by name instead would file it wherever its
    /// Provider's name falls in the alphabet, which is a fact about this kit's
    /// naming and not about anything the person did.
    ///
    /// **Not stored, and deliberately not.** It is the entry file's own
    /// modification time, read when the entry is read. A Provider announces by
    /// writing that file exactly once, so the file already knows when — and
    /// taking the answer from the file rather than from a field means every
    /// announcement carries one, including announcements written by a build of
    /// this crate that predates the ordering entirely. A stored field would
    /// have made a Provider pinned to an older version sort first forever, and
    /// Providers pin versions.
    ///
    /// Wall clock rather than monotonic, because the readers are other
    /// processes and there is nothing for a monotonic instant to be relative
    /// to. A clock that jumps, or a filesystem too coarse to tell two
    /// announcements apart, falls back on the stable key below, so the order
    /// stays total either way.
    #[serde(skip)]
    pub announced_at_epoch_ms: u64,
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

pub(crate) fn unpublished() -> RegistryHandle {
    RegistryHandle { path: None }
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
    waits_for_decision: bool,
    access_url: &str,
    window_url: Option<&str>,
) -> RegistryHandle {
    if registry_opted_out() {
        return RegistryHandle { path: None };
    }
    // Announcing is a convenience, never a precondition. A session that cannot
    // write here still works; it is only invisible to `afui session list`.
    let path = write_entry(
        metadata,
        subject,
        waits_for_decision,
        access_url,
        window_url,
    )
    .ok();
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
    waits_for_decision: bool,
    access_url: &str,
) -> Result<RegistryHandle> {
    if registry_opted_out() {
        return Ok(RegistryHandle { path: None });
    }
    Ok(RegistryHandle {
        path: Some(write_entry(
            metadata,
            subject,
            waits_for_decision,
            access_url,
            None,
        )?),
    })
}

fn write_entry(
    metadata: &UiSessionMetadata,
    subject: Option<String>,
    waits_for_decision: bool,
    access_url: &str,
    window_url: Option<&str>,
) -> Result<PathBuf> {
    let directory = sessions_directory()?;
    fs::create_dir_all(&directory).map_err(Error::Io)?;
    restrict_to_owner(&directory, 0o700)?;
    let entry = UiSessionEntry {
        session_id: metadata.session_id.as_str().to_owned(),
        provider_id: metadata.provider_id.clone(),
        ui_kind: metadata.ui_kind.clone(),
        subject,
        waits_for_decision,
        access_url_secret: access_url.to_owned(),
        window_url_secret: window_url.map(str::to_owned),
        // Read back from the file this call is about to write; nothing is
        // stamped into the entry itself.
        announced_at_epoch_ms: 0,
        owner_pid: std::process::id(),
    };
    let path = directory.join(format!("{}.json", entry.session_id));
    let body = serde_json::to_vec_pretty(&entry)
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
    write_private(&path, &body)?;
    Ok(path)
}

/// Publish a complete, private entry atomically from the same directory.
///
/// The temporary file is private from creation, gets its final permissions
/// before holding a credential, and is ignored by registry readers. Its unique
/// name permits concurrent publication; failed writes or installs remove it.
fn write_private(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;

    let parent = path.parent().ok_or_else(|| {
        Error::Io(std::io::Error::other(
            "session entry path has no parent directory",
        ))
    })?;
    let mut file = tempfile::Builder::new()
        .prefix(".entry-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(Error::Io)?;
    restrict_to_owner(file.path(), 0o600)?;
    file.write_all(body).map_err(Error::Io)?;
    // `std::fs::rename`, not `NamedTempFile::persist`. On Windows `persist`
    // replaces through `MoveFileEx`, which is refused while any reader has the
    // old entry open, so a provider republishing while another process lists
    // sessions failed with access denied. std renames with POSIX semantics
    // there, which replace a file that readers opened with delete sharing, as
    // std opens every file. `keep` first, so the installed entry does not
    // carry the temporary-file attribute on Windows; the temporary is then
    // ours to remove if the rename fails.
    let temp = file
        .into_temp_path()
        .keep()
        .map_err(|error| Error::Io(error.error))?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ignored = fs::remove_file(&temp);
        return Err(Error::Io(error));
    }
    Ok(())
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
            Some(_) => {
                let _ignored = fs::remove_file(&path);
            }
            // An unreadable or malformed record does not establish that its
            // session ended. Leave it in place for a later read or repair.
            None => {}
        }
    }
    // The order things happened, which is the order a person remembers them
    // in. The identity tuple only breaks ties, so two sessions announced in the
    // same millisecond still come back in one stable order rather than
    // swapping places between polls.
    entries.sort_by(|left, right| {
        (
            left.announced_at_epoch_ms,
            &left.provider_id,
            &left.ui_kind,
            &left.session_id,
        )
            .cmp(&(
                right.announced_at_epoch_ms,
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
    let mut entry: UiSessionEntry = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    entry.announced_at_epoch_ms = announced_at(path);
    Some(entry)
}

/// When this entry was written, which is when its session announced itself.
///
/// A filesystem that cannot say sorts the entry first rather than costing it
/// its listing: being in an odd place beats not being there.
fn announced_at(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub fn sessions_directory() -> Result<PathBuf> {
    Ok(global_afui_root()?.join("sessions"))
}

/// Whether `AFUI_NO_REGISTRY` keeps this process's sessions out of the registry.
#[must_use]
pub fn registry_opted_out() -> bool {
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

    /// Drain liveness probes so repeated listing cannot fill a listen backlog.
    struct AcceptingHost {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl AcceptingHost {
        fn start() -> (Self, String) {
            use std::sync::atomic::Ordering;

            let (listener, url) = listening();
            listener.set_nonblocking(true).unwrap();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_stop = std::sync::Arc::clone(&stop);
            let worker = std::thread::spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((_stream, _address)) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        Err(error) => panic!("accept a registry liveness probe: {error}"),
                    }
                }
            });
            (
                Self {
                    stop,
                    worker: Some(worker),
                },
                url,
            )
        }
    }

    impl Drop for AcceptingHost {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ignored = worker.join();
            }
        }
    }

    #[test]
    fn concurrent_publication_and_listing_preserve_a_complete_live_entry() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let metadata = metadata("reviewer", "case_review");
        let (_host, url) = AcceptingHost::start();
        let padding = "x".repeat(32 * 1024);
        let path = write_entry(
            &metadata,
            Some(format!("revision 0: {padding}")),
            true,
            &url,
            None,
        )
        .unwrap();
        let start = std::sync::Barrier::new(2);

        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                start.wait();
                for revision in 1..=128 {
                    write_entry(
                        &metadata,
                        Some(format!("revision {revision}: {padding}")),
                        true,
                        &url,
                        None,
                    )
                    .unwrap();
                }
            });
            start.wait();
            for _ in 0..128 {
                let entry = find_session(metadata.session_id.as_str())
                    .unwrap()
                    .expect("publishing another revision must not hide a live session");
                assert_eq!(entry.access_url_secret, url);
                assert!(entry.waits_for_decision);
                assert!(entry.subject.unwrap().starts_with("revision "));
                let bytes = fs::read(&path).expect("listing must not remove the live entry");
                let complete: UiSessionEntry = serde_json::from_slice(&bytes)
                    .expect("a concurrent reader must see a complete JSON record");
                assert_eq!(complete.session_id, metadata.session_id.as_str());
                assert_eq!(complete.access_url_secret, url);
            }
            writer.join().unwrap();
        });

        assert!(
            find_session(metadata.session_id.as_str())
                .unwrap()
                .is_some()
        );
        assert!(path.is_file());
    }

    #[test]
    fn malformed_registry_entries_are_left_in_place() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let directory = sessions_directory().unwrap();
        fs::create_dir_all(&directory).unwrap();
        let partial = directory.join("partial.json");
        let missing_fields = directory.join("missing-fields.json");
        let partial_body = br#"{"session_id":"incomplete""#;
        let missing_fields_body = br#"{"session_id":"missing-fields"}"#;
        fs::write(&partial, partial_body).unwrap();
        fs::write(&missing_fields, missing_fields_body).unwrap();

        let entries = list_sessions().unwrap();
        assert!(entries.iter().all(|entry| {
            entry.session_id != "incomplete" && entry.session_id != "missing-fields"
        }));
        assert_eq!(fs::read(&partial).unwrap(), partial_body);
        assert_eq!(fs::read(&missing_fields).unwrap(), missing_fields_body);
    }

    #[test]
    fn failed_publication_leaves_the_target_and_no_temporary_credentials() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("occupied.json");
        fs::create_dir(&target).unwrap();
        let sentinel = target.join("keep.txt");
        fs::write(&sentinel, b"existing target contents").unwrap();
        let before = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(matches!(
            write_private(&target, b"temporary-credential-canary"),
            Err(Error::Io(_))
        ));

        assert!(target.is_dir());
        assert_eq!(fs::read(&sentinel).unwrap(), b"existing target contents");
        let after = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            after, before,
            "failed publication must remove its temporary file"
        );
    }

    #[test]
    fn a_published_session_is_visible_then_disappears_when_it_ends() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _root = isolated_root();
        let metadata = metadata("inspector", "table_inspect");
        let (host, url) = listening();

        let handle = publish(
            &metadata,
            Some("public.orders".to_owned()),
            true,
            &url,
            None,
        );
        // Assert on this session rather than on the directory as a whole:
        // every session in this binary announces itself, so the registry is
        // shared and a total count would be a race, not a fact.
        let mine = find_session(metadata.session_id.as_str()).unwrap().unwrap();
        assert_eq!(mine.provider_id, "inspector");
        assert_eq!(mine.ui_kind, "table_inspect");
        assert_eq!(mine.subject.as_deref(), Some("public.orders"));
        assert_eq!(mine.access_url_secret, url);
        assert_eq!(mine.owner_pid, std::process::id());
        assert!(mine.waits_for_decision);

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
        std::mem::forget(publish(&metadata, None, false, &url, None));
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
        let handle = publish(&metadata, None, false, &url, None);
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
            waits_for_decision: false,
            access_url_secret: format!("{url}?handoff_secret=deadbeef"),
            window_url_secret: None,
            announced_at_epoch_ms: 0,
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
        let handle = publish(&published, None, false, &url, None);
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
        // Replacing an older entry must install a private inode even when the
        // previous file was accidentally made readable by other users.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_entry(&published, None, false, &url, None).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "republishing must restore owner-only access"
        );
        assert_eq!(read_entry(&path).unwrap().access_url_secret, url);
        drop(handle);
        drop(host);
    }
}
