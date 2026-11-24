//! Identity administration (`runtime/identity/http.ts`, `directory.ts`) under
//! `/api/v1/identity`, and SCIM 2.0 provisioning (`scim.ts`) under `/scim/v2`. The module checks
//! every actor: an administrator's browser session (writes need a fresh password proof) or a
//! SCIM bearer credential.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::error::{AppError, Result, scim_error};
use crate::http::Api;
use crate::scim_model::{self as scim, ENTERPRISE, GROUP, Kind, LIST, USER};

const READS: [&str; 7] = [
    "users", "user", "groups", "group", "clients", "tokens", "storage",
];

/// The directory as one actor sees it (`directoryService`).
pub struct Directory<'a> {
    runtime: &'a crate::http::Runtime,
    session_id: String,
    bearer: String,
    proof: String,
}

/// `checkMatch`: writes name the revision they read; `*` alone is not enough where one is required.
pub fn check_match(row: Option<&Value>, matched: Option<&str>, required: bool) -> Result<()> {
    if required && matched.is_none_or(|m| m.split(',').any(|v| v.trim() == "*")) {
        return Err(AppError::with(
            "preconditionRequired",
            428,
            "Read the current revision before changing this resource.",
        ));
    }
    if let Some(m) = matched.filter(|m| *m != "*") {
        let ok = row.is_some_and(|r| {
            m.split(',')
                .map(str::trim)
                .any(|v| v == format!("W/\"{}\"", r["revision"]))
        });
        if !ok {
            return Err(scim_error(
                412,
                "The resource changed. Read it and retry.",
                None,
            ));
        }
    }
    Ok(())
}

impl<'a> Directory<'a> {
    pub async fn call(&self, operation: &str, p: Value) -> Result<Value> {
        self.runtime
            .db
            .operation(
                "service_identity",
                operation,
                &READS,
                json!([
                    self.session_id,
                    self.bearer,
                    self.proof,
                    operation,
                    p.to_string()
                ]),
            )
            .await
    }

    pub async fn get(&self, kind: Kind, id: &str) -> Result<Value> {
        self.call(
            if kind == Kind::Users { "user" } else { "group" },
            json!({ "id": id }),
        )
        .await
    }

    pub async fn list(&self, kind: Kind) -> Result<Vec<Value>> {
        Ok(self
            .call(
                if kind == Kind::Users {
                    "users"
                } else {
                    "groups"
                },
                json!({}),
            )
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    pub async fn save(
        &self,
        kind: Kind,
        id: Option<&str>,
        input: &Value,
        matched: Option<&str>,
        patch: bool,
        required: bool,
    ) -> Result<Value> {
        self.call("authorizeWrite", json!({})).await?;
        let old = match id {
            Some(id) => Some(self.get(kind, id).await?),
            None => None,
        };
        check_match(old.as_ref(), matched, required && id.is_some())?;
        let source = if patch {
            scim::apply_patch(
                kind,
                &scim::resource(kind, old.as_ref().unwrap_or(&Value::Null), ""),
                input,
            )?
        } else {
            input.clone()
        };
        let (document, password) = scim::normalize(kind, &source)?;
        let verifier = match password {
            Some(p) => Some(self.runtime.provision_password(&p).await?),
            None => None,
        };
        let mut p = json!({ "id": id.map(str::to_owned).unwrap_or_else(crate::auth::uuid), "revision": old.as_ref().map_or(json!(0), |o| o["revision"].clone()), "document": Value::Object(document).to_string() });
        if let Some(v) = verifier {
            p["verifier"] = json!(v);
        }
        match self
            .call(
                if kind == Kind::Users {
                    "saveUser"
                } else {
                    "saveGroup"
                },
                p,
            )
            .await
        {
            Err(e) if e.code == "conflict" => Err(match matched {
                Some(_) => scim_error(412, "The resource changed. Read it and retry.", None),
                None => scim_error(
                    409,
                    "The resource changed or userName is already reserved.",
                    Some("uniqueness"),
                ),
            }),
            other => other,
        }
    }

    pub async fn remove(
        &self,
        kind: Kind,
        id: &str,
        matched: Option<&str>,
        required: bool,
    ) -> Result<()> {
        self.call("authorizeWrite", json!({})).await?;
        let old = self.get(kind, id).await?;
        check_match(Some(&old), matched, required)?;
        match self
            .call(
                if kind == Kind::Users {
                    "deleteUser"
                } else {
                    "deleteGroup"
                },
                json!({ "id": id, "revision": old["revision"] }),
            )
            .await
        {
            Err(e) if e.code == "conflict" => Err(scim_error(
                412,
                "The resource changed. Read it and retry.",
                None,
            )),
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    }
}

fn send(value: &Value, status: StatusCode) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        value.to_string(),
    )
        .into_response()
}

fn with_etag(mut response: Response, etag: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, v);
    }
    response
}

