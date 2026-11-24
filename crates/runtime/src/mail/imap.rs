//! An IMAP4rev1 client for what Dar's mirror needs: sign-in (PLAIN, LOGIN, XOAUTH2), LIST with
//! special-use roles, SELECT, UID FETCH of flags and whole messages, UID STORE, UID MOVE (or
//! COPY and EXPUNGE), APPEND, CREATE and RENAME. Responses are read with their literals and
//! parsed into a small tree.

use std::collections::HashMap;

use super::net::{Conn, NetError, NetResult, b64, err};

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Atom(String),
    Str(Vec<u8>),
    List(Vec<Item>),
    Nil,
}

impl Item {
    pub fn text(&self) -> String {
        match self {
            Item::Atom(a) => a.clone(),
            Item::Str(s) => String::from_utf8_lossy(s).into_owned(),
            _ => String::new(),
        }
    }
    fn list(&self) -> &[Item] {
        match self {
            Item::List(l) => l,
            _ => &[],
        }
    }
}

/// Parses `data` (a response with its literals inline) into items, stopping at a CRLF that
/// is not inside a literal.
pub fn parse(data: &[u8]) -> Vec<Item> {
    fn items(data: &[u8], pos: &mut usize, depth: usize) -> Vec<Item> {
        let mut out = Vec::new();
        while *pos < data.len() {
            match data[*pos] {
                b' ' | b'\r' | b'\n' => *pos += 1,
                b')' => {
                    *pos += 1;
                    if depth > 0 {
                        return out;
                    }
                }
                b'(' => {
                    *pos += 1;
                    out.push(Item::List(items(data, pos, depth + 1)));
                }
                b'"' => {
                    *pos += 1;
                    let mut s = Vec::new();
                    while *pos < data.len() && data[*pos] != b'"' {
                        if data[*pos] == b'\\' && *pos + 1 < data.len() {
                            *pos += 1;
                        }
                        s.push(data[*pos]);
                        *pos += 1;
                    }
                    *pos += 1;
                    out.push(Item::Str(s));
                }
                b'{' => {
                    let end = data[*pos..]
                        .iter()
                        .position(|b| *b == b'}')
                        .map(|e| *pos + e)
                        .unwrap_or(data.len());
                    let n: usize = std::str::from_utf8(&data[*pos + 1..end])
                        .unwrap_or("0")
                        .trim_end_matches('+')
                        .parse()
                        .unwrap_or(0);
                    let start = (end + 3).min(data.len());
                    let stop = (start + n).min(data.len());
                    out.push(Item::Str(data[start..stop].to_vec()));
                    *pos = stop;
                }
                _ => {
                    // An atom; brackets keep their spaces (`BODY[]`, `[UIDVALIDITY 3]`).
                    let start = *pos;
                    let mut bracket = 0;
                    while *pos < data.len() {
                        let c = data[*pos];
                        if c == b'[' {
                            bracket += 1;
                        } else if c == b']' {
                            bracket -= 1;
                        } else if bracket == 0 && matches!(c, b' ' | b'(' | b')' | b'\r' | b'\n') {
                            break;
                        }
                        *pos += 1;
                    }
                    let atom = String::from_utf8_lossy(&data[start..*pos]).into_owned();
                    out.push(if atom.eq_ignore_ascii_case("NIL") {
                        Item::Nil
                    } else {
                        Item::Atom(atom)
                    });
                }
            }
        }
        out
    }
    let mut pos = 0;
    items(data, &mut pos, 0)
}

