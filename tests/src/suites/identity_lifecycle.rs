//! Identity lifecycle (`identity-lifecycle.integration.test.ts`): native logout, password change,
//! SCIM suspension, code expiry, interaction CSRF, secret rotation, private table access control,
//! OIDC prompt/max_age, client/redirect binding and protocol revocation.

use super::core_support::{PATCH, authorize_with, form, model_key, query_param, scim};
use dar_acceptance::oidc::{Authorize, Client, authorize};
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "identity-lifecycle",
    cases: &[
        case!(logout_password_scim_revocation, 60),
        case!(expiry_csrf_rotation_and_private_tables, 60),
        case!(prompt_max_age_binding_and_revocation, 60),
    ],
};

pub async fn logout_password_scim_revocation(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let client = admin.default_client().await?;
    let (_, scim_secret) = admin.credential("write").await?;
    let username = format!("lifecycle-{}", &uuid()[..8]);

    let created = scim(
        &base,
        &scim_secret,
        "/Users",
        "POST",
        Some(&json!({ "userName": username, "password": PASSWORD })),
        &[],
    )
    .await?
    .expect(201)?;
    let user_id = created.json()?["id"]
        .as_str()
        .context("user id")?
        .to_owned();

    let first = login_as(&base, &username, PASSWORD).await?;
    let second = login_as(&base, &username, PASSWORD).await?;
    let a = client
        .exchange(&authorize(&base, &first, &client, Authorize::default()).await?)
        .await?;
    let b = client
        .exchange(&authorize(&base, &second, &client, Authorize::default()).await?)
        .await?;

    assert_eq!(
        Reply::from(first.request("POST", "/api/v1/auth/logout").send().await?)
            .await?
            .status,
        200
    );
    assert!(client.user_info(a.access_token()).await.is_err());
    assert_eq!(
        client.user_info(b.access_token()).await?["sub"],
        user_id.as_str()
    );

    let disable = json!({ "schemas": [PATCH], "Operations": [{ "op": "replace", "path": "active", "value": false }] });
    scim(
        &base,
        &scim_secret,
        &format!("/Users/{user_id}"),
        "PATCH",
        Some(&disable),
        &[],
    )
    .await?
    .expect(200)?;
    assert!(client.user_info(b.access_token()).await.is_err());
    assert!(
        client
            .refresh(b.refresh_token().context("refresh token b")?)
            .await
            .is_err()
    );
    assert_eq!(second.get("/api/v1/auth/session").await?.status, 401);

    let enable = json!({ "schemas": [PATCH], "Operations": [{ "op": "replace", "path": "active", "value": true }] });
    scim(
        &base,
        &scim_secret,
        &format!("/Users/{user_id}"),
        "PATCH",
        Some(&enable),
        &[],
    )
    .await?
    .expect(200)?;

    let fresh_session = login_as(&base, &username, PASSWORD).await?;
    let fresh = client
        .exchange(&authorize(&base, &fresh_session, &client, Authorize::default()).await?)
        .await?;
    let new_password = format!("{PASSWORD} replaced");
    let changed = fresh_session
        .post(
            "/api/v1/auth/password",
            &json!({ "currentPassword": PASSWORD, "password": new_password }),
        )
        .await?;
    assert_eq!(changed.status, 200);
    assert!(client.user_info(fresh.access_token()).await.is_err());
    assert!(
        client
            .refresh(fresh.refresh_token().context("fresh refresh token")?)
            .await
            .is_err()
    );

    let after = login_as(&base, &username, &new_password).await?;
    assert_eq!(after.get("/api/v1/auth/session").await?.status, 200);
    Ok(())
}

