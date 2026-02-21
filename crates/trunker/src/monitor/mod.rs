//! P25 monitor TUI subsystem.
//!
//! Provides real-time monitoring of P25 control channel activity
//! by parsing JSON output from the decoder and displaying it in
//! a terminal user interface.

use std::time::Duration;

pub mod error;
pub mod event;
pub mod parse;
pub mod state;
pub mod ui;

/// Configuration for the monitor TUI.
pub struct MonitorConfig {
    /// Duration after which voice grants are considered expired.
    pub grant_timeout: Duration,
}

/// Entry point for the monitor subcommand.
///
/// Creates monitor state and runs the event loop until the user quits.
pub fn run(config: MonitorConfig) -> Result<(), error::MonitorError> {
    let mut state = state::MonitorState::new(config.grant_timeout);
    event::run_event_loop(&mut state)
}
