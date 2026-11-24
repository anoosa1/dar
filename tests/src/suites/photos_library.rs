//! Photos library (`photos-library.integration.test.ts`): trash, Locked folder, Live Photos,
//! stacks, duplicates, video playback (remux & transcode), and upload folder preference.

use super::photos_books_apps_support::{
    ffmpeg, now_ms, photos_call, photos_raw, run_cmd, setup_admin_as, upload_raw,
};
use dar_acceptance::prelude::*;
use std::path::Path;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "photos-library",
    cases: &[case!(photos_library_features, 300)],
};

struct LibraryMedia {
    live: Vec<u8>,
    pair: Vec<u8>,
    sea: Vec<u8>,
    secret: Vec<u8>,
    raw: Vec<u8>,
    motion: Vec<u8>,
    odd: Vec<u8>,
}

async fn make_fixtures(dir: &Path) -> Result<LibraryMedia> {
    let still = |source: &'static str, file: &'static str| {
        let path = dir.join(file);
        async move {
            let p = path.to_string_lossy().into_owned();
            ffmpeg(&[
                "-f",
                "lavfi",
                "-i",
                &format!("{source}=size=480x360"),
                "-frames:v",
                "1",
                &p,
            ])
            .await?;
            Ok::<Vec<u8>, Error>(std::fs::read(&path)?)
        }
    };
    let live = still("testsrc2", "live.jpg").await?;
    let pair = still("smptebars", "pair.jpg").await?;
    let sea = still("mandelbrot", "sea.jpg").await?;
    let secret = still("rgbtestsrc", "secret.jpg").await?;

    let raw_path = dir.join("pair.tiff");
    let motion_path = dir.join("live.mov");
    let odd_path = dir.join("odd.mkv");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "smptebars=size=480x360",
        "-frames:v",
        "1",
        &raw_path.to_string_lossy(),
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=480x360:rate=10",
        "-t",
        "2",
        "-pix_fmt",
        "yuv420p",
        "-c:v",
        "libx264",
        &motion_path.to_string_lossy(),
    ])
    .await?;
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=10",
        "-t",
        "2",
        "-c:v",
        "mpeg4",
        &odd_path.to_string_lossy(),
    ])
    .await?;

    Ok(LibraryMedia {
        live,
        pair,
        sea,
        secret,
        raw: std::fs::read(raw_path)?,
        motion: std::fs::read(motion_path)?,
        odd: std::fs::read(odd_path)?,
    })
}

