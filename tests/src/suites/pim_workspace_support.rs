//! Helpers shared by the PIM and Workspace acceptance suites (HTTP/DAV helpers, pure-Rust
//! SHA-1/MD5 digests for WebDAV checksum assertions, chunked HTTP/1.1 PUT for streamed uploads,
//! date helpers, and SpacetimeDB CLI helpers for module upgrades).

#![allow(dead_code)]

use dar_acceptance::prelude::*;
use dar_acceptance::session::{self, Reply};
use dar_acceptance::stack::stack;
use std::process::{Command, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Formats a PIM/Workspace row's strong ETag (`"<id>:<revision>"`).
pub fn etag(row: &Value) -> String {
    format!(
        "\"{}:{}\"",
        row["id"].as_str().unwrap_or_default(),
        row["revision"].as_u64().unwrap_or_default()
    )
}

/// Formats an HTTP Basic authorization header value (`Basic <base64>`).
pub fn basic_auth(username: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        dar_acceptance::base64(format!("{username}:{secret}").as_bytes())
    )
}

/// Percent-encodes a query parameter or path segment (like JS `encodeURIComponent`).
pub fn encode_uri_component(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

/// Creates a scoped app password via `/api/v1/auth/app-passwords` and returns its JSON payload.
pub async fn app_password(session: &Session, label: &str, scope: &str) -> Result<Value> {
    let reply = session
        .post(
            "/api/v1/auth/app-passwords",
            &json!({"label": label, "scope": scope}),
        )
        .await?
        .expect(201)?;
    reply.json()
}

/// Sends an authenticated JSON request with optional body and `If-Match`, asserting `expected`.
pub async fn json_request(
    session: &Session,
    path: &str,
    method: &str,
    body: Option<&Value>,
    if_match: Option<&str>,
    expected: u16,
) -> Result<Reply> {
    let mut req = session
        .request(method, path)
        .header("content-type", "application/json");
    if let Some(tag) = if_match {
        req = req.header("if-match", tag);
    }
    if let Some(body) = body {
        req = req.body(body.to_string());
    }
    let reply = Reply::from(req.send().await?).await?;
    ensure!(
        reply.status == expected,
        "{method} {path}: expected {expected}, got {}: {}",
        reply.status,
        reply.text()
    );
    Ok(reply)
}

/// Sends a raw HTTP/DAV request (without cookies or automatic redirects).
pub async fn dav_request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Result<Reply> {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| Error::new(format!("invalid HTTP method {method}: {e}")))?;
    let mut req = session::client().request(method, url);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    if let Some(body) = body {
        req = req.body(body);
    }
    Reply::from(req.send().await?).await
}

/// Sends an HTTP/1.1 chunked PUT request (without `Content-Length`) over a raw TCP stream.
/// Used to exercise Finder-style streamed uploads with `X-Expected-Entity-Length` and `411`.
pub async fn chunked_put(
    url_str: &str,
    headers: &[(&str, &str)],
    chunks: &[&[u8]],
) -> Result<(u16, String)> {
    let parsed = url::Url::parse(url_str)?;
    let host = parsed.host_str().context("URL has no host")?;
    let port = parsed.port_or_known_default().context("URL has no port")?;
    let path = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let mut stream = TcpStream::connect((host, port)).await?;
    let mut head = format!(
        "PUT {path} HTTP/1.1\r\nHost: {host}:{port}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(chunk).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let text = String::from_utf8_lossy(&response).into_owned();
    let status_line = text.lines().next().context("empty HTTP response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .context("missing HTTP status code")?
        .parse()
        .map_err(|e| Error::new(format!("bad status in '{status_line}': {e}")))?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    Ok((status, body))
}

/// Parses an RFC 3339 timestamp string into UTC milliseconds since epoch.
pub fn parse_rfc3339_ms(input: &str) -> Result<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(input)
        .map_err(|e| Error::new(format!("parsing timestamp '{input}': {e}")))?;
    Ok(dt.timestamp_millis())
}

/// Parses an HTTP-date (IMF-fixdate / RFC 2822) into UTC milliseconds since epoch.
pub fn parse_http_date_ms(input: &str) -> Result<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(input) {
        return Ok(dt.timestamp_millis());
    }
    let naive = chrono::NaiveDateTime::parse_from_str(input.trim(), "%a, %d %b %Y %H:%M:%S GMT")
        .map_err(|e| Error::new(format!("parsing HTTP date '{input}': {e}")))?;
    Ok(naive.and_utc().timestamp_millis())
}

/// Runs `spacetime --root-dir <cli_root> <args>` with `DAR_ISSUER=<issuer>` and asserts exit 0.
pub fn run_spacetime(args: &[&str], issuer: &str) -> Result<String> {
    let s = stack();
    let output = Command::new(&s.artifacts.spacetime)
        .arg("--root-dir")
        .arg(&s.database.cli_root)
        .args(args)
        .env("DAR_ISSUER", issuer)
        .stdin(Stdio::null())
        .output()
        .context("running spacetime CLI")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("spacetime {args:?} failed:\n{combined}");
    }
    Ok(combined)
}

/// Runs a `git` command inside the repository root and returns stdout bytes.
pub fn run_git(args: &[&str]) -> Result<Vec<u8>> {
    let repo = &stack().artifacts.repo;
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!(
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

/// Computes lowercase hex SHA-1 (RFC 3174) for WebDAV checksum verification.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    format!(
        "{:08x}{:08x}{:08x}{:08x}{:08x}",
        h[0], h[1], h[2], h[3], h[4]
    )
}

/// Computes lowercase hex MD5 (RFC 1321) for WebDAV checksum verification.
pub fn md5_hex(data: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for block in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::with_capacity(32);
    for word in [a0, b0, c0, d0] {
        for byte in word.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}
