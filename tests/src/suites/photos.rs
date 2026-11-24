//! Photos (`photos.integration.test.ts`): indexing, thumbnails, views, albums, descriptions,
//! search, trash and isolation.

use super::photos_books_apps_support::{
    create_user, parse_iso_ms, photos_call, photos_media_fixtures, photos_raw, setup_admin_as,
    upload_raw, url_encode,
};
use dar_acceptance::prelude::*;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "photos",
    cases: &[case!(photos_api_and_isolation, 240)],
};

pub async fn photos_api_and_isolation(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-photos-acceptance")?;
    let media = photos_media_fixtures(&root).await?;
    let instance = Instance::isolated(&root, "dar-photos", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();

    let result: Result<()> = async {
        setup_admin_as(&base, "Photos fixture").await?;
        let session = login(&base).await?;

        let page = |query: &str| {
            let session = session.clone();
            let route = format!("/assets{query}");
            async move { photos_call(&session, &route, "GET", None).await }
        };

        // Uploads land in Photos/YYYY/MM (created once) and index immediately.
        let target = photos_call(&session, "/upload-target", "POST", Some(&json!({"year": 2024, "month": 4}))).await?;
        let target_parent = target["parentId"].as_str().context("target parentId")?.to_owned();
        let target_again = photos_call(&session, "/upload-target", "POST", Some(&json!({"year": 2024, "month": 4}))).await?;
        assert_eq!(target_again["parentId"], target_parent.as_str());

        let trip = upload_raw(&session, &target_parent, "trip.jpg", "image/jpeg", &media.trip).await?;
        let clip = upload_raw(&session, &target_parent, "clip.mp4", "video/mp4", &media.clip).await?;
        let broken = upload_raw(&session, &target_parent, "broken.jpg", "image/jpeg", b"not really a jpeg").await?;
        // The runtime's background scan (2 s after start, then every 30 s) can index an upload
        // between the upload and its index call, which then rightly answers `indexed: false`
        // (a job the scan has not stored yet is indexed again). Either way the upload is indexed
        // once the call returns, which the detail shows.
        for id in [&trip, &clip, &broken] {
            let reply = photos_call(&session, &format!("/assets/{id}/index"), "POST", Some(&json!({}))).await?;
            if reply != json!({"indexed": true}) {
                assert_eq!(reply, json!({"indexed": false}));
                photos_call(&session, &format!("/assets/{id}"), "GET", None).await?;
            }
        }
        assert_eq!(photos_call(&session, &format!("/assets/{trip}/index"), "POST", Some(&json!({}))).await?, json!({"indexed": false}));

        // Files outside Photos are found by the background scan (kicked by the overview); text files are ignored.
        let plain = upload_raw(&session, "", "plain.png", "image/png", &media.plain).await?;
        upload_raw(&session, "", "notes.txt", "text/plain", b"hello").await?;

        let mut library = json!({"items": [], "cursor": ""});
        for _ in 0..60 {
            photos_call(&session, "/overview", "GET", None).await?;
            library = page("").await?;
            if library["items"].as_array().is_some_and(|items| items.len() >= 4) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let items = library["items"].as_array().context("library items")?;
        assert_eq!(items.len(), 4);
        assert!(!items.iter().any(|i| i["name"] == "notes.txt"));

        let detail = photos_call(&session, &format!("/assets/{trip}"), "GET", None).await?;
        assert_eq!(detail["takenAt"], parse_iso_ms("2024-04-05T07:20:30Z"));
        assert_eq!(detail["width"], 480);
        assert_eq!(detail["height"], 640);
        assert_eq!(detail["exif"]["model"], "Pixel 9");
        assert_eq!(detail["exif"]["fNumber"], 1.8);
        assert_eq!(detail["latitude"], 41);
        assert_eq!(detail["longitude"], 28.97);
        assert_eq!(detail["parentId"], target_parent.as_str());

        let video = items.iter().find(|i| i["id"] == clip.as_str()).context("clip in library")?;
        assert_eq!(video["kind"], "video");
        assert_eq!(video["duration"], 3);
        assert_eq!(video["takenAt"], parse_iso_ms("2025-01-02T03:04:05Z"));
        assert_eq!(items.iter().find(|i| i["id"] == broken.as_str()).context("broken in library")?["thumbnail"], "failed");

        // Newest first; paging returns the rest without repeats.
        let taken: Vec<i64> = items.iter().filter_map(|i| i["takenAt"].as_i64()).collect();
        let mut sorted_taken = taken.clone();
        sorted_taken.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(taken, sorted_taken);

        let first = page("?limit=2").await?;
        let cursor = first["cursor"].as_str().context("first page cursor")?;
        let second = page(&format!("?limit=2&cursor={}", url_encode(cursor))).await?;
        let first_items = first["items"].as_array().context("first items")?;
        let second_items = second["items"].as_array().context("second items")?;
        assert_eq!(first_items.len(), 2);
        let paged_ids: Vec<&str> = first_items.iter().chain(second_items.iter()).filter_map(|i| i["id"].as_str()).collect();
        let all_ids: Vec<&str> = items.iter().filter_map(|i| i["id"].as_str()).collect();
        assert_eq!(paged_ids, all_ids);

        // Thumbnails: WebP with an ETag, a larger preview, and the original as a fallback for decodable images.
        let small = photos_raw(&session, &format!("/assets/{trip}/thumbnail"), "GET", None, &[]).await?;
        assert_eq!(small.status, 200);
        assert_eq!(small.header("content-type"), Some("image/webp"));
        assert_eq!(&small.body[8..12], b"WEBP");
        let etag = small.header("etag").context("thumbnail etag")?;
        assert_eq!(photos_raw(&session, &format!("/assets/{trip}/thumbnail"), "GET", None, &[("if-none-match", etag)]).await?.status, 304);
        assert_eq!(photos_raw(&session, &format!("/assets/{clip}/thumbnail?size=m"), "GET", None, &[]).await?.header("content-type"), Some("image/webp"));
        let fallback = photos_raw(&session, &format!("/assets/{broken}/thumbnail"), "GET", None, &[]).await?;
        assert_eq!(fallback.status, 200);
        assert_eq!(fallback.text(), "not really a jpeg");

        // Favorites, videos and archive views with counts.
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [trip, plain], "favorite": true}))).await?;
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [plain], "archived": true}))).await?;
        let overview = photos_call(&session, "/overview", "GET", None).await?;
        assert_eq!(
            overview["counts"],
            json!({"library": 3, "folder": 3, "favorites": 2, "videos": 1, "archive": 1, "trash": 0, "duplicates": 0, "shared": 0})
        );
        assert_eq!(overview["uploadFolder"], "pics");

        let mut fav_ids: Vec<String> = page("?view=favorites").await?["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect();
        fav_ids.sort();
        let mut expected_fav = vec![trip.clone(), plain.clone()];
        expected_fav.sort();
        assert_eq!(fav_ids, expected_fav);
        assert_eq!(page("?view=videos").await?["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![clip.as_str()]);
        assert_eq!(page("?view=archive").await?["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![plain.as_str()]);
        assert!(!page("").await?["items"].as_array().unwrap().iter().any(|i| i["id"] == plain.as_str()));

        // The Photos folder view leaves out photos elsewhere in Files; All photos (library) keeps them.
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [plain], "archived": false}))).await?;
        let mut folder_ids: Vec<String> = page("?view=folder").await?["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect();
        folder_ids.sort();
        let mut expected_folder = vec![trip.clone(), clip.clone(), broken.clone()];
        expected_folder.sort();
        assert_eq!(folder_ids, expected_folder);
        assert!(page("").await?["items"].as_array().unwrap().iter().any(|i| i["id"] == plain.as_str()));
        let counts = &photos_call(&session, "/overview", "GET", None).await?["counts"];
        assert_eq!(json!({"library": counts["library"], "folder": counts["folder"]}), json!({"library": 4, "folder": 3}));
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [plain], "archived": true}))).await?;
        assert_eq!(photos_raw(&session, "/assets", "PATCH", Some(&json!({"ids": [trip]})), &[]).await?.status, 400);

        // Descriptions use revision checks.
        let detail_rev = detail["revision"].as_u64().context("detail revision")?;
        let described = photos_call(
            &session,
            &format!("/assets/{trip}/description"),
            "PUT",
            Some(&json!({"description": "  Bosphorus at dawn ", "revision": detail_rev + 1})),
        )
        .await?;
        assert_eq!(photos_call(&session, &format!("/assets/{trip}"), "GET", None).await?["description"], "Bosphorus at dawn");
        let stale = photos_raw(&session, &format!("/assets/{trip}/description"), "PUT", Some(&json!({"description": "x", "revision": detail_rev})), &[]).await?;
        assert_eq!(stale.status, 409);
        assert!(stale.json()?["error"]["message"].as_str().unwrap_or_default().contains("changed elsewhere"));
        assert!(described["revision"].as_u64().unwrap_or_default() > detail_rev);

        // Albums.
        let album = photos_call(&session, "/albums", "POST", Some(&json!({"name": "Istanbul", "ids": [trip]}))).await?;
        let album_id = album["id"].as_str().context("album id")?.to_owned();
        assert_eq!(album["count"], 1);
        assert_eq!(album["coverId"], trip.as_str());
        assert_eq!(photos_call(&session, &format!("/albums/{album_id}/add"), "POST", Some(&json!({"ids": [trip, clip, plain]}))).await?["added"], 2);
        let album_page = page(&format!("?view=album&album={album_id}")).await?;
        let album_items: Vec<&str> = album_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["id"].as_str())
            .collect();
        assert_eq!(album_items, vec![plain.as_str(), clip.as_str(), trip.as_str()]);
        assert_eq!(
            photos_call(&session, &format!("/assets/{trip}"), "GET", None).await?["albums"],
            json!([{"id": album_id, "name": "Istanbul"}])
        );
        let album_rev = photos_call(&session, "/overview", "GET", None).await?["albums"][0]["revision"].as_u64().context("album revision")?;
        let renamed = photos_call(&session, &format!("/albums/{album_id}"), "PATCH", Some(&json!({"name": "Istanbul 2024", "revision": album_rev}))).await?;
        assert_eq!(renamed["name"], "Istanbul 2024");
        assert_eq!(photos_raw(&session, &format!("/albums/{album_id}"), "PATCH", Some(&json!({"name": "Old", "revision": 1})), &[]).await?.status, 409);
        assert_eq!(photos_call(&session, &format!("/albums/{album_id}/remove"), "POST", Some(&json!({"ids": [clip]}))).await?["removed"], 1);
        assert_eq!(photos_call(&session, "/overview", "GET", None).await?["albums"][0]["count"], 2);

        // Search finds photos by name and description, with a thumbnail URL.
        let found = session.get_json("/api/v1/search?q=bosphorus").await?;
        let photo_hits: Vec<(&str, &str)> = found["items"]
            .as_array()
            .context("search items")?
            .iter()
            .filter(|i| i["group"] == "Photos")
            .map(|i| (i["id"].as_str().unwrap_or_default(), i["thumbnail"].as_str().unwrap_or_default()))
            .collect();
        let expected_thumb = format!("/api/v1/photos/assets/{trip}/thumbnail");
        assert_eq!(photo_hits, vec![(trip.as_str(), expected_thumb.as_str())]);
        let album_search = session.get_json("/api/v1/search?q=istanbul").await?;
        assert!(
            album_search["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["group"] == "Photos" && i["href"] == format!("/photos?album={album_id}"))
        );

        // Another account sees nothing and cannot use these IDs.
        create_user(&session, "other", "Other", PASSWORD).await?;
        let other = login_as(&base, "other", PASSWORD).await?;
        assert_eq!(photos_call(&other, "/assets", "GET", None).await?["items"], json!([]));
        for route in [format!("/assets/{trip}"), format!("/assets/{trip}/thumbnail")] {
            assert_eq!(photos_raw(&other, &route, "GET", None, &[]).await?.status, 404);
        }
        assert_eq!(photos_raw(&other, "/assets", "PATCH", Some(&json!({"ids": [trip], "favorite": false})), &[]).await?.status, 404);
        let theirs = photos_call(&other, "/albums", "POST", Some(&json!({"name": "Mine"}))).await?;
        let theirs_id = theirs["id"].as_str().context("other album id")?;
        assert_eq!(photos_raw(&other, &format!("/albums/{theirs_id}/add"), "POST", Some(&json!({"ids": [trip]})), &[]).await?.status, 404);
        assert_eq!(photos_raw(&other, &format!("/albums/{album_id}"), "DELETE", None, &[]).await?.status, 404);
        assert_eq!(photos_raw(&other, &format!("/assets/{trip}/index"), "POST", None, &[]).await?.status, 404);

        // Trashing in Files hides the photo everywhere; its metadata survives a restore from trash.
        let db = session.connect().await?;
        let trash_check: Result<()> = async {
            reducer(|done| db.reducers.trash_entry_then(trip.clone(), 1, done)).await?;
            assert!(!page("").await?["items"].as_array().unwrap().iter().any(|i| i["id"] == trip.as_str()));
            assert!(photos_call(&session, &format!("/assets/{trip}"), "GET", None).await?["trashedAt"].as_i64().unwrap_or(0) > 0);
            assert_eq!(
                page("?view=trash").await?["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(),
                vec![trip.as_str()]
            );
            assert_eq!(photos_raw(&session, &format!("/assets/{trip}/description"), "PUT", Some(&json!({"description": "x", "revision": 99})), &[]).await?.status, 404);
            assert_eq!(photos_call(&session, "/overview", "GET", None).await?["albums"][0]["count"], 1);
            Ok(())
        }
        .await;
        db.disconnect();
        trash_check?;

        // Deleting an album keeps its photos.
        photos_call(&session, &format!("/albums/{album_id}"), "DELETE", None).await?;
        assert_eq!(photos_call(&session, "/overview", "GET", None).await?["albums"].as_array().map(Vec::len), Some(0));
        assert!(page("").await?["items"].as_array().unwrap().iter().any(|i| i["id"] == clip.as_str()));
        assert_eq!(photos_raw(&session, "/assets/not-an-id", "GET", None, &[]).await?.status, 404);

        Ok(())
    }
    .await;

    instance.close().await;
    result
}
