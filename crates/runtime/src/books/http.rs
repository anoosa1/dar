//! Books HTTP API for browser sessions (`next/runtime/books/http.ts`). Every request resolves the
//! file through Files' `service_download` check (owner or grant, not deleted, Locked folder rules),
//! so a book is readable here exactly when Files would serve it.

use std::collections::HashMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::blobs::BlobFile;
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response, ok};

use super::formats::{BookFormat, book_format, percent_decode_utf8, unsupported_book};
use super::runtime::BooksRuntime;

const BOOKS_READ_OPERATIONS: &[&str] = &["detail"];
const CACHED: &str = "private, max-age=86400";

// Book documents may style themselves and load their own images and fonts, never scripts, forms,
// popups or other origins. The sandbox keeps allow-scripts only so the reader's own listeners
// (page turns) work in WebKit; script-src 'none' still stops every script in the book.
const DOCUMENT_POLICY: &str = "default-src 'none'; script-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; media-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'; sandbox allow-same-origin allow-scripts";

fn parse_route(route: &str) -> Option<(&str, Option<&str>, Option<&str>)> {
    let after_slash = route.strip_prefix('/')?;
    let (id, part_and_rest) = match after_slash.split_once('/') {
        Some((id, tail)) => (id, Some(tail)),
        None => (after_slash, None),
    };
    if !crate::auth::is_uuid(id) {
        return None;
    }
    let Some(tail) = part_and_rest else {
        return Some((id, None, None));
    };
    let (part, rest) = match tail.split_once('/') {
        Some((part, rest)) if !rest.is_empty() => (part, Some(rest)),
        Some(_) => return None,
        None => (tail, None),
    };
    if !matches!(part, "cover" | "state" | "marks" | "pages" | "epub") {
        return None;
    }
    Some((id, Some(part), rest))
}

fn build_bytes_response(
    method: &Method,
    headers: &HeaderMap,
    data: Vec<u8>,
    content_type: &str,
    blob_id: &str,
    csp: Option<&str>,
) -> Response {
    let len = data.len();
    let etag = format!("\"{blob_id}-{len}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str())
    {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, CACHED)
            .body(Body::empty())
            .expect("304 response");
    }
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(data)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, CACHED)
        .header("x-content-type-options", "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            csp.unwrap_or("default-src 'none'; sandbox"),
        )
        .body(body)
        .expect("bytes response")
}

