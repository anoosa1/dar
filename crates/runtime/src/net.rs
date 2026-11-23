//! Outbound fetches for feeds and calendar subscriptions (`runtime/news/net.ts`): GET with
//! redirects re-checked at every hop, never reaching private or local networks unless the
//! operator allows it. Addresses are resolved and checked here, then the transfer is pinned to
//! them (`curl --resolve`), so a DNS answer cannot change between the check and the connection.
//! `curl` does TLS and decompression within the size limit; no cookies or credentials are sent.

use std::net::IpAddr;
use std::time::Duration;

use crate::error::{AppError, Result};

const AGENT: &str = "Dar-News/1.0 (feed reader)";

pub struct FetchOptions<'a> {
    pub accept: &'a str,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub max_bytes: u64,
    pub timeout: Duration,
    pub allow_private: bool,
}

impl Default for FetchOptions<'_> {
    fn default() -> Self {
        FetchOptions { accept: "*/*", etag: None, last_modified: None, max_bytes: 5 * 1024 * 1024, timeout: Duration::from_secs(20), allow_private: false }
    }
}

pub struct FetchResult {
    pub status: u16,
    pub url: String,
    pub content_type: String,
    pub etag: String,
    pub last_modified: String,
    pub body: Vec<u8>,
}

fn in_v4(a: [u8; 4], net: [u8; 4], bits: u32) -> bool {
    let (a, net) = (u32::from_be_bytes(a), u32::from_be_bytes(net));
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    a & mask == net & mask
}

fn in_v6(a: [u16; 8], net: [u16; 8], bits: u32) -> bool {
    let to = |s: [u16; 8]| s.iter().fold(0u128, |acc, p| (acc << 16) | *p as u128);
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    to(a) & mask == to(net) & mask
}

/// Whether an address is public: not private, loopback, link-local, documentation or reserved.
/// IPv4-mapped IPv6 addresses are checked as the IPv4 address they carry.
pub fn public_address(address: IpAddr) -> bool {
    const V4: [([u8; 4], u32); 14] = [
        ([0, 0, 0, 0], 8), ([10, 0, 0, 0], 8), ([100, 64, 0, 0], 10), ([127, 0, 0, 0], 8), ([169, 254, 0, 0], 16), ([172, 16, 0, 0], 12), ([192, 0, 0, 0], 24),
        ([192, 0, 2, 0], 24), ([192, 168, 0, 0], 16), ([198, 18, 0, 0], 15), ([198, 51, 100, 0], 24), ([203, 0, 113, 0], 24), ([224, 0, 0, 0], 4), ([240, 0, 0, 0], 4),
    ];
    const V6: [([u16; 8], u32); 8] = [
        ([0; 8], 128), ([0, 0, 0, 0, 0, 0, 0, 1], 128), ([0x100, 0, 0, 0, 0, 0, 0, 0], 64), ([0x2001, 0, 0, 0, 0, 0, 0, 0], 32), ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
        ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7), ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10), ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
    ];
    match address {
        IpAddr::V4(v4) => !V4.iter().any(|(net, bits)| in_v4(v4.octets(), *net, *bits)),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_address(IpAddr::V4(v4)),
            None => !V6.iter().any(|(net, bits)| in_v6(v6.segments(), *net, *bits)),
        },
    }
}

fn not_allowed() -> AppError {
    AppError::with("addressNotAllowed", 400, "That address points to a private or local network.")
}

/// A URL resolved against a base (`new URL(location, base)`).
fn join(base: &str, location: &str) -> Option<String> {
    reqwest::Url::parse(base).ok()?.join(location).ok().map(|u| u.to_string())
}

/// GET with redirects re-checked at every hop.
pub async fn safe_fetch(input: &str, options: &FetchOptions<'_>) -> Result<FetchResult> {
    let mut current = input.to_owned();
    for _ in 0..6 {
        let result = once(&current, options).await?;
        if [301, 302, 303, 307, 308].contains(&result.0.status) {
            let location = result.1;
            if location.is_empty() {
                return Err(AppError::with("feedUnavailable", 502, "The site redirected without a destination."));
            }
            current = join(&current, &location).ok_or_else(|| AppError::with("feedUnavailable", 502, "The site redirected without a destination."))?;
            continue;
        }
        return Ok(FetchResult { url: current, ..result.0 });
    }
    Err(AppError::with("feedUnavailable", 502, "The site redirected too many times."))
}