pub async fn photos_library_features(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-photos-library")?;
    let media = make_fixtures(&root).await?;
    let instance = Instance::isolated(&root, "dar-photos", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();

    let result: Result<()> = async {
        setup_admin_as(&base, "Photos fixture").await?;
        let session = login(&base).await?;

        let locked_api = |action: &str, body: Option<Value>, as_session: &Session| {
            let as_session = as_session.clone();
            let method = if action.is_empty() { "GET" } else { "POST" };
            let path = format!("/api/v1/files/locked{action}");
            async move {
                let mut req = as_session
                    .request(method, &path)
                    .header("content-type", "application/json");
                if let Some(body) = body {
                    req = req.body(body.to_string());
                }
                Reply::from(req.send().await?).await
            }
        };

        let put = |contents: &[u8], file_name: &str, media_type: &str, parent_id: &str| {
            let session = session.clone();
            let contents = contents.to_vec();
            let file_name = file_name.to_owned();
            let media_type = media_type.to_owned();
            let parent_id = parent_id.to_owned();
            async move {
                let id =
                    upload_raw(&session, &parent_id, &file_name, &media_type, &contents).await?;
                assert_eq!(
                    photos_call(
                        &session,
                        &format!("/assets/{id}/index"),
                        "POST",
                        Some(&json!({}))
                    )
                    .await?,
                    json!({"indexed": true})
                );
                Ok::<String, Error>(id)
            }
        };

        let page = |query: &str| {
            let session = session.clone();
            let route = format!("/assets{query}");
            async move {
                let res = photos_call(&session, &route, "GET", None).await?;
                Ok::<Vec<Value>, Error>(res["items"].as_array().cloned().unwrap_or_default())
            }
        };

        let names = |items: &[Value]| -> Vec<String> {
            let mut list: Vec<String> = items
                .iter()
                .filter_map(|i| i["name"].as_str().map(str::to_owned))
                .collect();
            list.sort();
            list
        };

        // Uploads go to pics/YYYY/MM by default; the folder is a per-account preference.
        let parent_id = photos_call(
            &session,
            "/upload-target",
            "POST",
            Some(&json!({"year": 2024, "month": 5})),
        )
        .await?["parentId"]
            .as_str()
            .context("parentId")?
            .to_owned();
        assert_eq!(
            photos_call(&session, "/overview", "GET", None).await?["uploadFolder"],
            "pics"
        );

        let id_live = put(&media.live, "live.jpg", "image/jpeg", &parent_id).await?;
        let id_motion = put(&media.motion, "live.mov", "video/quicktime", &parent_id).await?;
        let id_pair = put(&media.pair, "pair.jpg", "image/jpeg", &parent_id).await?;
        let id_raw = put(&media.raw, "pair.dng", "image/x-adobe-dng", &parent_id).await?;
        let id_sea = put(&media.sea, "sea.jpg", "image/jpeg", &parent_id).await?;
        let id_copy = put(&media.sea, "sea copy.jpg", "image/jpeg", &parent_id).await?;
        let id_secret = put(&media.secret, "secret.jpg", "image/jpeg", &parent_id).await?;
        let id_odd = put(&media.odd, "odd.mkv", "video/x-matroska", &parent_id).await?;

        let db = session.connect().await?;
        let folder_details =
            procedure(|done| db.procedures.file_details_then(parent_id.clone(), done)).await;
        db.disconnect();
        let folder_details = folder_details?;
        let mut path_parts: Vec<&str> = folder_details["ancestors"]
            .as_array()
            .context("ancestors")?
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect();
        path_parts.push(
            folder_details["entry"]["name"]
                .as_str()
                .context("entry name")?,
        );
        assert_eq!(path_parts, vec!["pics", "2024", "05"]);

        // A Live Photo is one tile; RAW+JPEG stack under the JPEG.
        let mut library = page("").await?;
        assert_eq!(
            names(&library),
            vec![
                "live.jpg",
                "odd.mkv",
                "pair.jpg",
                "sea copy.jpg",
                "sea.jpg",
                "secret.jpg"
            ]
        );
        let still = library
            .iter()
            .find(|i| i["id"] == id_live.as_str())
            .context("live still")?;
        assert_eq!(still["live"], id_motion.as_str());
        let cover = library
            .iter()
            .find(|i| i["id"] == id_pair.as_str())
            .context("pair cover")?;
        assert_eq!(cover["stack"]["count"], 2);
        let stack_id = cover["stack"]["id"].as_str().context("stack id")?;
        let stack = photos_call(&session, &format!("/stacks/{stack_id}"), "GET", None).await?;
        assert_eq!(stack["primaryId"], id_pair.as_str());
        let mut stack_ids: Vec<String> = stack["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect();
        stack_ids.sort();
        let mut expected_stack = vec![id_pair.clone(), id_raw.clone()];
        expected_stack.sort();
        assert_eq!(stack_ids, expected_stack);
        assert_eq!(
            photos_call(&session, "/overview", "GET", None).await?["counts"]["library"],
            6
        );

        // Videos: H.264 in MOV is remuxed, MPEG-4 Part 2 in MKV is transcoded; both stream with ranges.
        assert_eq!(
            photos_call(&session, &format!("/assets/{id_motion}"), "GET", None).await?["playback"],
            "remux"
        );
        let odd_detail = photos_call(&session, &format!("/assets/{id_odd}"), "GET", None).await?;
        assert_eq!(odd_detail["playback"], "transcode");
        assert_eq!(odd_detail["videoCodec"], "mpeg4");
        for id in [&id_odd, &id_motion] {
            assert_eq!(
                photos_raw(&session, &format!("/assets/{id}/video"), "GET", None, &[])
                    .await?
                    .status,
                404
            );
            let mut state = json!({"state": "preparing", "url": ""});
            for _ in 0..120 {
                state = photos_call(
                    &session,
                    &format!("/assets/{id}/playback"),
                    "POST",
                    Some(&json!({})),
                )
                .await?;
                if state["state"] != "preparing" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            assert_eq!(state["state"], "ready");
            let url = state["url"].as_str().context("playback url")?;
            let partial = Reply::from(
                session
                    .request("GET", url)
                    .header("range", "bytes=0-99")
                    .send()
                    .await?,
            )
            .await?;
            assert_eq!(partial.status, 206);
            assert_eq!(partial.header("content-type"), Some("video/mp4"));
            assert_eq!(partial.body.len(), 100);
            let full = session.get(url).await?.expect(200)?;
            let out_file = root.join(format!("{id}.mp4"));
            std::fs::write(&out_file, &full.body)?;
            let codec = run_cmd(
                "ffprobe",
                &[
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=codec_name",
                    "-of",
                    "csv=p=0",
                    &out_file.to_string_lossy(),
                ],
            )
            .await?;
            assert_eq!(codec, "h264");
        }

        // Manual stacks: cover, change of cover, unstack.
        let manual = photos_call(
            &session,
            "/stacks",
            "POST",
            Some(&json!({"ids": [id_secret, id_odd]})),
        )
        .await?;
        let manual_id = manual["id"].as_str().context("manual stack id")?;
        library = page("").await?;
        assert!(!library.iter().any(|i| i["id"] == id_odd.as_str()));
        assert_eq!(
            library
                .iter()
                .find(|i| i["id"] == id_secret.as_str())
                .unwrap()["stack"]["count"],
            2
        );
        photos_call(
            &session,
            &format!("/stacks/{manual_id}"),
            "PATCH",
            Some(&json!({"primaryId": id_odd})),
        )
        .await?;
        assert!(
            page("")
                .await?
                .iter()
                .any(|i| i["id"] == id_odd.as_str() && !i["stack"].is_null())
        );
        photos_call(&session, &format!("/stacks/{manual_id}"), "DELETE", None).await?;
        assert_eq!(page("").await?.len(), 6);

        // Duplicates: identical copies group (the RAW+JPEG stack does not); keeping one merges albums and favorites and trashes the rest.
        let album = photos_call(
            &session,
            "/albums",
            "POST",
            Some(&json!({"name": "Coast", "ids": [id_copy]})),
        )
        .await?;
        let album_id = album["id"].as_str().context("album id")?;
        photos_call(
            &session,
            "/assets",
            "PATCH",
            Some(&json!({"ids": [id_copy], "favorite": true})),
        )
        .await?;
        let duplicates = photos_call(&session, "/duplicates", "GET", None).await?;
        let groups = duplicates["groups"]
            .as_array()
            .context("duplicate groups")?;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["identical"], json!(true));
        let mut dup_ids: Vec<String> = groups[0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect();
        dup_ids.sort();
        let mut expected_dup = vec![id_sea.clone(), id_copy.clone()];
        expected_dup.sort();
        assert_eq!(dup_ids, expected_dup);
        assert_eq!(groups[0]["items"][0]["path"], "pics/2024/05");
        assert_eq!(
            photos_call(&session, "/overview", "GET", None).await?["counts"]["duplicates"],
            1
        );
        let dup_key = groups[0]["key"].as_str().context("duplicate key")?;
        assert_eq!(
            photos_raw(
                &session,
                "/duplicates/resolve",
                "POST",
                Some(&json!({"key": dup_key, "keep": [id_live]})),
                &[]
            )
            .await?
            .status,
            409
        );
        let resolved = photos_call(
            &session,
            "/duplicates/resolve",
            "POST",
            Some(&json!({"key": dup_key, "keep": [id_sea]})),
        )
        .await?;
        assert_eq!(resolved["trashed"], json!([id_copy]));
        let kept = photos_call(&session, &format!("/assets/{id_sea}"), "GET", None).await?;
        assert_eq!(kept["favorite"], json!(true));
        assert_eq!(
            kept["albums"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![album_id]
        );
        let overview = photos_call(&session, "/overview", "GET", None).await?;
        assert_eq!(overview["counts"]["duplicates"], 0);
        assert_eq!(overview["counts"]["trash"], 1);

        // Trash: list, restore, delete permanently; a Live Photo moves with its motion part.
        let mut trash = page("?view=trash").await?;
        assert_eq!(
            trash
                .iter()
                .map(|i| i["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![id_copy.as_str()]
        );
        assert!(trash[0]["trashedAt"].as_i64().unwrap_or(0) > 0);
        assert_eq!(
            photos_raw(
                &session,
                &format!("/assets/{id_copy}/thumbnail"),
                "GET",
                None,
                &[]
            )
            .await?
            .status,
            200
        );
        photos_call(
            &session,
            "/assets/restore",
            "POST",
            Some(&json!({"ids": [id_copy]})),
        )
        .await?;
        assert!(page("").await?.iter().any(|i| i["id"] == id_copy.as_str()));
        let dup_again = photos_call(&session, "/duplicates", "GET", None).await?;
        assert_eq!(dup_again["groups"].as_array().map(Vec::len), Some(1));
        let dup_again_key = dup_again["groups"][0]["key"].as_str().context("dup key")?;
        photos_call(
            &session,
            "/duplicates/dismiss",
            "POST",
            Some(&json!({"key": dup_again_key})),
        )
        .await?;
        assert_eq!(
            photos_call(&session, "/duplicates", "GET", None).await?["groups"]
                .as_array()
                .map(Vec::len),
            Some(0)
        );

        photos_call(
            &session,
            "/assets/trash",
            "POST",
            Some(&json!({"ids": [id_copy]})),
        )
        .await?;
        assert_eq!(
            photos_call(
                &session,
                "/assets/delete",
                "POST",
                Some(&json!({"ids": [id_copy]}))
            )
            .await?,
            json!({"deleted": [id_copy]})
        );
        assert_eq!(
            photos_raw(&session, &format!("/assets/{id_copy}"), "GET", None, &[])
                .await?
                .status,
            404
        );

        let mut live_trashed: Vec<String> = photos_call(
            &session,
            "/assets/trash",
            "POST",
            Some(&json!({"ids": [id_live]})),
        )
        .await?["trashed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        live_trashed.sort();
        let mut expected_live = vec![id_live.clone(), id_motion.clone()];
        expected_live.sort();
        assert_eq!(live_trashed, expected_live);
        trash = page("?view=trash").await?;
        assert_eq!(
            trash
                .iter()
                .map(|i| i["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![id_live.as_str()]
        );
        assert_eq!(trash[0]["live"], id_motion.as_str());
        let mut live_restored: Vec<String> = photos_call(
            &session,
            "/assets/restore",
            "POST",
            Some(&json!({"ids": [id_live]})),
        )
        .await?["restored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        live_restored.sort();
        assert_eq!(live_restored, expected_live);
        assert_eq!(
            page("")
                .await?
                .iter()
                .find(|i| i["id"] == id_live.as_str())
                .unwrap()["live"],
            id_motion.as_str()
        );

        photos_call(
            &session,
            "/assets/trash",
            "POST",
            Some(&json!({"ids": [id_odd]})),
        )
        .await?;
        assert_eq!(
            photos_call(&session, "/trash/empty", "POST", Some(&json!({}))).await?,
            json!({"deleted": 1, "remaining": false})
        );
        assert_eq!(page("?view=trash").await?.len(), 0);

        // Locked folder: needs the account password, hides photos everywhere else, and moves them in Files.
        let locked_initial = locked_api("", None, &session).await?.json()?;
        assert_eq!(locked_initial["folderId"], "");
        assert_eq!(locked_initial["unlockedUntil"].as_f64(), Some(0.0));
        assert_eq!(
            photos_raw(
                &session,
                "/assets/lock",
                "POST",
                Some(&json!({"ids": [id_secret]})),
                &[]
            )
            .await?
            .status,
            423
        );
        assert_eq!(
            photos_raw(&session, "/assets?view=locked", "GET", None, &[])
                .await?
                .status,
            423
        );
        assert_eq!(
            locked_api(
                "/unlock",
                Some(json!({"password": "wrong password for this test"})),
                &session
            )
            .await?
            .status,
            403
        );
        let unlocked = locked_api("/unlock", Some(json!({"password": PASSWORD})), &session)
            .await?
            .expect(200)?
            .json()?;
        let unlocked_folder_id = unlocked["folderId"]
            .as_str()
            .context("folderId")?
            .to_owned();
        let uuid_re = regex::Regex::new("^[a-f0-9-]{36}$").unwrap();
        assert!(
            uuid_re.is_match(&unlocked_folder_id),
            "folderId={unlocked_folder_id}"
        );
        assert!(unlocked["unlockedUntil"].as_f64().unwrap_or(0.0) > (now_ms() + 9 * 60_000) as f64);

        let moved = photos_call(
            &session,
            "/assets/lock",
            "POST",
            Some(&json!({"ids": [id_secret]})),
        )
        .await?["moved"]
            .clone();
        assert_eq!(moved, json!([{"id": id_secret, "parentId": parent_id}]));
        assert!(
            !page("")
                .await?
                .iter()
                .any(|i| i["id"] == id_secret.as_str())
        );
        assert_eq!(
            page("?view=locked")
                .await?
                .iter()
                .map(|i| i["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![id_secret.as_str()]
        );
        assert_eq!(
            photos_call(&session, &format!("/assets/{id_secret}"), "GET", None).await?["locked"],
            json!(true)
        );
        let found = session.get_json("/api/v1/search?q=secret").await?;
        assert_eq!(found["items"].as_array().map(Vec::len), Some(0));
        assert_eq!(
            photos_raw(
                &session,
                "/assets/trash",
                "POST",
                Some(&json!({"ids": [id_secret]})),
                &[]
            )
            .await?
            .status,
            404
        );

        let files = session.connect().await?;
        let files_check: Result<()> = async {
            // Files: the unlocked session browses the Locked folder; it is not in All files and refuses the trash.
            reducer(|done| {
                files.reducers.browse_then(
                    String::new(),
                    "files".into(),
                    String::new(),
                    String::new(),
                    done,
                )
            })
            .await?;
            assert!(
                !files
                    .db
                    .my_files()
                    .iter()
                    .any(|e| e.id == unlocked_folder_id)
            );
            reducer(|done| {
                files.reducers.browse_then(
                    unlocked_folder_id.clone(),
                    "files".into(),
                    String::new(),
                    String::new(),
                    done,
                )
            })
            .await?;
            let entries: Vec<_> = files.db.my_files().iter().collect();
            assert_eq!(
                entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                vec!["secret.jpg"]
            );
            let entry = &entries[0];
            assert!(
                reducer(|done| files.reducers.trash_entry_then(
                    entry.id.clone(),
                    entry.revision,
                    done
                ))
                .await
                .err()
                .is_some_and(|e| e.to_string().contains("lockedFolder"))
            );
            assert!(
                reducer(|done| files.reducers.rename_entry_then(
                    unlocked_folder_id.clone(),
                    "Open".into(),
                    1,
                    done
                ))
                .await
                .err()
                .is_some_and(|e| e.to_string().contains("forbidden"))
            );
            assert!(
                reducer(|done| files.reducers.grant_access_then(
                    uuid(),
                    entry.id.clone(),
                    "admin".into(),
                    "read".into(),
                    entry.revision,
                    done
                ))
                .await
                .err()
                .is_some_and(|e| e.to_string().contains("lockedFolder"))
            );
            Ok(())
        }
        .await;
        files.disconnect();
        files_check?;

        // Another browser session of the same account sees nothing until it unlocks too.
        let second = login(&base).await?;
        assert_eq!(
            photos_raw(
                &second,
                &format!("/assets/{id_secret}/thumbnail"),
                "GET",
                None,
                &[]
            )
            .await?
            .status,
            423
        );
        assert_eq!(
            photos_raw(&second, "/assets?view=locked", "GET", None, &[])
                .await?
                .status,
            423
        );
        assert_eq!(
            second
                .get(&format!("/api/v1/files/{id_secret}/content"))
                .await?
                .status,
            404
        );
        let other = second.connect().await?;
        let other_check: Result<()> = async {
            assert!(
                reducer(|done| other.reducers.browse_then(
                    unlocked_folder_id.clone(),
                    "files".into(),
                    String::new(),
                    String::new(),
                    done
                ))
                .await
                .err()
                .is_some_and(|e| e.to_string().contains("notFound"))
            );
            assert!(
                procedure(|done| other.procedures.file_details_then(id_secret.clone(), done))
                    .await
                    .err()
                    .is_some_and(|e| e.to_string().contains("notFound"))
            );
            Ok(())
        }
        .await;
        other.disconnect();
        other_check?;

        // Undo returns the photo where it was; deleting inside the Locked folder is permanent.
        photos_call(
            &session,
            "/assets/unlock",
            "POST",
            Some(&json!({"ids": [id_secret], "to": moved})),
        )
        .await?;
        assert!(
            page("")
                .await?
                .iter()
                .any(|i| i["id"] == id_secret.as_str())
        );
        assert_eq!(
            photos_call(&session, &format!("/assets/{id_secret}"), "GET", None).await?["parentId"],
            parent_id.as_str()
        );
        photos_call(
            &session,
            "/assets/lock",
            "POST",
            Some(&json!({"ids": [id_secret]})),
        )
        .await?;
        photos_call(
            &session,
            "/assets/delete",
            "POST",
            Some(&json!({"ids": [id_secret]})),
        )
        .await?;
        assert_eq!(page("?view=locked").await?.len(), 0);
        assert_eq!(
            photos_raw(&session, &format!("/assets/{id_secret}"), "GET", None, &[])
                .await?
                .status,
            404
        );
        locked_api("/lock", Some(json!({})), &session)
            .await?
            .expect(200)?;
        assert_eq!(
            photos_raw(&session, "/assets?view=locked", "GET", None, &[])
                .await?
                .status,
            423
        );

        // The upload folder preference.
        let preferences = session.get_json("/api/v1/pim/preferences").await?;
        let pref_rev = preferences["revision"]
            .as_u64()
            .context("preferences revision")?;
        let save = |value: &str, rev: u64| {
            let session = session.clone();
            let value = value.to_owned();
            async move {
                Reply::from(
                    session
                        .request("PUT", "/api/v1/pim/preferences")
                        .header("if-match", format!("\"preferences:{rev}\""))
                        .json(&json!({"photosUploadFolder": value}))
                        .send()
                        .await?,
                )
                .await
            }
        };
        assert_eq!(save("../escape", pref_rev).await?.status, 400);
        assert_eq!(save(" Media / Camera ", pref_rev).await?.status, 200);
        assert_eq!(
            photos_call(&session, "/overview", "GET", None).await?["uploadFolder"],
            "Media/Camera"
        );
        let moved2 = photos_call(
            &session,
            "/upload-target",
            "POST",
            Some(&json!({"year": 2024, "month": 5})),
        )
        .await?["parentId"]
            .as_str()
            .context("moved2 parentId")?
            .to_owned();
        assert_ne!(moved2, parent_id);

        Ok(())
    }
    .await;

    instance.close().await;
    result
}
