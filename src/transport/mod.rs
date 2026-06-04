pub mod ble;
pub mod nfc;

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

/// A DeviceEngagement received over NFC, with the BLE service UUID extracted
/// from its retrieval methods.
pub struct Engagement {
    /// Raw `DeviceEngagement` CBOR bytes (to feed into isomdl).
    pub bytes: Vec<u8>,
    /// BLE service UUID announced inside the engagement; we use it to find
    /// the phone over GATT.
    pub ble_service_uuid: Uuid,
}

#[async_trait]
pub trait Channel: Send {
    async fn send(&mut self, frame: &[u8]) -> Result<()>;
    async fn recv(&mut self) -> Result<Vec<u8>>;
}

/// One-shot: wait for an NFC tap, read DeviceEngagement, extract BLE UUID.
pub async fn wait_for_engagement() -> Result<Engagement> {
    nfc::poll_for_engagement().await
}

/// Open the BLE channel announced inside the engagement.
pub async fn open_channel(eng: &Engagement) -> Result<Box<dyn Channel>> {
    Ok(Box::new(ble::connect(eng).await?))
}