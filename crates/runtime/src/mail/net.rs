//! Connections to mail servers: TLS from the start or after STARTTLS, line-oriented reads with
//! limits, and a small HTTP/1.1 client (JMAP, OAuth token endpoints) on the same TLS stack.
//! Servers come only from administrator-configured providers. Certificates are checked against
//! the system's trust store (`SSL_CERT_FILE` and `SSL_CERT_DIR` are honoured).

use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

pub const TIMEOUT: Duration = Duration::from_secs(60);
const LINE_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
pub struct NetError(pub String);

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type NetResult<T> = std::result::Result<T, NetError>;

pub fn err<T>(message: impl Into<String>) -> NetResult<T> {
    Err(NetError(message.into()))
}

fn config(alpn: &[&[u8]]) -> NetResult<Arc<rustls::ClientConfig>> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| b.with_platform_verifier())
        .map_err(|e| NetError(format!("TLS could not be set up: {e}")))?
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(config))
}

fn plain_config() -> NetResult<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(c) = CONFIG.get() {
        return Ok(c.clone());
    }
    let c = config(&[])?;
    Ok(CONFIG.get_or_init(|| c).clone())
}

fn http_config() -> NetResult<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(c) = CONFIG.get() {
        return Ok(c.clone());
    }
    let c = config(&[b"http/1.1"])?;
    Ok(CONFIG.get_or_init(|| c).clone())
}

async fn tcp(host: &str, port: u16) -> NetResult<TcpStream> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, port))).await {
        Ok(Ok(s)) => {
            let _ = s.set_nodelay(true);
            Ok(s)
        }
        Ok(Err(e)) => err(format!("{host}:{port} could not be reached ({})", e.kind())),
        Err(_) => err(format!("{host}:{port} did not answer in time")),
    }
}

async fn handshake(
    stream: TcpStream,
    host: &str,
    config: Arc<rustls::ClientConfig>,
) -> NetResult<TlsStream<TcpStream>> {
    let name = rustls::pki_types::ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    )
    .map_err(|_| NetError(format!("{host} is not a valid server name")))?;
    match tokio::time::timeout(
        Duration::from_secs(20),
        TlsConnector::from(config).connect(name, stream),
    )
    .await
    {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => err(format!("A secure connection to {host} failed: {e}")),
        Err(_) => err(format!("A secure connection to {host} timed out")),
    }
}

/// A plain stream before STARTTLS, a TLS stream after (or from the start).
pub enum Io {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    Taken,
}

impl AsyncRead for Io {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            Io::Taken => Poll::Ready(Err(std::io::ErrorKind::NotConnected.into())),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            Io::Taken => Poll::Ready(Err(std::io::ErrorKind::NotConnected.into())),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_flush(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            Io::Taken => Poll::Ready(Ok(())),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            Io::Taken => Poll::Ready(Ok(())),
        }
    }
}

/// A buffered, line-oriented connection to an IMAP or SMTP server.
pub struct Conn {
    io: BufReader<Io>,
    host: String,
}

impl Conn {
    /// Connects; with `tls` the stream is encrypted from the start, otherwise the caller must
    /// upgrade with `starttls` before sending anything secret.
    pub async fn open(host: &str, port: u16, tls: bool) -> NetResult<Conn> {
        let stream = tcp(host, port).await?;
        let io = if tls {
            Io::Tls(Box::new(handshake(stream, host, plain_config()?).await?))
        } else {
            Io::Plain(stream)
        };
        Ok(Conn {
            io: BufReader::new(io),
            host: host.to_owned(),
        })
    }

    pub async fn starttls(&mut self) -> NetResult<()> {
        if !self.io.buffer().is_empty() {
            return err("The server sent data before the secure connection started");
        }
        let io = std::mem::replace(self.io.get_mut(), Io::Taken);
        let Io::Plain(stream) = io else {
            return err("The connection is already secure");
        };
        *self.io.get_mut() = Io::Tls(Box::new(
            handshake(stream, &self.host, plain_config()?).await?,
        ));
        Ok(())
    }

    pub fn secure(&self) -> bool {
        matches!(self.io.get_ref(), Io::Tls(_))
    }

