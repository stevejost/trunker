//! Error types for the P25 monitor TUI.

use thiserror::Error;

/// Errors that can occur in the monitor subsystem.
#[derive(Debug, Error)]
pub enum MonitorError {
    /// Failed to parse a JSON line from the decoder.
    #[error("failed to parse JSON line: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// Terminal I/O error (crossterm/ratatui).
    #[error("terminal I/O error: {0}")]
    TerminalIo(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_parse_error_display() {
        let bad_json = serde_json::from_str::<serde_json::Value>("not json");
        let error = MonitorError::from(bad_json.unwrap_err());
        let message = error.to_string();
        assert!(message.contains("failed to parse JSON line"));
    }

    #[test]
    fn terminal_io_error_display() {
        let io_error = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe closed");
        let error = MonitorError::from(io_error);
        let message = error.to_string();
        assert!(message.contains("terminal I/O error"));
        assert!(message.contains("pipe closed"));
    }

    #[test]
    fn json_parse_error_from_conversion() {
        let result: Result<serde_json::Value, _> = serde_json::from_str("{invalid}");
        let monitor_error: MonitorError = result.unwrap_err().into();
        assert!(matches!(monitor_error, MonitorError::JsonParse(_)));
    }

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let monitor_error: MonitorError = io_err.into();
        assert!(matches!(monitor_error, MonitorError::TerminalIo(_)));
    }
}
