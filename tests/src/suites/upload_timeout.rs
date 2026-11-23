//! Large uploads on slow links (`upload-timeout.integration.test.ts`): a body that keeps moving
//! may take longer than the minute that used to cap every request, while slow request headers
//! are still cut off quickly.

use dar_acceptance::prelude::*;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SUITE: Suite = Suite { name: "upload-timeout", cases: &[case!(slow_upload_and_slow_headers, 180)] };

pub async fn slow_upload_and_slow_headers(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let session = login(&instance.base).await?;
    let (chunk, seconds) = (1024usize, 75usize);
    let reservation = session.post("/api/v1/files/uploads", &json!({"name": format!("slow-{}.bin", chrono::Utc::now().timestamp_millis()), "size": chunk * seconds, "mediaType": "application/octet-stream", "parentId": ""})).await?.json()?;
    let id = reservation["id"].as_str().context("reservation id")?.to_owned();
    let address = format!("127.0.0.1:{}", instance.port);
    let slow = async {
        // One byte pattern per second for 75 s over plain HTTP/1.1.
        let started = Instant::now();
        let mut stream = tokio::net::TcpStream::connect(&address).await?;
        let head = format!("PUT /api/v1/files/uploads/{id} HTTP/1.1\r\nHost: {address}\r\ncookie: {}\r\norigin: {}\r\nx-csrf-token: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            session.cookie, session.base, session.csrf_token, chunk * seconds);
        stream.write_all(head.as_bytes()).await?;
        for sent in 0..seconds {
            stream.write_all(&vec![(sent % 251) as u8; chunk]).await?;
            if sent + 1 < seconds {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        let status: u16 = String::from_utf8_lossy(&response).split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok::<_, Error>((status, started.elapsed()))
    };
    let headers = async {
        // A request line and then nothing: the header timeout (15 s) still closes the connection.
        let started = Instant::now();
        let mut stream = tokio::net::TcpStream::connect(&address).await?;
        stream.write_all(b"GET / HTTP/1.1\r\nHost: dar\r\n").await?;
        let mut sink = Vec::new();
        let _ = stream.read_to_end(&mut sink).await;
        Ok::<_, Error>(started.elapsed())
    };
    let (upload, closed_after) = tokio::join!(slow, headers);
    let (status, elapsed) = upload?;
    let closed_after = closed_after?;
    assert_eq!(status, 200, "a 75-second upload completes");
    assert!(elapsed >= Duration::from_secs(seconds as u64 - 2));
    assert!(closed_after < Duration::from_secs(40), "slow headers were dropped after {closed_after:?}");
    Ok(())
}
