//! A JMAP client (RFC 8620, 8621) for the mirror: session discovery, mailboxes, email lists with
//! keywords, blob download and upload, keyword and mailbox changes, import and submission.

use serde_json::{Value, json};

use super::net::{NetError, NetResult, err, http, http_follow};

const USING: [&str; 3] = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"];

pub struct Jmap {
    auth: String,
    api: String,
    download: String,
    upload: String,
    pub account: String,
}

pub struct RemoteMailbox {
    pub id: String,
    pub name: String,
    pub parent_id: String,
    pub role: String,
}

pub struct RemoteEmail {
    pub id: String,
    pub blob_id: String,
    pub size: f64,
    pub keywords: Vec<String>,
}

impl Jmap {
    /// `authorization` is a complete header value (`Basic …` or `Bearer …`).
    pub async fn connect(url: &str, authorization: String) -> NetResult<Jmap> {
        let (base, response) = http_follow("GET", url, &[("authorization", authorization.clone()), ("accept", "application/json".into())], Vec::new(), 4 * 1024 * 1024).await?;
        if response.status == 401 || response.status == 403 {
            return Err(NetError("signIn:The JMAP server rejected the sign-in".into()));
        }
        if response.status != 200 {
            return err(format!("The JMAP server answered {}", response.status));
        }
        let session = response.json()?;
        let resolve = |key: &str| -> NetResult<String> {
            let raw = session[key].as_str().ok_or_else(|| NetError(format!("The JMAP session has no {key}")))?;
            reqwest::Url::parse(&base).ok().and_then(|b| b.join(raw).ok()).map(|u| u.to_string().replace("%7B", "{").replace("%7D", "}")).ok_or_else(|| NetError(format!("The JMAP {key} is invalid")))
        };
        let account = session["primaryAccounts"]["urn:ietf:params:jmap:mail"].as_str().ok_or_else(|| NetError("The JMAP account has no mail".into()))?.to_owned();
        Ok(Jmap { auth: authorization, api: resolve("apiUrl")?, download: resolve("downloadUrl")?, upload: resolve("uploadUrl")?, account })
    }

    /// One request with several method calls; returns each call's arguments in order.
    pub async fn call(&self, calls: Vec<(&str, Value)>) -> NetResult<Vec<Value>> {
        let body = json!({
            "using": USING,
            "methodCalls": calls.iter().enumerate().map(|(i, (name, args))| json!([name, args, format!("c{i}")])).collect::<Vec<_>>(),
        });
        let response = http("POST", &self.api, &[("authorization", self.auth.clone()), ("content-type", "application/json".into()), ("accept", "application/json".into())], body.to_string().into_bytes(), 64 * 1024 * 1024).await?;
        if response.status == 401 {
            return Err(NetError("signIn:The JMAP server rejected the sign-in".into()));
        }
        if response.status != 200 {
            return err(format!("The JMAP server answered {}", response.status));
        }
        let value = response.json()?;
        let mut out = Vec::new();
        for r in value["methodResponses"].as_array().cloned().unwrap_or_default() {
            if r[0] == "error" {
                return err(format!("The JMAP server refused a request: {}", r[1]["type"].as_str().unwrap_or("error")));
            }
            out.push(r[1].clone());
        }
        if out.len() != calls.len() {
            return err("The JMAP server answered partially");
        }
        Ok(out)
    }

    pub async fn mailboxes(&self) -> NetResult<Vec<RemoteMailbox>> {
        let r = self.call(vec![("Mailbox/get", json!({ "accountId": self.account, "ids": null, "properties": ["id", "name", "parentId", "role"] }))]).await?;
        Ok(r[0]["list"].as_array().cloned().unwrap_or_default().iter().filter_map(|m| {
            Some(RemoteMailbox {
                id: m["id"].as_str()?.to_owned(),
                name: m["name"].as_str().unwrap_or("Mailbox").to_owned(),
                parent_id: m["parentId"].as_str().unwrap_or_default().to_owned(),
                role: match m["role"].as_str().unwrap_or_default() {
                    r @ ("inbox" | "sent" | "drafts" | "trash" | "junk" | "archive") => r.to_owned(),
                    _ => String::new(),
                },
            })
        }).filter(|m| !matches!(m.name.as_str(), "") ).collect())
    }

