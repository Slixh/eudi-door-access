use crate::reader::VerifiedResponse;

pub fn should_unlock(resp: &VerifiedResponse) -> bool {
    if !resp.mso_valid || !resp.device_auth_valid {
        return false;
    }
    // Example policy: must be 21+ and the issuer-signed credential must be valid.
    matches!(
        resp.elements.get("age_over_21"),
        Some(serde_json::Value::Bool(true))
    )
}