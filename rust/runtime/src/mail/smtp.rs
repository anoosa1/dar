//! SMTP submission (RFC 6409): TLS from the start (465) or STARTTLS (587), AUTH PLAIN, LOGIN or
//! XOAUTH2, then one message to its envelope recipients.

use super::net::{Conn, NetError, NetResult, b64, err};

pub enum Auth<'a> {
    Password(&'a str, &'a str),
    OAuth(&'a str, &'a str),
}

struct Smtp {
    conn: Conn,
    host: String,
}

impl Smtp {
    /// A reply: its code and text lines.
    async fn reply(&mut self) -> NetResult<(u16, String)> {
        let mut text = Vec::new();
        loop {
            let line = String::from_utf8_lossy(&self.conn.line().await?).into_owned();
            if line.len() < 3 {
                return err("The mail server sent an unreadable reply");
            }
            let code: u16 = line[..3].parse().map_err(|_| NetError("The mail server sent an unreadable reply".into()))?;
            text.push(line.get(4..).unwrap_or_default().to_owned());
            if line.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, text.join("\n")));
            }
        }
    }

    async fn expect(&mut self, command: &str, ok: &[u16]) -> NetResult<String> {
        if !command.is_empty() {
            self.conn.send(format!("{command}\r\n").as_bytes()).await?;
        }
        let (code, text) = self.reply().await?;
        if !ok.contains(&code) {
            let verb = command.split([' ', ':']).next().unwrap_or("the connection");
            return err(format!("{} refused {}: {code} {}", self.host, if verb.is_empty() { "the connection" } else { verb }, text.lines().next().unwrap_or_default()));
        }
        Ok(text)
    }

    async fn ehlo(&mut self) -> NetResult<Vec<String>> {
        let text = self.expect("EHLO dar.localhost", &[250]).await?;
        Ok(text.lines().skip(1).map(|l| l.to_ascii_uppercase()).collect())
    }
}

/// Sends `data` from `from` to every address in `to`.
pub async fn send(host: &str, port: u16, security: &str, auth: Auth<'_>, from: &str, to: &[String], data: &[u8]) -> NetResult<()> {
    if to.is_empty() {
        return err("The message has no recipients");
    }
    for address in std::iter::once(from).chain(to.iter().map(String::as_str)) {
        if address.contains(['\r', '\n', '<', '>']) || !address.contains('@') {
            return err(format!("{address} is not a valid address"));
        }
    }
    let conn = Conn::open(host, port, security == "tls").await?;
    let mut s = Smtp { conn, host: host.to_owned() };
    s.expect("", &[220]).await?;
    let mut caps = s.ehlo().await?;
    if security == "starttls" {
        if !caps.iter().any(|c| c == "STARTTLS") {
            return err(format!("{host} does not offer STARTTLS"));
        }
        s.expect("STARTTLS", &[220]).await?;
        s.conn.starttls().await?;
        caps = s.ehlo().await?;
    }
    if !s.conn.secure() {
        return err("The connection is not encrypted");
    }
    let mechanisms: Vec<String> = caps.iter().filter_map(|c| c.strip_prefix("AUTH ").or_else(|| c.strip_prefix("AUTH="))).flat_map(|m| m.split_whitespace().map(str::to_owned).collect::<Vec<_>>()).collect();
    let signed = match auth {
        Auth::OAuth(user, token) => {
            s.conn.send(format!("AUTH XOAUTH2 {}\r\n", b64(format!("user={user}\x01auth=Bearer {token}\x01\x01").as_bytes())).as_bytes()).await?;
            let (mut code, mut text) = s.reply().await?;
            if code == 334 {
                // The error details; an empty line ends the exchange.
                s.conn.send(b"\r\n").await?;
                (code, text) = s.reply().await?;
            }
            (code == 235).then_some(()).ok_or(text)
        }
        Auth::Password(user, password) if mechanisms.iter().any(|m| m == "PLAIN") || !mechanisms.iter().any(|m| m == "LOGIN") => {
            s.conn.send(format!("AUTH PLAIN {}\r\n", b64(format!("\0{user}\0{password}").as_bytes())).as_bytes()).await?;
            let (code, text) = s.reply().await?;
            (code == 235).then_some(()).ok_or(text)
        }
        Auth::Password(user, password) => {
            s.expect("AUTH LOGIN", &[334]).await?;
            s.expect(&b64(user.as_bytes()), &[334]).await?;
            s.conn.send(format!("{}\r\n", b64(password.as_bytes())).as_bytes()).await?;
            let (code, text) = s.reply().await?;
            (code == 235).then_some(()).ok_or(text)
        }
    };
    if let Err(text) = signed {
        return Err(NetError(format!("signIn:The outgoing server rejected the sign-in: {}", text.lines().next().unwrap_or_default())));
    }
    s.expect(&format!("MAIL FROM:<{from}>"), &[250]).await?;
    for address in to {
        s.expect(&format!("RCPT TO:<{address}>"), &[250, 251]).await?;
    }
    s.expect("DATA", &[354]).await?;
    s.conn.send(&dot_stuff(data)).await?;
    s.expect("", &[250]).await?;
    let _ = s.expect("QUIT", &[221]).await;
    Ok(())
}

/// CRLF line endings, a leading dot doubled, and the final `.` line.
pub fn dot_stuff(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    let mut start = true;
    let mut previous = 0u8;
    for &b in data {
        if start && b == b'.' {
            out.push(b'.');
        }
        if b == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        start = b == b'\n';
        previous = b;
    }
    if !start {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn stuffs_dots_and_line_endings() {
        assert_eq!(super::dot_stuff(b"a\n.b\r\n..c"), b"a\r\n..b\r\n...c\r\n.\r\n".to_vec());
    }
}