    /// Every email in a mailbox with its keywords and blob.
    pub async fn list(&self, mailbox: &str) -> NetResult<Vec<RemoteEmail>> {
        let mut ids: Vec<String> = Vec::new();
        loop {
            let r = self.call(vec![("Email/query", json!({ "accountId": self.account, "filter": { "inMailbox": mailbox }, "position": ids.len(), "limit": 500, "calculateTotal": true }))]).await?;
            let page: Vec<String> = r[0]["ids"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            let total = r[0]["total"].as_u64().unwrap_or(0) as usize;
            let done = page.is_empty();
            ids.extend(page);
            if done || ids.len() >= total || ids.len() > 500_000 {
                break;
            }
        }
        let mut out = Vec::new();
        for chunk in ids.chunks(500) {
            let r = self.call(vec![("Email/get", json!({ "accountId": self.account, "ids": chunk, "properties": ["id", "blobId", "size", "keywords", "mailboxIds"] }))]).await?;
            for e in r[0]["list"].as_array().cloned().unwrap_or_default() {
                let Some(id) = e["id"].as_str() else { continue };
                out.push(RemoteEmail {
                    id: id.to_owned(),
                    blob_id: e["blobId"].as_str().unwrap_or_default().to_owned(),
                    size: e["size"].as_f64().unwrap_or(0.0),
                    keywords: e["keywords"].as_object().map(|k| k.iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k.to_lowercase()).collect()).unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }

    pub async fn download(&self, blob_id: &str) -> NetResult<Vec<u8>> {
        let url = self.download.replace("{accountId}", &self.account).replace("{blobId}", blob_id).replace("{type}", "message%2Frfc822").replace("{name}", "message.eml");
        let response = http("GET", &url, &[("authorization", self.auth.clone())], Vec::new(), 80 * 1024 * 1024).await?;
        if response.status != 200 {
            return err(format!("The JMAP server did not send a message ({})", response.status));
        }
        Ok(response.body)
    }

    async fn upload(&self, data: &[u8]) -> NetResult<String> {
        let url = self.upload.replace("{accountId}", &self.account);
        let response = http("POST", &url, &[("authorization", self.auth.clone()), ("content-type", "message/rfc822".into())], data.to_vec(), 1024 * 1024).await?;
        if !(200..300).contains(&response.status) {
            return err(format!("The JMAP server did not take the message ({})", response.status));
        }
        response.json()?["blobId"].as_str().map(str::to_owned).ok_or_else(|| NetError("The JMAP upload gave no blob".into()))
    }

    /// Patches emails (`{"keywords/$seen": true}`, `{"mailboxIds/a": null, …}`).
    pub async fn update(&self, patches: Vec<(String, Value)>) -> NetResult<()> {
        if patches.is_empty() {
            return Ok(());
        }
        let update: serde_json::Map<String, Value> = patches.into_iter().collect();
        let r = self.call(vec![("Email/set", json!({ "accountId": self.account, "update": update }))]).await?;
        if let Some(failed) = r[0]["notUpdated"].as_object().filter(|m| !m.is_empty()) {
            return err(format!("The JMAP server did not change {} messages", failed.len()));
        }
        Ok(())
    }

    pub async fn destroy(&self, ids: &[String]) -> NetResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.call(vec![("Email/set", json!({ "accountId": self.account, "destroy": ids }))]).await?;
        Ok(())
    }

    /// Adds a message to a mailbox; returns its email ID.
    pub async fn import(&self, mailbox: &str, keywords: &[&str], data: &[u8]) -> NetResult<String> {
        let blob = self.upload(data).await?;
        let keywords: serde_json::Map<String, Value> = keywords.iter().map(|k| ((*k).to_owned(), json!(true))).collect();
        let r = self.call(vec![("Email/import", json!({ "accountId": self.account, "emails": { "k": { "blobId": blob, "mailboxIds": { mailbox: true }, "keywords": keywords } } }))]).await?;
        r[0]["created"]["k"]["id"].as_str().map(str::to_owned).ok_or_else(|| NetError(format!("The JMAP server did not import the message: {}", r[0]["notCreated"]["k"]["type"].as_str().unwrap_or("error"))))
    }

    pub async fn create_mailbox(&self, name: &str, parent: &str) -> NetResult<String> {
        let parent = if parent.is_empty() { Value::Null } else { json!(parent) };
        let r = self.call(vec![("Mailbox/set", json!({ "accountId": self.account, "create": { "m": { "name": name, "parentId": parent } } }))]).await?;
        r[0]["created"]["m"]["id"].as_str().map(str::to_owned).ok_or_else(|| NetError("The JMAP server did not create the mailbox".into()))
    }

    pub async fn rename_mailbox(&self, id: &str, name: &str) -> NetResult<()> {
        let r = self.call(vec![("Mailbox/set", json!({ "accountId": self.account, "update": { id: { "name": name } } }))]).await?;
        if r[0]["notUpdated"].as_object().is_some_and(|m| !m.is_empty()) {
            return err("The JMAP server did not rename the mailbox");
        }
        Ok(())
    }

    /// Imports the message into Sent and submits it with the identity for `from`.
    pub async fn submit(&self, sent: &str, from: &str, rcpt: &[String], data: &[u8]) -> NetResult<String> {
        let identities = self.call(vec![("Identity/get", json!({ "accountId": self.account, "ids": null }))]).await?;
        let list = identities[0]["list"].as_array().cloned().unwrap_or_default();
        let identity = list.iter().find(|i| i["email"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(from))).or_else(|| list.first()).and_then(|i| i["id"].as_str()).ok_or_else(|| NetError("The JMAP account has no sending identity".into()))?.to_owned();
        let email = self.import(sent, &["$seen"], data).await?;
        let envelope = json!({ "mailFrom": { "email": from }, "rcptTo": rcpt.iter().map(|r| json!({ "email": r })).collect::<Vec<_>>() });
        let r = self.call(vec![("EmailSubmission/set", json!({ "accountId": self.account, "create": { "s": { "identityId": identity, "emailId": email, "envelope": envelope } } }))]).await?;
        if r[0]["created"]["s"].is_null() {
            let _ = self.destroy(std::slice::from_ref(&email)).await;
            return err(format!("The JMAP server did not send the message: {}", r[0]["notCreated"]["s"]["description"].as_str().or(r[0]["notCreated"]["s"]["type"].as_str()).unwrap_or("error")));
        }
        Ok(email)
    }
}
