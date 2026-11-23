//! Media tools (`next/runtime/photos/media.ts`): exiftool, libvips (`vipsthumbnail`),
//! ffmpeg and ffprobe run as subprocesses with an argument list (no shell), a minimal
//! environment, time and output limits. Any tool may be missing: facts then fall back to
//! the file's time and thumbnails fail.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use crate::auth::uuid;
use crate::photos::name_parts;

const EXIF_TAGS: &[&str] = &[
    "DateTimeOriginal",
    "CreateDate",
    "MediaCreateDate",
    "OffsetTimeOriginal",
    "ImageWidth",
    "ImageHeight",
    "ExifImageWidth",
    "ExifImageHeight",
    "Orientation",
    "GPSLatitude",
    "GPSLongitude",
    "Make",
    "Model",
    "LensModel",
    "FNumber",
    "ExposureTime",
    "ISO",
    "FocalLength",
    "Duration",
    "Rotation",
    "ContentIdentifier",
    "BurstUUID",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoFacts {
    pub taken_at: f64,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub exif: Map<String, Value>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub phash: String,
    pub playback: String,
    pub content_id: String,
    pub burst_id: String,
    pub sha1: String,
}

fn tool(name: &str) -> String {
    let var = format!("DAR_{}", name.to_ascii_uppercase());
    std::env::var(&var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

fn build_command(name: &str) -> Command {
    let mut cmd = Command::new(tool(name));
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", "/nonexistent")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

async fn run_bytes(name: &str, args: &[String], timeout_ms: u64, max_buffer: usize) -> Option<Vec<u8>> {
    let mut cmd = build_command(name);
    cmd.args(args);
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let fut = async {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = stdout.read(&mut buf).await.ok()?;
            if n == 0 {
                break;
            }
            if out.len() + n > max_buffer {
                let _ = child.kill().await;
                return None;
            }
            out.extend_from_slice(&buf[..n]);
        }
        let status = child.wait().await.ok()?;
        if status.success() && !out.is_empty() { Some(out) } else { None }
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await.ok().flatten()
}

async fn run_text(name: &str, args: &[String], timeout_ms: u64, max_buffer: usize) -> (bool, String) {
    let mut cmd = build_command(name);
    cmd.args(args);
    let Ok(mut child) = cmd.spawn() else {
        return (false, String::new());
    };
    let Some(mut stdout) = child.stdout.take() else {
        return (false, String::new());
    };
    let fut = async {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let Ok(n) = stdout.read(&mut buf).await else {
                let _ = child.kill().await;
                return (false, String::new());
            };
            if n == 0 {
                break;
            }
            if out.len() + n > max_buffer {
                let _ = child.kill().await;
                return (false, String::new());
            }
            out.extend_from_slice(&buf[..n]);
        }
        let ok = child.wait().await.is_ok_and(|s| s.success());
        (ok, String::from_utf8_lossy(&out).into_owned())
    };
    match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(res) => res,
        Err(_) => (false, String::new()),
    }
}

/// EXIF dates have no zone unless `OffsetTimeOriginal` is present; then they are read as server-local time.
fn exif_date(value: Option<&Value>, offset: Option<&Value>, utc: bool) -> Option<f64> {
    let s = value?.as_str()?;
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    if b[4] != b':' || b[7] != b':' || (b[10] != b' ' && b[10] != b'T') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y_str = &s[0..4];
    let mo_str = &s[5..7];
    let d_str = &s[8..10];
    let h_str = &s[11..13];
    let mi_str = &s[14..16];
    let s_str = &s[17..19];
    if y_str == "0000"
        || ![y_str, mo_str, d_str, h_str, mi_str, s_str]
            .iter()
            .all(|part| part.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let zone = offset
        .and_then(Value::as_str)
        .filter(|o| {
            let ob = o.as_bytes();
            ob.len() == 6
                && (ob[0] == b'+' || ob[0] == b'-')
                && ob[1].is_ascii_digit()
                && ob[2].is_ascii_digit()
                && ob[3] == b':'
                && ob[4].is_ascii_digit()
                && ob[5].is_ascii_digit()
        })
        .or(if utc { Some("Z") } else { None });
    let time_ms: i64 = if let Some(z) = zone {
        let iso = format!("{y_str}-{mo_str}-{d_str}T{h_str}:{mi_str}:{s_str}{z}");
        DateTime::parse_from_rfc3339(&iso).ok()?.timestamp_millis()
    } else {
        let y: i32 = y_str.parse().ok()?;
        let mo: u32 = mo_str.parse().ok()?;
        let d: u32 = d_str.parse().ok()?;
        let h: u32 = h_str.parse().ok()?;
        let mi: u32 = mi_str.parse().ok()?;
        let sec: u32 = s_str.parse().ok()?;
        let naive = NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, sec)?;
        Local.from_local_datetime(&naive).earliest()?.timestamp_millis()
    };
    (time_ms > 0).then_some(time_ms as f64)
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn short(value: Option<&Value>) -> Option<String> {
    let s = value.and_then(Value::as_str)?.trim();
    if s.is_empty() {
        return None;
    }
    Some(s.chars().take(120).collect())
}

fn identifier(value: Option<&Value>) -> String {
    let Some(s) = value.and_then(Value::as_str).map(str::trim) else {
        return String::new();
    };
    if (1..=200).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        s.to_owned()
    } else {
        String::new()
    }
}

/// Which browsers play without help: MP4 with H.264/VP9/AV1 and AAC/MP3/Opus, or WebM with
/// VP8/VP9/AV1. Other H.264 files are remuxed into MP4; everything else is transcoded.
pub fn playback_for(name: &str, media_type: &str, video: &str, audio: &str) -> &'static str {
    let (_, ext) = name_parts(name);
    let t = media_type.to_ascii_lowercase();
    let container = if ext == "mp4" || ext == "m4v" || t == "video/mp4" {
        "mp4"
    } else if ext == "webm" || t == "video/webm" {
        "webm"
    } else {
        "other"
    };
    let audio_in = |list: &[&str]| audio.is_empty() || list.contains(&audio);
    if container == "mp4" && ["h264", "av1", "vp9"].contains(&video) && audio_in(&["aac", "mp3", "opus"]) {
        return "original";
    }
    if container == "webm" && ["vp8", "vp9", "av1"].contains(&video) && audio_in(&["opus", "vorbis"]) {
        return "original";
    }
    if video == "h264" { "remux" } else { "transcode" }
}

async fn probe(file: &Path) -> (bool, String, String) {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "stream=codec_type,codec_name".into(),
        "-of".into(),
        "json".into(),
        file.to_string_lossy().into_owned(),
    ];
    let (ok, stdout) = run_text("ffprobe", &args, 60_000, 1024 * 1024).await;
    let streams: Vec<Value> = if ok {
        serde_json::from_str::<Value>(&stdout)
            .ok()
            .and_then(|v| v.get("streams").and_then(Value::as_array).cloned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let codec = |kind: &str| -> String {
        streams
            .iter()
            .find(|s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
            .and_then(|s| short(s.get("codec_name")))
            .map(|c| c.to_ascii_lowercase().chars().take(30).collect())
            .unwrap_or_default()
    };
    (ok, codec("video"), codec("audio"))
}

pub async fn read_facts(
    file: &Path,
    kind: &str,
    fallback_time: f64,
    name: &str,
    media_type: &str,
) -> PhotoFacts {
    let mut facts = PhotoFacts {
        taken_at: fallback_time,
        width: 0,
        height: 0,
        duration: 0.0,
        exif: Map::new(),
        latitude: None,
        longitude: None,
        phash: String::new(),
        playback: String::new(),
        content_id: String::new(),
        burst_id: String::new(),
        sha1: String::new(),
    };
    if kind == "video" {
        let (ok, video, audio) = probe(file).await;
        if ok {
            facts.playback = playback_for(name, media_type, &video, &audio).to_owned();
            if !video.is_empty() {
                facts.exif.insert("videoCodec".into(), json!(video));
            }
            if !audio.is_empty() {
                facts.exif.insert("audioCodec".into(), json!(audio));
            }
        }
    }
    let mut args = vec!["-j".into(), "-n".into(), "-fast".into()];
    for tag in EXIF_TAGS {
        args.push(format!("-{tag}"));
    }
    args.push("--".into());
    args.push(file.to_string_lossy().into_owned());
    let (ok, stdout) = run_text("exiftool", &args, 60_000, 4 * 1024 * 1024).await;
    if !ok {
        return facts;
    }
    let data = serde_json::from_str::<Value>(&stdout)
        .ok()
        .and_then(|v| v.as_array().and_then(|a| a.first()).cloned())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if data.is_empty() {
        return facts;
    }
    // QuickTime CreateDate is UTC by specification; photo EXIF dates are local.
    // Empty dates ("0000:00:00 ...") are skipped so the next candidate applies.
    facts.taken_at = exif_date(data.get("DateTimeOriginal"), data.get("OffsetTimeOriginal"), false)
        .or_else(|| {
            if kind == "video" {
                exif_date(data.get("MediaCreateDate"), None, true)
            } else {
                None
            }
        })
        .or_else(|| exif_date(data.get("CreateDate"), data.get("OffsetTimeOriginal"), kind == "video"))
        .unwrap_or(fallback_time);

    let mut width = finite(data.get("ExifImageWidth"))
        .or_else(|| finite(data.get("ImageWidth")))
        .unwrap_or(0.0);
    let mut height = finite(data.get("ExifImageHeight"))
        .or_else(|| finite(data.get("ImageHeight")))
        .unwrap_or(0.0);
    let orientation = finite(data.get("Orientation"));
    let rotation = finite(data.get("Rotation"));
    if orientation.is_some_and(|o| (5.0..=8.0).contains(&o))
        || rotation == Some(90.0)
        || rotation == Some(270.0)
    {
        std::mem::swap(&mut width, &mut height);
    }
    facts.width = width.round().clamp(0.0, u32::MAX as f64) as u32;
    facts.height = height.round().clamp(0.0, u32::MAX as f64) as u32;
    facts.duration = if kind == "video" {
        finite(data.get("Duration")).unwrap_or(0.0).max(0.0)
    } else {
        0.0
    };
    if let (Some(lat), Some(lon)) = (finite(data.get("GPSLatitude")), finite(data.get("GPSLongitude")))
        && lat.abs() <= 90.0
        && lon.abs() <= 180.0
        && !(lat == 0.0 && lon == 0.0)
    {
        facts.latitude = Some(lat);
        facts.longitude = Some(lon);
    }
    if let Some(v) = short(data.get("Make")) {
        facts.exif.insert("make".into(), json!(v));
    }
    if let Some(v) = short(data.get("Model")) {
        facts.exif.insert("model".into(), json!(v));
    }
    if let Some(v) = short(data.get("LensModel")) {
        facts.exif.insert("lens".into(), json!(v));
    }
    if let Some(v) = finite(data.get("FNumber")) {
        facts.exif.insert("fNumber".into(), json!(v));
    }
    if let Some(v) = finite(data.get("ExposureTime")) {
        facts.exif.insert("exposure".into(), json!(v));
    }
    if let Some(v) = finite(data.get("ISO")) {
        facts.exif.insert("iso".into(), json!(v));
    }
    if let Some(v) = finite(data.get("FocalLength")) {
        facts.exif.insert("focalLength".into(), json!(v));
    }
    if let Some(v) = orientation {
        facts.exif.insert("orientation".into(), json!(v));
    }
    facts.content_id = identifier(data.get("ContentIdentifier"));
    facts.burst_id = identifier(data.get("BurstUUID"));
    facts
}

/// 64-bit difference hash of a thumbnail: shrink to 9x8 grey pixels and compare neighbours.
/// Resized or re-encoded copies of a picture share it.
pub async fn perceptual_hash(file: &Path) -> String {
    let args = vec![
        "-nostdin".into(),
        "-v".into(),
        "error".into(),
        "-i".into(),
        file.to_string_lossy().into_owned(),
        "-vf".into(),
        "scale=9:8:flags=area,format=gray".into(),
        "-f".into(),
        "rawvideo".into(),
        "pipe:1".into(),
    ];
    let Some(bytes) = run_bytes("ffmpeg", &args, 30_000, 4096).await else {
        return String::new();
    };
    if bytes.len() != 72 {
        return String::new();
    }
    let mut hash: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            hash = (hash << 1) | u64::from(bytes[y * 9 + x] < bytes[y * 9 + x + 1]);
        }
    }
    format!("{hash:016x}")
}

fn valid_cache_id(blob_id: &str) -> bool {
    (8..=128).contains(&blob_id.len())
        && blob_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn cache_file(cache: &Path, blob_id: &str, suffix: &str) -> std::io::Result<PathBuf> {
    if !valid_cache_id(blob_id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Unexpected blob ID."));
    }
    Ok(cache.join(&blob_id[..2]).join(format!("{blob_id}-{suffix}")))
}

pub fn cache_path(cache: &Path, blob_id: &str, size: &str) -> std::io::Result<PathBuf> {
    let size = if size == "m" { "m" } else { "s" };
    cache_file(cache, blob_id, &format!("{size}.webp"))
}

/// Browser-playable MP4 made from a video that needs a remux or transcode.
pub fn video_path(cache: &Path, blob_id: &str) -> std::io::Result<PathBuf> {
    cache_file(cache, blob_id, "v.mp4")
}

async fn exists_non_empty(file: &Path) -> bool {
    tokio::fs::metadata(file).await.is_ok_and(|m| m.len() > 0)
}

async fn make_work_dir(cache: &Path) -> std::io::Result<PathBuf> {
    let work = cache.join(format!(".work-{}", uuid()));
    tokio::fs::DirBuilder::new().mode(0o700).create(&work).await?;
    Ok(work)
}

/// Writes an auto-rotated WebP thumbnail; video posters come from a frame ffmpeg extracts.
pub async fn make_thumbnail(
    cache: &Path,
    source: &Path,
    blob_id: &str,
    kind: &str,
    size: &str,
) -> bool {
    let size = if size == "m" { "m" } else { "s" };
    let Ok(target) = cache_path(cache, blob_id, size) else {
        return false;
    };
    if exists_non_empty(&target).await {
        return true;
    }
    if let Some(parent) = target.parent()
        && tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .await
            .is_err()
    {
        return false;
    }
    let Ok(work) = make_work_dir(cache).await else {
        return false;
    };
    let result = async {
        let out = work.join("out.webp");
        let (edge, quality) = if size == "m" { (1800, 82) } else { (420, 72) };
        let vips = |input: PathBuf, out_path: PathBuf| async move {
            let args = vec![
                input.to_string_lossy().into_owned(),
                "--size".into(),
                format!("{edge}x{edge}>"),
                "-o".into(),
                format!("{}[Q={quality},strip]", out_path.display()),
            ];
            run_text("vipsthumbnail", &args, 120_000, 4 * 1024 * 1024).await.0
        };
        let ok = if kind == "video" {
            let frame = work.join("frame.png");
            for at in ["1", "0"] {
                let args = vec![
                    "-nostdin".into(),
                    "-v".into(),
                    "error".into(),
                    "-ss".into(),
                    at.into(),
                    "-i".into(),
                    source.to_string_lossy().into_owned(),
                    "-frames:v".into(),
                    "1".into(),
                    "-y".into(),
                    frame.to_string_lossy().into_owned(),
                ];
                if run_text("ffmpeg", &args, 120_000, 4 * 1024 * 1024).await.0
                    && exists_non_empty(&frame).await
                {
                    break;
                }
            }
            exists_non_empty(&frame).await && vips(frame, out.clone()).await
        } else {
            let mut ok = vips(source.to_path_buf(), out.clone()).await;
            if !ok {
                // RAW files: use the embedded JPEG preview.
                let preview = work.join("preview.jpg");
                for tag in ["-PreviewImage", "-JpgFromRaw"] {
                    let args = vec![
                        "-b".into(),
                        tag.into(),
                        "--".into(),
                        source.to_string_lossy().into_owned(),
                    ];
                    if let Some(bytes) = run_bytes("exiftool", &args, 60_000, 64 * 1024 * 1024).await {
                        if let Ok(mut f) = tokio::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .mode(0o600)
                            .open(&preview)
                            .await
                            && f.write_all(&bytes).await.is_ok()
                        {
                            break;
                        }
                    }
                }
                ok = exists_non_empty(&preview).await && vips(preview, out.clone()).await;
            }
            ok
        };
        if !ok || !exists_non_empty(&out).await {
            return false;
        }
        tokio::fs::rename(&out, &target).await.is_ok()
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    result
}

/// Makes a browser-playable MP4 (H.264/AAC, fast start). A remux copies the H.264 stream;
/// a transcode fits the picture in 1920x1920 with ffmpeg's automatic rotation. Progress
/// comes from ffmpeg's `-progress` output; the time limit grows with the video's length.
#[allow(clippy::too_many_arguments)]
pub async fn make_video<F>(
    cache: &Path,
    source: &Path,
    blob_id: &str,
    mode: &str,
    audio: &str,
    duration: f64,
    progress: F,
    mut cancel: watch::Receiver<bool>,
) -> bool
where
    F: Fn(f64) + Send + Sync,
{
    let Ok(target) = video_path(cache, blob_id) else {
        return false;
    };
    if exists_non_empty(&target).await {
        return true;
    }
    if let Some(parent) = target.parent()
        && tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .await
            .is_err()
    {
        return false;
    }
    let Ok(work) = make_work_dir(cache).await else {
        return false;
    };
    let result = async {
        if *cancel.borrow() {
            return false;
        }
        let out = work.join("out.mp4");
        let mut args: Vec<String> = vec![
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-nostats".into(),
            "-i".into(),
            source.to_string_lossy().into_owned(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "0:a:0?".into(),
            "-sn".into(),
            "-dn".into(),
        ];
        if mode == "remux" {
            args.extend(["-c:v", "copy", "-tag:v", "avc1"].into_iter().map(str::to_owned));
        } else {
            args.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "23",
                    "-pix_fmt",
                    "yuv420p",
                    "-vf",
                    "scale=w='min(1920,iw)':h='min(1920,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        if audio == "aac" {
            args.extend(["-c:a", "copy"].into_iter().map(str::to_owned));
        } else {
            args.extend(["-c:a", "aac", "-b:a", "160k"].into_iter().map(str::to_owned));
        }
        args.extend(
            [
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-f",
                "mp4",
                "-y",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        args.push(out.to_string_lossy().into_owned());

        let mut cmd = build_command("ffmpeg");
        cmd.args(&args);
        let Ok(mut child) = cmd.spawn() else {
            return false;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            return false;
        };
        let limit_ms = (duration.max(0.0) * 8_000.0).clamp(300_000.0, (4 * 3_600_000) as f64) as u64;
        let deadline = tokio::time::sleep(Duration::from_millis(limit_ms));
        tokio::pin!(deadline);

        let mut lines = BufReader::new(stdout).lines();
        let ok = loop {
            tokio::select! {
                _ = &mut deadline => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    break false;
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        break false;
                    }
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if let Some(us_str) = text.strip_prefix("out_time_us=")
                                && let Ok(us) = us_str.trim().parse::<f64>()
                                && duration > 0.0
                            {
                                progress((us / 1e6 / duration).min(0.99));
                            }
                        }
                        Ok(None) | Err(_) => {
                            break child.wait().await.is_ok_and(|s| s.success());
                        }
                    }
                }
            }
        };
        if !ok || !exists_non_empty(&out).await {
            return false;
        }
        tokio::fs::rename(&out, &target).await.is_ok()
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    result
}
