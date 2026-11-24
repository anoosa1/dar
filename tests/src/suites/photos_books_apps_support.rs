//! Helpers and fixtures shared by the photos, books and apps suites (ported from
//! `test/books-fixtures.ts` and shared media/HTTP helpers).

#![allow(dead_code)]

use dar_acceptance::prelude::*;
use flate2::write::{DeflateEncoder, ZlibEncoder};
use flate2::{Compression, Crc};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ---- Books fixtures (`test/books-fixtures.ts`) ------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

fn deflate_raw(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("deflate write");
    encoder.finish().expect("deflate finish")
}

fn deflate_zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("zlib write");
    encoder.finish().expect("zlib finish")
}

/// A ZIP archive (stored or deflated entries) for EPUB and CBZ fixtures.
pub fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut locals = Vec::new();
    let mut central = Vec::new();
    let mut offset: u32 = 0;
    for &(name, data, store) in entries {
        let packed = if store {
            data.to_vec()
        } else {
            deflate_raw(data)
        };
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let method: u16 = if store { 0 } else { 8 };

        let mut local = [0u8; 30];
        local[0..4].copy_from_slice(&0x04034b50u32.to_le_bytes());
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[6..8].copy_from_slice(&0x0800u16.to_le_bytes());
        local[8..10].copy_from_slice(&method.to_le_bytes());
        local[14..18].copy_from_slice(&crc.to_le_bytes());
        local[18..22].copy_from_slice(&(packed.len() as u32).to_le_bytes());
        local[22..26].copy_from_slice(&(data.len() as u32).to_le_bytes());
        local[26..28].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());

        let mut head = [0u8; 46];
        head[0..4].copy_from_slice(&0x02014b50u32.to_le_bytes());
        head[4..6].copy_from_slice(&20u16.to_le_bytes());
        head[6..8].copy_from_slice(&20u16.to_le_bytes());
        head[8..10].copy_from_slice(&0x0800u16.to_le_bytes());
        head[10..12].copy_from_slice(&method.to_le_bytes());
        head[16..20].copy_from_slice(&crc.to_le_bytes());
        head[20..24].copy_from_slice(&(packed.len() as u32).to_le_bytes());
        head[24..28].copy_from_slice(&(data.len() as u32).to_le_bytes());
        head[28..30].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        head[42..46].copy_from_slice(&offset.to_le_bytes());

        locals.extend_from_slice(&local);
        locals.extend_from_slice(name_bytes);
        locals.extend_from_slice(&packed);
        central.extend_from_slice(&head);
        central.extend_from_slice(name_bytes);
        offset += 30 + name_bytes.len() as u32 + packed.len() as u32;
    }

    let mut end = [0u8; 22];
    end[0..4].copy_from_slice(&0x06054b50u32.to_le_bytes());
    end[8..10].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    end[10..12].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    end[12..16].copy_from_slice(&(central.len() as u32).to_le_bytes());
    end[16..20].copy_from_slice(&offset.to_le_bytes());

    let mut out = Vec::with_capacity(locals.len() + central.len() + end.len());
    out.extend_from_slice(&locals);
    out.extend_from_slice(&central);
    out.extend_from_slice(&end);
    out
}

