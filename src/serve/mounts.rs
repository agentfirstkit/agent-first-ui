//! What this listener is currently proxying, and for how much longer.
//!
//! One session's whole life on this page is here: the credential it is reached
//! at, the clock that decides whether anybody is still watching, and the three
//! endings — the session stopping, the view lapsing, a person asking for it
//! back. Kept together because they are one decision made in four places
//! otherwise, and the interesting parts are the transitions rather than the
//! table.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use super::addressing::frame_address;
use super::attention::{Phase, UiAttentionPolicy};
use super::proxy::{Proxied, Upstream};
use crate::{Result, UiAccessToken, UiExpiry, UiSessionEntry};

/// The proxied sessions, indexed by the two things that look them up.
#[derive(Default)]
pub(super) struct Mounts {
    by_secret: HashMap<String, Mount>,
    by_session: HashMap<String, String>,
    /// Sessions whose remote view has lapsed. They are still running and still
    /// listed — this is the page's memory that it is not to mount them again
    /// until somebody asks, which is what stops a poll two seconds later from
    /// undoing the thing that just expired.
    lapsed: HashSet<String>,
}

/// One session, reachable through this listener.
struct Mount {
    token: UiAccessToken,
    proxied: Arc<Proxied>,
}

impl Mount {
    fn revoke(&self) {
        self.token.revoke();
        self.proxied.revoke();
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.revoke();
    }
}

