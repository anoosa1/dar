//! Helpers and fixtures shared by the core suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use dar_acceptance::identity::{Followed, Interaction};
use dar_acceptance::oidc::{Authorize, Client, Flow};
use dar_acceptance::prelude::*;
use flate2::Compression;
use flate2::write::{DeflateEncoder, ZlibEncoder};
use std::io::Write;

/// SCIM schema URNs (`support/identity.ts`).
pub const USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub const GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
pub const ENTERPRISE: &str = "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";
pub const PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/// The provider stores each record under `<Model>/<SHA-256 of its secret or ID>`.
pub fn model_key(model: &str, id: &str) -> String {
    format!("{model}/{}", dar_acceptance::sha256_hex(id))
}

/// A SCIM request with a bearer credential (`application/scim+json`).
pub async fn scim(base: &str, secret: &str, path: &str, method: &str, payload: Option<&Value>, headers: &[(&str, &str)]) -> Result<Reply> {
    // Given headers replace the defaults (the credential, for one).
    let mut map = reqwest::header::HeaderMap::new();
    map.insert("authorization", format!("Bearer {secret}").parse().unwrap());
    map.insert("content-type", "application/scim+json".parse().unwrap());
    for (name, value) in headers {
        map.insert(session::header(name), value.parse().map_err(|_| Error::new("header value"))?);
    }
    let mut request = session::client().request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{base}/scim/v2{path}")).headers(map);
    if let Some(payload) = payload {
        request = request.body(payload.to_string());
    }
    Reply::from(request.send().await?).await
}

/// An `application/x-www-form-urlencoded` body.
pub fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

