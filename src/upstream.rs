//! A session whose content something else already serves.
//!
//! Everywhere else in this crate a session ends with AFUI holding the socket: a
//! [`crate::UiSession`] is mounted on a [`crate::UiMount`], AFUI routes its
//! requests, AFUI minted the credential in its URL, and ending the session
//! revokes it. Some Provider UIs are not shaped like that at all: a
//! third-party web client, reverse-proxied by the Provider's own
//! authenticated listener, behind a credential the Provider mints, on a port
//! the Provider bound. There is nothing there for AFUI to serve and nothing
//! for it to mint.
//!
//! Until this existed, that meant the panel could not appear in `afui session
//! list` and could not be reached by `afui session serve` — so the case that
//! motivates the whole remote shell (an unattended fetch hits a bot wall and a
//! person has to solve it from a phone) was the one case the shell could not
//! show. This is §4.6 clause 1's other half: a session gives a web UI either by
//! serving one itself, or by naming one that already exists.
//!
//! **What AFUI owns here is the listing, and only the listing.** The credential
//! inside the URL belongs to whoever serves it: they minted it, they scoped it,
//! their TTL governs it, and they are the only ones who can revoke it. The
//! difference is structural rather than a promise in a comment —
//! [`AnnouncedUiUpstream`] holds no [`crate::UiAccessToken`], so there is
//! nothing for it to revoke. Withdrawing an announcement stops the session
//! being listed; it does not and cannot stop the URL working.
//!
//! Nothing new reaches the registry. An announced upstream writes the same
//! [`crate::UiSessionEntry`] a served session writes, so anything reading that
//! list — the CLI, the remote shell — never learns which kind it is holding.

use crate::frontend::validate_identifier;
use crate::registry::{self, RegistryHandle};
use crate::session::UiSessionId;
use crate::{Error, Result, UiSessionMetadata};

/// A live UI that this process does not serve, ready to be announced.
///
/// The three things §4.6 asks of any session are the caller's to satisfy here,
/// because AFUI cannot check them from the outside:
///
/// 1. **a web UI exists** — `access_url_secret` is it, already answering;
/// 2. **the ending is nameable** — announcing lasts exactly as long as the
///    returned [`AnnouncedUiUpstream`] is held, so the ending is whatever ends
///    that scope;
/// 3. **a process bounds it** — the announcing process is that process. A
///    listed session is pruned when either its host stops answering or its
///    announcer is gone, and for an upstream those are genuinely two different
///    processes.
///
/// ```no_run
/// use agent_first_ui::UiUpstream;
///
/// # fn example(panel_url_secret: &str) -> agent_first_ui::Result<()> {
/// let announced = UiUpstream::new("remote", "takeover", panel_url_secret)?
///     .with_subject("captcha wall")
///     .announce()?;
/// // Listed for as long as this value is held, and withdrawn when it is not.
/// drop(announced);
/// # Ok(())
/// # }
/// ```
pub struct UiUpstream {
    metadata: UiSessionMetadata,
    subject: Option<String>,
    access_url: String,
}

/// One announced upstream, listed for as long as this is held.
///
/// Deliberately holds no credential. A [`crate::ActiveUiSession`] carries the
/// [`crate::UiAccessToken`] AFUI minted for it and revokes it on the way out;
/// there is no equivalent here, because the credential in this session's URL
/// was minted elsewhere and revoking it is that owner's business. Dropping this
/// withdraws the listing and touches nothing else.
pub struct AnnouncedUiUpstream {
    metadata: UiSessionMetadata,
    access_url: String,
    /// Withdraws the entry from the cross-process registry when dropped.
    _announcement: RegistryHandle,
}

impl UiUpstream {
    /// Name a UI that is already being served somewhere else.
    ///
    /// `access_url_secret` is the whole address a person would open, credential
    /// included — such a panel often carries its credential as a query parameter, and
    /// the query is as much part of the address as the path is. It is
    /// `_secret`-suffixed because it carries one, and AFUI treats it the way it
    /// treats every session URL: stored `0600` in a private directory, handed
    /// to a window or reproduced upstream by the proxy, never printed.
    ///
    /// AFUI does not mint, check, scope, extend or revoke that credential, and
    /// cannot: it did not issue it and does not know its rules. Ending the
    /// announcement removes the listing and leaves the URL exactly as valid as
    /// its owner says it is.
    ///
    /// Only `http://` is accepted, with a host — the registry's liveness probe
    /// and the `afui session serve` proxy both read this URL back on that
    /// assumption. A fragment is refused because it is a browser-side artefact
    /// that never reaches the server and would only mislead whoever read it
    /// back.
    pub fn new(
        provider_id: impl Into<String>,
        ui_kind: impl Into<String>,
        access_url_secret: impl AsRef<str>,
    ) -> Result<Self> {
        let provider_id = provider_id.into();
        let ui_kind = ui_kind.into();
        validate_identifier("provider_id", &provider_id)?;
        validate_identifier("ui_kind", &ui_kind)?;
        Ok(Self {
            metadata: UiSessionMetadata {
                provider_id,
                ui_kind,
                session_id: UiSessionId::generate()?,
            },
            subject: None,
            access_url: normalise_upstream_url(access_url_secret.as_ref())?,
        })
    }

