//! Minimal LiveKit connection test with publisher grants.

use anyhow::Result;
use livekit::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn")
        .with_writer(std::io::stderr)
        .init();

    let url = std::env::var("LIVEKIT_URL").unwrap_or_else(|_| "ws://127.0.0.1:7880".into());
    let api_key = std::env::var("LIVEKIT_API_KEY").unwrap_or_else(|_| "devkey".into());
    let api_secret =
        std::env::var("LIVEKIT_API_SECRET").unwrap_or_else(|_| "secret_that_you_should_change".into());

    let token = livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
        .with_identity("rust-test")
        .with_grants(livekit_api::access_token::VideoGrants {
            room_join: true,
            room: "p25".to_string(),
            can_publish: true,
            can_subscribe: false,
            ..Default::default()
        })
        .to_jwt()?;

    eprintln!(">>> ATTEMPTING with single_peer_connection=true ...");

    let mut opts = RoomOptions::default();
    opts.auto_subscribe = false;
    opts.single_peer_connection = true;

    match Room::connect(&url, &token, opts).await {
        Ok((room, _rx)) => {
            eprintln!(">>> SUCCESS! Connected to room: {}", room.name());
            eprintln!(">>> Press Ctrl+C to exit");
            tokio::signal::ctrl_c().await?;
        }
        Err(e) => {
            eprintln!(">>> FAILED: {e}");
        }
    }

    Ok(())
}
