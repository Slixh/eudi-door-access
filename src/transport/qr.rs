use anyhow::{Context, Result};
use isomdl::definitions::{
    device_engagement::{
        BleOptions, CentralClientMode, DeviceEngagement, DeviceRetrievalMethod, Security,
    },
    helpers::{NonEmptyVec, Tag24},
    session::create_p256_ephemeral_keys,
};
use qrcode::{render::unicode, QrCode};
use uuid::Uuid;

/// Generates an ISO 18013-5 Device Engagement URI (`mdoc:<base64url>`)
/// for mDL discovery over BLE.
pub fn generate_iso_mdl_engagement_uri(ble_service_uuid: Uuid) -> Result<String> {
    let key_pair = create_p256_ephemeral_keys()
        .map_err(|e| anyhow::anyhow!("Failed to create ephemeral P-256 keys: {:?}", e))?;
    let public_key = Tag24::new(key_pair.1)
        .context("Failed to construct Tag24 public key")?;

    let ble_option = BleOptions {
        peripheral_server_mode: None,
        central_client_mode: Some(CentralClientMode { uuid: ble_service_uuid }),
    };

    let device_retrieval_methods =
        Some(NonEmptyVec::new(DeviceRetrievalMethod::BLE(ble_option)));

    let device_engagement = DeviceEngagement {
        version: "1.0".into(),
        security: Security(1, public_key),
        device_retrieval_methods,
        server_retrieval_methods: None,
        protocol_info: None,
    };

    let de_tag24 = Tag24::new(device_engagement)
        .context("Failed to wrap DeviceEngagement in Tag24")?;
    let qr_uri = de_tag24
        .to_qr_code_uri()
        .map_err(|e| anyhow::anyhow!("Failed to convert to QR code URI: {:?}", e))?;

    Ok(qr_uri)
}

/// Renders a string into a high-density ANSI/Unicode QR code suitable for printing directly to the terminal.
pub fn render_terminal_qr(content: &str) -> Result<String> {
    let code = QrCode::new(content.as_bytes()).context("Failed to generate QR code")?;
    let image = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Ok(image)
}

/// Renders a string into an SVG QR code suitable for direct inline DOM embedding.
pub fn render_svg_qr(content: &str) -> Result<String> {
    let code = QrCode::new(content.as_bytes()).context("Failed to generate QR code")?;
    let raw_svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#0a0e17"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();

    let clean_svg = if let Some(idx) = raw_svg.find("<svg") {
        &raw_svg[idx..]
    } else {
        &raw_svg
    };

    Ok(clean_svg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_iso_mdl_engagement_uri() {
        let test_uuid = Uuid::new_v4();
        let uri = generate_iso_mdl_engagement_uri(test_uuid).expect("should generate mdoc QR URI");
        assert!(uri.starts_with("mdoc:"), "ISO 18013-5 discovery URI must start with mdoc:");

        // Verify it can be decoded back by isomdl
        let parsed = Tag24::<DeviceEngagement>::from_qr_code_uri(&uri)
            .expect("should parse back from QR URI");
        assert_eq!(parsed.as_ref().version, "1.0");

        let rendered = render_terminal_qr(&uri).expect("rendering QR code should succeed");
        assert!(!rendered.is_empty());

        let svg = render_svg_qr(&uri).expect("rendering SVG QR should succeed");
        assert!(svg.starts_with("<svg"), "SVG QR must start with <svg tag");
        assert!(svg.contains("</svg>"), "SVG QR must contain </svg> closing tag");
    }
}
