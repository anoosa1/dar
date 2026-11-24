//! Identity browser journey (`identity-browser.mjs`): settings, reauthentication, one-time
//! credentials, directory/groups/quotas, SCIM, cross-site consent, parallel consent, deny,
//! account switching, scope claims, revoke, form_post and RP-initiated logout; desktop & phone.

use axum::extract::{Query, State as AxumState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use dar_acceptance::oidc::{Client, Flow, Tokens};
use dar_acceptance::prelude::*;
use dar_acceptance::stack::write_private;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "identity-browser",
    cases: &[case!(identity_browser_journey, 180)],
};

struct RpFlow {
    verifier: String,
    nonce: String,
}

#[derive(Default)]
struct RpState {
    client: Option<Client>,
    redirect: String,
    flows: HashMap<String, RpFlow>,
    last_tokens: Option<Tokens>,
    last_claims: Option<Value>,
}

async fn rp_start(
    AxumState(state): AxumState<Arc<Mutex<RpState>>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let dest = {
        let mut s = state.lock().unwrap();
        let Some(client) = s.client.clone() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "RP client not configured",
            )
                .into_response();
        };
        let verifier = dar_acceptance::base64url(&dar_acceptance::random_bytes(32));
        let challenge = {
            use sha2::Digest as _;
            dar_acceptance::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
        };
        let st = dar_acceptance::base64url(&dar_acceptance::random_bytes(16));
        let nonce = dar_acceptance::base64url(&dar_acceptance::random_bytes(16));
        s.flows.insert(
            st.clone(),
            RpFlow {
                verifier,
                nonce: nonce.clone(),
            },
        );
        let mut auth_params = vec![
            ("redirect_uri".into(), s.redirect.clone()),
            ("response_type".into(), "code".into()),
            (
                "scope".into(),
                "openid profile email groups offline_access".into(),
            ),
            ("code_challenge".into(), challenge),
            ("code_challenge_method".into(), "S256".into()),
            ("state".into(), st),
            ("nonce".into(), nonce),
            ("prompt".into(), "consent".into()),
        ];
        if params.get("mode").map(String::as_str) == Some("form_post") {
            auth_params.push(("response_mode".into(), "form_post".into()));
        }
        match client.authorization_url(&auth_params) {
            Ok(url) => url.to_string(),
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Relying-party verification failed.",
                )
                    .into_response();
            }
        }
    };
    Redirect::to(&dest).into_response()
}

async fn finish_callback(state: Arc<Mutex<RpState>>, pairs: HashMap<String, String>) -> Response {
    if pairs.contains_key("error") {
        return (
            StatusCode::OK,
            [("content-type", "text/plain")],
            "Access denied",
        )
            .into_response();
    }
    let (client, redirect, flow, st) = {
        let mut s = state.lock().unwrap();
        let Some(st) = pairs.get("state").cloned() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Relying-party verification failed.",
            )
                .into_response();
        };
        let Some(flow) = s.flows.remove(&st) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Relying-party verification failed.",
            )
                .into_response();
        };
        let Some(client) = s.client.clone() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Relying-party verification failed.",
            )
                .into_response();
        };
        (client, s.redirect.clone(), flow, st)
    };
    let mut callback = match url::Url::parse(&redirect) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Relying-party verification failed.",
            )
                .into_response();
        }
    };
    callback.query_pairs_mut().extend_pairs(pairs.iter());
    let flow_struct = Flow {
        url: callback,
        jar: CookieJar::default(),
        verifier: flow.verifier,
        state: st,
        nonce: flow.nonce,
        pkce: true,
    };
    match client.exchange(&flow_struct).await {
        Ok(tokens) => {
            let sub = tokens
                .claims
                .as_ref()
                .and_then(|c| c["sub"].as_str())
                .unwrap_or_default()
                .to_owned();
            let mut s = state.lock().unwrap();
            s.last_claims = tokens.claims.clone();
            s.last_tokens = Some(tokens);
            (
                StatusCode::OK,
                [("content-type", "text/plain")],
                format!("Signed in as {sub}"),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Relying-party verification failed: {err}"),
        )
            .into_response(),
    }
}

async fn rp_callback_get(
    AxumState(state): AxumState<Arc<Mutex<RpState>>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    finish_callback(state, params).await
}

