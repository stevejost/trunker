//! Decode event model and consumer abstraction.
//!
//! This module defines [`DecoderEvent`], the unified event type produced
//! by the P25 decode pipeline, and [`EventSink`], the trait that consumers
//! implement to handle events.

pub mod error;
pub mod event;

pub use error::DecodeError;
pub use event::{DecoderEvent, EventSink};