impl Mounts {
    /// Bring the table in line with what is actually running.
    ///
    /// This is the whole lifecycle: a session that has appeared gets a
    /// credential, a session that has ended has its credential revoked and its
    /// path removed, and a session nobody has attended to long enough loses its
    /// credential without losing itself. Nothing is asked of the Provider, and
    /// nothing here survives the session it belongs to.
    ///
    /// Reading this list is deliberately **not** attending to anything. The page
    /// polls every couple of seconds whether or not a person is in front of it,
    /// so a poll that renewed what it reported would be a clock that never ran
    /// down. Attention arrives through [`Self::attend`] and through traffic to a
    /// frame, and through nothing else.
    pub(super) fn reconcile(
        &mut self,
        entries: &[UiSessionEntry],
        attention_policy: &UiAttentionPolicy,
        shell_authority: Option<&str>,
        unattended: bool,
    ) -> Result<Vec<Value>> {
        if unattended {
            self.lapse_all();
            self.lapsed
                .extend(entries.iter().map(|entry| entry.session_id.clone()));
        }
        let Mounts {
            by_secret,
            by_session,
            lapsed,
        } = self;
        let live: HashSet<&str> = entries
            .iter()
            .map(|entry| entry.session_id.as_str())
            .collect();
        by_session.retain(|session_id, secret| {
            if live.contains(session_id.as_str()) {
                return true;
            }
            if let Some(mount) = by_secret.remove(secret) {
                mount.revoke();
            }
            false
        });
        // A session that ended is forgotten entirely, lapse included: if it
        // comes back it is a new session with a new id, and holding a grudge
        // against an id nothing is using is just a leak.
        lapsed.retain(|session_id| live.contains(session_id.as_str()));

        let mut cards = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(upstream) = Upstream::parse(&entry.access_url_secret) else {
                continue;
            };
            let mut phase = Phase::Live;
            if lapsed.contains(&entry.session_id) {
                phase = Phase::Dead;
            } else {
                let usable = by_session
                    .get(&entry.session_id)
                    .and_then(|secret| by_secret.get_mut(secret))
                    .filter(|mount| mount.proxied.upstream == upstream)
                    .map(|mount| {
                        if mount.token.is_revoked() {
                            Phase::Dead
                        } else {
                            mount.proxied.attention.phase(attention_policy)
                        }
                    });
                match usable {
                    Some(Phase::Dead) | None => {
                        if let Some(stale) = by_session.remove(&entry.session_id)
                            && let Some(mount) = by_secret.remove(&stale)
                        {
                            // Revoked, not merely dropped: a frame address that
                            // leaked is this credential, and its whole promise
                            // is that it stops working here.
                            mount.revoke();
                        }
                        if usable == Some(Phase::Dead) {
                            lapsed.insert(entry.session_id.clone());
                            phase = Phase::Dead;
                        } else {
                            let token = UiAccessToken::issue(UiExpiry::Never)?;
                            let secret = token.secret().expose_secret().to_owned();
                            by_secret.insert(
                                secret.clone(),
                                Mount {
                                    token,
                                    proxied: Arc::new(Proxied::new(upstream)),
                                },
                            );
                            by_session.insert(entry.session_id.clone(), secret);
                        }
                    }
                    Some(known) => phase = known,
                }
            }

            let mut card = json!({
                "session_id": entry.session_id,
                "provider_id": entry.provider_id,
                "ui_kind": entry.ui_kind,
                "state": phase.word(),
            });
            if let Some(subject) = &entry.subject {
                card["subject"] = Value::String(subject.clone());
            }
            if let Phase::Expiring { remaining } = phase {
                card["expires_in_ms"] = json!(remaining.as_millis());
            }
            if phase != Phase::Dead {
                let Some(secret) = by_session.get(&entry.session_id) else {
                    continue;
                };
                let Some(framed_at) = frame_address(secret, shell_authority) else {
                    continue;
                };
                // Where this session is framed. It carries the only credential
                // the Provider UI is given, so it is named for what it is. The
                // session's own `access_url_secret` never leaves this process.
                //
                // Absent while lapsed, which is what takes the frame off the
                // page: a card with no address has nothing to show, rather than
                // an iframe left holding the last thing it painted.
                card["proxy_url_secret"] = Value::String(framed_at);
            }
            cards.push(card);
        }
        Ok(cards)
    }

    /// Revoke every mount, including streams holding a clone of the proxy.
    pub(super) fn lapse_all(&mut self) {
        self.lapsed.extend(self.by_session.keys().cloned());
        self.by_session.clear();
        self.by_secret.clear();
    }

    /// Somebody is looking at these. A session already lapsed is skipped rather
    /// than revived — coming back is [`Self::revive`], an act a person performs
    /// on one card, not something a heartbeat does to everything it can reach.
    pub(super) fn attend(&mut self, session_ids: &[String], policy: &UiAttentionPolicy) {
        for session_id in session_ids {
            let Some(secret) = self.by_session.get(session_id).cloned() else {
                continue;
            };
            if let Some(mount) = self.by_secret.get_mut(&secret) {
                if mount.proxied.attention.phase(policy) == Phase::Dead {
                    mount.revoke();
                } else {
                    mount.proxied.attention.attend();
                }
            }
        }
    }

    /// Stop refusing to mount this session. Mounting it again is
    /// [`Self::reconcile`]'s job on the next poll — this only takes the refusal
    /// away, and the credential that lapsed stays revoked.
    pub(super) fn revive(&mut self, session_id: &str) -> bool {
        self.lapsed.remove(session_id)
    }

    /// The live mount belonging to a session already listed on the shell.
    pub(super) fn authorize_session(
        &mut self,
        session_id: &str,
        attention_policy: &UiAttentionPolicy,
    ) -> Option<Arc<Proxied>> {
        let secret = self.by_session.get(session_id)?.clone();
        self.authorize(&secret, attention_policy)
    }

    /// The session a proxy credential names, if it is still live.
    ///
    /// The lookup is a hash of the candidate; the check that decides is
    /// constant-time and honours revocation. A credential that fails it never
    /// reaches this session's cookie jar, which is what keeps one session's
    /// cookies out of another's requests.
    pub(super) fn authorize(
        &mut self,
        secret: &str,
        attention_policy: &UiAttentionPolicy,
    ) -> Option<Arc<Proxied>> {
        let mount = self.by_secret.get_mut(secret)?;
        if !mount.token.authorize(secret) {
            return None;
        }
        // Still served while the warning runs: five minutes of a broken frame
        // is not a warning, it is the outage arriving early.
        if mount.proxied.attention.phase(attention_policy) == Phase::Dead {
            mount.revoke();
            return None;
        }
        mount.proxied.attention.attend();
        Some(Arc::clone(&mount.proxied))
    }
}
