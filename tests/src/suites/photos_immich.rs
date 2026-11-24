//! Immich (`photos-immich.integration.test.ts`): mobile-app API (discovery, sign-in, backup,
//! duplicates, media, sync stream with foreign-key/checksum enforcement, Locked folder, OAuth
//! PKCE sign-in with Dar, directory email) and the Immich server connector.

use super::photos_books_apps_support::{
    ffmpeg, multipart_immich, setup_admin_as, sha1_base64, upload_raw,
};
use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use dar_acceptance::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

pub const SUITE: Suite = Suite {
    name: "photos-immich",
    cases: &[case!(photos_immich_api_and_connector, 300)],
};

#[derive(Clone)]
struct FakeAsset {
    id: String,
    bytes: Vec<u8>,
    name: String,
    content_type: String,
    favorite: bool,
    created_at: String,
    size: Option<u64>,
}

#[derive(Clone)]
struct FakeAlbum {
    id: String,
    album_name: String,
    asset_ids: Vec<String>,
}

struct FakeImmich {
    url: String,
    received: Arc<Mutex<Vec<(String, String)>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeImmich {
    async fn start(assets: Vec<FakeAsset>, albums: Vec<FakeAlbum>) -> Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new((assets, albums, received.clone()));
        let app = Router::new().fallback(
            move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
                let state = state.clone();
                async move { handle_fake_immich(method, uri, headers, body, state).await }
            },
        );
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(FakeImmich {
            url: format!("http://127.0.0.1:{port}"),
            received,
            shutdown: Some(tx),
        })
    }

    fn close(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn handle_fake_immich(
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    state: Arc<(
        Vec<FakeAsset>,
        Vec<FakeAlbum>,
        Arc<Mutex<Vec<(String, String)>>>,
    )>,
) -> Response {
    let (assets, albums, received) = state.as_ref();
    let path = uri.path();
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let json_resp = |status: StatusCode, value: Value| {
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response()
    };

    // A proxy in front that moved Immich: same host, permanent redirect.
    if let Some(rest) = path.strip_prefix("/old") {
        let location = format!("{rest}{query}");
        return (
            StatusCode::PERMANENT_REDIRECT,
            [(header::LOCATION, location)],
        )
            .into_response();
    }
    if path == "/api/server/ping" {
        return json_resp(StatusCode::OK, json!({"res": "pong"}));
    }
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("good-key") {
        return json_resp(
            StatusCode::UNAUTHORIZED,
            json!({"message": "Invalid API key", "statusCode": 401}),
        );
    }
    if path == "/api/users/me" {
        return json_resp(
            StatusCode::OK,
            json!({"id": uuid(), "name": "Remote", "email": "remote@example.com"}),
        );
    }
    if path == "/api/search/metadata" && method == Method::POST {
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let list: Vec<&FakeAsset> = if let Some(album_id) = payload["albumIds"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
        {
            let member_ids: HashSet<&str> = albums
                .iter()
                .find(|al| al.id == album_id)
                .map(|al| al.asset_ids.iter().map(String::as_str).collect())
                .unwrap_or_default();
            assets
                .iter()
                .filter(|a| member_ids.contains(a.id.as_str()))
                .collect()
        } else {
            assets.iter().collect()
        };
        let items: Vec<Value> = list
            .into_iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "checksum": sha1_base64(&a.bytes),
                    "originalFileName": a.name,
                    "originalMimeType": a.content_type,
                    "fileCreatedAt": a.created_at,
                    "fileModifiedAt": a.created_at,
                    "isFavorite": a.favorite,
                    "visibility": "timeline",
                    "isTrashed": false,
                    "type": "IMAGE",
                    "exifInfo": {
                        "description": if a.favorite { json!("from immich") } else { Value::Null },
                        "fileSizeInByte": a.size.unwrap_or(a.bytes.len() as u64),
                    }
                })
            })
            .collect();
        let count = items.len();
        return json_resp(
            StatusCode::OK,
            json!({
                "assets": {"items": items, "nextPage": null, "count": count, "total": count, "facets": [], "nextCursor": null},
                "albums": {"items": [], "total": 0, "count": 0, "facets": []}
            }),
        );
    }
    if let Some(rest) = path.strip_prefix("/api/assets/")
        && let Some(id) = rest.strip_suffix("/original")
        && !id.contains('/')
    {
        if let Some(asset) = assets.iter().find(|x| x.id == id) {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, asset.content_type.clone())],
                asset.bytes.clone(),
            )
                .into_response();
        }
        return json_resp(StatusCode::NOT_FOUND, json!({}));
    }
    if path == "/api/albums" {
        let items: Vec<Value> = albums
            .iter()
            .map(|a| json!({"id": a.id, "albumName": a.album_name, "description": "", "assetCount": a.asset_ids.len()}))
            .collect();
        return json_resp(StatusCode::OK, Value::Array(items));
    }
    if path == "/api/assets" && method == Method::POST {
        let checksum = headers
            .get("x-immich-checksum")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let text = String::from_utf8_lossy(&body);
        let name = regex::Regex::new(r#"filename="([^"]+)""#)
            .unwrap()
            .captures(&text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_default();
        received.lock().unwrap().push((checksum, name));
        return json_resp(
            StatusCode::CREATED,
            json!({"id": uuid(), "status": "created"}),
        );
    }
    json_resp(StatusCode::NOT_FOUND, json!({}))
}

#[derive(Default)]
struct DeviceState {
    users: HashSet<String>,
    assets: HashSet<String>,
    albums: HashSet<String>,
    checksums: HashMap<String, String>,
}

impl DeviceState {
    /// Mirrors the Immich mobile app's SQLite foreign-key and unique-(owner, checksum) constraints.
    fn apply(&mut self, line: &Value) -> Result<()> {
        let kind = line["type"].as_str().context("line type")?;
        let d = &line["data"];
        let need = |held: &HashSet<String>, id: Option<&str>, what: &str| -> Result<()> {
            ensure!(
                id.is_some_and(|i| held.contains(i)),
                "{kind} names {what} {id:?} the app does not hold"
            );
            Ok(())
        };
        match kind {
            "SyncResetV1" => {
                self.users.clear();
                self.assets.clear();
                self.albums.clear();
                self.checksums.clear();
            }
            "UserV1" => {
                self.users
                    .insert(d["id"].as_str().context("UserV1 id")?.to_owned());
            }
            "AssetV2" | "AlbumAssetCreateV2" => {
                let owner_id = d["ownerId"].as_str();
                need(&self.users, owner_id, "owner")?;
                let id = d["id"].as_str().context("asset id")?.to_owned();
                let checksum = d["checksum"].as_str().unwrap_or_default();
                let key = format!("{}/{checksum}", owner_id.unwrap());
                let replaced: Vec<String> = self
                    .checksums
                    .iter()
                    .filter(|(other_id, other_key)| *other_key == &key && *other_id != &id)
                    .map(|(other_id, _)| other_id.clone())
                    .collect();
                for old_id in replaced {
                    self.assets.remove(&old_id);
                    self.checksums.remove(&old_id);
                }
                self.assets.insert(id.clone());
                self.checksums.insert(id, key);
            }
            "AssetDeleteV1" => {
                let id = d["assetId"].as_str().context("assetId")?;
                self.assets.remove(id);
                self.checksums.remove(id);
            }
            "AssetExifV1" | "AlbumAssetExifCreateV1" => {
                need(&self.assets, d["assetId"].as_str(), "asset")?;
            }
            "AlbumV2" => {
                if let Some(cover) = d["thumbnailAssetId"].as_str() {
                    need(&self.assets, Some(cover), "cover")?;
                }
                self.albums
                    .insert(d["id"].as_str().context("album id")?.to_owned());
            }
            "AlbumDeleteV1" => {
                self.albums
                    .remove(d["albumId"].as_str().context("albumId")?);
            }
            "AlbumUserV1" => {
                need(&self.albums, d["albumId"].as_str(), "album")?;
                need(&self.users, d["userId"].as_str(), "user")?;
            }
            "AlbumToAssetV1" => {
                need(&self.albums, d["albumId"].as_str(), "album")?;
                need(&self.assets, d["assetId"].as_str(), "asset")?;
            }
            "StackV1" => {
                need(&self.users, d["ownerId"].as_str(), "owner")?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn app_cookie(reply: &Reply) -> String {
    reply
        .set_cookies()
        .into_iter()
        .map(|c| c.split(';').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("; ")
}

pub async fn photos_immich_api_and_connector(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-photos-immich")?;
    let beach_path = root.join("beach.jpg");
    let wave_path = root.join("wave.mp4");
    let remote_path = root.join("remote.jpg");
    let solo_path = root.join("solo.jpg");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240",
        "-frames:v",
        "1",
        &beach_path.to_string_lossy(),
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=10",
        "-t",
        "2",
        "-pix_fmt",
        "yuv420p",
        &wave_path.to_string_lossy(),
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "mandelbrot=size=320x240",
        "-frames:v",
        "1",
        &remote_path.to_string_lossy(),
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "rgbtestsrc=size=320x240",
        "-frames:v",
        "1",
        &solo_path.to_string_lossy(),
    ])
    .await?;
    let beach = std::fs::read(&beach_path)?;
    let wave = std::fs::read(&wave_path)?;
    let remote_bytes = std::fs::read(&remote_path)?;
    let solo = std::fs::read(&solo_path)?;

    let instance = Instance::isolated(&root, "dar-photos", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();
    let mut fake: Option<FakeImmich> = None;

    let result: Result<()> = async {
        setup_admin_as(&base, "Anas").await?;
        let session = login(&base).await?;
        // Trigger the initial empty background scan now so it switches to its 30s cadence
        // instead of waking at t=2s mid-upload and re-indexing an asset during /sync/stream.
        session.get_json("/api/v1/photos/overview").await?;

        let app_password = |scope: &str| {
            let session = session.clone();
            let scope = scope.to_owned();
            async move {
                let r = session.post("/api/v1/auth/app-passwords", &json!({"label": "Phone", "scope": scope})).await?.expect(201)?;
                r.json()
            }
        };
        let photos_password = app_password("v2 photos:read photos:write").await?;
        let files_password = app_password("v2 files:read").await?;
        assert_eq!(photos_password["immichUrl"], base.as_str());
        let photos_secret = photos_password["secret"].as_str().context("photos secret")?.to_owned();
        let files_secret = files_password["secret"].as_str().context("files secret")?.to_owned();

        // Discovery and public server information.
        let client = session::client();
        assert_eq!(Reply::from(client.get(format!("{base}/.well-known/immich")).send().await?).await?.json()?, json!({"api": {"endpoint": "/dav/immich/api"}}));
        let api = format!("{base}/dav/immich/api");
        assert_eq!(Reply::from(client.get(format!("{api}/server/ping")).send().await?).await?.json()?, json!({"res": "pong"}));
        let version = Reply::from(client.get(format!("{api}/server/version")).send().await?).await?.json()?;
        assert_eq!((version["major"].as_u64(), version["minor"].as_u64(), version["patch"].as_u64()), (Some(3), Some(4), Some(1)));
        let features = Reply::from(client.get(format!("{api}/server/features")).send().await?).await?.json()?;
        assert_eq!(
            json!({"passwordLogin": features["passwordLogin"], "oauth": features["oauth"], "oauthAutoLaunch": features["oauthAutoLaunch"]}),
            json!({"passwordLogin": true, "oauth": true, "oauthAutoLaunch": false})
        );
        assert_eq!(Reply::from(client.get(format!("{api}/server/config")).send().await?).await?.json()?["oauthButtonText"], "Sign in with Dar");
        let media_types = Reply::from(client.get(format!("{api}/server/media-types")).send().await?).await?.json()?;
        assert!(media_types["image"].as_array().unwrap().iter().any(|x| x == ".heic"));

        // Sign-in: the username before "@", and an app password that allows Photos.
        let sign_in = |email: &str, password: &str| {
            let url = format!("{api}/auth/login");
            let body = json!({"email": email, "password": password});
            async move { Reply::from(session::client().post(url).json(&body).send().await?).await }
        };
        assert_eq!(sign_in("admin@dar", PASSWORD).await?.status, 401);
        let refused = sign_in("admin@dar", &files_secret).await?;
        assert_eq!(refused.status, 401);
        assert!(refused.json()?["message"].as_str().unwrap_or_default().contains("no Photos access"));
        let signed = sign_in("Admin@example.org", &photos_secret).await?;
        assert_eq!(signed.status, 201);
        let signed_body = signed.json()?;
        let access_token = signed_body["accessToken"].as_str().context("accessToken")?.to_owned();
        assert_eq!(signed_body["userEmail"], "admin@dar");

        let immich_call = |route: &str, method: &str, body: Option<Value>| {
            let route = route.to_owned();
            let url = format!("{api}{route}");
            let token = access_token.clone();
            let method = method.to_owned();
            async move {
                let mut req = session::client()
                    .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url)
                    .header("x-immich-user-token", &token);
                if let Some(body) = body {
                    req = req.json(&body);
                }
                let r = Reply::from(req.send().await?).await?;
                ensure!((200..300).contains(&r.status), "{route}: {} {}", r.status, r.text());
                if r.status == 204 { Ok(Value::Null) } else { r.json() }
            }
        };

        assert_eq!(client.get(format!("{api}/users/me")).header("x-immich-user-token", "x".repeat(43)).send().await?.status().as_u16(), 401);
        assert_eq!(immich_call("/users/me", "GET", None).await?["email"], "admin@dar");
        let preferences = immich_call("/users/me/preferences", "GET", None).await?;
        assert_eq!(preferences["memories"], json!({"enabled": false, "duration": 5, "sidebarWeb": false}));
        assert_eq!(preferences["people"], json!({"enabled": false, "sidebarWeb": false, "updateStrategy": "self"}));
        assert_eq!(client.get(format!("{api}/users/me")).bearer_auth(&access_token).send().await?.status().as_u16(), 200);
        let signed_cookie = app_cookie(&signed);
        assert!(signed_cookie.contains(&format!("immich_access_token={access_token}")));
        assert_eq!(client.get(format!("{api}/users/me")).header("cookie", &signed_cookie).send().await?.status().as_u16(), 200);

        // Backup: multipart upload into pics/YYYY/MM, duplicates by SHA-1.
        let upload_asset = |bytes: &[u8], file: &str, content_type: &str, extra_fields: Vec<(&str, &str)>, token: &str| {
            let url = format!("{api}/assets");
            let mut fields = vec![
                ("deviceAssetId", file),
                ("deviceId", "phone"),
                ("fileCreatedAt", "2024-03-05T10:00:00.000Z"),
                ("fileModifiedAt", "2024-03-05T10:00:00.000Z"),
                ("filename", file),
            ];
            fields.extend(extra_fields);
            let (mp_type, mp_body) = multipart_immich(&fields, file, content_type, bytes);
            let checksum = sha1_base64(bytes);
            let token = token.to_owned();
            async move {
                let r = Reply::from(
                    session::client()
                        .post(url)
                        .header("x-immich-user-token", token)
                        .header("content-type", mp_type)
                        .header("x-immich-checksum", checksum)
                        .body(mp_body)
                        .send()
                        .await?,
                )
                .await?;
                let mut value = r.json()?;
                value["http"] = json!(r.status);
                Ok::<Value, Error>(value)
            }
        };

        let photo = upload_asset(&beach, "beach.jpg", "image/jpeg", vec![("isFavorite", "true")], &access_token).await?;
        assert_eq!((photo["http"].as_u64(), photo["status"].as_str()), (Some(201), Some("created")));
        let photo_id = photo["id"].as_str().context("photo id")?.to_owned();
        let dup = upload_asset(&beach, "beach copy.jpg", "image/jpeg", vec![], &access_token).await?;
        assert_eq!(dup, json!({"http": 200, "id": photo_id, "status": "duplicate"}));
        let video = upload_asset(&wave, "wave.mp4", "application/octet-stream", vec![], &access_token).await?;
        assert_eq!(video["http"], 201);
        let video_id = video["id"].as_str().context("video id")?.to_owned();

        let check = immich_call(
            "/assets/bulk-upload-check",
            "POST",
            Some(json!({"assets": [{"id": "a", "checksum": sha1_base64(&beach)}, {"id": "b", "checksum": sha1_base64(b"new")}]})),
        )
        .await?;
        assert_eq!(
            check["results"],
            json!([{"id": "a", "action": "reject", "reason": "duplicate", "assetId": photo_id, "isTrashed": false}, {"id": "b", "action": "accept"}])
        );
        let detail = immich_call(&format!("/assets/{photo_id}"), "GET", None).await?;
        assert_eq!(detail["isFavorite"], json!(true));
        assert_eq!(detail["checksum"], sha1_base64(&beach));
        assert_eq!(detail["exifInfo"]["exifImageWidth"], 320);

        let db = session.connect().await?;
        let where_res = procedure(|done| db.procedures.file_details_then(photo_id.clone(), done)).await;
        db.disconnect();
        let where_res = where_res?;
        assert_eq!(
            where_res["ancestors"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["pics", "2024", "03"]
        );

        // Media.
        let thumb = Reply::from(client.get(format!("{api}/assets/{photo_id}/thumbnail?size=thumbnail")).header("x-immich-user-token", &access_token).send().await?).await?;
        assert_eq!(thumb.header("content-type"), Some("image/webp"));
        let orig = Reply::from(client.get(format!("{api}/assets/{photo_id}/original")).header("x-immich-user-token", &access_token).send().await?).await?;
        assert_eq!(orig.body, beach);
        let play = Reply::from(
            client
                .get(format!("{api}/assets/{video_id}/video/playback"))
                .header("x-immich-user-token", &access_token)
                .header("range", "bytes=0-9")
                .send()
                .await?,
        )
        .await?;
        assert_eq!(play.status, 206);
        assert_eq!(play.header("content-type"), Some("video/mp4"));

        // Sync stream: everything first, then only changes, deletes and resets.
        let sync_types = json!(["AuthUsersV2", "UsersV1", "AssetsV2", "AssetExifsV1", "AlbumsV2", "AlbumUsersV1", "AlbumToAssetsV1", "AlbumAssetsV2", "StacksV1", "PeopleV1", "MemoriesV2"]);
        let mut device = DeviceState::default();

        async fn run_stream(api: &str, token: &str, types: &Value, device: &mut DeviceState) -> Result<Vec<Value>> {
            let r = Reply::from(
                session::client()
                    .post(format!("{api}/sync/stream"))
                    .header("x-immich-user-token", token)
                    .header("accept", "application/jsonlines+json")
                    .json(&json!({"types": types}))
                    .send()
                    .await?,
            )
            .await?
            .expect(200)?;
            let mut lines = Vec::new();
            for line in r.text().trim().lines() {
                if !line.is_empty() {
                    let parsed: Value = serde_json::from_str(line)?;
                    device.apply(&parsed)?;
                    lines.push(parsed);
                }
            }
            let mut last: HashMap<String, String> = HashMap::new();
            for l in &lines {
                if let Some(ack) = l["ack"].as_str() {
                    let prefix = ack.split('|').next().unwrap_or_default().to_owned();
                    last.insert(prefix, ack.to_owned());
                }
            }
            let acks: Vec<String> = last.into_values().collect();
            let ack_resp = Reply::from(
                session::client()
                    .post(format!("{api}/sync/ack"))
                    .header("x-immich-user-token", token)
                    .json(&json!({"acks": acks}))
                    .send()
                    .await?,
            )
            .await?;
            ensure!((200..300).contains(&ack_resp.status), "/sync/ack: {} {}", ack_resp.status, ack_resp.text());
            Ok(lines)
        }
        let kinds = |lines: &[Value]| -> Vec<String> { lines.iter().filter_map(|l| l["type"].as_str().map(str::to_owned)).collect() };

        let mut lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert_eq!(kinds(&lines).last().map(String::as_str), Some("SyncCompleteV1"));
        assert_eq!(kinds(&lines).first().map(String::as_str), Some("AuthUserV2"));
        let assets: Vec<&Value> = lines.iter().filter(|l| l["type"] == "AssetV2").collect();
        let mut asset_ids: Vec<String> = assets.iter().map(|l| l["data"]["id"].as_str().unwrap().to_owned()).collect();
        asset_ids.sort();
        let mut expected_pv = vec![photo_id.clone(), video_id.clone()];
        expected_pv.sort();
        assert_eq!(asset_ids, expected_pv);
        let synced = &assets.iter().find(|l| l["data"]["id"] == photo_id.as_str()).unwrap()["data"];
        assert_eq!(synced["checksum"], sha1_base64(&beach));
        assert_eq!(synced["isFavorite"], json!(true));
        assert_eq!(synced["type"], "IMAGE");
        assert_eq!(synced["visibility"], "timeline");
        assert_eq!(assets.iter().find(|l| l["data"]["id"] == video_id.as_str()).unwrap()["data"]["duration"], 2000);
        assert_eq!(lines.iter().filter(|l| l["type"] == "AssetExifV1").count(), 2);

        assert_eq!(kinds(&run_stream(&api, &access_token, &sync_types, &mut device).await?), vec!["AuthUserV2", "UserV1", "SyncCompleteV1"]);

        immich_call(&format!("/assets/{photo_id}"), "PUT", Some(json!({"isFavorite": false}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        let asset_kinds: Vec<String> = kinds(&lines).into_iter().filter(|t| t.starts_with("Asset")).collect();
        assert_eq!(asset_kinds, vec!["AssetV2", "AssetExifV1"]);
        assert_eq!(lines.iter().find(|l| l["type"] == "AssetV2").unwrap()["data"]["isFavorite"], json!(false));

        let album = immich_call("/albums", "POST", Some(json!({"albumName": "Beach", "assetIds": [photo_id]}))).await?;
        assert_eq!(album["albumName"], "Beach");
        let album_id = album["id"].as_str().context("album id")?.to_owned();
        let my_id = immich_call("/users/me", "GET", None).await?["id"].as_str().context("user id")?.to_owned();
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert_eq!(lines.iter().find(|l| l["type"] == "AlbumV2").unwrap()["data"]["name"], "Beach");
        assert_eq!(lines.iter().find(|l| l["type"] == "AlbumUserV1").unwrap()["data"], json!({"albumId": album_id, "userId": my_id, "role": "owner"}));
        assert_eq!(lines.iter().find(|l| l["type"] == "AlbumToAssetV1").unwrap()["data"], json!({"albumId": album_id, "assetId": photo_id}));
        assert_eq!(immich_call(&format!("/albums/{album_id}/assets"), "DELETE", Some(json!({"ids": [photo_id]}))).await?, json!([{"id": photo_id, "success": true}]));
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert_eq!(lines.iter().find(|l| l["type"] == "AlbumToAssetDeleteV1").unwrap()["data"], json!({"albumId": album_id, "assetId": photo_id}));

        // Trash keeps the asset with deletedAt; force deletes it; a trashed folder resets the device.
        immich_call("/assets", "DELETE", Some(json!({"ids": [video_id]}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(!lines.iter().find(|l| l["type"] == "AssetV2" && l["data"]["id"] == video_id.as_str()).unwrap()["data"]["deletedAt"].is_null());
        immich_call("/trash/restore/assets", "POST", Some(json!({"ids": [video_id]}))).await?;
        immich_call("/assets", "DELETE", Some(json!({"ids": [video_id], "force": true}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(lines.iter().any(|l| l["type"] == "AssetDeleteV1" && l["data"]["assetId"] == video_id.as_str()));

        // An album whose photo the device does not hold (its folder trashed) names neither it nor its cover.
        immich_call("/albums/assets", "PUT", Some(json!({"albumIds": [album_id], "assetIds": [photo_id]}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert_eq!(lines.iter().find(|l| l["type"] == "AlbumV2").unwrap()["data"]["thumbnailAssetId"], photo_id.as_str());

        let files = session.connect().await?;
        let folder_reset_check: Result<()> = async {
            let photo_details = procedure(|done| files.procedures.file_details_then(photo_id.clone(), done)).await?;
            let parent_id = photo_details["ancestors"][0]["id"].as_str().context("ancestor id")?.to_owned();
            let folder_rev = procedure(|done| files.procedures.file_details_then(parent_id.clone(), done)).await?["entry"]["revision"]
                .as_u64()
                .context("folder revision")? as u32;
            reducer(|done| files.reducers.trash_entry_then(parent_id.clone(), folder_rev, done)).await?;
            assert_eq!(kinds(&run_stream(&api, &access_token, &sync_types, &mut device).await?), vec!["SyncResetV1"]);
            lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
            assert_eq!(lines.iter().filter(|l| l["type"] == "AssetV2").count(), 0);
            assert_eq!(kinds(&lines).last().map(String::as_str), Some("SyncCompleteV1"));
            assert_eq!(lines.iter().find(|l| l["type"] == "AlbumV2").unwrap()["data"]["thumbnailAssetId"], Value::Null);
            assert!(!lines.iter().any(|l| l["type"] == "AlbumToAssetV1"));
            reducer(|done| files.reducers.restore_entry_then(parent_id.clone(), folder_rev + 1, done)).await?;
            assert_eq!(kinds(&run_stream(&api, &access_token, &sync_types, &mut device).await?), vec!["SyncResetV1"]);
            lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
            assert!(lines.iter().any(|l| l["type"] == "AssetV2" && l["data"]["id"] == photo_id.as_str()));
            assert!(lines.iter().any(|l| l["type"] == "AlbumToAssetV1" && l["data"]["assetId"] == photo_id.as_str()));
            Ok(())
        }
        .await;
        files.disconnect();
        folder_reset_check?;

        let solo_photo = upload_asset(&solo, "solo.jpg", "image/jpeg", vec![], &access_token).await?;
        assert_eq!(solo_photo["http"], 201);
        let solo_id = solo_photo["id"].as_str().context("solo id")?.to_owned();

        // A photo moved to the Locked folder leaves the device with its album rows; moved out, both return.
        immich_call("/albums/assets", "PUT", Some(json!({"albumIds": [album_id], "assetIds": [solo_id]}))).await?;
        run_stream(&api, &access_token, &sync_types, &mut device).await?;
        session.post("/api/v1/files/locked/unlock", &json!({"password": PASSWORD})).await?.expect(200)?;
        session.post("/api/v1/photos/assets/lock", &json!({"ids": [solo_id]})).await?.expect(200)?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(lines.iter().any(|l| l["type"] == "AssetDeleteV1" && l["data"]["assetId"] == solo_id.as_str()));
        assert!(!lines.iter().any(|l| l["type"] == "AlbumToAssetV1"));
        immich_call("/sync/ack", "DELETE", Some(json!({}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(!lines.iter().any(|l| (l["type"] == "AlbumToAssetV1" && l["data"]["assetId"] == solo_id.as_str()) || l["data"]["thumbnailAssetId"] == solo_id.as_str()));
        session.post("/api/v1/photos/assets/unlock", &json!({"ids": [solo_id]})).await?.expect(200)?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(lines.iter().any(|l| l["type"] == "AlbumToAssetV1" && l["data"]["assetId"] == solo_id.as_str()));

        // A photo backed up while a stream runs waits for the next one: no later channel (EXIF, albums) names it.
        let service = instance.database()?;
        let token_digest = support::digest_token(&access_token);
        immich_call("/sync/ack", "DELETE", Some(json!({}))).await?;
        let begun = service.call("service_immich", json!([token_digest, "syncStart", json!({"reset": false}).to_string()])).await?;
        let up_to = begun["upTo"].as_i64().context("upTo")?;
        let mut during_bytes = solo.clone();
        during_bytes.push(2);
        let during = upload_asset(&during_bytes, "during.jpg", "image/jpeg", vec![], &access_token).await?;
        assert_eq!(during["http"], 201);
        let during_id = during["id"].as_str().context("during id")?.to_owned();
        for channel in ["assets", "exifs", "albumToAssets", "albums"] {
            let page = service
                .call("service_immich", json!([token_digest, "syncPage", json!({"channel": channel, "after": -1, "limit": 500, "upTo": up_to}).to_string()]))
                .await?;
            assert!(
                !page["lines"].as_array().unwrap().iter().any(|l| l["data"]["id"] == during_id.as_str() || l["data"]["assetId"] == during_id.as_str()),
                "{channel}"
            );
        }
        assert!(run_stream(&api, &access_token, &sync_types, &mut device).await?.iter().any(|l| l["type"] == "AssetExifV1" && l["data"]["assetId"] == during_id.as_str()));

        // A copy of a picture elsewhere in Files (same SHA-1): the app keeps one asset per checksum, so the copy
        // gets its own; both stay on the device, and the EXIF of each finds its asset.
        let copy_id = upload_raw(&session, "", "beach copy.jpg", "image/jpeg", &beach).await?;
        assert_eq!(session.post(&format!("/api/v1/photos/assets/{copy_id}/index"), &json!({})).await?.json()?, json!({"indexed": true}));
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        let copies: Vec<&Value> = lines
            .iter()
            .filter(|l| l["type"] == "AssetV2" && (l["data"]["id"] == photo_id.as_str() || l["data"]["id"] == copy_id.as_str()))
            .collect();
        assert_eq!(copies.len(), 2);
        let unique_checksums: HashSet<&str> = copies.iter().filter_map(|l| l["data"]["checksum"].as_str()).collect();
        assert_eq!(unique_checksums.len(), 2);
        assert!(unique_checksums.contains(sha1_base64(&beach).as_str()));
        assert!(device.assets.contains(&photo_id) && device.assets.contains(&copy_id));
        immich_call("/sync/ack", "DELETE", Some(json!({}))).await?;
        run_stream(&api, &access_token, &sync_types, &mut device).await?;
        assert!(device.assets.contains(&photo_id) && device.assets.contains(&copy_id));

        // A device whose EXIF checkpoint lags its asset checkpoint gets the assets again before their EXIF.
        device.assets.remove(&during_id);
        immich_call("/sync/ack", "DELETE", Some(json!({"types": ["AssetExifV1"]}))).await?;
        lines = run_stream(&api, &access_token, &sync_types, &mut device).await?;
        let asset_pos = lines.iter().position(|l| l["type"] == "AssetV2" && l["data"]["id"] == during_id.as_str()).context("AssetV2 for during")?;
        let exif_pos = lines.iter().position(|l| l["type"] == "AssetExifV1" && l["data"]["assetId"] == during_id.as_str()).context("AssetExifV1 for during")?;
        assert!(asset_pos < exif_pos);
        assert!(!run_stream(&api, &access_token, &sync_types, &mut device).await?.iter().any(|l| l["type"] == "AssetV2"));

        // The web app sees the devices; signing out ends the token.
        let devices = session.get_json("/api/v1/photos/immich").await?["devices"].as_array().cloned().context("devices")?;
        assert_eq!(devices.len(), 1);
        immich_call("/auth/logout", "POST", Some(json!({}))).await?;
        assert_eq!(client.get(format!("{api}/users/me")).header("x-immich-user-token", &access_token).send().await?.status().as_u16(), 401);

        // Sign in with Dar: the app's OAuth flow through Dar's own OIDC provider (PKCE, the app's scheme).
        let verifier = dar_acceptance::base64url(&dar_acceptance::random_bytes(42));
        let code_challenge = {
            use sha2::Digest as _;
            dar_acceptance::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
        };
        let oauth_state = uuid().replace('-', "");
        let authorize = |redirect_uri: &str| {
            let url = format!("{api}/oauth/authorize");
            let body = json!({"redirectUri": redirect_uri, "state": oauth_state, "codeChallenge": code_challenge});
            async move { Reply::from(session::client().post(url).json(&body).send().await?).await }
        };
        assert_eq!(authorize("https://evil.example/callback").await?.status, 400);
        let started = authorize("app.immich:///oauth-callback").await?.expect(201)?;
        let start_url = started.json()?["url"].as_str().context("oauth start url")?.to_owned();
        let followed = follow(&base, &session, &start_url, false, None, |_| {}).await?;
        assert_eq!(followed.url.scheme(), "app.immich");

        let finish = |callback: &str, code_verifier: &str, state: &str| {
            let url = format!("{api}/oauth/callback");
            let body = json!({"url": callback, "state": state, "codeVerifier": code_verifier});
            async move {
                Reply::from(
                    session::client()
                        .post(url)
                        .header("deviceModel", "Pixel 6")
                        .header("deviceType", "Android")
                        .json(&body)
                        .send()
                        .await?,
                )
                .await
            }
        };
        assert_eq!(finish(followed.url.as_str(), &verifier, "other").await?.status, 401);
        assert_eq!(finish(followed.url.as_str(), &dar_acceptance::base64url(&dar_acceptance::random_bytes(42)), &oauth_state).await?.status, 401);
        let finished = finish(followed.url.as_str(), &verifier, &oauth_state).await?.expect(201)?;
        let oauth_login = finished.json()?;
        assert_eq!(oauth_login["userEmail"], "admin@dar");
        assert_eq!(client.get(format!("{api}/users/me")).header("cookie", app_cookie(&finished)).send().await?.status().as_u16(), 200);
        assert_eq!(finish(followed.url.as_str(), &verifier, &oauth_state).await?.status, 401);
        let oauth_token = oauth_login["accessToken"].as_str().context("oauth accessToken")?.to_owned();
        assert_eq!(
            Reply::from(client.get(format!("{api}/users/me")).header("x-immich-user-token", &oauth_token).send().await?).await?.json()?["email"],
            "admin@dar"
        );
        let passwords = || {
            let session = session.clone();
            async move { Ok::<Vec<Value>, Error>(session.get_json("/api/v1/auth/app-passwords").await?["items"].as_array().cloned().unwrap_or_default()) }
        };
        let pixel_scopes: Vec<String> = passwords()
            .await?
            .into_iter()
            .filter(|p| p["label"] == "Immich on Pixel 6")
            .filter_map(|p| p["scope"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(pixel_scopes, vec!["v2 photos:read photos:write"]);
        let mut solo_again = solo.clone();
        solo_again.push(1);
        assert_eq!(upload_asset(&solo_again, "solo again.jpg", "image/jpeg", vec![], &oauth_token).await?["http"], 201);
        client
            .post(format!("{api}/auth/logout"))
            .header("x-immich-user-token", &oauth_token)
            .json(&json!({}))
            .send()
            .await?;
        assert_eq!(client.get(format!("{api}/users/me")).header("x-immich-user-token", &oauth_token).send().await?.status().as_u16(), 401);
        assert!(!passwords().await?.iter().any(|p| p["label"] == "Immich on Pixel 6"));

        // With a directory email, the app shows it and signs in with it.
        let directory = IdentityAdmin::new(&base).await?;
        let user_id = directory
            .get("/users")
            .await?["items"]
            .as_array()
            .and_then(|arr| arr.iter().find(|u| u["userName"] == "admin"))
            .and_then(|u| u["id"].as_str())
            .context("admin user id")?
            .to_owned();
        let current = directory.api(&format!("/users/{user_id}"), "GET", None).await?.expect(200)?;
        let etag = current.header("etag").context("user etag")?.to_owned();
        directory
            .write(
                &format!("/users/{user_id}"),
                "PATCH",
                Some(&json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": [{"op": "add", "path": "emails", "value": [{"value": "Anas@Example.org", "type": "work", "primary": true}]}]
                })),
                Some(&etag),
            )
            .await?
            .expect(200)?;
        let by_email = sign_in("anas@example.org", &photos_secret).await?.expect(201)?;
        assert_eq!(by_email.json()?["userEmail"], "anas@example.org");
        assert_eq!(
            Reply::from(client.get(format!("{api}/users/me")).header("cookie", app_cookie(&by_email)).send().await?).await?.json()?["email"],
            "anas@example.org"
        );
        assert_eq!(sign_in("someone@example.org", &photos_secret).await?.status, 401);

        // The connector: Immich has one new photo (a favorite) and one Dar already has, in an album.
        // A video over Dar's 16 GiB limit and a photo with a bogus capture year must not stop the import.
        let remote_id = uuid();
        let same_id = uuid();
        let mut undated_bytes = solo.clone();
        undated_bytes.push(0);
        let started_fake = FakeImmich::start(
            vec![
                FakeAsset { id: uuid(), bytes: wave.clone(), name: "huge.mov".into(), content_type: "video/quicktime".into(), favorite: false, created_at: "2023-01-01T00:00:00.000Z".into(), size: Some(17 * 1024 * 1024 * 1024) },
                FakeAsset { id: uuid(), bytes: undated_bytes, name: "undated.jpg".into(), content_type: "image/jpeg".into(), favorite: false, created_at: "0001-01-01T00:00:00.000Z".into(), size: None },
                FakeAsset { id: remote_id.clone(), bytes: remote_bytes.clone(), name: "remote.jpg".into(), content_type: "image/jpeg".into(), favorite: true, created_at: "2023-08-01T12:00:00.000Z".into(), size: None },
                FakeAsset { id: same_id.clone(), bytes: beach.clone(), name: "beach.jpg".into(), content_type: "image/jpeg".into(), favorite: false, created_at: "2024-03-05T10:00:00.000Z".into(), size: None },
            ],
            vec![FakeAlbum { id: uuid(), album_name: "Remote trip".into(), asset_ids: vec![remote_id.clone(), same_id.clone()] }],
        )
        .await?;
        let fake_url = started_fake.url.clone();
        let fake_received = started_fake.received.clone();
        fake = Some(started_fake);

        let connect_immich = |api_key: &str, address: &str| {
            let session = session.clone();
            let body = json!({"url": address, "apiKey": api_key, "intervalMinutes": 60, "enabled": true, "revision": 0});
            async move { session.put("/api/v1/photos/immich/connection", &body).await }
        };
        let unused_port = dar_acceptance::stack::free_port()?;
        let refused_conn = connect_immich("good-key", &format!("http://127.0.0.1:{unused_port}")).await?;
        assert_eq!(refused_conn.status, 400);
        assert!(refused_conn.json()?["error"]["message"].as_str().unwrap_or_default().contains("Nothing accepted the connection"));
        let rejected = connect_immich("bad-key", &format!("{fake_url}/old")).await?;
        assert_eq!(rejected.status, 400);
        assert!(rejected.json()?["error"]["message"].as_str().unwrap_or_default().contains("rejected the API key"));
        assert_eq!(connect_immich("good-key", &format!("{fake_url}/old")).await?.status, 200);
        assert_eq!(
            session
                .put(
                    "/api/v1/photos/immich/connection",
                    &json!({"url": "http://169.254.169.254", "apiKey": "good-key", "direction": "import", "intervalMinutes": 60, "enabled": true, "revision": 1}),
                )
                .await?
                .status,
            400
        );

        let conn_state = || {
            let session = session.clone();
            async move { Ok::<Value, Error>(session.get_json("/api/v1/photos/immich").await?["connection"].clone()) }
        };
        assert_eq!(session.post("/api/v1/photos/immich/connection/sync", &json!({})).await?.status, 202);
        let mut status = conn_state().await?;
        for _ in 0..120 {
            if status["lastRunAt"].as_i64().unwrap_or(0) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            status = conn_state().await?;
        }
        assert!(
            status["lastError"]
                .as_str()
                .unwrap_or_default()
                .starts_with("1 file could not be imported (huge.mov: it is larger than 16 GiB"),
            "lastError={}",
            status["lastError"]
        );
        assert_eq!((status["summary"]["imported"].as_u64(), status["summary"]["albums"].as_u64()), (Some(2), Some(1)));

        let page_items = session.get_json("/api/v1/photos/assets").await?["items"].as_array().cloned().context("assets items")?;
        let imported = page_items.iter().find(|i| i["name"] == "remote.jpg").context("remote.jpg")?;
        assert!(page_items.iter().any(|i| i["name"] == "undated.jpg"));
        let imported_id = imported["id"].as_str().context("imported id")?;
        let imported_detail = session.get_json(&format!("/api/v1/photos/assets/{imported_id}")).await?;
        assert_eq!(imported_detail["favorite"], json!(true));
        assert_eq!(imported_detail["description"], "from immich");
        let taken_ms = imported_detail["takenAt"].as_i64().context("takenAt")?;
        assert_eq!(chrono::DateTime::from_timestamp_millis(taken_ms).unwrap().format("%Y").to_string(), "2023");

        let overview = session.get_json("/api/v1/photos/overview").await?;
        assert_eq!(
            overview["albums"]
                .as_array()
                .and_then(|arr| arr.iter().find(|a| a["name"] == "Remote trip"))
                .and_then(|a| a["count"].as_u64()),
            Some(2)
        );
        // Import only: nothing goes to Immich; once done there is no progress left.
        assert!(fake_received.lock().unwrap().is_empty());
        assert_eq!(session.get_json("/api/v1/photos/overview").await?["immich"], Value::Null);
        assert_eq!(
            session
                .put(
                    "/api/v1/photos/immich/connection",
                    &json!({"url": fake_url, "apiKey": "good-key", "direction": "twoWay", "intervalMinutes": 60, "enabled": true, "revision": 1}),
                )
                .await?
                .status,
            400
        );

        session.post("/api/v1/photos/immich/connection/sync", &json!({})).await?;
        let first_run = status["lastRunAt"].as_i64().unwrap_or(0);
        for _ in 0..120 {
            status = conn_state().await?;
            if status["lastRunAt"].as_i64().unwrap_or(0) != first_run {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert_eq!(status["summary"]["imported"], 0);
        let remote_count = session.get_json("/api/v1/photos/assets").await?["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["name"].as_str().unwrap_or_default().starts_with("remote"))
            .count();
        assert_eq!(remote_count, 1);
        assert_eq!(session.delete("/api/v1/photos/immich/connection").await?.status, 200);

        Ok(())
    }
    .await;

    if let Some(mut f) = fake {
        f.close();
    }
    instance.close().await;
    result
}
