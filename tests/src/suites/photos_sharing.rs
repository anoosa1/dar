//! Photos sharing (`photos-sharing.integration.test.ts`): albums shared with people (viewer &
//! editor), shared links with password, expiry, download/metadata toggles, and visitor uploads.

use super::photos_books_apps_support::{
    create_user, ffmpeg, now_ms, photos_call, photos_raw, setup_admin_as, upload_raw,
};
use dar_acceptance::prelude::*;
use std::path::Path;

pub const SUITE: Suite = Suite { name: "photos-sharing", cases: &[case!(photos_sharing_and_links, 240)] };

async fn still(dir: &Path, source: &str, name: &str) -> Result<Vec<u8>> {
    let path = dir.join(name);
    ffmpeg(&["-f", "lavfi", "-i", &format!("{source}=size=320x240"), "-frames:v", "1", &path.to_string_lossy()]).await?;
    Ok(std::fs::read(path)?)
}

pub async fn photos_sharing_and_links(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-photos-sharing")?;
    let media_a = still(&root, "testsrc2", "a.jpg").await?;
    let media_b = still(&root, "smptebars", "b.jpg").await?;
    let media_m = still(&root, "mandelbrot", "m.jpg").await?;
    let media_y = still(&root, "rgbtestsrc", "y.jpg").await?;
    let media_v = still(&root, "testsrc", "visitor.jpg").await?;

    let instance = Instance::isolated(&root, "dar-photos", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();

    let result: Result<()> = async {
        setup_admin_as(&base, "Anas").await?;
        let admin = login(&base).await?;
        create_user(&admin, "maryam", "Maryam", PASSWORD).await?;
        create_user(&admin, "yusuf", "Yusuf", PASSWORD).await?;
        let maryam = login_as(&base, "maryam", PASSWORD).await?;
        let yusuf = login_as(&base, "yusuf", PASSWORD).await?;

        let put = |as_session: &Session, contents: &[u8], file_name: &str| {
            let as_session = as_session.clone();
            let contents = contents.to_vec();
            let file_name = file_name.to_owned();
            async move {
                let target = photos_call(&as_session, "/upload-target", "POST", Some(&json!({"year": 2025, "month": 6}))).await?;
                let parent_id = target["parentId"].as_str().context("parentId")?;
                let id = upload_raw(&as_session, parent_id, &file_name, "image/jpeg", &contents).await?;
                photos_call(&as_session, &format!("/assets/{id}/index"), "POST", Some(&json!({}))).await?;
                Ok::<String, Error>(id)
            }
        };

        let album_items = |as_session: &Session, album_id: &str| {
            let as_session = as_session.clone();
            let route = format!("/assets?view=album&album={album_id}");
            async move {
                let res = photos_call(&as_session, &route, "GET", None).await?;
                Ok::<Vec<Value>, Error>(res["items"].as_array().cloned().unwrap_or_default())
            }
        };

        let a = put(&admin, &media_a, "a.jpg").await?;
        let b = put(&admin, &media_b, "b.jpg").await?;
        let m = put(&maryam, &media_m, "m.jpg").await?;
        let y = put(&yusuf, &media_y, "y.jpg").await?;

        let album = photos_call(&admin, "/albums", "POST", Some(&json!({"name": "Trip", "ids": [a, b]}))).await?;
        let album_id = album["id"].as_str().context("album id")?.to_owned();
        assert_eq!(album["role"], "owner");
        assert_eq!(album["shared"], json!(false));

        // Sharing with people: an editor adds their own photos, a viewer only looks.
        assert_eq!(photos_raw(&admin, &format!("/albums/{album_id}/members"), "POST", Some(&json!({"username": "nobody", "role": "viewer"})), &[]).await?.status, 404);
        assert_eq!(photos_raw(&maryam, &format!("/albums/{album_id}/members"), "POST", Some(&json!({"username": "yusuf", "role": "viewer"})), &[]).await?.status, 404);
        photos_call(&admin, &format!("/albums/{album_id}/members"), "POST", Some(&json!({"username": "maryam", "role": "editor"}))).await?;
        photos_call(&admin, &format!("/albums/{album_id}/members"), "POST", Some(&json!({"username": "@Yusuf", "role": "viewer"}))).await?;

        let sharing = photos_call(&admin, &format!("/albums/{album_id}/sharing"), "GET", None).await?;
        let members: Vec<(&str, &str)> = sharing["members"]
            .as_array()
            .context("members")?
            .iter()
            .map(|x| (x["username"].as_str().unwrap_or_default(), x["role"].as_str().unwrap_or_default()))
            .collect();
        assert_eq!(members, vec![("maryam", "editor"), ("yusuf", "viewer")]);

        let seen = photos_call(&maryam, "/overview", "GET", None).await?;
        assert_eq!(seen["counts"]["shared"], 1);
        let shared_albums: Vec<(&str, &str, &str)> = seen["sharedAlbums"]
            .as_array()
            .context("sharedAlbums")?
            .iter()
            .map(|x| {
                (
                    x["name"].as_str().unwrap_or_default(),
                    x["role"].as_str().unwrap_or_default(),
                    x["owner"]["name"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(shared_albums, vec![("Trip", "editor", "Anas")]);
        assert_eq!(photos_call(&admin, "/overview", "GET", None).await?["albums"][0]["shared"], json!(true));

        let mut items = album_items(&maryam, &album_id).await?;
        let mut item_ids: Vec<String> = items.iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect();
        item_ids.sort();
        let mut expected_ab = vec![a.clone(), b.clone()];
        expected_ab.sort();
        assert_eq!(item_ids, expected_ab);
        assert!(items.iter().all(|i| i["owner"] == "Anas" && i["favorite"] != json!(true)));
        assert_eq!(photos_raw(&maryam, &format!("/assets/{a}/thumbnail"), "GET", None, &[]).await?.status, 200);
        assert_eq!(photos_raw(&maryam, &format!("/assets/{a}/original"), "GET", None, &[]).await?.status, 200);

        let theirs = photos_call(&maryam, &format!("/assets/{a}"), "GET", None).await?;
        assert_eq!(theirs["parentId"], "");
        assert_eq!(theirs["owner"], "Anas");
        assert_eq!(theirs["albums"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![album_id.as_str()]);
        assert_eq!(photos_raw(&maryam, "/assets", "PATCH", Some(&json!({"ids": [a], "favorite": true})), &[]).await?.status, 404);
        assert_eq!(
            photos_raw(&maryam, &format!("/assets/{a}/description"), "PUT", Some(&json!({"description": "x", "revision": theirs["revision"]})), &[]).await?.status,
            404
        );
        assert_eq!(photos_raw(&maryam, &format!("/albums/{album_id}"), "PATCH", Some(&json!({"name": "Mine", "revision": 1})), &[]).await?.status, 403);

        photos_call(&maryam, &format!("/albums/{album_id}/add"), "POST", Some(&json!({"ids": [m]}))).await?;
        assert_eq!(photos_raw(&maryam, &format!("/albums/{album_id}/add"), "POST", Some(&json!({"ids": [y]})), &[]).await?.status, 404);
        assert_eq!(photos_raw(&yusuf, &format!("/albums/{album_id}/add"), "POST", Some(&json!({"ids": [y]})), &[]).await?.status, 403);
        items = album_items(&admin, &album_id).await?;
        assert_eq!(items.iter().find(|i| i["id"] == m.as_str()).and_then(|i| i["owner"].as_str()), Some("Maryam"));
        assert_eq!(photos_raw(&admin, &format!("/assets/{m}/thumbnail"), "GET", None, &[]).await?.status, 200);
        assert_eq!(album_items(&yusuf, &album_id).await?.len(), 3);
        assert_eq!(photos_raw(&maryam, &format!("/albums/{album_id}/remove"), "POST", Some(&json!({"ids": [a]})), &[]).await?.status, 403);
        assert_eq!(photos_raw(&yusuf, &format!("/albums/{album_id}/remove"), "POST", Some(&json!({"ids": [m]})), &[]).await?.status, 403);

        // Search finds the shared album; a trashed photo leaves the shared view.
        let found = yusuf.get_json("/api/v1/search?q=trip").await?;
        assert!(found["items"].as_array().unwrap().iter().any(|i| i["href"] == format!("/photos?album={album_id}")));
        photos_call(&admin, "/assets/trash", "POST", Some(&json!({"ids": [b]}))).await?;
        assert!(!album_items(&maryam, &album_id).await?.iter().any(|i| i["id"] == b.as_str()));
        assert_eq!(photos_raw(&maryam, &format!("/assets/{b}/thumbnail"), "GET", None, &[]).await?.status, 404);
        photos_call(&admin, "/assets/restore", "POST", Some(&json!({"ids": [b]}))).await?;

        // Removing a person, and leaving.
        let yusuf_id = photos_call(&admin, &format!("/albums/{album_id}/sharing"), "GET", None).await?["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["username"] == "yusuf")
            .and_then(|x| x["userId"].as_str())
            .context("yusuf userId")?
            .to_owned();
        photos_call(&admin, &format!("/albums/{album_id}/members/{yusuf_id}"), "DELETE", None).await?;
        assert_eq!(photos_raw(&yusuf, &format!("/assets/{a}/thumbnail"), "GET", None, &[]).await?.status, 404);
        assert_eq!(photos_raw(&yusuf, &format!("/assets?view=album&album={album_id}"), "GET", None, &[]).await?.status, 404);
        let maryam_id = photos_call(&admin, &format!("/albums/{album_id}/sharing"), "GET", None).await?["members"][0]["userId"]
            .as_str()
            .context("maryam userId")?
            .to_owned();

        // Shared links: the token is in the URL only; the owner can copy it again.
        let link = photos_call(
            &admin,
            "/links",
            "POST",
            Some(&json!({"albumId": album_id, "expiresAt": 0, "allowDownload": true, "showMetadata": true})),
        )
        .await?;
        let link_id = link["id"].as_str().context("link id")?.to_owned();
        let link_url = link["url"].as_str().context("link url")?.to_owned();
        let token_re = regex::Regex::new(r"/s/([A-Za-z0-9_-]{43})$").unwrap();
        let token = token_re.captures(&link_url).context("link token regex")?[1].to_owned();
        assert_eq!(link_url, format!("{base}/s/{token}"));
        assert_eq!(photos_call(&admin, "/links", "GET", None).await?["links"][0]["url"], link_url.as_str());
        assert_eq!(session::client().get(format!("{base}/s/{token}")).send().await?.status().as_u16(), 200);

        let visitor = |route: &str, method: &str, body: Option<Vec<u8>>, content_type: Option<&str>, cookie: Option<&str>| {
            let url = format!("{base}/public/photos/{token}{route}");
            let base = base.clone();
            let method = method.to_owned();
            let content_type = content_type.unwrap_or("application/json").to_owned();
            let cookie = cookie.map(str::to_owned);
            async move {
                let mut req = session::client()
                    .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url)
                    .header("origin", &base)
                    .header("content-type", content_type);
                if let Some(cookie) = cookie {
                    req = req.header("cookie", cookie);
                }
                if let Some(body) = body {
                    req = req.body(body);
                }
                Reply::from(req.send().await?).await
            }
        };

        let info = visitor("", "GET", None, None, None).await?.json()?;
        assert_eq!(info["needsPassword"], json!(false));
        assert_eq!(info["title"], "Trip");
        assert_eq!(info["owner"], "Anas");
        assert_eq!(info["count"], 3);
        assert_eq!(info["allowUpload"], json!(false));

        let public_items = visitor("/assets", "GET", None, None, None).await?.json()?["items"]
            .as_array()
            .cloned()
            .context("public items")?;
        assert_eq!(public_items.len(), 3);
        assert!(public_items.iter().all(|i| i["owner"].is_null() || i["owner"] == ""));
        assert_eq!(visitor(&format!("/assets/{a}/thumbnail"), "GET", None, None, None).await?.status, 200);
        assert_eq!(visitor(&format!("/assets/{y}/thumbnail"), "GET", None, None, None).await?.status, 404);
        let original = visitor(&format!("/assets/{m}/original"), "GET", None, None, None).await?;
        assert_eq!(original.status, 200);
        assert_eq!(original.body.len(), media_m.len());
        assert!(visitor(&format!("/assets/{a}"), "GET", None, None, None).await?.json()?["takenAt"].as_i64().unwrap_or(0) > 0);

        let link_rev = link["revision"].as_u64().context("link revision")?;
        let mut edited = photos_call(
            &admin,
            &format!("/links/{link_id}"),
            "PATCH",
            Some(&json!({"revision": link_rev, "allowDownload": false, "showMetadata": false})),
        )
        .await?;
        assert_eq!(visitor(&format!("/assets/{a}/original"), "GET", None, None, None).await?.status, 403);
        assert!(visitor(&format!("/assets/{a}"), "GET", None, None, None).await?.json()?.get("takenAt").is_none());
        assert_eq!(
            photos_raw(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": link_rev, "allowDownload": true})), &[]).await?.status,
            409
        );
        assert_eq!(
            photos_raw(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": edited["revision"], "expiresAt": now_ms() - 1000})), &[]).await?.status,
            400
        );
        assert_eq!(
            photos_raw(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": edited["revision"], "password": "abc"})), &[]).await?.status,
            400
        );

        // A password: visitors see nothing until they enter it, then a path-scoped pass cookie opens the link.
        edited = photos_call(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": edited["revision"], "password": "open sesame"}))).await?;
        assert_eq!(edited["hasPassword"], json!(true));
        assert_eq!(visitor("", "GET", None, None, None).await?.json()?, json!({"needsPassword": true}));
        assert_eq!(visitor("/assets", "GET", None, None, None).await?.status, 403);
        assert_eq!(visitor(&format!("/assets/{a}/thumbnail"), "GET", None, None, None).await?.status, 403);
        assert_eq!(visitor("/password", "POST", Some(json!({"password": "wrong"}).to_string().into_bytes()), None, None).await?.status, 403);
        // Without Origin header -> 403.
        let no_origin = Reply::from(
            session::client()
                .post(format!("{base}/public/photos/{token}/password"))
                .header("content-type", "application/json")
                .body(json!({"password": "open sesame"}).to_string())
                .send()
                .await?,
        )
        .await?;
        assert_eq!(no_origin.status, 403);

        let opened = visitor("/password", "POST", Some(json!({"password": "open sesame"}).to_string().into_bytes()), None, None).await?;
        assert_eq!(opened.status, 200);
        let pass = opened.header("set-cookie").context("pass set-cookie")?;
        let pass_re = regex::Regex::new(&format!(r"^dar_link_pass=[A-Za-z0-9_-]{{43}}; Path=/public/photos/{token}; HttpOnly; SameSite=Strict; Max-Age=43200$")).unwrap();
        assert!(pass_re.is_match(pass), "unexpected set-cookie: {pass}");
        let pass_cookie = pass.split(';').next().unwrap_or_default().to_owned();
        let info = visitor("", "GET", None, None, Some(&pass_cookie)).await?.json()?;
        assert_eq!(info["needsPassword"], json!(false));
        assert_eq!(visitor("/assets", "GET", None, None, Some(&pass_cookie)).await?.status, 200);

        // Changing the password signs visitors out.
        edited = photos_call(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": edited["revision"], "password": "another secret"}))).await?;
        assert_eq!(visitor("/assets", "GET", None, None, Some(&pass_cookie)).await?.status, 403);
        edited = photos_call(&admin, &format!("/links/{link_id}"), "PATCH", Some(&json!({"revision": edited["revision"], "password": null, "allowUpload": true}))).await?;
        assert_eq!(edited["hasPassword"], json!(false));

        // Visitor uploads land in the owner's Photos folder and the album, with a free name.
        for _ in 0..2 {
            let begin = visitor(
                "/uploads",
                "POST",
                Some(json!({"name": "visitor.jpg", "size": media_v.len(), "mediaType": "image/jpeg", "year": 2025, "month": 7}).to_string().into_bytes()),
                None,
                None,
            )
            .await?;
            assert_eq!(begin.status, 201, "{}", begin.text());
            let upload_id = begin.json()?["id"].as_str().context("visitor upload id")?.to_owned();
            let sent = visitor(&format!("/uploads/{upload_id}"), "PUT", Some(media_v.clone()), Some("application/octet-stream"), None).await?;
            assert_eq!(sent.status, 200, "{}", sent.text());
        }
        items = album_items(&admin, &album_id).await?;
        let uploaded: Vec<&Value> = items.iter().filter(|i| i["name"].as_str().unwrap_or_default().starts_with("visitor")).collect();
        let mut uploaded_names: Vec<&str> = uploaded.iter().filter_map(|i| i["name"].as_str()).collect();
        uploaded_names.sort_unstable();
        assert_eq!(uploaded_names, vec!["visitor (2).jpg", "visitor.jpg"]);
        assert!(uploaded.iter().all(|i| i["owner"].is_null() || i["owner"] == ""));
        let first_uploaded_id = uploaded[0]["id"].as_str().context("uploaded id")?;
        assert_eq!(photos_call(&admin, &format!("/assets/{first_uploaded_id}"), "GET", None).await?["width"], 320);
        assert_eq!(
            visitor(
                "/uploads",
                "POST",
                Some(json!({"name": "notes.txt", "size": 3, "mediaType": "text/plain", "year": 2025, "month": 7}).to_string().into_bytes()),
                None,
                None,
            )
            .await?
            .status,
            400
        );

        // Links to chosen photos; no uploads there; removing a link ends it.
        let chosen = photos_call(&admin, "/links", "POST", Some(&json!({"ids": [b]}))).await?;
        assert_eq!(chosen["count"], 1);
        assert_eq!(chosen["albumName"], "");
        assert_eq!(photos_raw(&admin, "/links", "POST", Some(&json!({"ids": [b], "allowUpload": true})), &[]).await?.status, 400);
        assert_eq!(photos_raw(&maryam, "/links", "POST", Some(&json!({"ids": [a]})), &[]).await?.status, 404);
        let chosen_url = chosen["url"].as_str().context("chosen url")?;
        let chosen_token = chosen_url.rsplit_once("/s/").context("chosen token")?.1;
        let chosen_assets = Reply::from(session::client().get(format!("{base}/public/photos/{chosen_token}/assets")).send().await?).await?.json()?;
        assert_eq!(chosen_assets["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![b.as_str()]);
        let chosen_id = chosen["id"].as_str().context("chosen id")?;
        photos_call(&admin, &format!("/links/{chosen_id}"), "DELETE", None).await?;
        assert_eq!(session::client().get(format!("{base}/public/photos/{chosen_token}")).send().await?.status().as_u16(), 404);
        assert_eq!(session::client().get(format!("{base}/public/photos/{}", "x".repeat(43))).send().await?.status().as_u16(), 404);

        // Leaving; deleting the album ends its links.
        photos_call(&maryam, &format!("/albums/{album_id}/members/{maryam_id}"), "DELETE", None).await?;
        assert_eq!(photos_call(&maryam, "/overview", "GET", None).await?["sharedAlbums"].as_array().map(Vec::len), Some(0));
        photos_call(&admin, &format!("/albums/{album_id}"), "DELETE", None).await?;
        assert_eq!(visitor("", "GET", None, None, None).await?.status, 404);

        Ok(())
    }
    .await;

    instance.close().await;
    result
}
