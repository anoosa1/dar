//! Host-configured OIDC clients (`oidc-config.integration.test.ts`): PKCE opt-out for
//! confidential clients, validation, secret rotation, disabling and revision checks.

use super::core_support::{form, query_param};
use dar_acceptance::oidc::{Authorize, Client, Flow, authorize};
use dar_acceptance::prelude::*;
use dar_acceptance::stack::write_private;

pub const SUITE: Suite = Suite {
    name: "oidc-config",
    cases: &[case!(pkce_opt_out_confidential_clients, 60), case!(text_configured_clients, 60)],
};

pub async fn pkce_opt_out_confidential_clients(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let db = instance.database()?;
    let root = instance.root();
    let admin = IdentityAdmin::new(&base).await?;
    let directory = t.temp("dar-pkce")?;

    let secret = "isolated-pkce-compatibility-secret-1234567890";
    let id = "gitea-compatible-rp";
    let redirect = "http://127.0.0.1:49999/callback";
    write_private(&directory.join("secret"), secret.as_bytes())?;
    let entry = json!({
        "id": id,
        "name": "Gitea compatible",
        "type": "confidential",
        "redirectUris": [redirect],
        "scopes": ["openid", "profile"],
        "secretFile": "secret",
    });
    let filename = directory.join("oidc.json");
    let apply = |value: Value| -> Result<String> {
        std::fs::write(&filename, json!({ "clients": [value] }).to_string())?;
        support::configure_oidc(&db, root, &base, &filename, false)
    };
    let client = Client::discover(&base, id, Some(secret), redirect).await?;
    let flow = || {
        authorize(
            &base,
            &admin.session,
            &client,
            Authorize { no_pkce: true, scope: Some("openid profile".into()), ..Authorize::default() },
        )
    };

    apply(entry.clone())?;
    assert_eq!(query_param(&flow().await?.url, "error").as_deref(), Some("invalid_request"), "default requires PKCE");

    let mut bad_type = entry.clone();
    bad_type["requirePkce"] = json!("false");
    assert!(apply(bad_type).is_err());

    let mut public_entry = entry.clone();
    public_entry.as_object_mut().unwrap().remove("secretFile");
    public_entry["type"] = json!("public");
    public_entry["requirePkce"] = json!(false);
    assert!(apply(public_entry).is_err());

    let mut opt_out = entry.clone();
    opt_out["requirePkce"] = json!(false);
    apply(opt_out.clone())?;

    let detail = admin.get(&format!("/clients/{id}")).await?;
    assert_eq!(detail["requirePkce"], json!(false));

    let allowed = flow().await?;
    let tokens = client.exchange(&allowed).await?;
    assert!(tokens.id_token().is_some());

    let wrong_secret_flow = flow().await?;
    let code = query_param(&wrong_secret_flow.url, "code").context("authorization code")?;
    let rejected = session::client()
        .post(format!("{base}/oidc/token"))
        .basic_auth(id, Some("wrong-secret"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&[("grant_type", "authorization_code"), ("redirect_uri", redirect), ("code", &code)]))
        .send()
        .await?;
    assert_eq!(rejected.status().as_u16(), 401);

    let protected_flow = authorize(&base, &admin.session, &client, Authorize { scope: Some("openid profile".into()), ..Authorize::default() }).await?;
    let wrong_verifier = Flow { verifier: dar_acceptance::base64url(&dar_acceptance::random_bytes(32)), ..protected_flow };
    assert!(client.exchange(&wrong_verifier).await.is_err(), "supplied PKCE remains enforced");

    apply(entry)?;
    assert_eq!(query_param(&flow().await?.url, "error").as_deref(), Some("invalid_request"), "re-enabling PKCE applies to the next request");

    let public_client = admin.client("public", "openid", redirect).await?;
    let public_no_pkce = authorize(
        &base,
        &admin.session,
        &public_client,
        Authorize { no_pkce: true, scope: Some("openid".into()), ..Authorize::default() },
    )
    .await?;
    assert_eq!(query_param(&public_no_pkce.url, "error").as_deref(), Some("invalid_request"));

    apply(opt_out)?;
    Ok(())
}