fn iso(ms: &Value) -> String {
    chrono::DateTime::from_timestamp_millis(ms.as_f64().unwrap_or(0.0) as i64)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A registered client as administration shows it (`clientDocument`).
fn client_document(row: &Value) -> Value {
    let p: Value =
        serde_json::from_str(row["metadata"].as_str().unwrap_or("{}")).unwrap_or_default();
    let public = p["token_endpoint_auth_method"] == "none";
    json!({ "id": row["id"], "revision": row["revision"], "enabled": row["enabled"], "modifiedAt": iso(&row["modifiedAt"]), "name": p["client_name"], "type": if public { "public" } else { "confidential" },
        "redirectUris": p["redirect_uris"], "postLogoutRedirectUris": p["post_logout_redirect_uris"], "scopes": p["scope"], "requirePkce": public || p["require_pkce"] != json!(false) })
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn text(value: Option<&Value>, max: usize) -> Result<String> {
    match value.and_then(Value::as_str) {
        Some(s) if s.len() <= max => Ok(s.to_owned()),
        _ => Err(AppError::new("invalidInput", 400)),
    }
}

fn secret() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::auth::random_bytes::<32>())
}

/// `/api/v1/identity/...` for a signed-in administrator (connected applications for everyone).
pub async fn handle(r: &Api<'_>, body: Body) -> Result<Response> {
    let runtime = r.runtime;
    let method = r.method.clone();
    let path = r.path.strip_prefix("/api/v1/identity").unwrap_or_default();
    let session_id = r.session_id().to_owned();
    let ok = |v: Value| Ok(send(&v, StatusCode::OK));
    if path == "/connections" && method == Method::GET {
        return ok(
            json!({ "items": runtime.db.read("service_connected_applications", json!([session_id, ""])).await?, "nextCursor": "" }),
        );
    }
    if let Some(id) = path.strip_prefix("/connections/")
        && method == Method::DELETE
    {
        let id = crate::git::decode_component(id).ok_or_else(AppError::invalid)?;
        return ok(
            json!({ "items": runtime.db.call("service_connected_applications", json!([session_id, id])).await?, "nextCursor": "" }),
        );
    }
    if !r.administrator() {
        return Err(AppError::new("forbidden", 403));
    }
    let proof = header(r.headers, "x-identity-proof")
        .map(crate::auth::digest_token)
        .unwrap_or_default();
    let directory = Directory {
        runtime,
        session_id: session_id.clone(),
        bearer: String::new(),
        proof,
    };
    let body_of =
        async |body: Body| -> Result<Value> { Ok(Value::Object(r.body(body, 256 * 1024).await?)) };
    let matched = header(r.headers, "if-match");
    let base = format!("{}/scim/v2", runtime.config.origin);
    match (method.clone(), path) {
        (Method::POST, "/confirmation") => {
            let key = format!("identity:{}", r.peer);
            runtime.attempt_up_to(&key, 20)?;
            let p = body_of(body).await?;
            let verifier = runtime
                .confirm(r.username(), &text(p.get("password"), 1024)?)
                .await?;
            runtime.succeeded(&key);
            let (token, digest) = crate::auth::opaque_token();
            runtime
                .db
                .call(
                    "service_identity_proof",
                    json!([session_id, digest, verifier]),
                )
                .await?;
            let expires = chrono::Utc::now().timestamp_millis() + 300_000;
            return ok(json!({ "proof": token, "expiresAt": iso(&json!(expires)) }));
        }
        (Method::GET, "/status") => {
            let origin = &runtime.config.origin;
            return ok(
                json!({ "issuer": format!("{origin}/oidc"), "scimUrl": format!("{origin}/scim/v2"), "keys": runtime.identity.status() }),
            );
        }
        (Method::POST, "/keys/rotate") => {
            directory.call("adminWrite", json!({})).await?;
            return ok(runtime.identity.rotate()?);
        }
        (Method::GET, "/storage") => {
            return ok(
                json!({ "items": directory.call("storage", json!({})).await?, "nextCursor": "" }),
            );
        }
        (Method::GET, "/clients") => {
            let items = directory.call("clients", json!({})).await?;
            return ok(
                json!({ "items": items.as_array().into_iter().flatten().map(client_document).collect::<Vec<_>>(), "nextCursor": "" }),
            );
        }
        (Method::GET, "/credentials") => {
            let items = directory.call("tokens", json!({})).await?;
            let items: Vec<Value> = items
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    let mut c = c.clone();
                    c["expiresAt"] = json!(iso(&c["expiresAt"]));
                    c
                })
                .collect();
            return ok(json!({ "items": items, "nextCursor": "" }));
        }
        (Method::POST, "/credentials") => {
            let p = body_of(body).await?;
            let label = text(p.get("label"), 100)?.trim().to_owned();
            let days = p
                .get("days")
                .and_then(Value::as_f64)
                .filter(|d| d.fract() == 0.0 && (1.0..=365.0).contains(d));
            let scope = text(p.get("scope"), 10)?;
            let (Some(days), false, true) = (
                days,
                label.is_empty(),
                ["read", "write"].contains(&scope.as_str()),
            ) else {
                return Err(AppError::new("invalidInput", 400));
            };
            let (token, digest) = crate::auth::opaque_token();
            let id = crate::auth::uuid();
            let expires = chrono::Utc::now().timestamp_millis() + days as i64 * 86_400_000;
            directory.call("createToken", json!({ "id": id, "digest": digest, "label": label, "scope": scope, "expiresAt": expires })).await?;
            return Ok(send(
                &json!({ "id": id, "secret": token, "expiresAt": iso(&json!(expires)) }),
                StatusCode::CREATED,
            ));
        }
        _ => {}
    }
    if let Some(id) = path
        .strip_prefix("/credentials/")
        .filter(|i| !i.is_empty() && !i.contains('/'))
        && method == Method::DELETE
    {
        directory.call("revokeToken", json!({ "id": id })).await?;
        return ok(json!({ "ok": true }));
    }
    if let Some(id) = path
        .strip_prefix("/users/")
        .and_then(|p| p.strip_suffix("/storage"))
        .filter(|i| !i.is_empty() && !i.contains('/'))
        && method == Method::PUT
    {
        let p = body_of(body).await?;
        let quota = match p.get("quota") {
            Some(Value::Null) => 0.0,
            Some(v) => v
                .as_f64()
                .filter(|q| q.fract() == 0.0 && *q >= 0.0 && *q <= 9_007_199_254_740_991.0)
                .ok_or_else(|| {
                    AppError::with(
                        "invalidInput",
                        400,
                        "Use a whole number of bytes, or null for unlimited.",
                    )
                })?,
            None => {
                return Err(AppError::with(
                    "invalidInput",
                    400,
                    "Use a whole number of bytes, or null for unlimited.",
                ));
            }
        };
        let id = crate::git::decode_component(id).ok_or_else(AppError::invalid)?;
        return ok(directory
            .call("setStorageQuota", json!({ "id": id, "quota": quota }))
            .await?);
    }
    for (prefix, kind) in [("/users", Kind::Users), ("/groups", Kind::Groups)] {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        let id = match rest.strip_prefix('/') {
            Some(i) if !i.is_empty() && !i.contains('/') => Some(i),
            None if rest.is_empty() => None,
            _ => continue,
        };
        match (method.clone(), id) {
            (Method::GET, Some(id)) => {
                let row = directory.get(kind, id).await?;
                return Ok(with_etag(
                    send(&scim::resource(kind, &row, &base), StatusCode::OK),
                    &scim::version(&row),
                ));
            }
            (Method::GET, None) => {
                let items: Vec<Value> = directory
                    .list(kind)
                    .await?
                    .iter()
                    .map(|v| scim::resource(kind, v, &base))
                    .collect();
                return ok(json!({ "items": items, "nextCursor": "" }));
            }
            (Method::POST, None) | (Method::PUT | Method::PATCH, Some(_)) => {
                let input = body_of(body).await?;
                let row = directory
                    .save(kind, id, &input, matched, method == Method::PATCH, true)
                    .await?;
                let status = if id.is_some() {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                };
                return Ok(with_etag(
                    send(&scim::resource(kind, &row, &base), status),
                    &scim::version(&row),
                ));
            }
            (Method::DELETE, Some(id)) => {
                directory.remove(kind, id, matched, true).await?;
                return ok(json!({ "ok": true }));
            }
            _ => {}
        }
    }
    let clients = path
        .strip_prefix("/clients/")
        .map(|rest| match rest.strip_suffix("/secret") {
            Some(id) => (id, true),
            None => (rest, false),
        });
    let identity = &runtime.identity;
    let client_row = async |id: &str| -> Result<Option<Value>> {
        let row = runtime
            .db
            .call(
                "service_provider",
                json!(["client", json!({ "id": id }).to_string()]),
            )
            .await?;
        Ok((!row.is_null()).then_some(row))
    };
    if (method == Method::POST && path == "/clients")
        || matches!(clients, Some((_, false))) && method == Method::PUT
    {
        let p = r.body(body, 256 * 1024).await?;
        let id = clients
            .map(|c| c.0.to_owned())
            .unwrap_or_else(crate::auth::uuid);
        let old = client_row(&id).await?;
        check_match(old.as_ref(), matched, clients.is_some())?;
        if clients.is_some() && old.is_none() {
            return Err(AppError::not_found());
        }
        if p.get("enabled").is_some_and(|e| !e.is_boolean()) {
            return Err(AppError::new("invalidInput", 400));
        }
        let previous = old.as_ref().and_then(|o| {
            identity
                .clients
                .decrypt(o["encrypted"].as_str().unwrap_or_default())
        });
        let secret = (p.get("type") == Some(&json!("confidential"))).then(|| {
            previous
                .as_ref()
                .and_then(|v| v["client_secret"].as_str().map(str::to_owned))
                .unwrap_or_else(secret)
        });
        let metadata =
            crate::oidc::client_metadata(&p, &id, identity.development, secret.as_deref())?;
        let mut public = metadata.clone();
        public
            .as_object_mut()
            .expect("metadata")
            .remove("client_secret");
        let revision = old
            .as_ref()
            .and_then(|o| o["revision"].as_u64())
            .unwrap_or(0);
        directory.call("saveClient", json!({ "id": id, "revision": revision, "metadata": public.to_string(), "encrypted": identity.clients.encrypt(&metadata), "enabled": p.get("enabled") != Some(&json!(false)) })).await?;
        identity.load_client(&id).await;
        let mut value = json!({ "id": id });
        let was_public = previous
            .as_ref()
            .is_some_and(|v| v["token_endpoint_auth_method"] == "none");
        if let Some(secret) = secret.filter(|_| old.is_none() || was_public) {
            value["secret"] = json!(secret);
        }
        let status = if old.is_some() {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        };
        return Ok(with_etag(
            send(&value, status),
            &format!("W/\"{}\"", revision + 1),
        ));
    }
    if let Some((id, is_secret)) = clients {
        if !is_secret && method == Method::GET {
            let row = client_row(id).await?.ok_or_else(AppError::not_found)?;
            return Ok(with_etag(
                send(&client_document(&row), StatusCode::OK),
                &format!("W/\"{}\"", row["revision"]),
            ));
        }
        if is_secret && method == Method::POST {
            let old = client_row(id).await?.ok_or_else(AppError::not_found)?;
            check_match(Some(&old), matched, true)?;
            let mut metadata = identity
                .clients
                .decrypt(old["encrypted"].as_str().unwrap_or_default())
                .ok_or_else(AppError::internal)?;
            if metadata["token_endpoint_auth_method"] == "none" {
                return Err(AppError::new("invalidClient", 400));
            }
            let fresh = secret();
            metadata["client_secret"] = json!(fresh);
            let mut payload = old.clone();
            payload["encrypted"] = json!(identity.clients.encrypt(&metadata));
            directory.call("saveClient", payload).await?;
            identity.load_client(id).await;
            let revision = old["revision"].as_u64().unwrap_or(0) + 1;
            return Ok(with_etag(
                send(&json!({ "id": old["id"], "secret": fresh }), StatusCode::OK),
                &format!("W/\"{revision}\""),
            ));
        }
        if !is_secret && method == Method::DELETE {
            let old = client_row(id).await?.ok_or_else(AppError::not_found)?;
            check_match(Some(&old), matched, true)?;
            directory
                .call(
                    "deleteClient",
                    json!({ "id": old["id"], "revision": old["revision"] }),
                )
                .await?;
            identity.load_client(id).await;
            return ok(json!({ "ok": true }));
        }
    }
    Err(AppError::not_found())
}