pub async fn expiry_csrf_rotation_and_private_tables(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let db = instance.database()?;
    let admin = IdentityAdmin::new(&base).await?;
    let client = admin.default_client().await?;

    let flow = authorize(&base, &admin.session, &client, Authorize::default()).await?;
    let code = query_param(&flow.url, "code").context("authorization code")?;
    let id = model_key("AuthorizationCode", &code);
    let mut row = db.provider("find", json!({ "id": id })).await?;
    assert!(!row.is_null(), "authorization code row must exist");
    assert!(
        !row["encrypted"]
            .as_str()
            .unwrap_or_default()
            .contains(&flow.nonce)
    );

    let expired_at = chrono::Utc::now().timestamp_millis() - 1000;
    row["expiresAt"] = json!(expired_at);
    row["expiryKey"] = json!(format!("{expired_at:016}"));
    db.provider("upsert", row).await?;
    assert!(client.exchange(&flow).await.is_err());

    let verifier = dar_acceptance::base64url(&dar_acceptance::random_bytes(32));
    let challenge = {
        use sha2::Digest as _;
        dar_acceptance::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    let start_url = client.authorization_url(&[
        ("redirect_uri".into(), client.redirect.clone()),
        ("response_type".into(), "code".into()),
        ("scope".into(), "openid".into()),
        ("code_challenge".into(), challenge),
        ("code_challenge_method".into(), "S256".into()),
    ])?;
    let mut jar = CookieJar::new(Some(&admin.session.cookie));
    let started = jar.get(start_url.as_str()).await?;
    let location = url::Url::parse(&base)?
        .join(started.header("location").context("interaction redirect")?)?;
    let uid = location
        .path_segments()
        .and_then(|mut s| s.next_back())
        .context("interaction uid")?
        .to_owned();

    let interaction_url = format!("{base}/api/v1/identity/interactions/{uid}");
    let body = json!({ "action": "login", "password": PASSWORD }).to_string();
    let evil = jar
        .fetch(
            session::client()
                .post(&interaction_url)
                .header("origin", "https://evil.example")
                .header("content-type", "application/json")
                .header("x-csrf-token", &admin.session.csrf_token)
                .body(body.clone()),
        )
        .await?;
    assert_eq!(evil.status, 403);

    let no_csrf = jar
        .fetch(
            session::client()
                .post(&interaction_url)
                .header("origin", &base)
                .header("content-type", "application/json")
                .body(body),
        )
        .await?;
    assert_eq!(no_csrf.status, 403);

    let good = client
        .exchange(&authorize(&base, &admin.session, &client, Authorize::default()).await?)
        .await?;
    let list = admin.get("/clients").await?;
    let revision = list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|v| v["id"] == client.id.as_str()))
        .and_then(|v| v["revision"].as_u64())
        .context("client revision")?;
    let rotation = admin
        .write(
            &format!("/clients/{}/secret", client.id),
            "POST",
            None,
            Some(&format!("W/\"{revision}\"")),
        )
        .await?
        .expect(200)?;
    let replacement = rotation.json()?["secret"]
        .as_str()
        .context("rotated secret")?
        .to_owned();
    assert_ne!(Some(replacement.as_str()), client.secret.as_deref());
    assert!(
        client
            .refresh(good.refresh_token().context("good refresh token")?)
            .await
            .is_err()
    );

    let new_client =
        Client::discover(&base, &client.id, Some(&replacement), &client.redirect).await?;
    let fresh = new_client
        .exchange(&authorize(&base, &admin.session, &new_client, Authorize::default()).await?)
        .await?;
    assert!(fresh.id_token().is_some());

    let sql_url = format!("{}/v1/database/{}/sql", db.url, db.name);
    for table in [
        "provider_models",
        "identity_clients",
        "directory_profiles",
        "directory_members",
        "provisioning_tokens",
        "identity_proofs",
    ] {
        let query = format!("SELECT * FROM {table}");
        db.sql(&query)
            .await
            .context(format!("owner can query existing private table {table}"))?;
        let response = session::client()
            .post(&sql_url)
            .bearer_auth(&admin.session.token)
            .header("content-type", "text/plain")
            .body(query)
            .send()
            .await?;
        assert!(
            !response.status().is_success(),
            "browser token must not query private table {table}"
        );
        let _ = response.bytes().await;
    }

    let without_proof = admin
        .api(
            "/credentials",
            "POST",
            Some(&json!({ "label": "not confirmed", "scope": "write", "days": 1 })),
        )
        .await?;
    assert_eq!(without_proof.status, 403);
    Ok(())
}

pub async fn prompt_max_age_binding_and_revocation(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let client = admin
        .client(
            "public",
            "openid profile",
            "http://127.0.0.1:49999/callback",
        )
        .await?;
    let other = admin
        .client(
            "public",
            "openid profile",
            "http://127.0.0.1:49999/callback",
        )
        .await?;

    let mut auth_methods: Vec<String> = client.metadata["token_endpoint_auth_methods_supported"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    auth_methods.sort();
    assert_eq!(auth_methods, vec!["client_secret_basic", "none"]);

    let initial = authorize(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            ..Authorize::default()
        },
    )
    .await?;
    client.exchange(&initial).await?;

    let mut rendered_interaction = false;
    let silent = authorize_with(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            prompt: Some("none".into()),
            jar: Some(initial.jar.clone()),
            ..Authorize::default()
        },
        |_| rendered_interaction = true,
    )
    .await?;
    assert!(
        !rendered_interaction,
        "prompt=none must not render interaction"
    );
    assert!(query_param(&silent.url, "code").is_some());
    client.exchange(&silent).await?;

    let mut confirmed = false;
    let forced = authorize_with(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            prompt: Some("login".into()),
            jar: Some(initial.jar.clone()),
            ..Authorize::default()
        },
        |d| {
            if d.prompt == "login" {
                let _: bool = d.requires_password;
                confirmed = true;
            }
        },
    )
    .await?;
    assert!(confirmed);
    client.exchange(&forced).await?;

    let mut reauthenticated = false;
    let aged = authorize_with(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            prompt: Some("consent".into()),
            params: vec![("max_age".into(), "0".into())],
            jar: Some(initial.jar.clone()),
            ..Authorize::default()
        },
        |d| {
            if d.prompt == "login" {
                reauthenticated = true;
            }
        },
    )
    .await?;
    let aged_tokens = client.exchange(&aged).await?;
    assert!(reauthenticated);
    let claims = aged_tokens
        .claims
        .as_ref()
        .context("aged ID token claims")?;
    assert!(claims["auth_time"].is_number());

    let mismatched = authorize(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            ..Authorize::default()
        },
    )
    .await?;
    assert!(other.exchange(&mismatched).await.is_err());

    let redirect_mismatch = authorize(
        &base,
        &admin.session,
        &client,
        Authorize {
            scope: Some("openid profile".into()),
            ..Authorize::default()
        },
    )
    .await?;
    let code = query_param(&redirect_mismatch.url, "code").context("redirect mismatch code")?;
    let rejected_redirect = Reply::from(
        session::client()
            .post(format!("{base}/oidc/token"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client.id),
                ("code", &code),
                ("code_verifier", &redirect_mismatch.verifier),
                ("redirect_uri", "http://127.0.0.1:49999/unregistered"),
            ]))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(rejected_redirect.status, 400);
    assert_eq!(rejected_redirect.json()?["error"], json!("invalid_grant"));

    let revoked = session::client()
        .post(format!("{base}/oidc/token/revocation"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&[
            ("client_id", &client.id),
            ("token", aged_tokens.access_token()),
            ("token_type_hint", "access_token"),
        ]))
        .send()
        .await?;
    assert_eq!(revoked.status().as_u16(), 200);
    assert!(client.user_info(aged_tokens.access_token()).await.is_err());
    Ok(())
}
