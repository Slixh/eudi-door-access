mod policy;
mod reader;
mod transport;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,eudi_access=debug".into()),
        )
        .init();

    info!("Door reader starting…");

    // Trusted IACA roots (issuer CA certificates) — load from disk in real use.
    let trust_anchors = reader::load_trust_anchors("trust/iaca/")?;

    loop {
        // Wait for any transport to surface a DeviceEngagement
        let engagement = transport::wait_for_engagement().await?;
        info!("Got DeviceEngagement ({} bytes)", engagement.bytes.len());

        // Drive the ISO 18013-5 session via that transport
        match reader::run_session(engagement, &trust_anchors).await {
            Ok(response) => {
                if policy::should_unlock(&response) {
                    info!("ACCESS GRANTED");
                    //door.unlock_for_secs(3).await?;
                } else {
                    info!("ACCESS DENIED by policy");
                }
            }
            Err(e) => tracing::warn!("Session failed: {e:#}"),
        }
    }
}