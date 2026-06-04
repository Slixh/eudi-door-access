use crate::transport::Engagement;
use anyhow::{Context, Result};
use isomdl::definitions::device_request::Namespaces;
use isomdl::definitions::helpers::{NonEmptyMap, Tag24};
use isomdl::definitions::x509::trust_anchor::{
    PemTrustAnchor, TrustAnchorRegistry, TrustPurpose,
};
use isomdl::definitions::DeviceEngagement;
use isomdl::presentation::authentication::AuthenticationStatus;
use isomdl::presentation::reader::SessionManager;
use std::collections::BTreeMap;
use std::path::Path;

/// What we want to ask for from the holder. ISO 18013-5 mDL namespace.
const MDL_NAMESPACE: &str = "org.iso.18013.5.1";

/// Elements requested for door access. Set `true` for "intent to retain".
fn requested_elements() -> Namespaces {
    let elements = NonEmptyMap::new("age_over_21".to_string(), false)
        .tap_mut(|m| {
            m.insert("given_name".to_string(), false);
            m.insert("family_name".to_string(), false);
            m.insert("portrait".to_string(), false);
        });
    NonEmptyMap::new(MDL_NAMESPACE.to_string(), elements)
}

// Tiny helper because NonEmptyMap doesn't expose a builder pattern.
trait TapMut: Sized {
    fn tap_mut(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}
impl<T> TapMut for T {}

pub struct VerifiedResponse {
    /// Disclosed elements from the mDL namespace, as parsed by isomdl (JSON).
    pub elements: BTreeMap<String, serde_json::Value>,
    pub mso_valid: bool,
    pub device_auth_valid: bool,
}

/// Load issuer (IACA) certificates from disk and build a trust anchor registry
/// for MSO validation. Each `*.pem` file in `dir` is treated as an IACA root.
pub fn load_trust_anchors(dir: impl AsRef<Path>) -> Result<TrustAnchorRegistry> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        tracing::warn!(
            "trust anchor directory {} not found; MSO validation will fail",
            dir.display()
        );
        return Ok(TrustAnchorRegistry::default());
    }

    let mut certs = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading trust anchor directory {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pem") {
            continue;
        }
        let certificate_pem = std::fs::read_to_string(&path)
            .with_context(|| format!("reading certificate {}", path.display()))?;
        certs.push(PemTrustAnchor {
            certificate_pem,
            purpose: TrustPurpose::Iaca,
        });
    }

    TrustAnchorRegistry::from_pem_certificates(certs).context("building trust anchor registry")
}

pub async fn run_session(
    engagement: Engagement,
    trust_anchors: &TrustAnchorRegistry,
) -> Result<VerifiedResponse> {
    // 1. `establish_session` expects the QR-code URI form of the DeviceEngagement,
    //    so re-encode the raw CBOR we received over NFC into that form. It then
    //    performs ECDH + HKDF and returns the encrypted request bytes.
    let qr_code = Tag24::<DeviceEngagement>::from_bytes(engagement.bytes.clone())
        .context("parsing DeviceEngagement")?
        .to_qr_code_uri()
        .context("encoding DeviceEngagement as QR URI")?;

    let (mut session, request_bytes, _ble_ident) = SessionManager::establish_session(
        qr_code,
        requested_elements(),
        trust_anchors.clone(),
    )
    .context("establishing reader session")?;

    // 2. Open the actual transport channel chosen during engagement.
    let mut channel = crate::transport::open_channel(&engagement).await?;

    // 3. Send request, read response.
    channel.send(&request_bytes).await?;
    let response_bytes = channel.recv().await?;

    // 4. Decrypt + validate. `handle_response` decrypts with the session keys,
    //    verifies the issuer MSO and DeviceAuth COSE signatures against the
    //    trust anchors, and returns the outcome directly (not a `Result`):
    //    any failure is recorded in `errors` and the authentication statuses.
    let validated = session.handle_response(&response_bytes);

    // 5. Collect the disclosed elements from the mDL namespace.
    let mut out = BTreeMap::new();
    if let Some(serde_json::Value::Object(ns)) = validated.response.get(MDL_NAMESPACE) {
        for (k, v) in ns {
            out.insert(k.clone(), v.clone());
        }
    }

    Ok(VerifiedResponse {
        elements: out,
        mso_valid: validated.issuer_authentication == AuthenticationStatus::Valid,
        device_auth_valid: validated.device_authentication == AuthenticationStatus::Valid,
    })
}
