use super::Engagement;
use anyhow::{anyhow, Context, Result};
use pcsc::*;
use uuid::Uuid;

/// Standard NFC Forum NDEF Application AID
const NDEF_AID: [u8; 7] = [0xD2, 0x76, 0x00, 0x00, 0x85, 0x01, 0x01];

/// Poll a PC/SC NFC reader (e.g. ACR122U), SELECT the NDEF applet, then read
/// the NDEF handover that carries the `DeviceEngagement` and the BLE UUID.
pub async fn poll_for_engagement() -> Result<Engagement> {
    // The pcsc crate is sync; do the blocking work on a worker thread so we
    // don't stall the tokio runtime.
    tokio::task::spawn_blocking(|| poll_blocking()).await?
}

fn poll_blocking() -> Result<Engagement> {
    let ctx = pcsc::Context::establish(Scope::User).context("pcsc context")?;
    let mut readers_buf = [0u8; 2048];
    let mut readers = ctx.list_readers(&mut readers_buf)?;
    let reader = readers
        .next()
        .ok_or_else(|| anyhow!("no PC/SC NFC reader connected"))?;

    tracing::info!("Using NFC reader: {:?}", reader);

    // Block until a card/phone is present.
    let card = loop {
        match ctx.connect(reader, ShareMode::Shared, Protocols::ANY) {
            Ok(card) => break card,
            Err(Error::NoSmartcard) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => return Err(e.into()),
        }
    };

    // 1. SELECT standard NDEF Application AID
    let mut select = vec![0x00, 0xA4, 0x04, 0x00, NDEF_AID.len() as u8];
    select.extend_from_slice(&NDEF_AID);
    select.push(0x00);

    let mut resp = [0u8; 4096];
    let resp = card.transmit(&select, &mut resp)?;
    ensure_sw_ok(resp)?;

    // 2. The Android EUDI wallet is now ready for us to read the NDEF file.
    let ndef = read_ndef_file(&card)?;

    // 3. Parse NDEF, extract the DeviceEngagement bytes and the BLE service UUID.
    let (engagement_bytes, ble_uuid) = parse_handover_ndef(&ndef)?;

    Ok(Engagement {
        bytes: engagement_bytes,
        ble_service_uuid: ble_uuid,
        ndef_bytes: ndef, // Keep the raw bytes for the isomdl SessionTranscript hash
    })
}

fn ensure_sw_ok(resp: &[u8]) -> Result<()> {
    if resp.len() < 2 {
        return Err(anyhow!("short APDU response"));
    }
    let sw = &resp[resp.len() - 2..];
    if sw != [0x90, 0x00] {
        return Err(anyhow!("APDU error SW={:02X}{:02X}", sw[0], sw[1]));
    }
    Ok(())
}

fn read_ndef_file(card: &Card) -> Result<Vec<u8>> {
    // 1. Select Capability Container (CC) file first (0xE103)
    // Many Android HCE implementations strictly require this before E104.
    tracing::info!("TX: SELECT CC (E103)");
    let select_cc = [0x00, 0xA4, 0x00, 0x0C, 0x02, 0xE1, 0x03];
    let mut buf = [0u8; 256];
    let resp = card.transmit(&select_cc, &mut buf)?;
    tracing::info!("RX: {:02X?}", resp);

    // 2. SELECT EF NDEF (file id 0xE104)
    tracing::info!("TX: SELECT NDEF (E104)");
    let select_ef = [0x00, 0xA4, 0x00, 0x0C, 0x02, 0xE1, 0x04];
    let mut buf = [0u8; 256];
    let resp = card.transmit(&select_ef, &mut buf)?;
    tracing::info!("RX: {:02X?}", resp);
    ensure_sw_ok(resp)?;

    // 3. Read NDEF length (First 2 bytes of the file)
    tracing::info!("TX: READ BINARY (Length)");
    let read_len = [0x00, 0xB0, 0x00, 0x00, 0x02];
    let mut buf = [0u8; 16];
    let resp = card.transmit(&read_len, &mut buf)?;
    tracing::info!("RX: {:02X?}", resp);
    ensure_sw_ok(resp)?;
    let nlen = u16::from_be_bytes([resp[0], resp[1]]) as usize;

    // 4. Read NDEF payload in chunks
    let mut data = Vec::with_capacity(nlen);
    let mut offset: u16 = 2;
    while data.len() < nlen {
        let remaining = nlen - data.len();
        let chunk = remaining.min(0xFF) as u8;
        tracing::info!("TX: READ BINARY (Offset: {}, Chunk: {})", offset, chunk);

        let apdu = [0x00, 0xB0, (offset >> 8) as u8, offset as u8, chunk];
        let mut buf = [0u8; 260];
        let resp = card.transmit(&apdu, &mut buf)?;
        tracing::info!("RX: {:02X?}", resp);

        ensure_sw_ok(resp)?;
        data.extend_from_slice(&resp[..resp.len() - 2]);
        offset += chunk as u16;
    }

    tracing::info!("Successfully read {} NDEF bytes", data.len());
    Ok(data)
}