/// Streams a local cache file with byte ranges and a strong ETag (`sendFile` in `photos/http.ts`).
async fn send_file(
    method: &Method,
    headers: &HeaderMap,
    path: &Path,
    content_type: &str,
    tag: &str,
    cache: &str,
) -> Result<Response> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::not_found())?;
    let size = file
        .metadata()
        .await
        .map_err(|_| AppError::not_found())?
        .len();
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(tag)
    {
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, tag)
            .header(header::CACHE_CONTROL, cache)
            .body(Body::empty())
            .expect("304 response"));
    }

    let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), StatusCode::OK);
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let if_range = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok());
    let mut content_range: Option<String> = None;

    if let Some(range) = range.filter(|_| if_range.is_none_or(|r| r == tag)) {
        let invalid_range = || {
            Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Body::empty())
                .expect("416 response")
        };
        let Some(spec) = range.strip_prefix("bytes=") else {
            return Ok(invalid_range());
        };
        let Some((from, to)) = spec.split_once('-') else {
            return Ok(invalid_range());
        };
        if (from.is_empty() && to.is_empty())
            || size == 0
            || !from.bytes().all(|b| b.is_ascii_digit())
            || !to.bytes().all(|b| b.is_ascii_digit())
        {
            return Ok(invalid_range());
        }
        if from.is_empty() {
            let Ok(suffix) = to.parse::<u64>() else {
                return Ok(invalid_range());
            };
            start = size.saturating_sub(suffix);
        } else {
            let Ok(parsed_start) = from.parse::<u64>() else {
                return Ok(invalid_range());
            };
            start = parsed_start;
            if !to.is_empty() {
                let Ok(parsed_end) = to.parse::<u64>() else {
                    return Ok(invalid_range());
                };
                end = end.min(parsed_end);
            }
        }
        if start > end || start >= size {
            return Ok(invalid_range());
        }
        status = StatusCode::PARTIAL_CONTENT;
        content_range = Some(format!("bytes {start}-{end}/{size}"));
    }

    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ETAG, tag)
        .header(header::CACHE_CONTROL, cache)
        .header(header::ACCEPT_RANGES, "bytes")
        .header("x-content-type-options", "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
        .header(header::CONTENT_LENGTH, length);
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }

    if *method == Method::HEAD || size == 0 {
        return Ok(builder.body(Body::empty()).expect("file head response"));
    }
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|_| AppError::internal())?;
    let stream = tokio_util::io::ReaderStream::new(file.take(length));
    Ok(builder
        .body(Body::from_stream(stream))
        .expect("file stream response"))
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_http(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    books: &BooksRuntime,
    body: Body,
) -> Result<Response> {
    let route = path.strip_prefix("/api/v1/books").unwrap_or("");
    if route == "/library" && *method == Method::GET {
        return ok(books.library(session_id).await?);
    }

    let Some((id, part, rest)) = parse_route(route) else {
        return Err(AppError::not_found());
    };

    let raw_entry = runtime
        .db
        .read("service_download", json!([session_id, id, ""]))
        .await?;
    let locked = raw_entry.get("locked") != Some(&Value::Bool(false));
    let entry: BlobFile = serde_json::from_value(raw_entry).map_err(|_| AppError::internal())?;

    let Some(format) = book_format(&entry.name) else {
        return Err(if unsupported_book(&entry.name) {
            AppError::new("unsupportedBook", 415)
        } else {
            AppError::not_found()
        });
    };

    let call = |operation: &'static str, value: Value| {
        let db = runtime.db.clone();
        let session = session_id.to_owned();
        async move {
            db.operation(
                "service_books",
                operation,
                BOOKS_READ_OPERATIONS,
                json!([session, operation, value.to_string()]),
            )
            .await
        }
    };

    let reading = *method == Method::GET || *method == Method::HEAD;

    if part.is_none() && *method == Method::GET {
        let info = books.info(&entry.blob_id, format, &entry.name).await;
        let detail = call("detail", json!({ "fileId": id })).await?;
        let manifest = match format {
            BookFormat::Pdf => json!({ "kind": "pdf" }),
            BookFormat::Epub => json!({
                "kind": "epub",
                "spine": info.epub.as_ref().map(|e| e.spine.as_slice()).unwrap_or(&[]),
                "toc": info.epub.as_ref().map(|e| e.toc.as_slice()).unwrap_or(&[]),
                "rightToLeft": info.meta.right_to_left,
                "language": info.meta.language,
            }),
            _ => json!({
                "kind": "comic",
                "pages": info.pages.as_ref().map_or(0, Vec::len),
                "rightToLeft": info.meta.right_to_left,
            }),
        };
        let mut out = match detail {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        out.insert("manifest".into(), manifest);
        out.insert(
            "meta".into(),
            serde_json::to_value(&info.meta).map_err(|_| AppError::internal())?,
        );
        return ok(Value::Object(out));
    }

    if part == Some("cover") && reading {
        let Some(file) = books.cover(&entry.blob_id, format, &entry.name).await else {
            return Err(AppError::not_found());
        };
        let tag = format!("\"{}-cover\"", entry.blob_id);
        let cache = if !locked { CACHED } else { "private, no-store" };
        return send_file(method, headers, &file, "image/webp", &tag, cache).await;
    }

    if part == Some("pages") && reading {
        let Some(rest_str) = rest else {
            return Err(AppError::not_found());
        };
        if rest_str.is_empty()
            || !rest_str.bytes().all(|b| b.is_ascii_digit())
            || format == BookFormat::Epub
            || format == BookFormat::Pdf
        {
            return Err(AppError::not_found());
        }
        let Ok(index) = rest_str.parse::<usize>() else {
            return Err(AppError::not_found());
        };
        let Some((data, mime)) = books.page(&entry.blob_id, format, &entry.name, index).await
        else {
            return Err(AppError::not_found());
        };
        return Ok(build_bytes_response(
            method,
            headers,
            data,
            mime,
            &entry.blob_id,
            None,
        ));
    }

    if part == Some("epub")
        && reading
        && let Some(rest_str) = rest
    {
        // The first segment carries the file revision so a replaced book is never served from the browser cache.
        let mut decoded = Vec::new();
        for segment in rest_str.split('/').skip(1) {
            let seg = percent_decode_utf8(segment).map_err(|_| AppError::not_found())?;
            decoded.push(seg);
        }
        let name = decoded.join("/");
        if format != BookFormat::Epub || name.is_empty() {
            return Err(AppError::not_found());
        }
        let Some((data, mut mime)) = books.resource(&entry.blob_id, &name).await else {
            return Err(AppError::not_found());
        };
        // Sloppy XHTML that the XML parser rejects can be read with the HTML parser instead.
        if mime == "application/xhtml+xml" && query.get("as").map(String::as_str) == Some("html") {
            mime = "text/html; charset=utf-8";
        }
        let csp = (mime.contains("xhtml") || mime.contains("html") || mime.contains("svg"))
            .then_some(DOCUMENT_POLICY);
        return Ok(build_bytes_response(
            method,
            headers,
            data,
            mime,
            &entry.blob_id,
            csp,
        ));
    }

    if part == Some("state") && *method == Method::PUT {
        let mut p = body_object(body, 64 * 1024, headers).await?;
        p.insert("fileId".into(), json!(id));
        return match call("saveState", Value::Object(p)).await {
            Ok(value) => ok(value),
            Err(error) if error.code == "conflict" => {
                // The newer state comes back so the reader can offer to jump to it.
                let current = call("detail", json!({ "fileId": id })).await?;
                Ok(json_response(
                    &json!({
                        "error": { "code": "conflict", "message": "Your place in this book changed on another device." },
                        "state": current["state"],
                    }),
                    StatusCode::CONFLICT,
                ))
            }
            Err(error) => Err(error),
        };
    }

    if part == Some("marks") && *method == Method::POST && rest.is_none() {
        let mut p = body_object(body, 64 * 1024, headers).await?;
        p.insert("fileId".into(), json!(id));
        let value = call("addMark", Value::Object(p)).await?;
        return Ok(json_response(&value, StatusCode::CREATED));
    }

    if part == Some("marks")
        && *method == Method::DELETE
        && let Some(mark_id) = rest
        && crate::auth::is_uuid(mark_id)
    {
        return ok(call("deleteMark", json!({ "id": mark_id })).await?);
    }

    if reading || part.is_none() {
        Err(AppError::not_found())
    } else {
        Err(AppError::new("methodNotAllowed", 405))
    }
}
