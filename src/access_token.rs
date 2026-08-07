use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_first_data::value_source::SecretString;
use getrandom::fill;
use subtle::ConstantTimeEq;

use crate::{Error, Result};

const TOKEN_BYTES: usize = 32;

/// When a UI credential stops being usable.
///
/// This is a property of *delivery*, not of the session. A window the person
/// can see is bounded by the window itself, so its credential never needs to
/// expire; a URL handed to someone who may never open it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiExpiry {
    /// No expiry. The delivery bounds the session — used for window delivery,
    /// where closing the window ends everything.
    Never,
    /// A fixed lifetime measured from issue, however much the person uses it.
    After(Duration),
    /// A sliding lifetime: expires only after this long with no authorized
    /// request. A page that keeps polling keeps its credential alive.
    Idle(Duration),
}

/// A revocable credential for one UI session.
#[derive(Clone)]
pub struct UiAccessToken {
    inner: Arc<Inner>,
}

struct Inner {
    secret: [u8; TOKEN_BYTES],
    expiry: UiExpiry,
    /// `None` for `UiExpiry::Never`. Interior mutability so an idle expiry can
    /// slide forward from the request path without threading `&mut` through it.
    expires_at: Mutex<Option<Instant>>,
    revoked: AtomicBool,
}

impl UiAccessToken {
    pub fn issue(expiry: UiExpiry) -> Result<Self> {
        let lifetime = match expiry {
            UiExpiry::Never => None,
            UiExpiry::After(duration) | UiExpiry::Idle(duration) => {
                if duration.is_zero() {
                    return Err(Error::InvalidTtl);
                }
                Some(duration)
            }
        };
        let mut secret = [0_u8; TOKEN_BYTES];
        fill(&mut secret)
            .map_err(|error| Error::Random(std::io::Error::other(error.to_string())))?;
        Ok(Self {
            inner: Arc::new(Inner {
                secret,
                expiry,
                expires_at: Mutex::new(lifetime.map(|lifetime| Instant::now() + lifetime)),
                revoked: AtomicBool::new(false),
            }),
        })
    }

    #[must_use]
    /// The secret form used when constructing a handoff URL.
    ///
    /// AFDATA's `SecretString`: `Debug` and `Display` redact it, and the value
    /// comes out only through `expose_secret`, at the boundary that builds the
    /// URL. afui used to hand-roll that newtype; there is one now, and every
    /// spore holding a credential shares it.
    pub fn secret(&self) -> SecretString {
        SecretString::new(hex::encode(self.inner.secret))
    }

    /// Constant-time check that `candidate_secret` is this credential, and that
    /// it is still live. An idle expiry slides forward on success.
    #[must_use]
    pub fn authorize(&self, candidate_secret: &str) -> bool {
        if self.is_expired() || self.is_revoked() {
            return false;
        }
        let Ok(candidate) = hex::decode(candidate_secret) else {
            return false;
        };
        let matches: bool =
            candidate.len() == TOKEN_BYTES && self.inner.secret.ct_eq(candidate.as_slice()).into();
        if matches {
            self.touch();
        }
        matches
    }

    /// Slide an idle expiry forward. A no-op for every other expiry kind, so
    /// the request path can call it unconditionally.
    pub fn touch(&self) {
        let UiExpiry::Idle(window) = self.inner.expiry else {
            return;
        };
        if let Ok(mut expires_at) = self.inner.expires_at.lock() {
            *expires_at = Some(Instant::now() + window);
        }
    }

    pub fn revoke(&self) {
        self.inner.revoked.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.inner.revoked.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at()
            .is_some_and(|expires_at| Instant::now() >= expires_at)
    }

    /// How long this credential has left, or `None` when it never expires.
    ///
    /// Callers race this against the person acting. `None` means there is
    /// nothing to race — do not substitute a default.
    #[must_use]
    pub fn remaining_ttl(&self) -> Option<Duration> {
        self.expires_at()
            .map(|expires_at| expires_at.saturating_duration_since(Instant::now()))
    }

    fn expires_at(&self) -> Option<Instant> {
        self.inner
            .expires_at
            .lock()
            .ok()
            .and_then(|expires_at| *expires_at)
    }
}

impl fmt::Debug for UiAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiAccessToken")
            .field("secret", &"***")
            .field("revoked", &self.is_revoked())
            .field("expired", &self.is_expired())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_authorizes_until_revoked() {
        let token = UiAccessToken::issue(UiExpiry::After(Duration::from_secs(60))).unwrap();
        let secret = token.secret();
        assert!(token.authorize(secret.expose_secret()));
        assert!(!token.authorize("not-a-token"));
        token.revoke();
        assert!(!token.authorize(secret.expose_secret()));
    }

    #[test]
    fn secret_is_redacted_in_formatting() {
        let token = UiAccessToken::issue(UiExpiry::After(Duration::from_secs(60))).unwrap();
        let secret = token.secret();
        assert_eq!(secret.to_string(), "***");
        assert!(!format!("{token:?}").contains(secret.expose_secret()));
    }

    #[test]
    fn a_window_credential_never_expires() {
        let token = UiAccessToken::issue(UiExpiry::Never).unwrap();
        assert_eq!(token.remaining_ttl(), None);
        assert!(!token.is_expired());
        // Nothing to race against: a window session ends when the window does.
        token.touch();
        assert_eq!(token.remaining_ttl(), None);
    }

    #[test]
    fn an_idle_credential_slides_forward_on_use_and_a_fixed_one_does_not() {
        // Compare the two credentials against *each other* after the same
        // elapsed time and the same operation, rather than either against its
        // own earlier reading. Two readings of one credential taken moments
        // apart differ only by scheduling noise, so asserting on that ordering
        // is a coin flip under parallel test load — which is exactly how this
        // test used to fail. The gap between the two credentials, by contrast,
        // is the whole window that was given back, and nothing else.
        const WINDOW: Duration = Duration::from_millis(400);
        const ELAPSED: Duration = Duration::from_millis(150);

        let idle = UiAccessToken::issue(UiExpiry::Idle(WINDOW)).unwrap();
        let fixed = UiAccessToken::issue(UiExpiry::After(WINDOW)).unwrap();
        std::thread::sleep(ELAPSED);
        assert!(idle.authorize(idle.secret().expose_secret()));
        assert!(fixed.authorize(fixed.secret().expose_secret()));

        let idle_left = idle.remaining_ttl().unwrap();
        let fixed_left = fixed.remaining_ttl().unwrap();
        // The fixed credential spent ELAPSED of its window and cannot get it
        // back, however much it is used.
        assert!(
            fixed_left < WINDOW - ELAPSED / 2,
            "a fixed window must keep counting down: {fixed_left:?}"
        );
        // The idle one restarted its window on that authorized request, so it
        // now leads the fixed one by about ELAPSED — a margin far larger than
        // any scheduling jitter between these two reads.
        assert!(
            idle_left > fixed_left + ELAPSED / 2,
            "an idle window must slide forward on use: idle {idle_left:?} vs fixed {fixed_left:?}"
        );
    }

    #[test]
    fn a_zero_lifetime_is_rejected_but_never_is_not() {
        assert!(UiAccessToken::issue(UiExpiry::After(Duration::ZERO)).is_err());
        assert!(UiAccessToken::issue(UiExpiry::Idle(Duration::ZERO)).is_err());
        assert!(UiAccessToken::issue(UiExpiry::Never).is_ok());
    }
}
