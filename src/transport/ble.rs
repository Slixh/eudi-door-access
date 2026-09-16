#[cfg(target_os = "linux")]
use super::{Channel, Engagement};
#[cfg(target_os = "linux")]
use anyhow::{anyhow, Result};
#[cfg(target_os = "linux")]
use bluer::{gatt::remote::Characteristic, Session, AdapterEvent};
#[cfg(target_os = "linux")]
use futures::StreamExt;
#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(target_os = "linux")]
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use tokio::time::timeout;

#[cfg(not(target_os = "linux"))]
use super::{Channel, Engagement};
#[cfg(not(target_os = "linux"))]
use anyhow::{anyhow, Result};

#[cfg(not(target_os = "linux"))]
pub struct BleChannel;

#[cfg(not(target_os = "linux"))]
#[async_trait::async_trait]
impl Channel for BleChannel {
    async fn send(&mut self, _frame: &[u8]) -> Result<()> {
        Err(anyhow!("BLE is only supported on Linux"))
    }
    async fn recv(&mut self) -> Result<Vec<u8>> {
        Err(anyhow!("BLE is only supported on Linux"))
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn connect(_eng: &Engagement) -> Result<BleChannel> {
    Err(anyhow!("BLE is only supported on Linux"))
}

#[cfg(target_os = "linux")]
// ISO 18013-5 standardized Characteristic UUIDs for mdoc BLE transfer
const MDOC_STATE_UUID: &str = "00000001-a123-48ce-896b-4c76973373e6";
#[cfg(target_os = "linux")]
const MDOC_C2S_UUID: &str   = "00000002-a123-48ce-896b-4c76973373e6";
#[cfg(target_os = "linux")]
const MDOC_S2C_UUID: &str   = "00000003-a123-48ce-896b-4c76973373e6";

#[cfg(target_os = "linux")]
pub async fn connect(eng: &Engagement) -> Result<BleChannel> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    tracing::info!("Scanning BLE for service {}", eng.ble_service_uuid);

    // 1. Start discovery (we keep the stream alive to force the radio to actively scan)
    let _discover = adapter.discover_devices().await?;

    let find_device = async {
        loop {
            // Iterate over all devices BlueZ currently sees in the vicinity
            for addr in adapter.device_addresses().await? {
                let device = adapter.device(addr)?;

                // If BlueZ has resolved the BLE UUIDs for this device, check them
                if let Ok(Some(uuids)) = device.uuids().await {
                    if uuids.contains(&eng.ble_service_uuid) {
                        return Ok::<_, anyhow::Error>(device);
                    }
                }
            }

            // Wait 100ms before checking again so we don't hammer the D-Bus
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    let device = timeout(Duration::from_secs(10), find_device)
        .await
        .map_err(|_| anyhow!("Timed out scanning for the phone's BLE broadcast"))??;

    tracing::info!("Found device: {}. Connecting...", device.address());

    // 2. Connect to the device
    if !device.is_connected().await? {
        device.connect().await?;
    }

    // 3. Resolve the mdoc GATT service and its characteristics
    let mut c2s_char = None;
    let mut s2c_char = None;
    let mut state_char = None;

    let services = device.services().await?;
    for service in services {
        if service.uuid().await? == eng.ble_service_uuid {
            for charac in service.characteristics().await? {
                let char_uuid = charac.uuid().await?.to_string();
                match char_uuid.as_str() {
                    MDOC_STATE_UUID => state_char = Some(charac),
                    MDOC_C2S_UUID   => c2s_char = Some(charac),
                    MDOC_S2C_UUID   => s2c_char = Some(charac),
                    _ => {}
                }
            }
            break; // Found the target service, no need to check others
        }
    }

    let client2server = c2s_char.ok_or_else(|| anyhow!("Missing Client2Server char"))?;
    let server2client = s2c_char.ok_or_else(|| anyhow!("Missing Server2Client char"))?;
    let state = state_char.ok_or_else(|| anyhow!("Missing State char"))?;

    // 4. Subscribe to Server2Client notifications and pipe them into mpsc
    tracing::info!("Subscribing to Server2Client notifications...");

    // Notice we removed `mut` here, because `pin!` will handle mutability
    let notif_stream = server2client.notify().await?;
    let (tx, rx) = mpsc::channel(100);

    tokio::spawn(async move {
        // PIN IT HERE: This pins the stream to the stack of this async task
        tokio::pin!(notif_stream);

        while let Some(event) = notif_stream.next().await {
            // Send the raw BLE chunk to our Channel implementation
            if tx.send(event).await.is_err() {
                break; // The channel was dropped, kill the background task
            }
        }
    });

    // 5. CRITICAL: Inform the phone that the reader is ready
    // ISO 18013-5 § 8.3.3.1.1.3 mandates writing `0x01` to the State char.
    tracing::info!("Writing 0x01 to State characteristic (Ready)");
    state.write(&[0x01]).await?;

    Ok(BleChannel {
        client2server,
        server2client,
        notif_rx: rx,
    })
}

#[cfg(target_os = "linux")]
pub struct BleChannel {
    pub client2server: Characteristic,
    pub server2client: Characteristic,
    pub notif_rx: mpsc::Receiver<Vec<u8>>,
}

#[cfg(target_os = "linux")]
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