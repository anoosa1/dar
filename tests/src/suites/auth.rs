//! Native sign-in (`auth.integration.test.ts`): racing first-account setups, private views per
//! session, per-session revocation that clears only that connection's rows, and service calls
//! a person may not make.

use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite { name: "auth", cases: &[case!(native_login_and_revocation, 30)] };

pub async fn native_login_and_revocation(t: &Ctx) -> Result<()> {
    // Its own instance: the first-account setup race needs one that is not set up yet.
    let instance = Instance::isolated(&t.temp("auth")?, "auth", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();
    let status = session::client().get(instance.url("/api/v1/auth/status")).send().await?.json::<Value>().await?;
    if status["initialized"] != json!(true) {
        let token = dar_acceptance::read_to_string(&dar_acceptance::stack::stack().bootstrap_token_file)?.trim().to_owned();
        let body = json!({"username": "admin", "displayName": "Test Administrator", "password": PASSWORD, "bootstrapToken": token});
        let setup = || session::client().post(instance.url("/api/v1/auth/setup")).header("origin", &base).json(&body).send();
        let (one, two) = tokio::join!(setup(), setup());
        let mut statuses = vec![one?.status().as_u16(), two?.status().as_u16()];
        statuses.sort();
        assert_eq!(statuses, vec![200, 409]);
    }
    let first = login(&base).await?;
    let second = login(&base).await?;
    let a = Live::connect(&first, &[]).await?;
    let b = Live::connect(&second, &[]).await?;
    let queries = ["SELECT * FROM me", "SELECT * FROM my_files"];
    let (subscribed_a, subscribed_b) = tokio::join!(a.subscribe(&queries), b.subscribe(&queries));
    subscribed_a?;
    subscribed_b?;
    assert_eq!(a.db.me().count(), 1);
    assert_eq!(b.db.me().count(), 1);
    assert!(procedure(|done| a.procedures.service_account_then("admin".into(), done)).await.is_err());
    reducer(|done| a.reducers.create_folder_then(uuid(), String::new(), format!("Proof {}", chrono::Utc::now().timestamp_millis()), done)).await?;
    assert!(a.db.my_files().count() > 0);
    let (removed, gone) = tokio::sync::oneshot::channel();
    let removed = std::sync::Mutex::new(Some(removed));
    a.db.me().on_delete(move |_, _| if let Some(sender) = removed.lock().unwrap().take() { let _ = sender.send(()); });
    assert_eq!(first.request("POST", "/api/v1/auth/logout").send().await?.status().as_u16(), 200);
    tokio::time::timeout(std::time::Duration::from_secs(10), gone).await.context("the session's rows were not removed")?.ok();
    assert_eq!(a.db.me().count(), 0);
    assert_eq!(a.db.my_files().count(), 0);
    assert_eq!(b.db.me().count(), 1);
    assert!(reducer(|done| a.reducers.create_folder_then(uuid(), String::new(), "Denied".into(), done)).await.is_err());
    assert_eq!(first.request("POST", "/api/v1/auth/token").send().await?.status().as_u16(), 401);
    Ok(())
}