async fn once(target: &str, options: &FetchOptions<'_>) -> Result<(FetchResult, String)> {
    let url = reqwest::Url::parse(target).map_err(|_| AppError::with("invalidInput", 400, "Enter a web address starting with http:// or https://."))?;
    if !["http", "https"].contains(&url.scheme()) || !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::with("invalidInput", 400, "Only http and https addresses without credentials are supported."));
    }
    let host = url.host_str().ok_or_else(|| AppError::with("invalidInput", 400, "Enter a web address starting with http:// or https://."))?.trim_start_matches('[').trim_end_matches(']').to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| AppError::with("feedUnavailable", 502, "The site could not be reached (ENOTFOUND)."))?.map(|a| a.ip()).collect(),
    };
    let allowed: Vec<IpAddr> = addresses.into_iter().filter(|a| options.allow_private || public_address(*a)).collect();
    let Some(address) = allowed.first() else { return Err(not_allowed()) };
    let work = tempdir().await?;
    let (headers_file, body_file) = (work.join("headers"), work.join("body"));
    let pin = match address {
        IpAddr::V4(a) => format!("{host}:{port}:{a}"),
        IpAddr::V6(a) => format!("{host}:{port}:[{a}]"),
    };
    let mut command = tokio::process::Command::new("curl");
    command
        .args(["--silent", "--show-error", "--proto", "=http,https", "--max-redirs", "0", "--compressed", "--globoff"])
        .arg("--max-time")
        .arg(options.timeout.as_secs().max(1).to_string())
        .arg("--max-filesize")
        .arg(options.max_bytes.to_string())
        .arg("--resolve")
        .arg(&pin)
        .args(["-H", &format!("user-agent: {AGENT}"), "-H", &format!("accept: {}", options.accept)])
        .arg("-D")
        .arg(&headers_file)
        .arg("-o")
        .arg(&body_file);
    if let Some(etag) = options.etag.filter(|e| !e.is_empty()) {
        command.args(["-H", &format!("if-none-match: {etag}")]);
    }
    if let Some(modified) = options.last_modified.filter(|e| !e.is_empty()) {
        command.args(["-H", &format!("if-modified-since: {modified}")]);
    }
    command.arg("--").arg(url.as_str()).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let output = command.output().await.map_err(|_| AppError::with("feedUnavailable", 502, "The site could not be reached (curl is missing)."));
    let read = async {
        let output = output?;
        match output.status.code() {
            Some(0) => {}
            Some(28) => return Err(AppError::with("feedTimedOut", 504, "The site did not respond in time.")),
            Some(63) => return Err(AppError::with("feedTooLarge", 413, "The response is too large.")),
            Some(code) => return Err(AppError::with("feedUnavailable", 502, &format!("The site could not be reached (curl error {code})."))),
            None => return Err(AppError::with("feedUnavailable", 502, "The site could not be reached.")),
        }
        let headers = tokio::fs::read_to_string(&headers_file).await.unwrap_or_default();
        let body = tokio::fs::read(&body_file).await.unwrap_or_default();
        if body.len() as u64 > options.max_bytes {
            return Err(AppError::with("feedTooLarge", 413, "The response is too large."));
        }
        // The last header block is the response's (a 100 Continue may precede it).
        let block = headers.trim_end().rsplit("\r\n\r\n").next().unwrap_or_default();
        let mut lines = block.lines();
        let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut header = std::collections::HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                header.insert(k.trim().to_ascii_lowercase(), v.trim().chars().take(500).collect::<String>());
            }
        }
        let get = |name: &str| header.get(name).cloned().unwrap_or_default();
        let redirect = (300..400).contains(&status) || status == 304;
        Ok((
            FetchResult { status, url: target.to_owned(), content_type: if redirect { String::new() } else { get("content-type").to_lowercase() }, etag: get("etag"), last_modified: get("last-modified"), body: if redirect { Vec::new() } else { body } },
            if redirect { get("location") } else { String::new() },
        ))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    read
}

async fn tempdir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("dar-fetch-{}", crate::auth::uuid()));
    tokio::fs::DirBuilder::new().mode(0o700).create(&dir).await.map_err(|_| AppError::internal())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for private in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "169.254.1.1", "::1", "fe80::1", "fd00::1", "::ffff:127.0.0.1", "100.64.0.1"] {
            assert!(!public_address(private.parse().unwrap()), "{private}");
        }
        for public in ["93.184.216.34", "2606:4700::1111", "::ffff:8.8.8.8"] {
            assert!(public_address(public.parse().unwrap()), "{public}");
        }
        assert_eq!(join("https://a.example/feed/x.xml", "../y.xml").as_deref(), Some("https://a.example/y.xml"));
    }
}
