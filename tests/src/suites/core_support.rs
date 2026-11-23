//! Helpers and fixtures shared by the core suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use dar_acceptance::prelude::*;

/// SCIM schema URNs (`support/identity.ts`).
pub const USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub const GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
pub const ENTERPRISE: &str = "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";
pub const PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/// The provider stores each record under `<Model>/<SHA-256 of its secret or ID>`.
pub fn model_key(model: &str, id: &str) -> String {
    format!("{model}/{}", dar_acceptance::sha256_hex(id))
}

/// A SCIM request with a bearer credential (`application/scim+json`).
pub async fn scim(base: &str, secret: &str, path: &str, method: &str, payload: Option<&Value>, headers: &[(&str, &str)]) -> Result<Reply> {
    // Given headers replace the defaults (the credential, for one).
    let mut map = reqwest::header::HeaderMap::new();
    map.insert("authorization", format!("Bearer {secret}").parse().unwrap());
    map.insert("content-type", "application/scim+json".parse().unwrap());
    for (name, value) in headers {
        map.insert(session::header(name), value.parse().map_err(|_| Error::new("header value"))?);
    }
    let mut request = session::client().request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{base}/scim/v2{path}")).headers(map);
    if let Some(payload) = payload {
        request = request.body(payload.to_string());
    }
    Reply::from(request.send().await?).await
}

/// An `application/x-www-form-urlencoded` body.
pub fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}
