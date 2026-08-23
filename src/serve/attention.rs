//! One lifecycle for every AFUI-owned remote page.
//!
//! A local window ends when the person closes it. A remote page has no such
//! physical bound: a phone can lock, a tab can disappear into the background,
//! and neither event closes the underlying session. AFUI therefore owns one
//! attention clock for both direct `link` delivery and `afui session serve`.
//! The clock warns first, while the page remains fully usable, and only then
//! makes that remote view unavailable.

use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Notify;

use crate::{Error, Result, global_afui_root};

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DEFAULT_GRACE_PERIOD: Duration = Duration::from_secs(5 * 60);

/// The global settings file read by AFUI-owned remote pages.
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    attention: AttentionConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AttentionConfig {
    idle_timeout_s: u64,
    grace_period_s: u64,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT.as_secs(),
            grace_period_s: DEFAULT_GRACE_PERIOD.as_secs(),
        }
    }
}

/// When an unattended AFUI page lapses, and how long it warns first.
///
/// This policy belongs to the delivery layer rather than to any Provider. A
/// direct link and a shared session page therefore read the same global AFUI
/// configuration and run the same state machine. What happens after the view
/// lapses remains delivery-specific: a direct link returns
/// [`crate::Outcome::Expired`], while a shared page only revokes that browser
/// view and leaves its registered sessions running.
///
/// By default a page warns after 15 minutes without attention and lapses after
/// a further 5 minutes. The global override is `config.json` under
/// [`crate::global_afui_root`] (or `AFUI_CONFIG_DIR` in an isolated runtime):
///
/// ```json
/// {
///   "attention": {
///     "idle_timeout_s": 900,
///     "grace_period_s": 300
///   }
/// }
/// ```
///
/// `idle_timeout_s: 0` disables attention expiry. `grace_period_s: 0` keeps
/// expiry but removes the warning interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiAttentionPolicy {
    idle_timeout: Option<Duration>,
    grace_period: Duration,
}

impl Default for UiAttentionPolicy {
    fn default() -> Self {
        Self {
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            grace_period: DEFAULT_GRACE_PERIOD,
        }
    }
}

impl UiAttentionPolicy {
    /// Read the global AFUI policy, or return the built-in policy when the file
    /// does not exist.
    pub fn load_global() -> Result<Self> {
        let path = global_afui_root()?.join(CONFIG_FILE);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&path).map_err(Error::Io)?;
        let config: Config = serde_json::from_slice(&bytes)
            .map_err(|source| Error::AttentionConfigInvalid { path, source })?;
        Ok(Self::default()
            .with_idle_timeout(Duration::from_secs(config.attention.idle_timeout_s))
            .with_grace_period(Duration::from_secs(config.attention.grace_period_s)))
    }

    /// Nothing lapses. The process or session lifetime is the only bound.
    #[must_use]
    pub fn never() -> Self {
        Self {
            idle_timeout: None,
            grace_period: Duration::ZERO,
        }
    }

    /// Replace the unattended interval. Zero disables attention expiry.
    #[must_use]
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = (!idle_timeout.is_zero()).then_some(idle_timeout);
        self
    }

    /// Replace the warning interval. Zero lapses without a warning.
    #[must_use]
    pub fn with_grace_period(mut self, grace_period: Duration) -> Self {
        self.grace_period = grace_period;
        self
    }

    #[must_use]
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }

    #[must_use]
    pub fn grace_period(&self) -> Duration {
        self.grace_period
    }
}

/// Where one attention clock stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Live,
    /// Warned, counting down, and still completely usable.
    Expiring {
        remaining: Duration,
    },
    /// Over. A remote credential is revoked and is never silently revived.
    Dead,
}

impl Phase {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Expiring { .. } => "expiring",
            Self::Dead => "dead",
        }
    }
}

/// One clock: the last time somebody was here, and how far that has run out.
#[derive(Debug)]
pub(super) struct Attention {
    last_attended: Instant,
    /// Sticky, so returning requires an explicit action rather than the next
    /// background request silently resurrecting a lapsed view.
    dead: bool,
}

impl Attention {
    pub(super) fn new() -> Self {
        Self {
            last_attended: Instant::now(),
            dead: false,
        }
    }

    pub(super) fn attend(&mut self) {
        if !self.dead {
            self.last_attended = Instant::now();
        }
    }

