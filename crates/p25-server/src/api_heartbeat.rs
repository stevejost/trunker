//! Periodic heartbeat to the trunker-web API.
//!
//! Sends `POST /api/feeder/{id}/heartbeat` every 30 seconds to keep
//! the feeder marked as ONLINE. Failures are logged but never crash
//! the server.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// Interval between heartbeat POSTs.
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Request timeout for heartbeat POST.
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Build the heartbeat URL from API base URL and feeder ID.
fn build_heartbeat_url(api_url: &str, feeder_id: &Uuid) -> String {
    format!(
        "{}/api/feeder/{}/heartbeat",
        api_url.trim_end_matches('/'),
        feeder_id
    )
}

/// Run the API heartbeat loop until `running` is set to false.
pub async fn run(api_url: String, feeder_id: Uuid, api_key: String, running: Arc<AtomicBool>) {
    let url = build_heartbeat_url(&api_url, &feeder_id);
    let client = reqwest::Client::new();

    loop {
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;

        if !running.load(Ordering::Relaxed) {
            break;
        }

        match client
            .post(&url)
            .bearer_auth(&api_key)
            .timeout(HEARTBEAT_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("API heartbeat sent");
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "API heartbeat failed");
            }
            Err(e) => {
                tracing::warn!(error = %e, "API heartbeat failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_interval_is_30_seconds() {
        assert_eq!(HEARTBEAT_INTERVAL.as_secs(), 30);
    }

    #[test]
    fn heartbeat_timeout_is_reasonable() {
        assert!(HEARTBEAT_TIMEOUT < HEARTBEAT_INTERVAL);
    }

    #[test]
    fn build_heartbeat_url_from_base() {
        let url = build_heartbeat_url("https://trunker.example.com", &Uuid::nil());
        assert_eq!(
            url,
            "https://trunker.example.com/api/feeder/00000000-0000-0000-0000-000000000000/heartbeat"
        );
    }

    #[test]
    fn build_heartbeat_url_strips_trailing_slash() {
        let url = build_heartbeat_url("https://trunker.example.com/", &Uuid::nil());
        assert!(!url.contains("//api"));
        assert!(url.contains("/api/feeder/"));
    }
}
