//! HTTP hardening (`http-security.integration.test.ts`): CSRF and origin rejection, operator
//! password recovery that revokes sessions, a UTF-8 character split across network writes,
//! body limits, uniform login errors and the login failure budget per peer.

use dar_acceptance::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SUITE: Suite = Suite {
    name: "http-security",
    cases: &[case!(limits_csrf_recovery_and_login_budget, 30)],
};

/// Sends a raw HTTP/1.1 POST in two writes (split at `at`) from `local` and returns the status.
async fn post_raw(
    port: u16,
    local: &str,
    path: &str,
    origin: &str,
    body: &[u8],
    at: usize,
) -> Result<u16> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.bind(format!("{local}:0").parse().unwrap())?;
    let mut stream = socket
        .connect(format!("127.0.0.1:{port}").parse().unwrap())
        .await?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\ncontent-type: application/json\r\norigin: {origin}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body[..at]).await?;
    stream.flush().await?;
    tokio::task::yield_now().await;
    stream.write_all(&body[at..]).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8_lossy(&response)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

pub async fn limits_csrf_recovery_and_login_budget(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let admin = login(&base).await?;
    let username = format!("recovery-{}", &uuid()[..8]);
    let client = session::client();
    let token = |origin: &str, csrf: bool| {
        let mut request = client
            .post(format!("{base}/api/v1/auth/token"))
            .header("cookie", &admin.cookie)
            .header("origin", origin);
        if csrf {
            request = request.header("x-csrf-token", &admin.csrf_token);
        }
        request.send()
    };
    assert_eq!(token(&base, false).await?.status().as_u16(), 403);
    assert_eq!(
        token("https://unrelated.example", true)
            .await?
            .status()
            .as_u16(),
        403
    );
    admin.post("/api/v1/auth/users", &json!({"username": username, "displayName": "Recovery proof", "password": "initial recovery password"})).await?.expect(201)?;
    let old = login_as(&base, &username, "initial recovery password").await?;
    let connection = old.connect().await?;
    let temporary = t.temp("recovery")?;
    let password = "  a recovered Unicode password 🔐  ";
    let file = temporary.join("password");
    dar_acceptance::stack::write_private(&file, format!("{password}\n").as_bytes())?;
    let (removed, gone) = tokio::sync::oneshot::channel();
    let removed = std::sync::Mutex::new(Some(removed));
    connection.db.me().on_delete(move |_, _| {
        if let Some(sender) = removed.lock().unwrap().take() {
            let _ = sender.send(());
        }
    });
    support::operator(
        Some(&instance.database()?),
        Some(instance.root()),
        &["recover-password", &username, &file.to_string_lossy()],
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(10), gone)
        .await
        .context("recovery did not revoke the live session")?
        .ok();
    assert_eq!(old.get("/api/v1/auth/session").await?.status, 401);
    // A four-byte UTF-8 character split across network writes: the body is decoded only once
    // complete, so the credentials stay byte for byte.
    let body = serde_json::to_vec(&json!({"username": username, "password": password}))?;
    let at = body
        .windows(4)
        .position(|w| w == "🔐".as_bytes())
        .context("the emoji")?
        + 2;
    assert_eq!(
        post_raw(
            instance.port,
            "127.0.0.1",
            "/api/v1/auth/login",
            &base,
            &body,
            at
        )
        .await?,
        200
    );
    let oversized = client
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", &base)
        .json(&json!({"username": username, "password": "x".repeat(20_000)}))
        .send()
        .await?;
    assert_eq!(oversized.status().as_u16(), 413);
    let mut errors = Vec::new();
    for name in [username.clone(), format!("missing-{}", uuid())] {
        let response = client
            .post(format!("{base}/api/v1/auth/login"))
            .header("origin", &base)
            .json(&json!({"username": name, "password": "wrong password"}))
            .send()
            .await?;
        assert_eq!(response.status().as_u16(), 401);
        errors.push(response.json::<Value>().await?);
    }
    assert_eq!(errors[0], errors[1]);
    // A fresh peer (127.0.0.2), independent of the suite's other sign-ins.
    let wrong = serde_json::to_vec(&json!({"username": username, "password": "wrong password"}))?;
    let mut limited = false;
    for _ in 0..11 {
        let status = post_raw(
            instance.port,
            "127.0.0.2",
            "/api/v1/auth/login",
            &base,
            &wrong,
            wrong.len(),
        )
        .await?;
        if status == 429 {
            limited = true;
            break;
        }
        assert_eq!(status, 401);
    }
    assert!(limited);
    Ok(())
}