/// IMAP's modified UTF-7 (RFC 3501 5.1.3) to UTF-8.
pub fn decode_name(name: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            let mut b = chunk.replace(',', "/");
            while b.len() % 4 != 0 {
                b.push('=');
            }
            match super::net::unb64(&b) {
                Some(bytes) => {
                    let units: Vec<u16> = bytes
                        .chunks(2)
                        .filter(|c| c.len() == 2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    out.push_str(&String::from_utf16_lossy(&units));
                }
                None => out.push_str(&rest[start..start + end + 2]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn encode_name(name: &str) -> String {
    let mut out = String::new();
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(b64(&bytes).trim_end_matches('=').replace('/', ",").as_str());
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-")
            } else {
                out.push(c)
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A compact UID set: `1:5,7,9:12`.
pub fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        i += 1;
    }
    parts.join(",")
}

pub struct Status {
    pub ok: bool,
    pub text: String,
}

/// The text inside `[CODE ...]` of a status line, if it is that code.
fn code<'a>(text: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let start = text.find('[')?;
    let end = text[start..].find(']')? + start;
    let inner: Vec<&str> = text[start + 1..end].split_whitespace().collect();
    (inner.first().is_some_and(|c| c.eq_ignore_ascii_case(name))).then(|| inner[1..].to_vec())
}

pub struct RemoteMailbox {
    pub remote: String,
    pub name: String,
    pub parent_remote: String,
    pub role: String,
}

pub struct RemoteMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub size: f64,
}

pub struct Imap {
    conn: Conn,
    tag: u32,
    pub caps: Vec<String>,
    selected: Option<String>,
}

impl Imap {
    pub async fn connect(host: &str, port: u16, security: &str) -> NetResult<Imap> {
        let conn = Conn::open(host, port, security == "tls").await?;
        let mut imap = Imap {
            conn,
            tag: 0,
            caps: Vec::new(),
            selected: None,
        };
        let greeting = imap.conn.line().await?;
        if !greeting.starts_with(b"* OK") && !greeting.starts_with(b"* PREAUTH") {
            return err(format!(
                "{host} is not an IMAP server or refused the connection"
            ));
        }
        imap.capability().await?;
        if security == "starttls" {
            if !imap.has("STARTTLS") {
                return err(format!("{host} does not offer STARTTLS"));
            }
            imap.simple("STARTTLS").await?;
            imap.conn.starttls().await?;
            imap.capability().await?;
        }
        if !imap.conn.secure() {
            return err("The connection is not encrypted");
        }
        Ok(imap)
    }

    fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c.eq_ignore_ascii_case(cap))
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("d{}", self.tag)
    }

    /// Reads untagged responses (each with its literals) until the tagged one.
    async fn read(&mut self, tag: &str) -> NetResult<(Vec<Vec<u8>>, Status)> {
        let mut untagged = Vec::new();
        loop {
            let mut response = self.line_with_literals().await?;
            if response.starts_with(tag.as_bytes()) && response.get(tag.len()) == Some(&b' ') {
                let text = String::from_utf8_lossy(&response[tag.len() + 1..]).into_owned();
                let ok = text.len() >= 2 && text[..2].eq_ignore_ascii_case("OK");
                return Ok((untagged, Status { ok, text }));
            }
            if response.starts_with(b"+") {
                // A continuation outside an expected one: answer with an empty line (SASL abort).
                self.conn.send(b"\r\n").await?;
                continue;
            }
            if response.starts_with(b"* ") {
                response.drain(..2);
                untagged.push(response);
            }
        }
    }

    async fn line_with_literals(&mut self) -> NetResult<Vec<u8>> {
        let mut out = self.conn.line().await?;
        loop {
            let Some(n) = literal_size(&out) else {
                return Ok(out);
            };
            if n > 80 * 1024 * 1024 {
                return err("The server sent a message larger than 80 MB");
            }
            out.extend_from_slice(b"\r\n");
            out.extend(self.conn.exact(n).await?);
            out.extend(self.conn.line().await?);
        }
    }

    async fn command(&mut self, command: &str) -> NetResult<(Vec<Vec<u8>>, Status)> {
        let tag = self.next_tag();
        self.conn
            .send(format!("{tag} {command}\r\n").as_bytes())
            .await?;
        self.read(&tag).await
    }

    async fn simple(&mut self, command: &str) -> NetResult<Vec<Vec<u8>>> {
        let (untagged, status) = self.command(command).await?;
        if !status.ok {
            return err(format!(
                "The server refused {}: {}",
                command.split_whitespace().next().unwrap_or(command),
                status.text
            ));
        }
        Ok(untagged)
    }

    async fn capability(&mut self) -> NetResult<()> {
        let untagged = self.simple("CAPABILITY").await?;
        self.caps = untagged
            .iter()
            .filter(|u| u.len() >= 10 && u[..10].eq_ignore_ascii_case(b"CAPABILITY"))
            .flat_map(|u| {
                String::from_utf8_lossy(&u[10..])
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok(())
    }

    /// Sends a command whose last argument is a literal, waiting for the continuation unless
    /// the server accepts non-synchronizing literals.
    async fn with_literal(&mut self, head: &str, data: &[u8]) -> NetResult<(Vec<Vec<u8>>, Status)> {
        let tag = self.next_tag();
        let plus = self.has("LITERAL+");
        self.conn
            .send(
                format!(
                    "{tag} {head} {{{}{}}}\r\n",
                    data.len(),
                    if plus { "+" } else { "" }
                )
                .as_bytes(),
            )
            .await?;
        if !plus {
            let line = self.conn.line().await?;
            if !line.starts_with(b"+") {
                return err(format!(
                    "The server refused the data: {}",
                    String::from_utf8_lossy(&line)
                ));
            }
        }
        self.conn.send(data).await?;
        self.conn.send(b"\r\n").await?;
        self.read(&tag).await
    }

    async fn authenticate(&mut self, mechanism: &str, initial: &[u8]) -> NetResult<Status> {
        let tag = self.next_tag();
        let encoded = b64(initial);
        if self.has("SASL-IR") {
            self.conn
                .send(format!("{tag} AUTHENTICATE {mechanism} {encoded}\r\n").as_bytes())
                .await?;
        } else {
            self.conn
                .send(format!("{tag} AUTHENTICATE {mechanism}\r\n").as_bytes())
                .await?;
            let line = self.conn.line().await?;
            if !line.starts_with(b"+") {
                let text = String::from_utf8_lossy(&line).into_owned();
                return Ok(Status { ok: false, text });
            }
            self.conn.send(format!("{encoded}\r\n").as_bytes()).await?;
        }
        Ok(self.read(&tag).await?.1)
    }

    pub async fn login(&mut self, user: &str, password: &str) -> NetResult<()> {
        let status = if self.has("AUTH=PLAIN") {
            self.authenticate("PLAIN", format!("\0{user}\0{password}").as_bytes())
                .await?
        } else if self.has("LOGINDISABLED") {
            return err("The server does not allow password sign-in");
        } else if user.is_ascii()
            && password.is_ascii()
            && !user.contains(['\r', '\n'])
            && !password.contains(['\r', '\n'])
        {
            self.command(&format!("LOGIN {} {}", quote(user), quote(password)))
                .await?
                .1
        } else {
            return err("The server cannot take this user name or password");
        };
        if !status.ok {
            return Err(NetError(format!(
                "signIn:The server rejected the sign-in: {}",
                strip_tag_text(&status.text)
            )));
        }
        self.capability().await
    }

    pub async fn xoauth2(&mut self, user: &str, token: &str) -> NetResult<()> {
        let status = self
            .authenticate(
                "XOAUTH2",
                format!("user={user}\x01auth=Bearer {token}\x01\x01").as_bytes(),
            )
            .await?;
        if !status.ok {
            return Err(NetError(format!(
                "signIn:The server rejected the sign-in: {}",
                strip_tag_text(&status.text)
            )));
        }
        self.capability().await
    }

    pub async fn mailboxes(&mut self) -> NetResult<Vec<RemoteMailbox>> {
        let untagged = self.simple("LIST \"\" \"*\"").await?;
        let mut out = Vec::new();
        for u in untagged {
            let items = parse(&u);
            if !items
                .first()
                .is_some_and(|i| i.text().eq_ignore_ascii_case("LIST"))
                || items.len() < 4
            {
                continue;
            }
            let flags: Vec<String> = items[1]
                .list()
                .iter()
                .map(|f| f.text().to_ascii_lowercase())
                .collect();
            let delimiter = items[2].text();
            let remote = items[3].text();
            if remote.is_empty() || flags.iter().any(|f| f == "\\nonexistent") {
                continue;
            }
            // Virtual mailboxes repeat messages already mirrored elsewhere.
            if flags
                .iter()
                .any(|f| matches!(f.as_str(), "\\all" | "\\flagged" | "\\important"))
            {
                continue;
            }
            let (parent_remote, leaf) = match (!delimiter.is_empty())
                .then(|| remote.rsplit_once(delimiter.as_str()))
                .flatten()
            {
                Some((parent, leaf)) => (parent.to_owned(), leaf.to_owned()),
                None => (String::new(), remote.clone()),
            };
            let selectable = !flags.iter().any(|f| f == "\\noselect");
            let role = if !selectable {
                "folder".to_owned()
            } else {
                role_of(&remote, &leaf, &flags)
            };
            let name = if role == "inbox" {
                "Inbox".to_owned()
            } else {
                decode_name(&leaf)
            };
            out.push(RemoteMailbox {
                remote,
                name,
                parent_remote,
                role,
            });
        }
        // Parents that exist only as part of a name.
        let names: Vec<String> = out.iter().map(|m| m.remote.clone()).collect();
        let mut extra = Vec::new();
        for m in &out {
            if !m.parent_remote.is_empty()
                && !names.contains(&m.parent_remote)
                && !extra
                    .iter()
                    .any(|e: &RemoteMailbox| e.remote == m.parent_remote)
            {
                extra.push(RemoteMailbox {
                    remote: m.parent_remote.clone(),
                    name: decode_name(
                        m.parent_remote
                            .rsplit(['/', '.'])
                            .next()
                            .unwrap_or(&m.parent_remote),
                    ),
                    parent_remote: String::new(),
                    role: "folder".into(),
                });
            }
        }
        out.extend(extra);
        Ok(out)
    }

    /// Selects a mailbox; returns its UIDVALIDITY and message count.
    pub async fn select(&mut self, remote: &str) -> NetResult<(String, u32)> {
        let (untagged, status) = self.command(&format!("SELECT {}", quote(remote))).await?;
        if !status.ok {
            self.selected = None;
            return err(format!(
                "The mailbox {} could not be opened: {}",
                decode_name(remote),
                strip_tag_text(&status.text)
            ));
        }
        self.selected = Some(remote.to_owned());
        let mut validity = String::new();
        let mut exists = 0;
        for u in &untagged {
            let text = String::from_utf8_lossy(u);
            if let Some(v) = code(&text, "UIDVALIDITY") {
                validity = v.first().map(|s| (*s).to_owned()).unwrap_or_default();
            }
            let mut words = text.split_whitespace();
            if let (Some(n), Some(w)) = (words.next(), words.next())
                && w.eq_ignore_ascii_case("EXISTS")
            {
                exists = n.parse().unwrap_or(0);
            }
        }
        Ok((validity, exists))
    }

    async fn ensure(&mut self, remote: &str) -> NetResult<()> {
        if self.selected.as_deref() != Some(remote) {
            self.select(remote).await?;
        }
        Ok(())
    }

    pub async fn list(&mut self, remote: &str) -> NetResult<(String, Vec<RemoteMessage>)> {
        let (validity, exists) = self.select(remote).await?;
        if exists == 0 {
            return Ok((validity, Vec::new()));
        }
        let untagged = self.simple("UID FETCH 1:* (UID FLAGS RFC822.SIZE)").await?;
        let mut out = Vec::new();
        for u in untagged {
            let items = parse(&u);
            if items.len() < 3 || !items[1].text().eq_ignore_ascii_case("FETCH") {
                continue;
            }
            let fields = fetch_fields(&items[2]);
            let Some(uid) = fields.get("UID").and_then(|v| v.text().parse::<u32>().ok()) else {
                continue;
            };
            let flags = fields
                .get("FLAGS")
                .map(|f| f.list().iter().map(Item::text).collect())
                .unwrap_or_default();
            let size = fields
                .get("RFC822.SIZE")
                .and_then(|v| v.text().parse::<f64>().ok())
                .unwrap_or(0.0);
            out.push(RemoteMessage { uid, flags, size });
        }
        Ok((validity, out))
    }

    /// Whole messages by UID, without marking them read.
    pub async fn fetch(&mut self, remote: &str, uids: &[u32]) -> NetResult<Vec<(u32, Vec<u8>)>> {
        self.ensure(remote).await?;
        let untagged = self
            .simple(&format!("UID FETCH {} (UID BODY.PEEK[])", uid_set(uids)))
            .await?;
        let mut out = Vec::new();
        for u in untagged {
            let items = parse(&u);
            if items.len() < 3 || !items[1].text().eq_ignore_ascii_case("FETCH") {
                continue;
            }
            let fields = fetch_fields(&items[2]);
            let (Some(uid), Some(Item::Str(body))) = (
                fields.get("UID").and_then(|v| v.text().parse::<u32>().ok()),
                fields.get("BODY[]"),
            ) else {
                continue;
            };
            out.push((uid, body.clone()));
        }
        Ok(out)
    }

    pub async fn store(
        &mut self,
        remote: &str,
        uids: &[u32],
        add: bool,
        flags: &[&str],
    ) -> NetResult<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.ensure(remote).await?;
        self.simple(&format!(
            "UID STORE {} {}FLAGS.SILENT ({})",
            uid_set(uids),
            if add { "+" } else { "-" },
            flags.join(" ")
        ))
        .await?;
        Ok(())
    }

    /// Moves messages; returns the UIDs they got in the destination when the server says.
    pub async fn move_to(
        &mut self,
        remote: &str,
        uids: &[u32],
        target: &str,
    ) -> NetResult<HashMap<u32, u32>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
        }
        self.ensure(remote).await?;
        let set = uid_set(uids);
        let (untagged, status) = if self.has("MOVE") {
            self.command(&format!("UID MOVE {set} {}", quote(target)))
                .await?
        } else {
            let copied = self
                .command(&format!("UID COPY {set} {}", quote(target)))
                .await?;
            if copied.1.ok {
                self.expunge(remote, uids).await?;
            }
            copied
        };
        if !status.ok {
            return err(format!(
                "The server could not move the messages: {}",
                strip_tag_text(&status.text)
            ));
        }
        let mut map = HashMap::new();
        for text in untagged
            .iter()
            .map(|u| String::from_utf8_lossy(u).into_owned())
            .chain([status.text.clone()])
        {
            if let Some(v) = code(&text, "COPYUID")
                && v.len() >= 3
            {
                let (from, to) = (expand(v[1]), expand(v[2]));
                for (a, b) in from.into_iter().zip(to) {
                    map.insert(a, b);
                }
            }
        }
        Ok(map)
    }

    /// Removes messages for good.
    pub async fn expunge(&mut self, remote: &str, uids: &[u32]) -> NetResult<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.store(remote, uids, true, &["\\Deleted"]).await?;
        if self.has("UIDPLUS") {
            self.simple(&format!("UID EXPUNGE {}", uid_set(uids)))
                .await?;
        } else {
            self.simple("EXPUNGE").await?;
        }
        Ok(())
    }

    /// Adds a message; returns its UID when the server says (UIDPLUS).
    pub async fn append(
        &mut self,
        remote: &str,
        flags: &[&str],
        data: &[u8],
    ) -> NetResult<Option<u32>> {
        let (_, status) = self
            .with_literal(
                &format!("APPEND {} ({})", quote(remote), flags.join(" ")),
                data,
            )
            .await?;
        if !status.ok {
            return err(format!(
                "The server did not accept the message: {}",
                strip_tag_text(&status.text)
            ));
        }
        self.selected = None;
        Ok(code(&status.text, "APPENDUID").and_then(|v| v.get(1).and_then(|u| u.parse().ok())))
    }

    pub async fn create(&mut self, remote: &str) -> NetResult<()> {
        let (_, status) = self.command(&format!("CREATE {}", quote(remote))).await?;
        if !status.ok && !status.text.to_ascii_uppercase().contains("ALREADYEXISTS") {
            return err(format!(
                "The server did not create the mailbox: {}",
                strip_tag_text(&status.text)
            ));
        }
        let _ = self.command(&format!("SUBSCRIBE {}", quote(remote))).await;
        Ok(())
    }

    pub async fn rename(&mut self, from: &str, to: &str) -> NetResult<()> {
        self.selected = None;
        self.simple(&format!("RENAME {} {}", quote(from), quote(to)))
            .await?;
        Ok(())
    }

    /// The hierarchy delimiter (for new mailboxes' names).
    pub async fn delimiter(&mut self) -> NetResult<String> {
        let untagged = self.simple("LIST \"\" \"\"").await?;
        Ok(untagged
            .iter()
            .map(|u| parse(u))
            .find(|i| i.len() >= 3)
            .map(|i| i[2].text())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "/".into()))
    }

    pub async fn logout(mut self) {
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(3), self.command("LOGOUT")).await;
    }
}