// ---- SCIM ------------------------------------------------------------------------------------

const ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";

fn scim_json(value: Option<&Value>, status: StatusCode) -> Response {
    let body = value.map(Value::to_string).unwrap_or_default();
    (
        status,
        [
            (header::CONTENT_TYPE, "application/scim+json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn scim_body(headers: &HeaderMap, body: Body) -> Result<Value> {
    let kind = header(headers, "content-type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ok = ["application/scim+json", "application/json"]
        .iter()
        .any(|t| {
            kind.strip_prefix(t)
                .is_some_and(|rest| rest.is_empty() || rest.trim_start().starts_with(';'))
        });
    if !ok {
        return Err(scim_error(415, "Use application/scim+json.", None));
    }
    let bytes = axum::body::to_bytes(body, 256 * 1024)
        .await
        .map_err(|_| scim_error(413, "The request body is too large.", None))?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) if v.is_object() => Ok(v),
        _ => Err(scim_error(400, "Invalid JSON.", Some("invalidSyntax"))),
    }
}

fn attribute(name: &str, kind: &str, extra: Value) -> Value {
    let mut a = json!({ "name": name, "type": kind, "multiValued": false, "description": name, "required": false, "caseExact": false, "mutability": "readWrite", "returned": "default", "uniqueness": "none" });
    if let Value::Object(extra) = extra {
        for (k, v) in extra {
            a[k] = v;
        }
    }
    a
}

/// The schemas SCIM clients discover (`scimSchemas`).
fn scim_schemas(base: &str) -> Vec<Value> {
    let common = vec![
        attribute(
            "id",
            "string",
            json!({ "mutability": "readOnly", "returned": "always", "caseExact": true, "uniqueness": "global" }),
        ),
        attribute("externalId", "string", json!({})),
    ];
    let s = |n: &str| attribute(n, "string", json!({}));
    let mut user = common.clone();
    user.extend([
        attribute("userName", "string", json!({ "required": true, "uniqueness": "server" })),
        s("displayName"),
        attribute("active", "boolean", json!({})),
        attribute("name", "complex", json!({ "subAttributes": (["formatted", "familyName", "givenName", "middleName", "honorificPrefix", "honorificSuffix"].map(s)) })),
        attribute("emails", "complex", json!({ "multiValued": true, "subAttributes": [s("value"), s("type"), attribute("primary", "boolean", json!({})), s("display")] })),
        attribute("password", "string", json!({ "mutability": "writeOnly", "returned": "never" })),
    ]);
    user.extend(
        [
            "title",
            "userType",
            "locale",
            "timezone",
            "preferredLanguage",
        ]
        .map(s),
    );
    user.push(attribute(
        "groups",
        "complex",
        json!({ "multiValued": true, "mutability": "readOnly", "subAttributes": [
            attribute("value", "string", json!({ "mutability": "readOnly", "caseExact": true })), attribute("$ref", "reference", json!({ "referenceTypes": ["Group"], "mutability": "readOnly" })),
            attribute("display", "string", json!({ "mutability": "readOnly" })), attribute("type", "string", json!({ "mutability": "readOnly" }))] }),
    ));
    let mut group = common;
    group.extend([
        attribute("displayName", "string", json!({ "required": true })),
        attribute(
            "members",
            "complex",
            json!({ "multiValued": true, "subAttributes": [attribute("value", "string", json!({ "required": true, "caseExact": true })), attribute("$ref", "reference", json!({ "referenceTypes": ["User"] })),
                attribute("display", "string", json!({ "mutability": "readOnly" })), attribute("type", "string", json!({ "canonicalValues": ["User"] }))] }),
        ),
    ]);
    let mut enterprise: Vec<Value> = [
        "employeeNumber",
        "costCenter",
        "organization",
        "division",
        "department",
    ]
    .map(s)
    .to_vec();
    enterprise.push(attribute(
        "manager",
        "complex",
        json!({ "subAttributes": [attribute("value", "string", json!({ "caseExact": true })), attribute("$ref", "reference", json!({ "referenceTypes": ["User"] })), attribute("displayName", "string", json!({ "mutability": "readOnly" }))] }),
    ));
    [(USER, "User", "Dar native account", user), (GROUP, "Group", "Directory groups do not grant administrator privileges", group), (ENTERPRISE, "EnterpriseUser", "Enterprise profile", enterprise)]
        .into_iter()
        .map(|(id, name, description, attributes)| {
            json!({ "id": id, "name": name, "description": description, "attributes": attributes, "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"],
                "meta": { "resourceType": "Schema", "location": format!("{base}/Schemas/{}", scim::encode(id)) } })
        })
        .collect()
}

/// `/scim/v2`: provisioning with a dedicated bearer credential (`handleScim`).
pub async fn handle_scim(
    runtime: &crate::http::Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    match scim(runtime, method, path, query, headers, body).await {
        Ok(response) => response,
        Err(error) => {
            let status =
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut value = json!({ "schemas": [ERROR], "status": error.status.to_string(), "detail": if error.status >= 500 && error.code != "scimError" { "The provisioning request could not be completed.".to_owned() } else { error.message.clone() } });
            if let Some(t) = &error.scim_type {
                value["scimType"] = json!(t);
            }
            let mut response = scim_json(Some(&value), status);
            if error.status == 401 {
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"Dar SCIM\""),
                );
            }
            if error.status == 413 {
                response
                    .headers_mut()
                    .insert(header::CONNECTION, HeaderValue::from_static("close"));
            }
            if error.status == 405 {
                let allow = if path.trim_end_matches('/').matches('/').count() > 3 {
                    "GET, PUT, PATCH, DELETE"
                } else {
                    "GET, POST"
                };
                response
                    .headers_mut()
                    .insert(header::ALLOW, HeaderValue::from_static(allow));
            }
            response
        }
    }
}

