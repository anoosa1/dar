//! Production NixOS nginx proxy (`production-proxy.mjs`): builds the actual NixOS module's
//! generated nginx config, runs TLS nginx + SpacetimeDB + Dar runtime, verifies blocked DB
//! routes, first-account setup, SDK websocket-token exchange & WebSocket subscriptions/reconnect,
//! persisted Files/Calendar writes, desktop/phone connection failure & retry, and peer IP
//! rate-limiting isolation.

use dar_acceptance::prelude::*;
use dar_acceptance::stack::{free_port, stack, tail, terminate, write_private};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SUITE: Suite = Suite {
    name: "production-proxy",
    cases: &[case!(generated_nixos_nginx_proxy, 240)],
};

fn run_cmd(
    executable: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
) -> Result<String> {
    let mut cmd = Command::new(executable);
    cmd.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .context(format!("running {}", executable.display()))?;
    ensure!(
        output.status.success(),
        "{} {:?} failed:\n{}{}",
        executable.display(),
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

struct ChildGuard(Option<Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.take() {
            terminate(child, Duration::from_secs(10));
        }
    }
}

pub async fn generated_nixos_nginx_proxy(t: &Ctx) -> Result<()> {
    let stack = stack();
    let project = &stack.artifacts.repo;
    let root = t.temp("dar-production-proxy")?;
    let port = free_port()?;
    let origin = format!("https://127.0.0.1:{port}");
    let name = format!("dar-proxy-{}", &uuid()[..8]);

    let system = if cfg!(target_arch = "aarch64") {
        "aarch64-linux"
    } else {
        "x86_64-linux"
    };
    let nix_bin = dar_acceptance::which("nix").context("nix executable")?;
    let fixture = std::path::PathBuf::from(run_cmd(
        &nix_bin,
        &[
            "build",
            &format!(".#checks.{system}.module-evaluation"),
            "--no-link",
            "--print-out-paths",
        ],
        Some(project),
        &[],
    )?);

    let cert = root.join("cert.pem");
    let key = root.join("key.pem");
    run_cmd(
        &fixture.join("openssl"),
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            &key.to_string_lossy(),
            "-out",
            &cert.to_string_lossy(),
            "-days",
            "1",
            "-subj",
            "/CN=127.0.0.1",
            "-addext",
            "subjectAltName=IP:127.0.0.1",
        ],
        None,
        &[],
    )?;

    // Isolated SpacetimeDB server trusting only this fixture's HTTPS issuer cert.
    let db_port = free_port()?;
    let db_url = format!("http://127.0.0.1:{db_port}");
    let cli_root = root.join("cli");
    let db_data = root.join("database");
    let db_log = root.join("database.log");
    std::fs::create_dir_all(&db_data)?;
    let db_log_file = std::fs::File::create(&db_log)?;
    let mut database = ChildGuard(Some(
        Command::new(&stack.artifacts.spacetime)
            .arg("--root-dir")
            .arg(&cli_root)
            .args(["start", "--data-dir"])
            .arg(&db_data)
            .args([
                "--listen-addr",
                &format!("127.0.0.1:{db_port}"),
                "--non-interactive",
            ])
            .env("SSL_CERT_FILE", &cert)
            .stdin(Stdio::null())
            .stdout(db_log_file.try_clone()?)
            .stderr(db_log_file)
            .spawn()
            .context("starting fixture SpacetimeDB")?,
    ));

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(child) = database.0.as_mut()
            && let Some(status) = child.try_wait()?
        {
            bail!("fixture SpacetimeDB stopped ({status}):\n{}", tail(&db_log));
        }
        if let Ok(resp) = session::client()
            .get(format!("{db_url}/v1/ping"))
            .timeout(Duration::from_millis(500))
            .send()
            .await
            && resp.status().is_success()
        {
            break;
        }
        if Instant::now() > deadline {
            bail!(
                "fixture SpacetimeDB did not become ready:\n{}",
                tail(&db_log)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    run_cmd(
        &stack.artifacts.spacetime,
        &[
            "--root-dir",
            &cli_root.to_string_lossy(),
            "publish",
            &name,
            "--bin-path",
            &stack.artifacts.module.to_string_lossy(),
            "--server",
            &db_url,
            "--yes",
            "--no-config",
        ],
        None,
        &[("DAR_ISSUER", &format!("{origin}/auth/issuer"))],
    )?;

    let cli_toml = dar_acceptance::read_to_string(cli_root.join("config/cli.toml"))?;
    let token = cli_toml
        .lines()
        .find_map(|line| {
            line.trim().strip_prefix("spacetimedb_token").map(|r| {
                r.trim_start_matches([' ', '='])
                    .trim()
                    .trim_matches('"')
                    .to_owned()
            })
        })
        .filter(|t| !t.is_empty())
        .context("spacetimedb_token")?;
    let service_token_file = root.join("database-token");
    let bootstrap_token_file = root.join("bootstrap-token");
    write_private(&service_token_file, token.as_bytes())?;
    write_private(
        &bootstrap_token_file,
        dar_acceptance::base64url(&dar_acceptance::random_bytes(32)).as_bytes(),
    )?;

    let runtime = Instance::start(InstanceConfig {
        port: 0,
        origin: Some(origin.clone()),
        root: root.join("state"),
        database_url: Some(db_url.clone()),
        database_name: name.clone(),
        service_token_file: Some(service_token_file),
        bootstrap_token_file: Some(bootstrap_token_file.clone()),
        development: false,
        ..InstanceConfig::default()
    })
    .await?;
    let backend = format!("http://127.0.0.1:{}", runtime.port);

    let mut config = dar_acceptance::read_to_string(fixture.join("nginx.conf"))?;
    let http_server_re =
        regex::Regex::new(r"(?s)    server \{\n        listen 0\.0\.0\.0:80;.*?\n    \}\n")?;
    config = http_server_re.replace(&config, "").into_owned();
    let temps = ["client_body", "proxy", "fastcgi", "uwsgi", "scgi"]
        .iter()
        .map(|k| format!("{k}_temp_path {}/{k};", root.display()))
        .collect::<Vec<_>>()
        .join("\n");
    config = config
        .replace(
            "listen 0.0.0.0:443 ssl;",
            &format!("listen 127.0.0.1:{port} ssl;"),
        )
        .replace("listen [::0]:443 ssl;", "")
        .replace(
            "/var/lib/acme/dar.example.test/fullchain.pem",
            &cert.to_string_lossy(),
        )
        .replace(
            "/var/lib/acme/dar.example.test/key.pem",
            &key.to_string_lossy(),
        )
        .replace(
            "/var/lib/acme/dar.example.test/chain.pem",
            &cert.to_string_lossy(),
        )
        .replace("dar.example.test", &format!("127.0.0.1:{port}"))
        .replace("http://127.0.0.1:4311", &backend)
        .replace("http://127.0.0.1:4310", &db_url)
        .replace(
            "/v1/database/dar/subscribe",
            &format!("/v1/database/{name}/subscribe"),
        )
        .replace(
            "pid /run/nginx/nginx.pid;",
            &format!("pid {}/nginx.pid;", root.display()),
        )
        .replace("http {", &format!("http {{\naccess_log off;\n{temps}"));
    std::fs::create_dir_all(root.join("logs"))?;
    let config_file = root.join("nginx.conf");
    std::fs::write(&config_file, &config)?;

    let nginx_log = root.join("nginx.log");
    let nginx_log_file = std::fs::File::create(&nginx_log)?;
    let mut proxy = ChildGuard(Some(
        Command::new(fixture.join("nginx"))
            .args([
                "-p",
                &format!("{}/", root.display()),
                "-c",
                &config_file.to_string_lossy(),
            ])
            .stdin(Stdio::null())
            .stdout(nginx_log_file.try_clone()?)
            .stderr(nginx_log_file)
            .spawn()
            .context("starting nginx")?,
    ));

    #[derive(Debug)]
    struct PinnedCertVerifier {
        expected: Vec<u8>,
    }
    impl rustls::client::danger::ServerCertVerifier for PinnedCertVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            if end_entity.as_ref() == self.expected.as_slice() {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            } else {
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::UnknownIssuer,
                ))
            }
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    let cert_pem = dar_acceptance::read_to_string(&cert)?;
    let cert_b64: String = cert_pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let cert_der = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(cert_b64)
            .map_err(|e| Error::new(e.to_string()))?
    };
    let tls_config = Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::new(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier { expected: cert_der }))
        .with_no_client_auth(),
    );
    let connector = tokio_rustls::TlsConnector::from(tls_config);

    let https_status = |method: &str, path: &str, body: Option<&str>| {
        let connector = connector.clone();
        let method = method.to_owned();
        let path = path.to_owned();
        let body = body.map(str::to_owned);
        async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
            let server_name = rustls::pki_types::ServerName::IpAddress(
                rustls::pki_types::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST.into()),
            );
            let mut tls = connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| Error::new(format!("TLS connect: {e}")))?;
            let payload = body.unwrap_or_default();
            let req = if payload.is_empty() && method == "GET" {
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                )
            } else {
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                )
            };
            tls.write_all(req.as_bytes()).await?;
            tls.flush().await?;
            let mut buf = Vec::new();
            let _ = tls.read_to_end(&mut buf).await;
            let text = String::from_utf8_lossy(&buf);
            let status: u16 = text
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| Error::new(format!("bad HTTP status from nginx: {text}")))?;
            Ok::<u16, Error>(status)
        }
    };

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions {
            ignore_https_errors: true,
            viewport: (1440, 950),
            ..ContextOptions::default()
        })
        .await?;

    let result = async {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_err: String;
        loop {
            if let Some(child) = proxy.0.as_mut()
                && let Some(status) = child.try_wait()?
            {
                bail!("nginx stopped ({status}):\n{}", tail(&nginx_log));
            }
            match https_status("GET", "/api/v1/auth/status", None).await {
                Ok(200) => break,
                Ok(code) => last_err = format!("HTTP {code}"),
                Err(e) => last_err = e.to_string(),
            }
            if Instant::now() > deadline {
                bail!("nginx did not become ready ({last_err}):\n{}", tail(&nginx_log));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        for route in [
            format!("/spacetime/v1/database/{name}/sql"),
            "/spacetime/v1/identity".into(),
            "/spacetime/v1/database/other/subscribe".into(),
        ] {
            assert_eq!(https_status("GET", &route, None).await?, 404, "{route}");
        }
        let exchange_path = "/spacetime/v1/identity/websocket-token";
        let exchange = format!("{origin}{exchange_path}");
        assert_eq!(https_status("GET", exchange_path, None).await?, 403);
        assert_eq!(https_status("POST", exchange_path, Some("{}")).await?, 401);

        let page = context.new_page().await?;
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let exchanges: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
        let err_sink = errors.clone();
        page.on_page_error(move |msg| err_sink.lock().unwrap().push(msg.clone()));
        let ex_sink = exchanges.clone();
        let ex_url = exchange.clone();
        page.on_response(move |resp| {
            if resp.url == ex_url {
                ex_sink.lock().unwrap().push(resp.status);
            }
        });

        page.track_sockets("/v1/database/").await?;
        page.goto(&origin).await?;
        page.get_by_label("Display name").fill("Proxy operator").await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        let bootstrap = dar_acceptance::read_to_string(&bootstrap_token_file)?;
        page.get_by_label(regex("^Instance setup key")).fill(bootstrap.trim()).await?;
        page.get_by_role("button", "Create your account").click().await?;
        page.get_by_role("heading", "No files").wait_for().await?;
        assert!(exchanges.lock().unwrap().contains(&200), "SDK must exchange its token through nginx before subscribing");

        page.get_by_role("button", "New").click().await?;
        page.get_by_role("menuitem", "Folder").click().await?;
        page.locator("[data-editor-pane]").get_by_label("Name").fill("Proxy persistence").await?;
        page.get_by_role("button", "Create folder").click().await?;
        page.get_by_role("button", "Proxy persistence").wait_for().await?;
        page.reload().await?;
        page.get_by_role("button", "Proxy persistence").wait_for().await?;

        page.goto(&format!("{origin}/calendar")).await?;
        page.get_by_role("heading", "No calendars").wait_for().await?;
        page.get_by_role("button", "Create calendar").click().await?;
        page.locator("[data-editor-pane]").get_by_label("Name").fill("Proxy calendar").await?;
        page.locator("[data-editor-pane]").get_by_role("button", "Save").click().await?;
        page.get_by_role("button", "Proxy calendar").wait_for().await?;

        let opened_before = page.sockets_opened().await?;
        assert!(opened_before >= 1);
        page.drop_sockets(4012, "Proxy reconnect proof").await?;
        page.get_by_role("status", ()).filter_has_text("Connection lost. Reconnecting…").wait_for().await?;
        page.wait_for_function(r#"() => !document.querySelector(".connection-state,.error-banner")"#).await?;
        assert!(page.sockets_opened().await? > opened_before);
        page.get_by_role("button", "Proxy calendar").wait_for().await?;

        // Reproduce the blocked exchange after login, then recover with the shared action.
        for (width, height) in [(1440u32, 950u32), (390, 844)] {
            page.set_viewport_size(width, height).await?;
            for app in ["files", "calendar"] {
                let route_id = page
                    .route("**/spacetime/v1/identity/websocket-token", None, |route| async move {
                        route.fulfill(Fulfill::text(503, "text/plain", "Unavailable")).await
                    })
                    .await?;
                let target = if app == "files" { format!("{origin}/") } else { format!("{origin}/calendar") };
                page.goto(&target).await?;
                let alert = page.get_by_role("alert", ()).filter_has_text("Unable to connect. Refresh to try again.");
                alert.wait_for().await?;
                assert_eq!(page.get_by_text("Connecting…").count().await?, 0);
                assert_eq!(page.locator(".file-skeleton,.pim-empty,.empty-state").count().await?, 0);
                page.screenshot(t.screenshot(&format!("proxy-{app}-{width}-error.png")), true).await?;
                page.unroute(route_id);
                alert.get_by_role("button", "Try again").click().await?;
                if app == "files" {
                    page.get_by_role("button", "Proxy persistence").wait_for().await?;
                } else {
                    page.locator(".calendar-month").wait_for().await?;
                    page.get_by_role("button", "Proxy calendar").include_hidden().wait_for_state(State::Attached).await?;
                }
                page.wait_for_function(r#"() => !document.querySelector('[role="alert"]') && !document.body.textContent.includes("Connecting…")"#).await?;
                assert_eq!(page.locator(".connection-state,.error-banner").count().await?, 0);
                assert_eq!(page.evaluate("() => document.documentElement.scrollWidth <= innerWidth").await?, json!(true), "No mobile overflow");
            }
        }
        assert!(errors.lock().unwrap().is_empty(), "page errors: {:?}", *errors.lock().unwrap());

        // nginx must overwrite client-supplied identity, and preserve separate peers.

        let bad_login = |local: &str, claimed: &str| {
            let origin = origin.clone();
            let connector = connector.clone();
            let local: std::net::Ipv4Addr = local.parse().unwrap();
            let claimed = claimed.to_owned();
            async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let socket = tokio::net::TcpSocket::new_v4()?;
                socket.bind(std::net::SocketAddr::from((local, 0)))?;
                let tcp = socket.connect(std::net::SocketAddr::from(([127, 0, 0, 1], port))).await?;
                let server_name = rustls::pki_types::ServerName::IpAddress(rustls::pki_types::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST.into()));
                let mut tls = connector.connect(server_name, tcp).await.map_err(|e| Error::new(format!("TLS connect: {e}")))?;
                let body = json!({ "username": "missing-proxy-probe", "password": "incorrect password" }).to_string();
                let req = format!(
                    "POST /api/v1/auth/login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nX-Dar-Client-IP: {claimed}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tls.write_all(req.as_bytes()).await?;
                tls.flush().await?;
                let mut buf = Vec::new();
                tls.read_to_end(&mut buf).await?;
                let text = String::from_utf8_lossy(&buf);
                let status: u16 = text
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse().ok())
                    .context("HTTP status line")?;
                Ok::<u16, Error>(status)
            }
        };

        for _ in 0..10 {
            assert_eq!(bad_login("127.0.0.2", "203.0.113.9").await?, 401);
        }
        assert_eq!(bad_login("127.0.0.2", "203.0.113.10").await?, 429);
        assert_eq!(bad_login("127.0.0.3", "203.0.113.9").await?, 401);

        Ok::<(), Error>(())
    }
    .await;

    context.close().await.ok();
    drop(proxy);
    runtime.close().await;
    drop(database);
    let _ = std::fs::remove_file(&key);
    result
}