fn strip_tag_text(text: &str) -> String {
    let t = text.splitn(2, ' ').nth(1).unwrap_or(text);
    let t = match t.find(']') {
        Some(end) if t.starts_with('[') => t[end + 1..].trim(),
        _ => t.trim(),
    };
    if t.is_empty() {
        "no reason given".into()
    } else {
        t.chars().take(200).collect()
    }
}

/// `{n}` (or `{n+}`) at the end of a line.
fn literal_size(line: &[u8]) -> Option<usize> {
    if line.last() != Some(&b'}') {
        return None;
    }
    let start = line.iter().rposition(|b| *b == b'{')?;
    std::str::from_utf8(&line[start + 1..line.len() - 1])
        .ok()?
        .trim_end_matches('+')
        .parse()
        .ok()
}

fn expand(set: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in set.split(',') {
        match part.split_once(':') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    let (lo, hi) = (a.min(b), a.max(b));
                    if hi - lo < 100_000 {
                        out.extend(lo..=hi);
                    }
                }
            }
            None => out.extend(part.parse::<u32>().ok()),
        }
    }
    out
}

fn fetch_fields(list: &Item) -> HashMap<String, Item> {
    let items = list.list();
    let mut out = HashMap::new();
    let mut i = 0;
    while i + 1 < items.len() {
        out.insert(items[i].text().to_ascii_uppercase(), items[i + 1].clone());
        i += 2;
    }
    out
}

