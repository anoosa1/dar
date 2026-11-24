//! Messages written in Markdown, sent as HTML: the same subset the client previews
//! (`client/src/markdown.rs`): headings, paragraphs with line breaks, quotes, lists and task
//! lists, fenced code, rules, and inline code, bold, italic and links. All text is escaped and
//! only http, https and mailto links are kept, so the HTML a message carries is Dar's own.

use crate::news::html::escape_html;

fn inline(source: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    let mut plain = 0;
    while at < source.len() {
        let rest = &source[at..];
        if matches!(rest.as_bytes()[0], b'`' | b'*' | b'[')
            && let Some((length, html)) = token(rest)
        {
            out.push_str(&escape_html(&source[plain..at]));
            out.push_str(&html);
            at += length;
            plain = at;
            continue;
        }
        at += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out.push_str(&escape_html(&source[plain..]));
    out
}

fn token(rest: &str) -> Option<(usize, String)> {
    let ok = |s: &str| !s.is_empty() && !s.contains('\n');
    if let Some(body) = rest.strip_prefix('`') {
        let close = body.find('`')?;
        return ok(&body[..close]).then(|| {
            (
                close + 2,
                format!("<code>{}</code>", escape_html(&body[..close])),
            )
        });
    }
    if let Some(body) = rest.strip_prefix("**")
        && let Some(close) = body.find("**")
        && ok(&body[..close])
        && !body[..close].contains('*')
    {
        return Some((
            close + 4,
            format!("<strong>{}</strong>", escape_html(&body[..close])),
        ));
    }
    if let Some(body) = rest.strip_prefix('*') {
        let close = body.find('*')?;
        return ok(&body[..close]).then(|| {
            (
                close + 2,
                format!("<em>{}</em>", escape_html(&body[..close])),
            )
        });
    }
    let body = rest.strip_prefix('[')?;
    let close = body.find(']')?;
    let label = &body[..close];
    let target_part = body[close + 1..].strip_prefix('(')?;
    let end = target_part.find(|c: char| c == ')' || c.is_whitespace())?;
    if label.contains('\n') || end == 0 || !target_part[end..].starts_with(')') {
        return None;
    }
    let target = &target_part[..end];
    if !(target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:"))
    {
        return None;
    }
    Some((
        1 + close + 2 + end + 1,
        format!(
            "<a href=\"{}\">{}</a>",
            escape_html(target),
            escape_html(label)
        ),
    ))
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((false, rest));
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| {
            trimmed[digits..]
                .strip_prefix(". ")
                .map(|rest| (true, rest))
        })
        .flatten()
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// The message body as HTML (a fragment, wrapped in a minimal document).
pub fn to_html(source: &str) -> String {
    let lines: Vec<&str> = source
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push_str(&format!(
                "<pre><code>{}</code></pre>\n",
                escape_html(&code.join("\n"))
            ));
            continue;
        }
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // "-- " starts a signature: kept as plain lines.
        if line == "-- " {
            let rest: Vec<String> = lines[i..].iter().map(|l| escape_html(l)).collect();
            out.push_str(&format!("<p>{}</p>\n", rest.join("<br>\n")));
            break;
        }
        if is_rule(line) {
            out.push_str("<hr>\n");
            i += 1;
            continue;
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes)
            && line[hashes..].starts_with(char::is_whitespace)
            && !line[hashes..].trim().is_empty()
        {
            out.push_str(&format!(
                "<h{hashes}>{}</h{hashes}>\n",
                inline(line[hashes..].trim())
            ));
            i += 1;
            continue;
        }
        if line.starts_with('>') {
            let mut quoted = Vec::new();
            while i < lines.len() && lines[i].starts_with('>') {
                let rest = &lines[i][1..];
                quoted.push(rest.strip_prefix(' ').unwrap_or(rest));
                i += 1;
            }
            out.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                to_html(&quoted.join("\n"))
            ));
            continue;
        }
        if let Some((ordered, _)) = list_item(line) {
            let tag = if ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>\n"));
            while let Some((_, text)) = lines.get(i).and_then(|l| list_item(l)) {
                i += 1;
                let check = text.strip_prefix('[').and_then(|t| {
                    let mark = t.chars().next()?;
                    (matches!(mark, ' ' | 'x' | 'X') && t[1..].starts_with(']'))
                        .then(|| (mark == ' ', t[2..].trim_start()))
                });
                match check {
                    Some((unchecked, rest)) => out.push_str(&format!(
                        "<li>{} {}</li>\n",
                        if unchecked { "\u{2610}" } else { "\u{2611}" },
                        inline(rest)
                    )),
                    None => out.push_str(&format!("<li>{}</li>\n", inline(text))),
                }
            }
            out.push_str(&format!("</{tag}>\n"));
            continue;
        }
        // A paragraph: following lines until a blank line or another block, joined by line breaks.
        let mut paragraph = vec![inline(line)];
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            let t = next.trim_start();
            if next.trim().is_empty()
                || t.starts_with("```")
                || next.starts_with('>')
                || list_item(next).is_some()
                || is_rule(next)
                || next == "-- "
                || (t.starts_with('#') && t.trim_start_matches('#').starts_with(' '))
            {
                break;
            }
            paragraph.push(inline(next));
            i += 1;
        }
        out.push_str(&format!("<p>{}</p>\n", paragraph.join("<br>\n")));
    }
    out
}

pub fn document(source: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"></head><body>\n{}</body></html>\n",
        to_html(source)
    )
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn renders_the_previewed_subset_safely() {
        let html = to_html(
            "# Plan\n\nHello **team**, see [the doc](https://x.test/a?b=1&c=2) and `code`.\nSecond line <script>\n\n- [x] done\n- [ ] next\n\n> quoted *text*\n> more\n\n```\n<b>raw</b>\n```\n[bad](javascript:alert(1))\n\n-- \nAnas",
        );
        assert!(html.contains("<h1>Plan</h1>"), "{html}");
        assert!(html.contains("<p>Hello <strong>team</strong>, see <a href=\"https://x.test/a?b=1&amp;c=2\">the doc</a> and <code>code</code>.<br>\nSecond line &lt;script&gt;</p>"), "{html}");
        assert!(
            html.contains("<ul>\n<li>\u{2611} done</li>\n<li>\u{2610} next</li>\n</ul>"),
            "{html}"
        );
        assert!(
            html.contains("<blockquote><p>quoted <em>text</em><br>\nmore</p>\n</blockquote>"),
            "{html}"
        );
        assert!(
            html.contains("<pre><code>&lt;b&gt;raw&lt;/b&gt;</code></pre>"),
            "{html}"
        );
        assert!(
            !html.contains("javascript:alert(1)\"") && html.contains("[bad](javascript:alert(1))"),
            "{html}"
        );
        assert!(html.ends_with("<p>-- <br>\nAnas</p>\n"), "{html}");
    }
}
