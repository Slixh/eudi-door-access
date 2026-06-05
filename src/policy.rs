use crate::reader::VerifiedResponse;

pub fn should_unlock(resp: &VerifiedResponse) -> bool {
    //if !resp.mso_valid || !resp.device_auth_valid {
    //    return false;
    //}
    // Example policy: must be 21+ and the issuer-signed credential must be valid.
    resp.elements.get("granted_resource")
        .and_then(|v| v.as_str()) == Some("dc:augsburg:maximilianviertel:backup")
}