/// Like [`dar_acceptance::oidc::authorize`], with an `on_interaction` callback.
pub async fn authorize_with(
    base: &str,
    session: &Session,
    client: &Client,
    options: Authorize,
    on_interaction: impl FnMut(&Interaction),
) -> Result<Flow> {
    let verifier = dar_acceptance::base64url(&dar_acceptance::random_bytes(32));
    let challenge = {
        use sha2::Digest as _;
        dar_acceptance::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    let (state, nonce) = (dar_acceptance::base64url(&dar_acceptance::random_bytes(16)), dar_acceptance::base64url(&dar_acceptance::random_bytes(16)));
    let mut params = vec![
        ("redirect_uri".to_owned(), client.redirect.clone()),
        ("response_type".to_owned(), "code".to_owned()),
        ("scope".to_owned(), options.scope.clone().unwrap_or_else(|| "openid profile email groups offline_access".into())),
    ];
    if !options.no_pkce {
        params.push(("code_challenge".into(), challenge));
        params.push(("code_challenge_method".into(), "S256".into()));
    }
    params.push(("state".into(), state.clone()));
    params.push(("nonce".into(), nonce.clone()));
    params.push(("prompt".into(), options.prompt.clone().unwrap_or_else(|| "consent".into())));
    params.extend(options.params.iter().cloned());
    let start = client.authorization_url(&params)?;
    let Followed { url, jar } = follow(base, session, start.as_str(), options.deny, options.jar.clone(), on_interaction).await?;
    Ok(Flow { url, jar, verifier, state, nonce, pkce: !options.no_pkce })
}

/// Extracts a single query parameter from a URL.
pub fn query_param(url: &url::Url, key: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

/// Standard RFC 1321 MD5 hex digest (for News worker fixtures).
pub fn md5_hex(input: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
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

/// IEEE CRC-32 for PNG and ZIP fixtures.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A small valid solid-colour PNG (`books-fixtures.ts:png`).
pub fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + data.len());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }
    let mut header = [0u8; 13];
    header[0..4].copy_from_slice(&width.to_be_bytes());
    header[4..8].copy_from_slice(&height.to_be_bytes());
    header[8] = 8;
    header[9] = 2;
    let stride = (width as usize) * 3 + 1;
    let mut rows = vec![0u8; stride * (height as usize)];
    for y in 0..(height as usize) {
        for x in 0..(width as usize) {
            let offset = y * stride + 1 + x * 3;
            rows[offset..offset + 3].copy_from_slice(&rgb);
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&rows).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
    out.extend_from_slice(&chunk(b"IHDR", &header));
    out.extend_from_slice(&chunk(b"IDAT", &compressed));
    out.extend_from_slice(&chunk(b"IEND", &[]));
    out
}

/// A ZIP archive (stored or deflated entries) for EPUB fixtures.
pub fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut locals = Vec::new();
    let mut central = Vec::new();
    for &(name, data, store) in entries {
        let packed = if store {
            data.to_vec()
        } else {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        };
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let method: u16 = if store { 0 } else { 8 };
        let offset = locals.len() as u32;

        locals.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        locals.extend_from_slice(&20u16.to_le_bytes());
        locals.extend_from_slice(&0x0800u16.to_le_bytes());
        locals.extend_from_slice(&method.to_le_bytes());
        locals.extend_from_slice(&[0u8; 4]);
        locals.extend_from_slice(&crc.to_le_bytes());
        locals.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
        locals.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        locals.extend_from_slice(&0u16.to_le_bytes());
        locals.extend_from_slice(name_bytes);
        locals.extend_from_slice(&packed);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0u8; 4]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let dir_offset = locals.len() as u32;
    let dir_len = central.len() as u32;
    let count = entries.len() as u16;
    let mut out = locals;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&dir_len.to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Minimal valid EPUB 3 fixture (`books-fixtures.ts:epub`).
pub fn epub(title: &str, author: &str) -> Vec<u8> {
    let chapter = |name: &str, paragraphs: usize| -> String {
        let body: String = (1..=paragraphs)
            .map(|i| format!(r#"<p id="p{i}">Paragraph {i} of {name}. The sea was calm and the wind came from the west, carrying the smell of salt and tar across the deck while the crew went about their work.</p>"#))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en"><head><title>{name}</title><style>p{{color:#000}}</style><script>document.title="script ran"</script></head>
<body><h1 id="top">{name}</h1>{body}
<p><a href="chapter2.xhtml#p3">Jump to the third paragraph of the second chapter</a> · <a href="https://example.com/outside">An outside link</a></p></body></html>"#
        )
    };
    let opf = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="id">urn:uuid:{}</dc:identifier><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator><dc:language>en</dc:language>
<dc:publisher>Fixture Press</dc:publisher><dc:date>1902-01-01</dc:date><dc:description>&lt;p&gt;A &lt;b&gt;test&lt;/b&gt; voyage.&lt;/p&gt;</dc:description><dc:subject>Sea stories</dc:subject>
<meta name="cover" content="cover-image"/></metadata>
<manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover-image" href="images/cover.png" media-type="image/png" properties="cover-image"/>
<item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
        uuid()
    );
    let nav = r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body>
<nav epub:type="toc"><ol><li><a href="chapter1.xhtml">One: Departure</a></li><li><a href="chapter2.xhtml">Two: The Storm</a><ol><li><a href="chapter2.xhtml#p3">The mast breaks</a></li></ol></li></ol></nav></body></html>"#;
    let container = r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let cover = png(60, 90, [40, 64, 111]);
    let c1 = chapter("Departure", 40);
    let c2 = chapter("The Storm", 60);
    zip(&[
        ("mimetype", b"application/epub+zip", true),
        ("META-INF/container.xml", container.as_bytes(), false),
        ("OEBPS/content.opf", opf.as_bytes(), false),
        ("OEBPS/nav.xhtml", nav.as_bytes(), false),
        ("OEBPS/images/cover.png", &cover, false),
        ("OEBPS/chapter1.xhtml", c1.as_bytes(), false),
        ("OEBPS/chapter2.xhtml", c2.as_bytes(), false),
    ])
}