async fn scim(
    runtime: &crate::http::Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response> {
    let credential = header(headers, "authorization")
        .and_then(|a| {
            a.get(..7)
                .filter(|p| p.eq_ignore_ascii_case("bearer "))
                .map(|_| &a[7..])
        })
        .filter(|t| crate::auth::is_token(t))
        .ok_or(AppError::new("unauthenticated", 401))?;
    let directory = Directory {
        runtime,
        session_id: String::new(),
        bearer: crate::auth::digest_token(credential),
        proof: String::new(),
    };
    // Authenticate even discovery and empty lists; cookies and OIDC tokens are never provisioning authority.
    directory.call("authorize", json!({})).await?;
    let base = format!("{}/scim/v2", runtime.config.origin);
    let path = path
        .strip_prefix("/scim/v2")
        .unwrap_or_default()
        .trim_end_matches('/');
    let ok = |v: &Value| Ok(scim_json(Some(v), StatusCode::OK));
    if *method == Method::GET && path == "/ServiceProviderConfig" {
        return ok(
            &json!({ "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"], "patch": { "supported": true }, "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
            "filter": { "supported": true, "maxResults": 200 }, "changePassword": { "supported": true }, "sort": { "supported": false }, "etag": { "supported": true },
            "authenticationSchemes": [{ "type": "oauthbearertoken", "name": "Provisioning bearer credential", "description": "Dedicated expiring administrator-issued SCIM credential", "primary": true }],
            "meta": { "resourceType": "ServiceProviderConfig", "location": format!("{base}/ServiceProviderConfig") } }),
        );
    }
    for discovery in ["/Schemas", "/ResourceTypes"] {
        let Some(rest) = path.strip_prefix(discovery) else {
            continue;
        };
        if *method != Method::GET || !(rest.is_empty() || rest.starts_with('/')) {
            continue;
        }
        let items: Vec<Value> = if discovery == "/Schemas" {
            scim_schemas(&base)
        } else {
            ["User", "Group"]
                .iter()
                .map(|k| {
                    let mut v = json!({ "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"], "id": k, "name": k, "endpoint": format!("/{k}s"), "schema": if *k == "User" { USER } else { GROUP },
                        "meta": { "resourceType": "ResourceType", "location": format!("{base}/ResourceTypes/{k}") } });
                    if *k == "User" {
                        v["schemaExtensions"] = json!([{ "schema": ENTERPRISE, "required": false }]);
                    }
                    v
                })
                .collect()
        };
        if let Some(id) = rest.strip_prefix('/').filter(|i| !i.is_empty()) {
            let id = crate::git::decode_component(id).ok_or_else(AppError::not_found)?;
            let item = items
                .iter()
                .find(|v| v["id"] == json!(id))
                .ok_or_else(AppError::not_found)?;
            return ok(item);
        }
        return ok(
            &json!({ "schemas": [LIST], "totalResults": items.len(), "startIndex": 1, "itemsPerPage": items.len(), "Resources": items }),
        );
    }
    let (kind, rest) = if let Some(rest) = path.strip_prefix("/Users") {
        (Kind::Users, rest)
    } else if let Some(rest) = path.strip_prefix("/Groups") {
        (Kind::Groups, rest)
    } else {
        return Err(AppError::not_found());
    };
    let id = match rest.strip_prefix('/') {
        Some(i) if !i.is_empty() && !i.contains('/') => {
            Some(crate::git::decode_component(i).ok_or_else(AppError::not_found)?)
        }
        None if rest.is_empty() => None,
        _ => return Err(AppError::not_found()),
    };
    if id.as_ref().is_some_and(|i| i.encode_utf16().count() > 100) {
        return Err(AppError::not_found());
    }
    let include = query.get("attributes").map(String::as_str);
    let exclude = query.get("excludedAttributes").map(String::as_str);
    let project = |v: &Value| scim::project(v, include, exclude);
    match (method.clone(), id.as_deref()) {
        (Method::GET, None) => {
            if query.contains_key("sortBy") || query.contains_key("sortOrder") {
                return Err(scim_error(
                    400,
                    "Sorting is unsupported.",
                    Some("invalidValue"),
                ));
            }
            let parse = |name: &str, fallback: u64| -> Result<u64> {
                match query.get(name) {
                    None => Ok(fallback),
                    Some(v)
                        if !v.is_empty()
                            && v.len() <= 9
                            && v.bytes().all(|b| b.is_ascii_digit()) =>
                    {
                        Ok(v.parse().unwrap_or(0))
                    }
                    Some(_) => Err(scim_error(
                        400,
                        &format!("Invalid {name}."),
                        Some("invalidValue"),
                    )),
                }
            };
            let start = parse("startIndex", 1)?.max(1);
            let count = parse("count", 100)?.min(200);
            let predicate = match query.get("filter") {
                Some(f) => Some(scim::filter_predicate(f)?),
                None => None,
            };
            let mut all: Vec<Value> = directory
                .list(kind)
                .await?
                .iter()
                .map(|v| scim::resource(kind, v, &base))
                .filter(|v| predicate.as_ref().is_none_or(|p| p(v)))
                .collect();
            all.sort_by(|a, b| {
                crate::http::locale_compare(
                    a["id"].as_str().unwrap_or_default(),
                    b["id"].as_str().unwrap_or_default(),
                )
            });
            let rows = all
                .iter()
                .skip((start - 1) as usize)
                .take(count as usize)
                .map(project)
                .collect::<Result<Vec<_>>>()?;
            ok(
                &json!({ "schemas": [LIST], "totalResults": all.len(), "startIndex": start, "itemsPerPage": rows.len(), "Resources": rows }),
            )
        }
        (Method::GET, Some(id)) => {
            let row = directory.get(kind, id).await?;
            let etag = scim::version(&row);
            if header(headers, "if-none-match") == Some(etag.as_str()) {
                return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), &etag));
            }
            Ok(with_etag(
                scim_json(
                    Some(&project(&scim::resource(kind, &row, &base))?),
                    StatusCode::OK,
                ),
                &etag,
            ))
        }
        (Method::POST, None) | (Method::PUT | Method::PATCH, Some(_)) => {
            let input = scim_body(headers, body).await?;
            let row = directory
                .save(
                    kind,
                    id.as_deref(),
                    &input,
                    header(headers, "if-match"),
                    *method == Method::PATCH,
                    false,
                )
                .await?;
            let status = if *method == Method::POST {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            let mut response = with_etag(
                scim_json(Some(&project(&scim::resource(kind, &row, &base))?), status),
                &scim::version(&row),
            );
            if let Ok(location) = HeaderValue::from_str(&format!(
                "{base}/{}/{}",
                kind.name(),
                scim::encode(row["id"].as_str().unwrap_or_default())
            )) {
                response.headers_mut().insert(header::LOCATION, location);
            }
            Ok(response)
        }
        (Method::DELETE, Some(id)) => {
            directory
                .remove(kind, id, header(headers, "if-match"), false)
                .await?;
            Ok(scim_json(None, StatusCode::NO_CONTENT))
        }
        _ => Err(scim_error(405, "Method not allowed.", None)),
    }
}