    pub(super) fn phase(&mut self, policy: &UiAttentionPolicy) -> Phase {
        if self.dead {
            return Phase::Dead;
        }
        let Some(idle_timeout) = policy.idle_timeout else {
            return Phase::Live;
        };
        let Some(past_idle) = self.last_attended.elapsed().checked_sub(idle_timeout) else {
            return Phase::Live;
        };
        if past_idle >= policy.grace_period {
            self.dead = true;
            return Phase::Dead;
        }
        Phase::Expiring {
            remaining: policy.grace_period - past_idle,
        }
    }

    fn remaining_until_dead(&mut self, policy: &UiAttentionPolicy) -> Option<Duration> {
        if self.dead {
            return Some(Duration::ZERO);
        }
        let idle_timeout = policy.idle_timeout?;
        let lifetime = idle_timeout.saturating_add(policy.grace_period);
        let remaining = lifetime.saturating_sub(self.last_attended.elapsed());
        if remaining.is_zero() {
            self.dead = true;
        }
        Some(remaining)
    }
}

/// A clock shared by request handlers and the direct-link waiter.
///
/// The served page advances the phase lazily on requests. Direct link delivery
/// also needs an outcome when nobody ever opens the URL, so this wrapper adds a
/// wake-up signal and an async deadline without introducing another clock.
pub(super) struct AttentionClock {
    attention: Mutex<Attention>,
    changed: Notify,
}

impl AttentionClock {
    pub(super) fn new() -> Self {
        Self {
            attention: Mutex::new(Attention::new()),
            changed: Notify::new(),
        }
    }

    pub(super) fn phase(&self, policy: &UiAttentionPolicy) -> Phase {
        self.attention
            .lock()
            .map(|mut attention| attention.phase(policy))
            .unwrap_or(Phase::Live)
    }

    pub(super) fn attend(&self) {
        if let Ok(mut attention) = self.attention.lock() {
            attention.attend();
            self.changed.notify_waiters();
        }
    }

    pub(super) async fn wait_until_dead(&self, policy: &UiAttentionPolicy) {
        loop {
            // Register before reading the deadline so attention arriving in
            // between cannot be lost and leave this sleeping on an old one.
            let changed = self.changed.notified();
            let remaining = self
                .attention
                .lock()
                .ok()
                .and_then(|mut attention| attention.remaining_until_dead(policy));
            match remaining {
                None => std::future::pending::<()>().await,
                Some(remaining) if remaining.is_zero() => return,
                Some(remaining) => {
                    tokio::select! {
                        () = tokio::time::sleep(remaining) => {}
                        () = changed => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use crate::{CONFIG_DIR_ENV, ENV_LOCK};

    #[test]
    fn default_policy_has_one_warning_after_one_idle_interval() {
        let policy = UiAttentionPolicy::default();
        assert_eq!(policy.idle_timeout(), Some(Duration::from_secs(900)));
        assert_eq!(policy.grace_period(), Duration::from_secs(300));
    }

    #[test]
    fn global_config_replaces_both_intervals_and_zero_disables_expiry() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(CONFIG_FILE);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(br#"{"attention":{"idle_timeout_s":42,"grace_period_s":7}}"#)
            .unwrap();
        // SAFETY: every AFUI unit test that mutates this variable uses ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_DIR_ENV, root.path()) };
        let configured = UiAttentionPolicy::load_global().unwrap();
        assert_eq!(configured.idle_timeout(), Some(Duration::from_secs(42)));
        assert_eq!(configured.grace_period(), Duration::from_secs(7));

        fs::write(
            &path,
            br#"{"attention":{"idle_timeout_s":0,"grace_period_s":9}}"#,
        )
        .unwrap();
        let unlimited = UiAttentionPolicy::load_global().unwrap();
        assert_eq!(unlimited.idle_timeout(), None);
        assert_eq!(unlimited.grace_period(), Duration::from_secs(9));
        // SAFETY: every AFUI unit test that mutates this variable uses ENV_LOCK.
        unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    }

    #[test]
    fn unknown_attention_config_fields_are_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(CONFIG_FILE),
            br#"{"attention":{"idle_timeout_s":42,"grace_period_s":7,"typo":1}}"#,
        )
        .unwrap();
        // SAFETY: every AFUI unit test that mutates this variable uses ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_DIR_ENV, root.path()) };
        let error = UiAttentionPolicy::load_global().unwrap_err().to_string();
        assert!(error.contains("typo"), "{error}");
        // SAFETY: every AFUI unit test that mutates this variable uses ENV_LOCK.
        unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    }
}
