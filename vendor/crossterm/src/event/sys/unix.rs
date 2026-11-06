#[cfg(feature = "event-stream")]
pub(crate) mod waker;

#[cfg(feature = "events")]
pub(crate) mod parse;

#[cfg(feature = "osc")]
pub(crate) mod osc;