async fn rp_callback_post(
    AxumState(state): AxumState<Arc<Mutex<RpState>>>,
    _headers: HeaderMap,
    body: String,
) -> Response {
    let pairs: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    finish_callback(state, pairs).await
}

pub async fn identity_browser_journey(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let db = instance.database()?;

    // Configure the host-configured PKCE-optional client (`gitea-compatible-rp`) for the edit/persistence check.
    let cfg_dir = t.temp("gitea-oidc")?;
    write_private(
        &cfg_dir.join("secret"),
        b"isolated-pkce-compatibility-secret-1234567890",
    )?;
    let cfg_file = cfg_dir.join("oidc.json");
    std::fs::write(
        &cfg_file,
        json!({
            "clients": [{
                "id": "gitea-compatible-rp",
                "name": "Gitea compatible",
                "type": "confidential",
                "redirectUris": ["http://127.0.0.1:49999/callback"],
                "scopes": ["openid", "profile"],
                "secretFile": "secret",
                "requirePkce": false,
            }]
        })
        .to_string(),
    )?;
    support::configure_oidc(&db, instance.root(), &base, &cfg_file, false)?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rp_port = listener.local_addr()?.port();
    let rp_origin = format!("http://localhost:{rp_port}");
    let redirect = format!("{rp_origin}/callback");

    let rp_state = Arc::new(Mutex::new(RpState {
        redirect: redirect.clone(),
        ..RpState::default()
    }));
    let app = axum::Router::new()
        .route("/start", get(rp_start))
        .route("/callback", get(rp_callback_get).post(rp_callback_post))
        .route("/signed-out", get(|| async { "Signed out of RP" }))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .with_state(rp_state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let browser = Browser::shared().await?;
    let context = browser.new_context_sized(1440, 950).await?;
    let result = async {
        let page = context.new_page().await?;
        let ignore = |text: &str| {
            let re = regex::Regex::new(r"401.*Unauthorized|favicon.*404").unwrap();
            re.is_match(text)
        };
        let failures = page.collect_errors(ignore);

        page.goto(&format!("{base}/settings?section=applications"))
            .await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("heading", "Registered applications")
            .wait_for()
            .await?;

        let name = format!("Browser identity {}", &uuid()[..6]);
        page.get_by_role("button", "Register application")
            .click()
            .await?;
        let dialog = page.get_by_role("dialog", ()).last();
        dialog.get_by_label("Application name").fill(&name).await?;
        dialog.get_by_label("Redirect URLs").fill(&redirect).await?;
        dialog
            .get_by_label("Post-logout redirect URLs")
            .fill(&format!("{rp_origin}/signed-out"))
            .await?;
        dialog
            .get_by_label("Allowed permissions")
            .fill("openid profile email groups offline_access")
            .await?;
        dialog.get_by_role("button", "Save").click().await?;

        let confirm = page.get_by_role("dialog", "Confirm your identity");
        confirm
            .get_by_label("Current password")
            .fill(PASSWORD)
            .await?;
        confirm
            .get_by_role("button", "Confirm identity")
            .click()
            .await?;

        let cred_dialog = page.get_by_role("dialog", "Save your credential");
        cred_dialog.wait_for().await?;
        let client_id = cred_dialog.get_by_label("Identifier").input_value().await?;
        let secret = cred_dialog.get_by_label("Secret").input_value().await?;
        assert!(!secret.is_empty());
        cred_dialog
            .get_by_role("button", "I saved it")
            .click()
            .await?;
        page.wait_for_function(
            r#"() => document.activeElement?.textContent === "Register application""#,
        )
        .await?;

        let compatible = page
            .locator(".identity-card")
            .filter_has_text("Gitea compatible");
        compatible
            .get_by_text("Confidential client · PKCE optional")
            .wait_for()
            .await?;
        compatible
            .get_by_role("button", "Edit application")
            .click()
            .await?;
        let edit_dialog = page.get_by_role("dialog", ()).last();
        edit_dialog.get_by_role("button", "Save").click().await?;
        edit_dialog.wait_for_state(State::Hidden).await?;

        let persisted_page = context.new_page().await?;
        persisted_page
            .goto(&format!("{base}/settings?section=applications"))
            .await?;
        persisted_page
            .locator(".identity-card")
            .filter_has_text("Gitea compatible")
            .get_by_text("Confidential client · PKCE optional")
            .wait_for()
            .await?;
        let preserved = persisted_page
            .request()
            .get(&format!(
                "{base}/api/v1/identity/clients/gitea-compatible-rp"
            ))
            .await?
            .json()?;
        assert_eq!(preserved["requirePkce"], json!(false));
        persisted_page.close().await?;

        let client = Client::discover(&base, &client_id, Some(&secret), &redirect).await?;
        rp_state.lock().unwrap().client = Some(client.clone());

        ui::open_search(&page).await?.fill(&name).await?;
        assert!(page.locator(".identity-card").count().await? >= 1);
        ui::close_search(&page).await?;
        page.screenshot(t.screenshot("identity-settings-desktop.png"), true)
            .await?;

        page.get_by_role("button", "Directory").click().await?;
        page.get_by_role("button", "Edit profile").click().await?;
        let profile_dialog = page.get_by_role("dialog", ()).last();
        assert_eq!(
            profile_dialog
                .get_by_label("Username")
                .get_attribute("readonly")
                .await?,
            Some(String::new())
        );
        assert!(
            profile_dialog
                .get_by_label("Replacement password (optional)")
                .is_disabled()
                .await?
        );
        profile_dialog
            .get_by_label("Email addresses")
            .fill("operator@example.test")
            .await?;
        profile_dialog.get_by_role("button", "Save").click().await?;
        profile_dialog.wait_for_state(State::Hidden).await?;

        page.get_by_role("button", "Create user").click().await?;
        let username = format!("browser-id-{}", &uuid()[..6]);
        let create_dialog = page.get_by_role("dialog", ()).last();
        create_dialog
            .get_by_label("Username")
            .fill(&username)
            .await?;
        create_dialog
            .get_by_label("Display name")
            .fill("Browser directory person")
            .await?;
        create_dialog
            .get_by_label("Email addresses")
            .fill("browser@example.com")
            .await?;
        create_dialog
            .get_by_label("Local password (optional)")
            .fill(PASSWORD)
            .await?;
        create_dialog.get_by_role("button", "Save").click().await?;
        page.get_by_role("button", format!("Edit {username}"))
            .wait_for()
            .await?;

        // Storage is unlimited by default; the Directory editor sets and clears a per-account limit.
        let person_row = page
            .locator(".identity-row")
            .filter_has_text(format!("@{username}"));
        person_row
            .get_by_text(regex("Unlimited"))
            .wait_for()
            .await?;
        page.get_by_role("button", format!("Edit {username}"))
            .click()
            .await?;
        let quota_dialog = page.get_by_role("dialog", ()).last();
        assert_eq!(
            quota_dialog
                .get_by_label("Storage limit (GB)")
                .input_value()
                .await?,
            ""
        );
        quota_dialog
            .get_by_label("Storage limit (GB)")
            .fill("2")
            .await?;
        quota_dialog.get_by_role("button", "Save").click().await?;
        quota_dialog.wait_for_state(State::Hidden).await?;
        person_row
            .get_by_text(regex(r"of 2\.0 GB"))
            .wait_for()
            .await?;

        {
            let users = page
                .request()
                .get(&format!("{base}/api/v1/identity/users"))
                .await?
                .json()?;
            let uid = users["items"]
                .as_array()
                .and_then(|arr| arr.iter().find(|u| u["userName"] == username.as_str()))
                .and_then(|u| u["id"].as_str())
                .context("created user id")?
                .to_owned();
            let storage = page
                .request()
                .get(&format!("{base}/api/v1/identity/storage"))
                .await?
                .json()?;
            let row = storage["items"]
                .as_array()
                .and_then(|arr| arr.iter().find(|r| r["userId"] == uid.as_str()))
                .context("storage row")?;
            assert_eq!(row["quota"], json!(2u64 * 1024 * 1024 * 1024));
        }

        page.get_by_role("button", format!("Edit {username}"))
            .click()
            .await?;
        let quota_dialog = page.get_by_role("dialog", ()).last();
        assert_eq!(
            quota_dialog
                .get_by_label("Storage limit (GB)")
                .input_value()
                .await?,
            "2"
        );
        quota_dialog
            .get_by_label("Storage limit (GB)")
            .fill("")
            .await?;
        quota_dialog.get_by_role("button", "Save").click().await?;
        quota_dialog.wait_for_state(State::Hidden).await?;
        person_row
            .get_by_text(regex("Unlimited"))
            .wait_for()
            .await?;

        page.get_by_role("button", "Create group").click().await?;
        let group_dialog = page.get_by_role("dialog", ()).last();
        group_dialog
            .get_by_label("Group name")
            .fill("Browser identity team")
            .await?;
        group_dialog
            .get_by_label(format!("Browser directory person (@{username})"))
            .check()
            .await?;
        group_dialog.get_by_role("button", "Save").click().await?;
        page.get_by_role("button", "Edit Browser identity team")
            .wait_for()
            .await?;

        let users = page
            .request()
            .get(&format!("{base}/api/v1/identity/users"))
            .await?
            .json()?;
        let person = users["items"]
            .as_array()
            .and_then(|arr| arr.iter().find(|u| u["userName"] == username.as_str()))
            .context("person")?
            .clone();
        let person_id = person["id"].as_str().context("person id")?.to_owned();
        let groups = page
            .request()
            .get(&format!("{base}/api/v1/identity/groups"))
            .await?
            .json()?;
        let group = groups["items"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|g| g["displayName"] == "Browser identity team")
            })
            .context("group")?
            .clone();
        let group_id = group["id"].as_str().context("group id")?.to_owned();
        assert_eq!(person["groups"][0]["value"], json!(group_id));

        page.get_by_role("button", "SCIM").click().await?;
        page.get_by_role("button", "Create SCIM credential")
            .click()
            .await?;
        let scim_dialog = page.get_by_role("dialog", ()).last();
        scim_dialog
            .get_by_label("Credential name")
            .fill("Browser provisioner")
            .await?;
        scim_dialog.get_by_role("button", "Save").click().await?;
        let save_scim = page.get_by_role("dialog", "Save your credential");
        let scim_secret = save_scim.get_by_label("Secret").input_value().await?;
        save_scim
            .get_by_role("button", "I saved it")
            .click()
            .await?;
        assert_eq!(
            session::client()
                .get(format!("{base}/scim/v2/Users"))
                .bearer_auth(&scim_secret)
                .send()
                .await?
                .status()
                .as_u16(),
            200
        );

        page.set_viewport_size(390, 844).await?;
        page.get_by_role("button", "Applications").click().await?;
        ui::open_search(&page).await?.fill(&name).await?;
        page.screenshot(t.screenshot("identity-settings-phone.png"), true)
            .await?;
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                .await?,
            json!(false)
        );
        ui::close_search(&page).await?;
        page.set_viewport_size(1440, 950).await?;

        // localhost RP -> 127.0.0.1 OP is cross-site. The native cookie remains SameSite=Strict.
        page.goto(&format!("{rp_origin}/start")).await?;
        page.get_by_role("button", "Continue").wait_for().await?;
        let password_field = page.get_by_label("Confirm your password");
        if password_field.count().await? > 0 {
            password_field.fill(PASSWORD).await?;
        }
        page.get_by_role("button", "Continue").click().await?;
        page.get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        page.screenshot(t.screenshot("identity-consent-desktop.png"), true)
            .await?;
        page.get_by_role("button", "Allow access").click().await?;
        page.get_by_text(regex("^Signed in as ")).wait_for().await?;
        let (admin_tokens, admin_claims) = {
            let s = rp_state.lock().unwrap();
            (
                s.last_tokens.clone().context("admin tokens")?,
                s.last_claims.clone().context("admin claims")?,
            )
        };
        assert!(admin_tokens.id_token().is_some());
        assert!(admin_claims.get("preferred_username").is_none());

        let cookies = context.cookies().await?;
        let native_session = cookies
            .iter()
            .find(|c| c.name == "dar_session")
            .context("dar_session cookie")?;
        let oidc_cookie = cookies
            .iter()
            .find(|c| c.name == "dar_oidc")
            .context("dar_oidc cookie")?;
        assert_eq!(native_session.same_site.as_deref(), Some("Strict"));
        assert_eq!(oidc_cookie.same_site.as_deref(), Some("Lax"));

        let parallel_page = context.new_page().await?;
        let parallel_failures = parallel_page.collect_errors(ignore);
        page.goto(&format!("{rp_origin}/start")).await?;
        page.get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        parallel_page.goto(&format!("{rp_origin}/start")).await?;
        parallel_page
            .get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        page.get_by_role("button", "Allow access").click().await?;
        page.get_by_text(regex("^Signed in as ")).wait_for().await?;
        parallel_page
            .get_by_role("button", "Allow access")
            .click()
            .await?;
        parallel_page
            .get_by_text(regex("^Signed in as "))
            .wait_for()
            .await?;
        assert!(
            parallel_failures.lock().unwrap().is_empty(),
            "parallel page errors: {:?}",
            *parallel_failures.lock().unwrap()
        );
        parallel_page.close().await?;

        page.goto(&format!("{rp_origin}/start")).await?;
        page.get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        page.get_by_role("button", "Deny").click().await?;
        page.get_by_text("Access denied").wait_for().await?;

        page.goto(&format!("{rp_origin}/start")).await?;
        page.get_by_role("button", "Switch account").click().await?;
        page.get_by_label("Username").fill(&username).await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        page.set_viewport_size(390, 844).await?;
        page.screenshot(t.screenshot("identity-consent-phone.png"), true)
            .await?;
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                .await?,
            json!(false)
        );
        page.get_by_role("button", "Allow access").click().await?;
        page.get_by_text(regex("^Signed in as ")).wait_for().await?;

        let (person_tokens, person_claims) = {
            let s = rp_state.lock().unwrap();
            (
                s.last_tokens.clone().context("person tokens")?,
                s.last_claims.clone().context("person claims")?,
            )
        };
        assert_eq!(person_claims["sub"], json!(person_id));
        assert_ne!(person_claims["sub"], admin_claims["sub"]);
        let info = client.user_info(person_tokens.access_token()).await?;
        assert_eq!(info["sub"], json!(person_id));
        assert_eq!(info["email"], json!("browser@example.com"));
        assert_eq!(info["email_verified"], json!(false));
        assert_eq!(info["groups"], json!([group_id]));

        page.goto(&format!("{base}/settings?section=connected"))
            .await?;
        page.get_by_role("heading", "Connected applications")
            .wait_for()
            .await?;
        assert_eq!(page.get_by_role("button", "Directory").count().await?, 0);
        page.get_by_role("button", "Revoke").click().await?;
        page.get_by_role("dialog", ())
            .get_by_role("button", "Revoke access")
            .click()
            .await?;
        page.get_by_text("No connected applications yet.")
            .wait_for()
            .await?;
        assert!(
            client
                .user_info(person_tokens.access_token())
                .await
                .is_err()
        );

        // form_post uses a self-hosted script and a registered form destination, without inline code.
        page.goto(&format!("{rp_origin}/start?mode=form_post"))
            .await?;
        page.get_by_role("heading", "Review requested access")
            .wait_for()
            .await?;
        page.get_by_role("button", "Allow access").click().await?;
        page.get_by_text(regex("^Signed in as ")).wait_for().await?;
        let (form_post_tokens, form_post_claims) = {
            let s = rp_state.lock().unwrap();
            (
                s.last_tokens.clone().context("form_post tokens")?,
                s.last_claims.clone().context("form_post claims")?,
            )
        };
        assert_eq!(form_post_claims["sub"], json!(person_id));

        let id_token_hint = form_post_tokens.id_token().context("form_post id_token")?;
        let post_logout = format!("{rp_origin}/signed-out");
        let logout = client.end_session_url(&[
            ("id_token_hint", id_token_hint),
            ("post_logout_redirect_uri", &post_logout),
            ("state", "browser-logout"),
        ])?;
        page.goto(logout.as_str()).await?;
        page.get_by_role("link", "Cancel").click().await?;
        page.get_by_role("heading", "Overview").wait_for().await?;
        assert_eq!(
            client.user_info(form_post_tokens.access_token()).await?["sub"],
            json!(person_id)
        );

        page.goto(logout.as_str()).await?;
        page.get_by_role("button", "Sign out").click().await?;
        page.get_by_text("Signed out of RP").wait_for().await?;
        assert!(
            client
                .user_info(form_post_tokens.access_token())
                .await
                .is_err()
        );
        assert_eq!(
            page.request()
                .get(&format!("{base}/api/v1/auth/session"))
                .await?
                .status,
            200
        );
        assert!(admin_tokens.id_token().is_some());

        let collected = failures.lock().unwrap().clone();
        assert!(collected.is_empty(), "browser errors: {collected:?}");
        Ok(())
    }
    .await;

    context.close().await.ok();
    server.abort();
    result
}
