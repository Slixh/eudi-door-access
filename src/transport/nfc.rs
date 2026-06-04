use super::Engagement;
use anyhow::{anyhow, Context, Result};
use pcsc::*;
use uuid::Uuid;

/// AID for ISO 18013-5 mdoc reader: A0 00 00 02 48 04 00
const MDOC_AID: [u8; 7] = [0xA0, 0x00, 0x00, 0x02, 0x48, 0x04, 0x00];

/// Poll a PC/SC NFC reader (e.g. ACR122U), SELECT the mdoc applet, then read
/// the NDEF handover that carries the `DeviceEngagement` and the BLE UUID.
pub async fn poll_for_engagement() -> Result<Engagement> {
    // The pcsc crate is sync; do the blocking work on a worker thread so we
    // don't stall the tokio runtime.
    tokio::task::spawn_blocking(|| poll_blocking()).await?
}

fn poll_blocking() -> Result<Engagement> {
    let ctx = Context::establish(Scope::User).context("pcsc context")?;
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

    // 1. SELECT mdoc AID.
    let mut select = vec![0x00, 0xA4, 0x04, 0x00, MDOC_AID.len() as u8];
    select.extend_from_slice(&MDOC_AID);
    select.push(0x00);
    let mut resp = [0u8; 4096];
    let resp = card.transmit(&select, &mut resp)?;
    ensure_sw_ok(resp)?;

    // 2. The mdoc applet exposes the engagement either through an NDEF file
    //    (static handover) or via a few APDU exchanges (negotiated handover).
    //    For brevity we only implement the simpler static handover path here:
    //    SELECT NDEF EF (0xE104), READ BINARY until done.
    let ndef = read_ndef_file(&card)?;

    // 3. Parse NDEF, find the Handover Select / mdoc record, and extract
    //    `DeviceEngagement` plus the BLE service UUID.
    let (engagement_bytes, ble_uuid) = parse_handover_ndef(&ndef)?;

    Ok(Engagement {
        bytes: engagement_bytes,
        ble_service_uuid: ble_uuid,
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
    // SELECT EF NDEF (file id 0xE104)
    let select_ef = [0x00, 0xA4, 0x00, 0x0C, 0x02, 0xE1, 0x04];
    let mut buf = [0u8; 256];
    let resp = card.transmit(&select_ef, &mut buf)?;
    ensure_sw_ok(resp)?;

    // First 2 bytes of the NDEF file are the NDEF length.
    let read_len = [0x00, 0xB0, 0x00, 0x00, 0x02];
    let mut buf = [0u8; 16];
    let resp = card.transmit(&read_len, &mut buf)?;
    ensure_sw_ok(resp)?;
    let nlen = u16::from_be_bytes([resp[0], resp[1]]) as usize;

    // Read NDEF payload in chunks.
    let mut data = Vec::with_capacity(nlen);
    let mut offset: u16 = 2;
    while data.len() < nlen {
        let remaining = nlen - data.len();
        let chunk = remaining.min(0xFF) as u8;
        let apdu = [0x00, 0xB0, (offset >> 8) as u8, offset as u8, chunk];
        let mut buf = [0u8; 260];
        let resp = card.transmit(&apdu, &mut buf)?;
        ensure_sw_ok(resp)?;
        data.extend_from_slice(&resp[..resp.len() - 2]);
        offset += chunk as u16;
    }
    Ok(data)
}

/// Pull the mdoc `DeviceEngagement` bytes and BLE service UUID out of the
/// NDEF Handover Select message. This is a placeholder — the real layout is
/// defined in ISO 18013-5 §8.2.2.2 (Handover Select / Alternative Carrier
/// records, plus a Bluetooth LE OOB record with the service UUID).
fn parse_handover_ndef(_ndef: &[u8]) -> Result<(Vec<u8>, Uuid)> {
    // TODO: walk NDEF records:
    //   1. Locate the record whose type is "iso.org:18013:deviceengagement"
    //      → its payload is the DeviceEngagement CBOR.
    //   2. Locate the Bluetooth LE OOB record (RTD "application/vnd.bluetooth.le.oob")
    //      → contains the 128-bit service UUID.
    Err(anyhow!("NDEF handover parser not implemented yet"))
}