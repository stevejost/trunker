//! WebSocket command channel for receiving server-to-feeder commands.
//!
//! Connects to `WS /api/feeder/{id}/ws?token={api_key}` and listens
//! for commands. Currently handles "reconnect" by requesting shutdown
//! (the process supervisor restarts the feeder with fresh config).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use serde::Deserialize;
use uuid::Uuid;

/// A command received from the server via WebSocket.
#[derive(Debug, Deserialize)]
struct WsCommand {
    #[serde(rename = "type")]
    command_type: String,
}

/// Maximum time between reconnection attempts.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// Run the WebSocket command listener with auto-reconnect.
pub async fn run(api_url: String, feeder_id: Uuid, api_key: String, running: Arc<AtomicBool>) {
    let ws_url = build_ws_url(&api_url, &feeder_id, &api_key);
    let mut backoff = std::time::Duration::from_secs(1);

    while running.load(Ordering::Relaxed) {
        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((stream, _)) => {
                tracing::info!("WebSocket command channel connected");
                backoff = std::time::Duration::from_secs(1);

                let (_write, mut read) = stream.split();
                while let Some(msg) = read.next().await {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    match msg {
                        Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                            handle_message(&text, &running);
                        }
                        Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                            tracing::info!("WebSocket command channel closed by server");
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "WebSocket error");
                            break;
                        }
                        _ => {} // Ping/pong handled by tungstenite.
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "WebSocket connection failed");
            }
        }

        if !running.load(Ordering::Relaxed) {
            break;
        }

        tracing::info!(retry_in = ?backoff, "reconnecting WebSocket command channel");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn build_ws_url(api_url: &str, feeder_id: &Uuid, api_key: &str) -> String {
    let base = api_url
        .trim_end_matches('/')
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{}/api/feeder/{}/ws?token={}", base, feeder_id, api_key)
}

fn handle_message(text: &str, running: &AtomicBool) {
    match serde_json::from_str::<WsCommand>(text) {
        Ok(cmd) => match cmd.command_type.as_str() {
            "reconnect" => {
                tracing::info!("received reconnect command from server, shutting down for restart");
                running.store(false, Ordering::Relaxed);
            }
            other => {
                tracing::debug!(command = other, "unknown WebSocket command, ignoring");
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, message = text, "failed to parse WebSocket command");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ws_url_from_https() {
        let url = build_ws_url("https://trunker.example.com", &Uuid::nil(), "key123");
        assert!(url.starts_with("wss://"));
        assert!(url.contains("/api/feeder/"));
        assert!(url.contains("?token=key123"));
    }

    #[test]
    fn build_ws_url_strips_trailing_slash() {
        let url = build_ws_url("https://trunker.example.com/", &Uuid::nil(), "key");
        assert!(!url.contains("//api"));
    }

    #[test]
    fn handle_reconnect_command_sets_running_false() {
        let running = AtomicBool::new(true);
        handle_message(r#"{"type": "reconnect"}"#, &running);
        assert!(!running.load(Ordering::Relaxed));
    }

    #[test]
    fn handle_unknown_command_does_not_crash() {
        let running = AtomicBool::new(true);
        handle_message(r#"{"type": "unknown_thing"}"#, &running);
        assert!(running.load(Ordering::Relaxed));
    }

    #[test]
    fn handle_invalid_json_does_not_crash() {
        let running = AtomicBool::new(true);
        handle_message("not json at all", &running);
        assert!(running.load(Ordering::Relaxed));
    }
}