fn role_of(remote: &str, leaf: &str, flags: &[String]) -> String {
    if remote.eq_ignore_ascii_case("INBOX") {
        return "inbox".into();
    }
    for (flag, role) in [
        ("\\sent", "sent"),
        ("\\drafts", "drafts"),
        ("\\trash", "trash"),
        ("\\junk", "junk"),
        ("\\archive", "archive"),
    ] {
        if flags.iter().any(|f| f == flag) {
            return role.into();
        }
    }
    match decode_name(leaf).to_lowercase().as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" => "sent".into(),
        "drafts" => "drafts".into(),
        "trash" | "deleted items" | "deleted messages" | "bin" => "trash".into(),
        "junk" | "spam" | "junk email" | "junk e-mail" => "junk".into(),
        "archive" | "archives" => "archive".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fetch_with_literal() {
        let data = b"12 FETCH (UID 40 FLAGS (\\Seen \\Flagged) BODY[] {5}\r\nhello)";
        let items = parse(data);
        let fields = fetch_fields(&items[2]);
        assert_eq!(fields["UID"].text(), "40");
        assert_eq!(fields["BODY[]"], Item::Str(b"hello".to_vec()));
        assert_eq!(fields["FLAGS"].list().len(), 2);
    }

    #[test]
    fn names_and_sets() {
        assert_eq!(decode_name("Entw&APw-rfe"), "Entwürfe");
        assert_eq!(encode_name("Entwürfe"), "Entw&APw-rfe");
        assert_eq!(encode_name("A&B"), "A&-B");
        assert_eq!(decode_name("&ZeVnLIqe-"), "日本語");
        assert_eq!(uid_set(&[1, 2, 3, 7, 9, 10]), "1:3,7,9:10");
        assert_eq!(expand("4:6,9"), vec![4, 5, 6, 9]);
        assert_eq!(literal_size(b"* 1 FETCH (BODY[] {120}"), Some(120));
        assert_eq!(
            code("OK [APPENDUID 38505 3955] APPEND completed", "APPENDUID"),
            Some(vec!["38505", "3955"])
        );
    }
}
