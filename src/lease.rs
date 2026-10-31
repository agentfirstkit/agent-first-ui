//! Keeping a credential someone else issued from lapsing.
//!
//! The counterpart to [`crate::UiMountAccess`], on the other side of the wire.
//! A process that was handed an access URL and told it idles out has to renew
//! it while a person is still using it, and give it back when they are done.
//!
//! What AFUI owns here is the cadence and the shape, and nothing else. Which
//! endpoint renews a credential, what it is authenticated with, and what its
//! request looks like are the Provider's own domain API — AFUI would be
//! inventing a wire protocol for someone else's service if it made the call
//! itself. So the caller supplies the call and this supplies the clock.

use std::time::Duration;

use crate::access_token::SECRET_HEX_DIGITS;

/// How many renewals happen inside one idle window.
///
/// Three, so a single lost renewal is not a lapsed credential: two more will
/// have been attempted before the deadline. Two would leave no margin at all,
/// and more than three is traffic bought for nothing.
const RENEWALS_PER_IDLE_WINDOW: u32 = 3;

/// The shortest interval this will renew at, whatever the idle timeout is.
///
/// A one-second idle window is a test fixture, not a deployment, and dividing
/// it further would turn a keep-alive into a busy loop.
const MINIMUM_RENEWAL_INTERVAL: Duration = Duration::from_millis(100);

/// Whether this is the shape of a credential AFUI issues.
///
/// A caller that reads an access URL apart to find the credential inside it
/// would otherwise hard-code "sixty-four hex digits" — a fact about
/// [`crate::UiAccessToken`] held outside this crate, which would go on being
/// true there long after it stopped being true here.
#[must_use]
pub fn is_ui_credential(candidate: &str) -> bool {
    candidate.len() == SECRET_HEX_DIGITS
        && candidate
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

/// The clock a credential's holder renews on.
#[derive(Debug, Clone, Copy)]
pub struct UiCredentialLease {
    idle_timeout: Duration,
}

impl UiCredentialLease {
    /// A lease against a credential that lapses after this long unattended.
    #[must_use]
    pub fn new(idle_timeout: Duration) -> Self {
        Self { idle_timeout }
    }

    /// How long to wait between renewals.
    #[must_use]
    pub fn renewal_interval(&self) -> Duration {
        (self.idle_timeout / RENEWALS_PER_IDLE_WINDOW).max(MINIMUM_RENEWAL_INTERVAL)
    }

    /// Renew forever, and resolve only when a renewal fails.
    ///
    /// Race this against whatever the credential is being held open *for* — a
    /// window, a delivery, a person. It never completes successfully, because
    /// there is no such thing as "done renewing": the thing that finishes is
    /// the work, and dropping this future is how that says so.
    ///
    /// The first renewal happens after one interval, not immediately: a
    /// credential was just issued, so renewing it in the same breath buys
    /// nothing.
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use agent_first_ui::UiCredentialLease;
    ///
    /// # async fn example(renew: impl AsyncFn() -> Result<(), String>) {
    /// let lease = UiCredentialLease::new(Duration::from_secs(600));
    /// let failure = lease.keep_alive(renew).await;
    /// # let _ = failure;
    /// # }
    /// ```
    pub async fn keep_alive<Renew, Failure>(&self, renew: Renew) -> Failure
    where
        Renew: AsyncFn() -> std::result::Result<(), Failure>,
    {
        let interval = self.renewal_interval();
        loop {
            tokio::time::sleep(interval).await;
            if let Err(failure) = renew().await {
                return failure;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn a_credential_is_recognised_by_shape_rather_than_by_a_length_a_caller_remembered() {
        let real = crate::UiAccessToken::issue(crate::UiExpiry::Never).unwrap();
        assert!(is_ui_credential(real.secret().expose_secret()));
        for wrong in [
            "",
            "abc",
            &"a".repeat(SECRET_HEX_DIGITS + 1),
            &"A".repeat(SECRET_HEX_DIGITS),
            &"g".repeat(SECRET_HEX_DIGITS),
        ] {
            assert!(!is_ui_credential(wrong), "{wrong} was accepted");
        }
    }

    /// Three renewals inside one idle window, so a single lost one is not a
    /// lapsed credential.
    #[test]
    fn renewal_happens_several_times_inside_one_idle_window() {
        let lease = UiCredentialLease::new(Duration::from_secs(30));
        assert_eq!(lease.renewal_interval(), Duration::from_secs(10));
    }

    /// A one-second idle window is a fixture, not a deployment; dividing it
    /// further is a busy loop rather than a keep-alive.
    #[test]
    fn a_very_short_idle_window_does_not_become_a_busy_loop() {
        let lease = UiCredentialLease::new(Duration::from_millis(30));
        assert_eq!(lease.renewal_interval(), MINIMUM_RENEWAL_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn keeping_alive_renews_until_one_fails_and_hands_that_failure_back() {
        let attempts = AtomicU32::new(0);
        let lease = UiCredentialLease::new(Duration::from_secs(30));
        let failure = lease
            .keep_alive(async || {
                if attempts.fetch_add(1, Ordering::Relaxed) < 2 {
                    Ok(())
                } else {
                    Err("the endpoint stopped answering")
                }
            })
            .await;
        assert_eq!(failure, "the endpoint stopped answering");
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }
}
