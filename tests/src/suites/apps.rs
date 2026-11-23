//! Installed apps (`apps.integration.test.ts`): user/app isolation, revision preconditions,
//! quotas, permissions, backup and restore of private app user data.

use super::photos_books_apps_support::create_user;
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite { name: "apps", cases: &[case!(installed_apps_isolation_quotas_and_restore, 60)] };

pub async fn installed_apps_isolation_quotas_and_restore(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let admin = login(&base).await?;
    let db = instance.database()?;

    let installed = admin.get_json("/api/v1/apps").await?;
    let items = installed["items"].as_array().context("installed apps items")?;
    assert!(!items.is_empty());
    let app = items[0]["id"].as_str().context("installed app id")?.to_owned();
    let endpoint = format!("/api/v1/apps/{app}/userData/");
    let key = format!("test-{}", uuid());

    let user = format!("app-user-{}", &uuid()[..6]);
    create_user(&admin, &user, "App isolation", PASSWORD).await?;
    let other = login_as(&base, &user, PASSWORD).await?;

    let put = |session: &Session, k: &str, revision: u64, value: Value| {
        let session = session.clone();
        let path = format!("{endpoint}{k}");
        async move {
            Reply::from(
                session
                    .request("PUT", &path)
                    .header("if-match", format!("\"{revision}\""))
                    .json(&json!({"schemaVersion": 1, "value": value}))
                    .send()
                    .await?,
            )
            .await
        }
    };

    assert_eq!(session::client().get(format!("{base}{endpoint}{key}")).send().await?.status().as_u16(), 401);
    assert_eq!(put(&admin, &key, 0, json!({"secret": "private"})).await?.status, 200);
    assert_eq!(put(&admin, &key, 0, json!({})).await?.status, 409);

    let fetched = admin.get(&format!("{endpoint}{key}")).await?;
    assert_eq!(fetched.header("etag"), Some("\"1\""));
    assert_eq!(other.get(&format!("{endpoint}{key}")).await?.json()?, Value::Null);

    let missing_if_match = admin.put(&format!("{endpoint}{key}"), &json!({"value": {}, "schemaVersion": 1})).await?;
    assert_eq!(missing_if_match.status, 428);

    let missing_csrf = Reply::from(
        session::client()
            .put(format!("{base}{endpoint}{key}"))
            .header("cookie", &admin.cookie)
            .header("origin", &base)
            .header("content-type", "application/json")
            .header("if-match", "\"1\"")
            .body(r#"{"schemaVersion":1,"value":{}}"#)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(missing_csrf.status, 403);

    assert_eq!(other.get("/api/v1/admin/installedApps").await?.status, 403);
    assert_eq!(put(&admin, "large", 0, json!("x".repeat(65536))).await?.status, 413);

    for i in 0..17 {
        assert_eq!(put(&admin, &format!("quota{i}"), 0, json!("x".repeat(60000))).await?.status, 200);
    }
    assert_eq!(put(&admin, "quota-overflow", 0, json!("x".repeat(60000))).await?.status, 507);
    for i in 0..17 {
        let deleted = Reply::from(
            admin
                .request("DELETE", &format!("{endpoint}quota{i}"))
                .header("if-match", "\"1\"")
                .send()
                .await?,
        )
        .await?;
        assert_eq!(deleted.status, 200);
    }

    assert_eq!(Reply::from(other.request("POST", "/api/v1/auth/logout").send().await?).await?.status, 200);
    assert_eq!(other.get(&format!("{endpoint}{key}")).await?.status, 401);

    let backup_accounts = db.call("service_backup", json!([uuid()])).await?;
    let admin_account = backup_accounts["accounts"]
        .as_array()
        .and_then(|arr| arr.iter().find(|u| u["username"] == "admin"))
        .context("admin account in backup")?;
    let logged_in = admin.get_json("/api/v1/auth/session").await?;
    assert_eq!(logged_in["user"]["id"], admin_account["id"]);

    let cookie_secret = admin.cookie.split_once('=').map(|(_, v)| v).unwrap_or_default();
    let current_session = db.call("service_session", json!([support::digest_token(cookie_secret)])).await?;
    let sessions = db.call("service_sessions", json!([current_session["session"]["id"]])).await?;
    let sid = sessions
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["current"] == json!(true)))
        .and_then(|s| s["id"].as_str())
        .context("current session id")?
        .to_owned();

    let second = json!({"id": "org.example.other", "permissions": ["ownUserData:read", "ownUserData:write"]});
    let mut with_second = items.clone();
    with_second.push(second.clone());
    db.call("service_installed_apps", json!([serde_json::to_string(&with_second)?])).await?;

    let second_check: Result<()> = async {
        assert_eq!(
            db.call("service_app_data", json!([sid, "org.example.other", "get", key, 0, 1, ""])).await?,
            Value::Null
        );
        let mut read_only = items.clone();
        read_only.push(json!({"id": "org.example.other", "permissions": ["ownUserData:read"]}));
        db.call("service_installed_apps", json!([serde_json::to_string(&read_only)?])).await?;
        let denied = db.call("service_app_data", json!([sid, "org.example.other", "put", key, 0, 1, "{}"])).await;
        assert!(denied.err().is_some_and(|e| e.code == "forbidden"));
        db.call("service_installed_apps", json!(["[]"])).await?;
        assert_eq!(admin.get(&format!("{endpoint}{key}")).await?.status, 404);
        Ok(())
    }
    .await;
    db.call("service_installed_apps", json!([serde_json::to_string(items)?])).await?;
    second_check?;

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    assert_eq!(snapshot["format"], "dar-backup-v12");
    assert!(
        snapshot["apps"]["data"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["key"] == key.as_str()))
    );

    let restore_name = format!("dar-app-restore-{}", &uuid()[..8]);
    instance::publish_module(&restore_name, &base)?;
    let restored = Database::new(&db.url, &restore_name, &db.token_file)?;
    restored.call("service_restore", json!([snapshot.to_string()])).await?;
    let recovered = restored.call("service_backup", json!([uuid()])).await?;
    assert_eq!(recovered["apps"]["data"], snapshot["apps"]["data"]);
    assert_eq!(recovered["apps"]["inventory"], json!([]));

    let final_delete = Reply::from(
        admin
            .request("DELETE", &format!("{endpoint}{key}"))
            .header("if-match", "\"1\"")
            .send()
            .await?,
    )
    .await?;
    assert_eq!(final_delete.status, 200);
    Ok(())
}
