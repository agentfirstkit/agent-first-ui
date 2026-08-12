//! How long a page nobody is looking at keeps working.
//!
//! A window ends when the person closes it; a served page ends when they walk
//! away, and walking away leaves no trace. So this is the clock that notices,
//! and the two-step ending that keeps noticing from being destructive: a
//! warning first, while everything still works, and only then the end.
//!
//! Deliberately free of anything else in this module. It is arithmetic over one
//! instant, it is where the surprising decisions about that arithmetic live,
//! and it is tested without a socket.

use std::time::{Duration, Instant};

/// How long an unattended page keeps working before it says so.
const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);

/// How long the warning lasts. Still fully usable — a warning that has already
/// broken the thing it is warning about is not a warning.
const DEFAULT_GRACE: Duration = Duration::from_secs(5 * 60);

/// When an unattended page stops working, and how much warning it gets first.
///
/// A remote view is the one delivery nobody is bounded by. A window ends when
/// the person closes it; this ends when they walk away, and walking away leaves
/// no trace — a phone goes in a pocket, a tab goes to the back of a browser, a
/// laptop sleeps. So the listener runs a clock, and the page's job is to say
/// "someone is still here" while someone still is.
///
/// The warning matters as much as the expiry. A credential that lapses silently
/// takes the frame with it, and a frame rebuilt underneath a person is a form
/// they were filling in, gone. So the end has two steps: `idle` of nobody
/// attending, then `grace` in which everything still works and the page says
/// out loud that it is about to stop.
///
/// # What this bounds, and what it does not
///
/// This bounds how long an **unattended page** keeps showing what it shows. It
/// is not an access control: whoever holds the listener's URL can reload it and
/// mount every live session again. What genuinely dies is the *view* — and, for
/// a lapsed session, its proxy credential, which is what a leaked frame address
/// was.
///
/// It also cannot see inside a frame. A Provider UI that polls on a timer is
/// traffic, and traffic is the only evidence there is that someone is using it,
/// so such a page holds its own credential open for as long as the browser
/// keeps running it. On a phone that ends when the screen locks, which is the
/// case this exists for.
///
/// The same fact has a less friendly shape on a desktop with several cards
/// open at once. A self-refreshing panel — a status poll, a log tail — keeps
/// generating frame traffic on its own, and every one of those requests
/// renews not only its own card but the *page's* shared clock too
/// (`ServeState::mount`), because a request through any frame is the only
/// evidence this listener has that somebody is at the page at all. So one
/// busy card suspends the idle protection for the whole page — including a
/// quiet neighbour, whose own view still lapses on its own schedule, but
/// whose page stays reachable and its lapsed card reopenable regardless, for
/// as long as the busy card keeps talking. A clock kept per session instead
/// was considered and rejected: it would end the page a person is actually
/// reading because a neighbouring card happened to be quiet at that moment,
/// which is the worse failure. What still bounds the whole thing is the
/// listener's own process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiIdlePolicy {
    idle: Option<Duration>,
    grace: Duration,
}

impl Default for UiIdlePolicy {
    fn default() -> Self {
        Self {
            idle: Some(DEFAULT_IDLE),
            grace: DEFAULT_GRACE,
        }
    }
}

impl UiIdlePolicy {
    /// Nothing lapses. The listener's own lifetime is the only bound.
    #[must_use]
    pub fn never() -> Self {
        Self {
            idle: None,
            grace: Duration::ZERO,
        }
    }

    /// Lapse after this long unattended, with the default warning.
    ///
    /// A zero duration is [`Self::never`] rather than an error: the caller of
    /// this is a CLI flag, and "0 means no limit" is the spelling a person
    /// reaches for.
    #[must_use]
    pub fn after(idle: Duration) -> Self {
        if idle.is_zero() {
            return Self::never();
        }
        Self {
            idle: Some(idle),
            grace: DEFAULT_GRACE,
        }
    }

    /// How long the warning lasts. Zero means the lapse is the first anyone
    /// hears of it.
    #[must_use]
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    #[must_use]
    pub fn idle(&self) -> Option<Duration> {
        self.idle
    }

    #[must_use]
    pub fn grace(&self) -> Duration {
        self.grace
    }
}

/// Where one clock stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Live,
    /// Warned, counting down, and still completely usable.
    Expiring {
        remaining: Duration,
    },
    /// Over. For a session that means its proxy credential is revoked and will
    /// not be minted again unasked; for the page it means the URL is done.
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
///
/// Advanced lazily by [`Self::phase`] rather than by a timer, because there is
/// nothing to do at the moment a credential lapses except answer the next
/// request differently — and something is always asking, since the page polls.
#[derive(Debug)]
pub(super) struct Attention {
    last_attended: Instant,
    /// Sticky, so that coming back has to be an explicit act rather than the
    /// next request arriving. Nothing else here is remembered — both deadlines
    /// are arithmetic on the field above.
    dead: bool,
}

impl Attention {
    pub(super) fn new() -> Self {
        Self {
            last_attended: Instant::now(),
            dead: false,
        }
    }

    /// Somebody is here. Renews, and cancels any warning in progress.
    pub(super) fn attend(&mut self) {
        if self.dead {
            return;
        }
        self.last_attended = Instant::now();
    }

    /// Advance the clock and say where it stands.
    ///
    /// Both deadlines are measured from the last time somebody was here, not
    /// from when this was last asked. A phone asleep for an hour would
    /// otherwise wake up to a fresh five-minute warning — the clock would only
    /// ever run while something was watching it, which is precisely backwards
    /// for a thing whose whole job is to notice that nothing is.
    pub(super) fn phase(&mut self, policy: &UiIdlePolicy) -> Phase {
        if self.dead {
            return Phase::Dead;
        }
        let Some(idle) = policy.idle else {
            return Phase::Live;
        };
        let Some(past_idle) = self.last_attended.elapsed().checked_sub(idle) else {
            return Phase::Live;
        };
        if past_idle >= policy.grace {
            self.dead = true;
            return Phase::Dead;
        }
        Phase::Expiring {
            remaining: policy.grace - past_idle,
        }
    }
}
