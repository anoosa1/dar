//! Port of `test/pim-dav-client.mjs`: real vdirsyncer client discovers both CardDAV and CalDAV
//! homes, uploads contacts/events/tasks and receives native edits with conditional round trips.

use super::pim_workspace_support::{app_password, basic_auth, dav_request, etag};
use dar_acceptance::prelude::*;
use regex::Regex;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const SUITE: Suite = Suite {
    name: "pim-dav-client",
    cases: &[case!(vdirsyncer_round_trip, 90)],
};

struct SyncCase {
    kind: &'static str,
    base: String,
    collection_id: String,
    dav_url: String,
    secret: String,
    local: PathBuf,
}

pub async fn vdirsyncer_round_trip(t: &Ctx) -> Result<()> {
    let origin = t.fixture().await?.base.clone();
    let session = login(&origin).await?;
    let root = t.temp("dar-pim-client")?;

    let mut config = format!(
        "[general]\nstatus_path = {}\n",
        serde_json::to_string(&root.join("status").to_string_lossy())?
    );
    let mut cases = Vec::new();
    let uid_re = Regex::new(r"UID:([^\r]+)").unwrap();
    let fn_summary_re = Regex::new(r"(FN|SUMMARY):[^\r]+").unwrap();

    for kind in ["contacts", "calendar"] {
        let base = format!(
            "/api/v1/{kind}/{}",
            if kind == "contacts" { "books" } else { "calendars" }
        );
        let collection = session
            .post(
                &base,
                &json!({"name": format!("Real client {kind}"), "color": "#2563eb"}),
            )
            .await?
            .expect(201)?
            .json()?;
        let collection_id = collection["id"]
            .as_str()
            .context("collection id")?
            .to_owned();
        let password = app_password(
            &session,
            &format!("Real client {kind}"),
            &format!("{kind}:write"),
        )
        .await?;
        let dav_url = password["davUrl"].as_str().context("davUrl")?.to_owned();
        let secret = password["secret"].as_str().context("secret")?.to_owned();
        let suffix = if kind == "contacts" { ".vcf" } else { ".ics" };

        let local = root.join(kind).join(&collection_id);
        std::fs::create_dir_all(&local)?;
        let content = if kind == "contacts" {
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:real-client-contact\r\nFN:Real client contact\r\nN:contact;Real client;;;\r\nEND:VCARD\r\n"
        } else {
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Client//EN\r\nBEGIN:VEVENT\r\nUID:real-client-event\r\nDTSTART:20261109T140000Z\r\nDTEND:20261109T150000Z\r\nSUMMARY:Real client event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        };
        if kind == "calendar" {
            let task = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:real-client-task\r\nSUMMARY:Real client task\r\nDUE;VALUE=DATE:20261110\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
            std::fs::write(local.join("task.ics"), task)?;
        }
        std::fs::write(local.join(format!("external{suffix}")), content)?;
        let anchor = uid_re.replace(content, "UID:$1-anchor");
        let anchor = fn_summary_re.replace(&anchor, "$1:Retained anchor");
        std::fs::write(local.join(format!("anchor{suffix}")), anchor.as_ref())?;

        let storage_type = if kind == "contacts" {
            "carddav"
        } else {
            "caldav"
        };
        let item_types = if kind == "calendar" {
            "item_types = [\"VEVENT\", \"VTODO\"]\n"
        } else {
            ""
        };
        config.push_str(&format!(
            "\n[pair {kind}]\na = \"{kind}_local\"\nb = \"{kind}_remote\"\ncollections = [{}]\n\n[storage {kind}_local]\ntype = \"filesystem\"\npath = {}\nfileext = {}\n\n[storage {kind}_remote]\ntype = \"{storage_type}\"\nurl = {}\nusername = \"admin\"\npassword = {}\n{item_types}",
            serde_json::to_string(&collection_id)?,
            serde_json::to_string(&root.join(kind).to_string_lossy())?,
            serde_json::to_string(suffix)?,
            serde_json::to_string(&dav_url)?,
            serde_json::to_string(&secret)?,
        ));
        cases.push(SyncCase {
            kind,
            base,
            collection_id,
            dav_url,
            secret,
            local,
        });
    }

    let config_file = root.join("config");
    dar_acceptance::stack::write_private(&config_file, config.as_bytes())?;

    let run_vdirsyncer = |subcommand: &'static str| {
        let config_file = config_file.clone();
        let root = root.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let bin = std::env::var("DAR_DAV_CLIENT").unwrap_or_else(|_| "vdirsyncer".into());
                let output = Command::new(&bin)
                    .arg("--config")
                    .arg(&config_file)
                    .arg(subcommand)
                    .env("HOME", &root)
                    .env("NO_PROXY", "127.0.0.1,localhost")
                    .stdin(Stdio::null())
                    .output()
                    .context(format!("running {bin} {subcommand}"))?;
                let combined = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                ensure!(
                    output.status.success(),
                    "vdirsyncer {subcommand} failed: {combined}"
                );
                Ok(combined)
            })
            .await
            .map_err(|e| Error::new(e.to_string()))?
        }
    };

    run_vdirsyncer("discover").await?;
    run_vdirsyncer("sync").await?;

    for c in &cases {
        let reply = session
            .get(&format!("{}/{}/items", c.base, c.collection_id))
            .await?
            .expect(200)?;
        let json = reply.json()?;
        let items = json["items"].as_array().context("items array")?;
        assert_eq!(items.len(), if c.kind == "calendar" { 3 } else { 2 });

        if c.kind == "calendar" {
            let task = items
                .iter()
                .find(|i| i["uid"] == "real-client-task")
                .context("missing synced task")?;
            let href = task["href"].as_str().context("task href")?;
            let changed = Reply::from(
                session
                    .request(
                        "PUT",
                        &format!("{}/{}/items/{href}", c.base, c.collection_id),
                    )
                    .header("if-match", etag(task))
                    .json(&json!({"component": "task", "data": {"title": "Changed native task", "status": "COMPLETED"}}))
                    .send()
                    .await?,
            )
            .await?;
            assert_eq!(changed.status, 200, "{}", changed.text());
        }

        let needle = if c.kind == "contacts" {
            "contact"
        } else {
            "event"
        };
        let row = items
            .iter()
            .find(|item| item["uid"].as_str().is_some_and(|u| u.ends_with(needle)))
            .context("missing synced contact/event")?;
        let href = row["href"].as_str().context("row href")?;
        let content = if let Some(text) = row["content"].as_str() {
            text.to_owned()
        } else {
            let auth = basic_auth("admin", &c.secret);
            dav_request(
                "GET",
                &format!("{}{}/{href}", c.dav_url, c.collection_id),
                &[("authorization", &auth)],
                None,
            )
            .await?
            .text()
        };
        let updated_content = fn_summary_re
            .replace(&content, "$1:Changed in native app")
            .into_owned();
        let changed = Reply::from(
            session
                .request(
                    "PUT",
                    &format!("{}/{}/items/{href}", c.base, c.collection_id),
                )
                .header("if-match", etag(row))
                .json(&json!({"content": updated_content}))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(changed.status, 200, "{}", changed.text());
    }

    run_vdirsyncer("sync").await?;

    for c in &cases {
        let mut contents = Vec::new();
        for entry in std::fs::read_dir(&c.local)? {
            let path = entry?.path();
            if path.is_file() {
                contents.push(std::fs::read_to_string(&path)?);
            }
        }
        assert!(
            contents.iter().any(|s| s.contains("Changed in native app")),
            "{}: server change did not reach independent client",
            c.kind
        );
        if c.kind == "calendar" {
            assert!(contents
                .iter()
                .any(|s| s.contains("Changed native task") && s.contains("STATUS:COMPLETED")));
        }
    }
    Ok(())
}