    /// Name what this session is about, in the Provider's own words.
    ///
    /// Identity, not presentation, exactly as on [`crate::UiSession`]: AFUI
    /// never parses it. It is what lets a person tell two panels apart.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        &self.metadata
    }

    /// Publish this session so other processes can see it.
    ///
    /// Fails loudly when the entry cannot be written, unlike the announcement a
    /// served session makes. There the announcement is a convenience — the
    /// Provider still has its window and its URL — but here it is the entire
    /// delivery, and an announcement nobody can see is not a degraded outcome,
    /// it is no outcome. `AFUI_NO_REGISTRY` is still honoured: opting out is a
    /// decision rather than a failure, so it succeeds and lists nothing.
    pub fn announce(self) -> Result<AnnouncedUiUpstream> {
        // No signal here: an announced upstream has no `T` for the automatic
        // half of `waits_for_decision` to read and, unlike `UiSession`, no
        // builder to declare it explicitly. See docs/providers.md.
        let announcement =
            registry::publish_strictly(&self.metadata, self.subject, false, &self.access_url)?;
        Ok(AnnouncedUiUpstream {
            metadata: self.metadata,
            access_url: self.access_url,
            _announcement: announcement,
        })
    }
}

impl AnnouncedUiUpstream {
    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        &self.metadata
    }

    /// The URL as announced, credential included.
    #[must_use]
    pub fn access_url(&self) -> &str {
        &self.access_url
    }
}

/// Check a foreign URL is one this crate can hand back and probe, and give it
/// the trailing-slash-free shape the rest of the crate stores.
///
/// The path matters twice over: [`crate::UiSessionEntry`] finds the authority to
/// probe by splitting at the first `/`, and the proxy splits the same URL into a
/// prefix it prepends to every forwarded request. A URL with a query but no path
/// would make the first of those read the query as part of the host.
fn normalise_upstream_url(supplied: &str) -> Result<String> {
    let invalid = || Error::InvalidUpstreamUrl {
        value: supplied.to_owned(),
    };
    let rest = supplied.strip_prefix("http://").ok_or_else(invalid)?;
    let boundary = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, remainder) = rest.split_at(boundary);
    if authority.is_empty() || remainder.contains('#') {
        return Err(invalid());
    }
    let (path, query) = match remainder.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (remainder, None),
    };
    let path = if path.is_empty() { "/" } else { path };
    Ok(match query {
        Some(query) => format!("http://{authority}{path}?{query}"),
        None => format!("http://{authority}{path}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_foreign_url_keeps_the_query_its_credential_lives_in() {
        // The awkward shape: the credential is a query parameter, so
        // dropping the query would announce a URL that answers 401.
        let announced =
            normalise_upstream_url("http://127.0.0.1:9222/takeover/panel?handoff_secret=deadbeef")
                .unwrap();
        assert_eq!(
            announced,
            "http://127.0.0.1:9222/takeover/panel?handoff_secret=deadbeef"
        );
    }

    #[test]
    fn a_url_with_no_path_still_has_one_before_its_query() {
        // Without this the registry's liveness probe would take
        // `127.0.0.1:9222?handoff_secret=deadbeef` for a host name.
        assert_eq!(
            normalise_upstream_url("http://127.0.0.1:9222?handoff_secret=deadbeef").unwrap(),
            "http://127.0.0.1:9222/?handoff_secret=deadbeef"
        );
        assert_eq!(
            normalise_upstream_url("http://127.0.0.1:9222").unwrap(),
            "http://127.0.0.1:9222/"
        );
    }

    #[test]
    fn a_url_this_crate_could_not_probe_or_proxy_is_refused() {
        for supplied in [
            // TLS is a terminator in front of the owner's listener, not
            // something the probe or the proxy speaks.
            "https://example.com/takeover/panel",
            "http:///takeover/panel",
            "127.0.0.1:9222/takeover/panel",
            // Never sent to a server, so storing it would only mislead.
            "http://127.0.0.1:9222/takeover/panel#top",
        ] {
            assert!(normalise_upstream_url(supplied).is_err(), "{supplied}");
        }
    }

    #[test]
    fn an_upstream_is_identified_the_same_way_a_served_session_is() {
        assert!(UiUpstream::new("remote", "takeover", "http://127.0.0.1:9222/p").is_ok());
        assert!(UiUpstream::new("Inspector", "takeover", "http://127.0.0.1:9222/p").is_err());
        assert!(UiUpstream::new("remote", "../panel", "http://127.0.0.1:9222/p").is_err());
    }
}