/// A small valid PNG in one colour.
pub fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let chunk = |kind: &[u8; 4], data: &[u8]| -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + data.len());
        body.extend_from_slice(kind);
        body.extend_from_slice(data);
        let mut out = Vec::with_capacity(12 + data.len());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    };
    let mut header = [0u8; 13];
    header[0..4].copy_from_slice(&width.to_be_bytes());
    header[4..8].copy_from_slice(&height.to_be_bytes());
    header[8] = 8;
    header[9] = 2;
    let stride = (width as usize) * 3 + 1;
    let mut rows = vec![0u8; stride * (height as usize)];
    for y in 0..(height as usize) {
        for x in 0..(width as usize) {
            let start = y * stride + 1 + x * 3;
            rows[start..start + 3].copy_from_slice(&rgb);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    out.extend_from_slice(&chunk(b"IHDR", &header));
    out.extend_from_slice(&chunk(b"IDAT", &deflate_zlib(&rows)));
    out.extend_from_slice(&chunk(b"IEND", &[]));
    out
}

fn epub_chapter(title: &str, paragraphs: usize) -> String {
    let body: String = (0..paragraphs)
        .map(|i| {
            format!(
                "<p id=\"p{}\">Paragraph {} of {title}. The sea was calm and the wind came from the west, carrying the smell of salt and tar across the deck while the crew went about their work.</p>",
                i + 1,
                i + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"en\"><head><title>{title}</title><style>p{{color:#000}}</style><script>document.title=\"script ran\"</script></head>\n<body><h1 id=\"top\">{title}</h1>{body}\n<p><a href=\"chapter2.xhtml#p3\">Jump to the third paragraph of the second chapter</a> \u{00b7} <a href=\"https://example.com/outside\">An outside link</a></p></body></html>"
    )
}

/// An EPUB 3 with a cover, a navigation document, Calibre series metadata and two chapters.
pub fn epub(title: &str, author: &str, series: Option<(&str, u32)>, drm: bool) -> Vec<u8> {
    let series_xml = match series {
        Some((name, index)) => format!(
            "<meta name=\"calibre:series\" content=\"{name}\"/><meta name=\"calibre:series_index\" content=\"{index}\"/>"
        ),
        None => String::new(),
    };
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"id\">urn:uuid:{}</dc:identifier><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator><dc:language>en</dc:language>\n<dc:publisher>Fixture Press</dc:publisher><dc:date>1902-01-01</dc:date><dc:description>&lt;p&gt;A &lt;b&gt;test&lt;/b&gt; voyage.&lt;/p&gt;</dc:description><dc:subject>Sea stories</dc:subject>\n{series_xml}<meta name=\"cover\" content=\"cover-image\"/></metadata>\n<manifest><item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"cover-image\" href=\"images/cover.png\" media-type=\"image/png\" properties=\"cover-image\"/>\n<item id=\"c1\" href=\"chapter1.xhtml\" media-type=\"application/xhtml+xml\"/><item id=\"c2\" href=\"chapter2.xhtml\" media-type=\"application/xhtml+xml\"/></manifest>\n<spine><itemref idref=\"c1\"/><itemref idref=\"c2\"/></spine></package>",
        uuid()
    );
    let nav = "<?xml version=\"1.0\" encoding=\"utf-8\"?><html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\"><head><title>Contents</title></head><body>\n<nav epub:type=\"toc\"><ol><li><a href=\"chapter1.xhtml\">One: Departure</a></li><li><a href=\"chapter2.xhtml\">Two: The Storm</a><ol><li><a href=\"chapter2.xhtml#p3\">The mast breaks</a></li></ol></li></ol></nav></body></html>";
    let container = "<?xml version=\"1.0\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>";
    let encryption = "<?xml version=\"1.0\"?><encryption xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" xmlns:enc=\"http://www.w3.org/2001/04/xmlenc#\"><enc:EncryptedData><enc:EncryptionMethod Algorithm=\"http://www.w3.org/2001/04/xmlenc#aes128-cbc\"/><enc:CipherData><enc:CipherReference URI=\"OEBPS/chapter1.xhtml\"/></enc:CipherData></enc:EncryptedData></encryption>";
    let cover = png(60, 90, [40, 64, 111]);
    let c1 = epub_chapter("Departure", 40);
    let c2 = epub_chapter("The Storm", 60);

    let mut entries: Vec<(&str, &[u8], bool)> = vec![
        ("mimetype", b"application/epub+zip", true),
        ("META-INF/container.xml", container.as_bytes(), false),
    ];
    if drm {
        entries.push(("META-INF/encryption.xml", encryption.as_bytes(), false));
    }
    entries.extend_from_slice(&[
        ("OEBPS/content.opf", opf.as_bytes(), false),
        ("OEBPS/nav.xhtml", nav.as_bytes(), false),
        ("OEBPS/images/cover.png", &cover, false),
        ("OEBPS/chapter1.xhtml", c1.as_bytes(), false),
        ("OEBPS/chapter2.xhtml", c2.as_bytes(), false),
    ]);
    zip(&entries)
}

/// A comic book ZIP with ComicInfo.xml (manga, right to left) and pages in natural order.
pub fn cbz() -> Vec<u8> {
    let comic_info = "<?xml version=\"1.0\"?><ComicInfo><Series>Fixture Comics</Series><Number>2</Number><Writer>Ada Panel</Writer><Manga>YesAndRightToLeft</Manga><Summary>Three pages.</Summary></ComicInfo>";
    let p10 = png(40, 60, [200, 40, 40]);
    let p2 = png(40, 60, [40, 200, 40]);
    let p1 = png(40, 60, [40, 40, 200]);
    zip(&[
        ("ComicInfo.xml", comic_info.as_bytes(), false),
        ("pages/page10.png", &p10, false),
        ("pages/page2.png", &p2, false),
        ("pages/page1.png", &p1, false),
        ("__MACOSX/pages/._page1.png", b"junk", false),
    ])
}

/// A two-page PDF with a title and author, with correct cross-reference offsets.
pub fn pdf() -> Vec<u8> {
    let stream = |text: &str| format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len());
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_owned(),
        stream("BT /F1 24 Tf 40 340 Td (Page one) Tj ET"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_owned(),
        stream("BT /F1 24 Tf 40 340 Td (Page two) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Title (A Fixture Paper) /Author (Grace Columns) >>".to_owned(),
    ];
    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(body.len());
        body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = body.len();
    body.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R /Info 8 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    body.into_bytes()
}

// ---- Shared process, crypto and HTTP helpers --------------------------------------------

/// Runs a command and returns trimmed stdout, failing with stderr if it exits non-zero.
pub async fn run_cmd(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .context(program)?;
    if !output.status.success() {
        bail!(
            "{program} {:?} failed ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub async fn ffmpeg(args: &[&str]) -> Result<()> {
    let mut full = vec!["-nostdin", "-v", "error", "-y"];
    full.extend_from_slice(args);
    run_cmd("ffmpeg", &full).await.map(|_| ())
}

pub async fn exiftool(args: &[&str]) -> Result<()> {
    run_cmd("exiftool", args).await.map(|_| ())
}

/// SHA-1 digest encoded as standard Base64 (matching Immich's asset checksums).
pub fn sha1_base64(input: &[u8]) -> String {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
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
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut digest = [0u8; 20];
    digest[0..4].copy_from_slice(&h0.to_be_bytes());
    digest[4..8].copy_from_slice(&h1.to_be_bytes());
    digest[8..12].copy_from_slice(&h2.to_be_bytes());
    digest[12..16].copy_from_slice(&h3.to_be_bytes());
    digest[16..20].copy_from_slice(&h4.to_be_bytes());
    dar_acceptance::base64(&digest)
}

/// Sets up the first account (`admin`) on a fresh isolated instance with a specific display name.
pub async fn setup_admin_as(base: &str, display_name: &str) -> Result<()> {
    let token =
        dar_acceptance::read_to_string(&dar_acceptance::stack::stack().bootstrap_token_file)?;
    let response = session::client()
        .post(format!("{base}/api/v1/auth/setup"))
        .header("origin", base)
        .json(&json!({
            "username": "admin",
            "displayName": display_name,
            "password": PASSWORD,
            "bootstrapToken": token.trim(),
        }))
        .send()
        .await?;
    let reply = Reply::from(response).await?;
    reply.expect(200)?;
    Ok(())
}

/// Creates a non-admin account through `/api/v1/auth/users`.
pub async fn create_user(
    admin: &Session,
    username: &str,
    display_name: &str,
    password: &str,
) -> Result<()> {
    admin
        .post(
            "/api/v1/auth/users",
            &json!({"username": username, "displayName": display_name, "password": password}),
        )
        .await?
        .expect(201)?;
    Ok(())
}

/// Reserves and uploads bytes through `/api/v1/files/uploads`, returning the uploaded file's id.
pub async fn upload_raw(
    session: &Session,
    parent_id: &str,
    file_name: &str,
    media_type: &str,
    contents: &[u8],
) -> Result<String> {
    let reservation = session
        .post(
            "/api/v1/files/uploads",
            &json!({"parentId": parent_id, "name": file_name, "size": contents.len(), "mediaType": media_type}),
        )
        .await?
        .expect(201)?;
    let reservation_id = reservation.json()?["id"]
        .as_str()
        .context("reservation id")?
        .to_owned();
    let transfer = Reply::from(
        session
            .request("PUT", &format!("/api/v1/files/uploads/{reservation_id}"))
            .body(contents.to_vec())
            .send()
            .await?,
    )
    .await?
    .expect(200)?;
    Ok(transfer.json()?["id"]
        .as_str()
        .context("uploaded file id")?
        .to_owned())
}

/// Sends a request to `/api/v1/photos{route}` with optional JSON body and extra headers.
pub async fn photos_raw(
    session: &Session,
    route: &str,
    method: &str,
    body: Option<&Value>,
    extra: &[(&str, &str)],
) -> Result<Reply> {
    let mut req = session
        .request(method, &format!("/api/v1/photos{route}"))
        .header("content-type", "application/json");
    for &(name, value) in extra {
        req = req.header(name, value);
    }
    if let Some(body) = body {
        req = req.body(body.to_string());
    }
    Reply::from(req.send().await?).await
}

/// Calls `/api/v1/photos{route}` and asserts a 2xx response, returning the parsed JSON.
pub async fn photos_call(
    session: &Session,
    route: &str,
    method: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let reply = photos_raw(session, route, method, body, &[]).await?;
    ensure!(
        (200..300).contains(&reply.status),
        "{method} {route}: {} {}",
        reply.status,
        reply.text()
    );
    reply.json()
}

pub fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub fn parse_iso_ms(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .expect("valid RFC 3339 timestamp")
        .timestamp_millis()
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Builds a `multipart/form-data` payload for Immich's `POST /api/assets`.
pub fn multipart_immich(
    fields: &[(&str, &str)],
    file_name: &str,
    file_type: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
    let boundary = format!("----dar{}", uuid());
    let mut body = Vec::new();
    for &(name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{file_name}\"\r\nContent-Type: {file_type}\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Media fixtures shared by `photos` and `photos-browser`.
pub struct PhotosMedia {
    pub trip: Vec<u8>,
    pub plain: Vec<u8>,
    pub clip: Vec<u8>,
    pub plain_path: PathBuf,
}

/// Generates `trip.jpg` (with EXIF/GPS/Orientation=6), `plain.png`, and `clip.mp4` (with CreateDate).
pub async fn photos_media_fixtures(dir: &Path) -> Result<PhotosMedia> {
    let trip_path = dir.join("trip.jpg");
    let plain_path = dir.join("plain.png");
    let clip_path = dir.join("clip.mp4");
    let trip_str = trip_path.to_string_lossy().into_owned();
    let plain_str = plain_path.to_string_lossy().into_owned();
    let clip_str = clip_path.to_string_lossy().into_owned();

    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x480",
        "-frames:v",
        "1",
        &trip_str,
    ])
    .await?;
    exiftool(&[
        "-q",
        "-overwrite_original",
        "-DateTimeOriginal=2024:04:05 10:20:30",
        "-OffsetTimeOriginal=+03:00",
        "-Make=Google",
        "-Model=Pixel 9",
        "-FNumber=1.8",
        "-ISO=80",
        "-GPSLatitude=41.0",
        "-GPSLatitudeRef=N",
        "-GPSLongitude=28.97",
        "-GPSLongitudeRef=E",
        "-Orientation#=6",
        &trip_str,
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x200",
        "-frames:v",
        "1",
        &plain_str,
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=10",
        "-t",
        "3",
        "-pix_fmt",
        "yuv420p",
        &clip_str,
    ])
    .await?;
    exiftool(&[
        "-q",
        "-overwrite_original",
        "-CreateDate=2025:01:02 03:04:05",
        &clip_str,
    ])
    .await?;

    Ok(PhotosMedia {
        trip: std::fs::read(&trip_path)?,
        plain: std::fs::read(&plain_path)?,
        clip: std::fs::read(&clip_path)?,
        plain_path,
    })
}
