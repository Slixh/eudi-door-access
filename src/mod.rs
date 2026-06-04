pub mod ble;
pub mod nfc;

use anyhow::Result;
use async_trait::async_trait;

/// One DeviceEngagement (the CBOR blob from QR/NFC handover/BLE).
pub struct Engagement {
    pub bytes: Vec<u8>,
    pub transport: TransportKind,
    // Parsed retrieval methods (BLE UUIDs, NFC parameters, etc.)
    pub ble_service_uuid: Option<uuid::Uuid>,
}

#[derive(Clone, Copy)]
pub enum TransportKind {
    Ble,
    Nfc,
}

#[async_trait::async_trait]
pub trait Channel: Send {
    async fn send(&mut self, frame: &[u8]) -> Result<()>;
    async fn recv(&mut self) -> Result<Vec<u8>>;
}

impl TransportKind {
    pub async fn connect(&self, eng: &Engagement) -> Result<Box<dyn Channel>> {
        match self {
            TransportKind::Ble => Ok(Box::new(ble::connect(eng).await?)),
            TransportKind::Nfc => Ok(Box::new(nfc::connect(eng).await?)),
        }
    }
}

/// Race BLE scanning + NFC polling and return whichever produces engagement first.
pub async fn wait_for_engagement() -> Result<Engagement> {
    tokio::select! {
        e = ble::scan_for_engagement() => e,
        e = nfc::poll_for_engagement()  => e,
    }
}