//! Decode event model, consumer abstraction, and decode loops.
//!
//! This module defines [`DecoderEvent`], the unified event type produced
//! by the P25 decode pipeline, [`EventSink`], the trait that consumers
//! implement to handle events, and the orchestration loops for CC-only
//! and wideband trunked decoding.

pub mod cc_handler;
pub mod control_channel;
pub mod error;
pub mod event;
pub mod heartbeat;
pub mod trunked;
pub mod voice_handler;

pub use control_channel::ControlChannelConfig;
pub use error::DecodeError;
pub use event::{DecoderEvent, EventSink};
pub use heartbeat::HeartbeatState;
pub use trunked::TrunkedDecoderConfig;
