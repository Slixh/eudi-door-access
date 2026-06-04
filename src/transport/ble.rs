use super::{Channel, Engagement};
use anyhow::{anyhow, Result};
use bluer::{gatt::remote::Characteristic, Session};
use tokio::sync::mpsc;

pub async fn connect(eng: &Engagement) -> Result<BleChannel> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    tracing::info!("Scanning BLE for service {}", eng.ble_service_uuid);

    // TODO:
    //  1. Start discovery, filter by service UUID == eng.ble_service_uuid.
    //  2. Connect to the first matching device.
    //  3. Resolve the mdoc GATT service and its characteristics:
    //       Client2Server  (Write)
    //       Server2Client  (Notify)
    //       State          (Write/Notify)
    //       Ident          (Read)
    //  4. Subscribe to Server2Client notifications and pipe them into mpsc.
    todo!("BLE GATT discovery against eng.ble_service_uuid")
}

pub struct BleChannel {
    pub client2server: Characteristic,
    pub server2client: Characteristic,
    pub notif_rx: mpsc::Receiver<Vec<u8>>,
}

#[async_trait::async_trait]
impl Channel for BleChannel {
    async fn send(&mut self, frame: &[u8]) -> Result<()> {
        // 18013-5 §8.3.3.1.1.4 fragmentation: first byte 0x00 = last chunk,
        // 0x01 = more follow. Use MTU-1 as chunk size.
        let chunk_size = 20;
        for (i, ch) in frame.chunks(chunk_size).enumerate() {
            let last = (i + 1) * chunk_size >= frame.len();
            let mut buf = Vec::with_capacity(ch.len() + 1);
            buf.push(if last { 0x00 } else { 0x01 });
            buf.extend_from_slice(ch);
            self.client2server.write(&buf).await?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(chunk) = self.notif_rx.recv().await {
            if chunk.is_empty() {
                return Err(anyhow!("empty BLE frame"));
            }
            out.extend_from_slice(&chunk[1..]);
            if chunk[0] == 0x00 {
                return Ok(out);
            }
        }
        Err(anyhow!("BLE channel closed"))
    }
}