//! Which of the three ways a session reaches a person, and the executor that
//! runs it.
//!
//! The word list and the executor sit in different tiers on purpose. Choosing
//! between `window`, `link`, and `session` is a parse over three strings and
//! one environment variable: no port, no credential, no browser. A Provider
//! whose human UI is a compiled-out option still has a `--mode` flag on its
//! CLI and still has to report which delivery a run used, so making it depend
//! on the executor would force it to invent a second, drifting copy of the
//! same three words. Running the delivery is the part that costs a server, and
//! that part stays behind `session`.

mod mode;

pub use mode::{DELIVERY_ENV, UiDeliveryMode};

#[cfg(feature = "session")]
mod plan;

#[cfg(feature = "session")]
pub use plan::{ActiveUiDelivery, ActiveUiUpstreamDelivery, UiDeliveryFacts, UiDeliveryPlan};