/// Pull the mdoc `DeviceEngagement` bytes and BLE service UUID out of the
/// NDEF Handover Select message.
fn parse_handover_ndef(ndef: &[u8]) -> Result<(Vec<u8>, Uuid)> {
    let mut engagement_bytes = None;
    let mut ble_uuid = None;

    let mut offset = 0;
    while offset < ndef.len() {
        // 1. Parse NDEF Header
        let header = ndef[offset];
        let sr = (header & 0x10) != 0; // Short Record
        let il = (header & 0x08) != 0; // ID Length Present
        let tnf = header & 0x07;       // Type Name Format
        offset += 1;

        if offset >= ndef.len() { break; }
        let type_len = ndef[offset] as usize;
        offset += 1;

        // 2. Parse Payload Length
        let payload_len: usize;
        if sr {
            if offset >= ndef.len() { break; }
            payload_len = ndef[offset] as usize;
            offset += 1;
        } else {
            if offset + 3 >= ndef.len() { break; }
            payload_len = u32::from_be_bytes([
                ndef[offset], ndef[offset+1], ndef[offset+2], ndef[offset+3]
            ]) as usize;
            offset += 4;
        }

        // 3. Parse ID Length (if present)
        let id_len = if il {
            if offset >= ndef.len() { break; }
            let l = ndef[offset] as usize;
            offset += 1;
            l
        } else { 0 };

        // Ensure we don't read out of bounds
        if offset + type_len + id_len + payload_len > ndef.len() {
            return Err(anyhow!("NDEF record length out of bounds"));
        }

        // 4. Extract Record Fields
        let record_type = &ndef[offset .. offset + type_len];
        offset += type_len;

        let _id = &ndef[offset .. offset + id_len];
        offset += id_len;

        let payload = &ndef[offset .. offset + payload_len];
        offset += payload_len;

        // --- MATCH RECORDS ---

        // Match: Device Engagement (External Type)
        if tnf == 0x04 && record_type == b"iso.org:18013:deviceengagement" {
            if !payload.is_empty() {
                // Check if the payload starts with the ISO 1-byte version prefix (usually 0x01)
                // If it starts with 0xA_ (CBOR Map), the wallet skipped the version byte.
                if payload[0] == 0x01 && payload.len() > 1 {
                    engagement_bytes = Some(payload[1..].to_vec());
                } else {
                    // It's already raw CBOR, take the whole thing
                    engagement_bytes = Some(payload.to_vec());
                }
            }
        }

        // Match: Bluetooth LE OOB (MIME Type)
        if tnf == 0x02 && record_type == b"application/vnd.bluetooth.le.oob" {
            ble_uuid = extract_ble_uuid(payload);
        }
    }

    let de = engagement_bytes.ok_or_else(|| anyhow!("Device Engagement record not found in NDEF"))?;
    let uuid = ble_uuid.ok_or_else(|| anyhow!("BLE OOB record not found in NDEF"))?;

    Ok((de, uuid))
}

/// Parses BLE Advertising Data (AD) to find a 128-bit Service UUID
fn extract_ble_uuid(payload: &[u8]) -> Option<Uuid> {
    let mut offset = 0;

    // Payload is a list of [Length][Type][Data] structures
    while offset < payload.len() {
        let len = payload[offset] as usize;

        // Break if zero-length or malformed
        if len == 0 || offset + 1 + len > payload.len() {
            break;
        }

        let ad_type = payload[offset + 1];
        let ad_data = &payload[offset + 2 .. offset + 1 + len];

        // 0x06 = Incomplete List of 128-bit UUIDs
        // 0x07 = Complete List of 128-bit UUIDs
        if (ad_type == 0x06 || ad_type == 0x07) && ad_data.len() >= 16 {
            let mut uuid_bytes = [0u8; 16];
            uuid_bytes.copy_from_slice(&ad_data[0..16]);

            // BLE transmits UUIDs in Little-Endian. We must reverse them
            // to standard Big-Endian for the Uuid crate.
            uuid_bytes.reverse();

            return Some(Uuid::from_bytes(uuid_bytes));
        }

        // Move to the next AD structure
        offset += 1 + len;
    }

    None
}