pub async fn text_configured_clients(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let db = instance.database()?;
    let root = instance.root();
    let id = "configured-rp";
    let secret = "isolated-configured-secret-1234567890";
    let redirect = "http://127.0.0.1:49999/callback";

    let read = || async { db.provider("client", json!({ "id": id })).await };
    let original = read().await?;
    assert_eq!(original["revision"], json!(1));
    assert!(!original["metadata"].as_str().unwrap_or_default().contains(secret));
    assert!(!original["encrypted"].as_str().unwrap_or_default().contains(secret));

    let client = Client::discover(&base, id, Some(secret), redirect).await?;
    let admin = IdentityAdmin::new(&base).await?;
    let tokens = client
        .exchange(&authorize(&base, &admin.session, &client, Authorize { scope: Some("openid profile".into()), ..Authorize::default() }).await?)
        .await?;
    assert!(!tokens.access_token().is_empty());

    let directory = t.temp("dar-oidc-config")?;
    let filename = directory.join("oidc.json");
    let entry = json!({
        "id": id,
        "name": "Configured RP",
        "type": "confidential",
        "redirectUris": [redirect],
        "scopes": ["openid", "profile"],
        "secretFile": "secret",
    });
    write_private(&directory.join("secret"), format!("{secret}\n").as_bytes())?;
    let apply = |clients: Value| -> Result<String> {
        std::fs::write(&filename, json!({ "clients": clients }).to_string())?;
        support::configure_oidc(&db, root, &base, &filename, false)
    };

    apply(json!([entry]))?;
    assert_eq!(read().await?["revision"], json!(1), "unchanged settings retain grants and revision");

    apply(json!([]))?;
    assert_eq!(read().await?["revision"], json!(1), "omitted clients are retained");

    let public_id = "configured-public-rp";
    apply(json!([{
        "id": public_id,
        "name": "Public RP",
        "type": "public",
        "redirectUris": ["https://public.example.test/callback"],
        "scopes": ["openid"],
    }]))?;
    assert_eq!(admin.get(&format!("/clients/{public_id}")).await?["type"], json!("public"), "a public client authenticates with no secret");

    let mut wildcard_redirect = entry.clone();
    wildcard_redirect["redirectUris"] = json!(["https://photos.example.test/*"]);
    assert!(apply(json!([wildcard_redirect])).is_err());

    let mut duplicate = entry.clone();
    duplicate["name"] = json!("Should not apply");
    assert!(apply(json!([duplicate, entry])).is_err());
    assert_eq!(read().await?["revision"], json!(1), "validate the entire file before writing");

    write_private(&directory.join("secret"), b"rotated-isolated-secret-1234567890")?;
    apply(json!([entry]))?;
    assert_eq!(read().await?["revision"], json!(2));

    assert!(
        db.provider("saveClient", json!({ "id": id, "revision": 1, "metadata": "{}", "encrypted": "stale", "enabled": true }))
            .await
            .is_err()
    );
    assert!(client.user_info(tokens.access_token()).await.is_err());

    let mut disabled = entry.clone();
    disabled["enabled"] = json!(false);
    apply(json!([disabled]))?;
    assert_eq!(read().await?["enabled"], json!(false));

    let rejected = session::client()
        .post(format!("{base}/oidc/token"))
        .basic_auth(id, Some("rotated-isolated-secret-1234567890"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&[("grant_type", "authorization_code"), ("redirect_uri", redirect), ("code", "never-issued")]))
        .send()
        .await?;
    assert_eq!(rejected.status().as_u16(), 401, "a disabled client no longer authenticates (an enabled one would get invalid_grant)");

    let missing = directory.join("missing");
    assert!(support::configure_oidc(&db, root, &base, &missing, false).is_err());
    support::configure_oidc(&db, root, &base, &missing, true)?;

    std::fs::write(&filename, r#"{"clients":["sensitive-invalid-content"]"#)?;
    let err = support::configure_oidc(&db, root, &base, &filename, false).unwrap_err();
    assert_eq!(err.to_string(), "Invalid OIDC configuration JSON.");
    Ok(())
}