    /// One line without its CRLF.
    pub async fn line(&mut self) -> NetResult<Vec<u8>> {
        let mut out = Vec::new();
        let read = async {
            loop {
                let available = self.io.fill_buf().await.map_err(|e| {
                    NetError(format!("The server connection failed ({})", e.kind()))
                })?;
                if available.is_empty() {
                    return err("The server closed the connection");
                }
                if let Some(end) = available.iter().position(|b| *b == b'\n') {
                    out.extend_from_slice(&available[..end]);
                    self.io.consume(end + 1);
                    break;
                }
                let n = available.len();
                out.extend_from_slice(available);
                self.io.consume(n);
                if out.len() > LINE_LIMIT {
                    return err("The server sent an overlong line");
                }
            }
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            Ok(())
        };
        tokio::time::timeout(TIMEOUT, read)
            .await
            .map_err(|_| NetError("The server stopped answering".into()))??;
        Ok(out)
    }

    pub async fn exact(&mut self, n: usize) -> NetResult<Vec<u8>> {
        let mut out = vec![0u8; n];
        tokio::time::timeout(TIMEOUT * 4, self.io.read_exact(&mut out))
            .await
            .map_err(|_| NetError("The server stopped sending".into()))?
            .map_err(|e| NetError(format!("The server connection failed ({})", e.kind())))?;
        Ok(out)
    }

    pub async fn send(&mut self, data: &[u8]) -> NetResult<()> {
        let io = self.io.get_mut();
        tokio::time::timeout(TIMEOUT * 4, async {
            io.write_all(data).await?;
            io.flush().await
        })
        .await
        .map_err(|_| NetError("The server stopped receiving".into()))?
        .map_err(|e| NetError(format!("The server connection failed ({})", e.kind())))
    }
}

// ---------- HTTP ----------

pub struct HttpResponse {
    pub status: u16,
    pub headers: axum::http::HeaderMap,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> NetResult<serde_json::Value> {
        serde_json::from_slice(&self.body)
            .map_err(|_| NetError(format!("The server answered {} without JSON", self.status)))
    }
}

/// One request without redirects; `max` bounds the response body.
pub async fn http(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
    max: usize,
) -> NetResult<HttpResponse> {
    let parsed =
        reqwest::Url::parse(url).map_err(|_| NetError(format!("{url} is not a valid address")))?;
    let tls = match parsed.scheme() {
        "https" => true,
        "http" => false,
        _ => return err("Only http and https addresses are supported"),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return err("Server addresses may not carry credentials");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| NetError(format!("{url} has no host")))?
        .to_owned();
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if tls { 443 } else { 80 });
    let stream = tcp(&host, port).await?;
    let authority = match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.clone(),
    };
    let mut target = parsed.path().to_owned();
    if let Some(q) = parsed.query() {
        target.push('?');
        target.push_str(q);
    }
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(target)
        .header("host", authority)
        .header("user-agent", "Dar-Mail/1.0");
    for (k, v) in headers {
        request = request.header(*k, v.as_str());
    }
    let request = request
        .body(Body::from(body))
        .map_err(|_| NetError("The request could not be built".into()))?;
    let exchange = async {
        let io = if tls {
            Io::Tls(Box::new(handshake(stream, &host, http_config()?).await?))
        } else {
            Io::Plain(stream)
        };
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake::<_, Body>(hyper_util::rt::TokioIo::new(io))
                .await
                .map_err(|e| NetError(format!("{host} did not speak HTTP: {e}")))?;
        let driver = tokio::spawn(async move {
            let _ = conn.await;
        });
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| NetError(format!("{host} did not answer: {e}")));
        let result = match response {
            Ok(response) => {
                let (parts, incoming) = response.into_parts();
                match axum::body::to_bytes(Body::new(incoming), max).await {
                    Ok(bytes) => Ok(HttpResponse {
                        status: parts.status.as_u16(),
                        headers: parts.headers,
                        body: bytes.to_vec(),
                    }),
                    Err(_) => err(format!("{host} sent too much or stopped sending")),
                }
            }
            Err(e) => Err(e),
        };
        driver.abort();
        result
    };
    tokio::time::timeout(TIMEOUT * 4, exchange)
        .await
        .map_err(|_| NetError(format!("{host} did not answer in time")))?
}

/// GET or POST, following up to three redirects (GET only).
pub async fn http_follow(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
    max: usize,
) -> NetResult<(String, HttpResponse)> {
    let mut current = url.to_owned();
    for _ in 0..4 {
        let response = http(method, &current, headers, body.clone(), max).await?;
        if method == "GET" && matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            let next = reqwest::Url::parse(&current)
                .ok()
                .and_then(|base| base.join(location).ok())
                .ok_or_else(|| NetError("The server redirected nowhere".into()))?;
            current = next.to_string();
            continue;
        }
        return Ok((current, response));
    }
    err("The server redirected too many times")
}

pub fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn unb64(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()
}
