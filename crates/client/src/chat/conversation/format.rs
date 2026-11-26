//! Safe inline message text formatting: linkification, Discord mentions, and inline Markdown.

use leptos::prelude::*;

/// A parsed inline fragment within a single line of a chat message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Text(String),
    Link { url: String, display: String },
    Mention(String),
    Code(String),
    Bold(Vec<Token>),
    Italic(Vec<Token>),
    Strike(Vec<Token>),
}

const MAX_DEPTH: u8 = 4;

fn prev_char(s: &str, idx: usize) -> Option<char> {
    s[..idx].chars().next_back()
}

fn push_text(tokens: &mut Vec<Token>, slice: &str) {
    if slice.is_empty() {
        return;
    }
    if let Some(Token::Text(prev)) = tokens.last_mut() {
        prev.push_str(slice);
    } else {
        tokens.push(Token::Text(slice.to_owned()));
    }
}

fn parse_url(s: &str, start: usize) -> Option<(usize, &str)> {
    if start > 0 && prev_char(s, start).is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    let rest = &s[start..];
    let scheme_len = if rest.starts_with("https://") {
        8
    } else if rest.starts_with("http://") {
        7
    } else {
        return None;
    };
    let mut end = start;
    for (off, ch) in rest.char_indices() {
        if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '`') {
            break;
        }
        end = start + off + ch.len_utf8();
    }
    while end > start + scheme_len {
        let slice = &s[start..end];
        let Some(last) = slice.chars().next_back() else {
            break;
        };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => slice.matches('(').count() < slice.matches(')').count(),
            ']' => slice.matches('[').count() < slice.matches(']').count(),
            '}' => slice.matches('{').count() < slice.matches('}').count(),
            _ => false,
        };
        if strip {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    if end <= start + scheme_len {
        return None;
    }
    Some((end, &s[start..end]))
}

fn parse_mention(s: &str, start: usize) -> Option<(usize, &str)> {
    if !s[start..].starts_with('@') {
        return None;
    }
    if start > 0 && !prev_char(s, start).is_some_and(char::is_whitespace) {
        return None;
    }
    let after = &s[start + 1..];
    let first = after.chars().next()?;
    if !(first.is_alphanumeric() || first == '_') {
        return None;
    }
    let mut end = start + 1;
    for (off, ch) in after.char_indices() {
        if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            end = start + 1 + off + ch.len_utf8();
        } else {
            break;
        }
    }
    while end > start + 1 && s[start..end].ends_with('.') {
        end -= 1;
    }
    if end <= start + 1 {
        return None;
    }
    Some((end, &s[start..end]))
}

fn find_inline_code_end(s: &str, from: usize) -> Option<usize> {
    let close = s[from..].find('`')?;
    (close > 0).then_some(from + close)
}

fn skip_code_span(s: &str, idx: usize) -> Option<usize> {
    find_inline_code_end(s, idx + 1).map(|end| end + 1)
}

fn find_closing_pair(s: &str, from: usize, delim: &str) -> Option<usize> {
    let mut idx = from;
    while idx < s.len() {
        let rest = &s[idx..];
        if rest.starts_with('`')
            && let Some(next) = skip_code_span(s, idx)
        {
            idx = next;
            continue;
        }
        if rest.starts_with(delim) {
            if idx == from || s[from..idx].trim().is_empty() {
                return None;
            }
            // Prefer the outer two asterisks of a `***` run when closing `**` wraps `*...*`.
            if delim == "**" && s[idx + 2..].starts_with('*') {
                let inner = &s[from..idx];
                if inner.matches('*').count() % 2 == 1 {
                    let mut end = idx;
                    while s[end + 2..].starts_with('*') {
                        end += 1;
                    }
                    return Some(end);
                }
            }
            return Some(idx);
        }
        let ch = rest.chars().next()?;
        idx += ch.len_utf8();
    }
    None
}

fn find_closing_italic(s: &str, start: usize, delim: char) -> Option<usize> {
    if let Some(prev) = prev_char(s, start)
        && (prev.is_alphanumeric() || prev == delim)
    {
        return None;
    }
    let after_open = s[start + 1..].chars().next()?;
    if after_open.is_whitespace() || after_open == delim {
        return None;
    }
    let mut idx = start + 1;
    while idx < s.len() {
        let rest = &s[idx..];
        if rest.starts_with('`')
            && let Some(next) = skip_code_span(s, idx)
        {
            idx = next;
            continue;
        }
        if delim == '*' && rest.starts_with("**") {
            idx += 2;
            continue;
        }
        let ch = rest.chars().next()?;
        if ch == delim {
            let before_ok = prev_char(s, idx).is_some_and(|c| !c.is_whitespace() && c != delim);
            let after_ok = s[idx + 1..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != delim);
            if before_ok && after_ok && idx > start + 1 {
                return Some(idx);
            }
        }
        idx += ch.len_utf8();
    }
    None
}

fn parse_inline_depth(
    s: &str,
    format_markdown: bool,
    is_discord: bool,
    depth: u8,
) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];

        if format_markdown && rest.starts_with('`') {
            if let Some(close) = find_inline_code_end(s, i + 1) {
                tokens.push(Token::Code(s[i + 1..close].to_owned()));
                i = close + 1;
                continue;
            }
            push_text(&mut tokens, "`");
            i += 1;
            continue;
        }

        if let Some((end, url)) = parse_url(s, i) {
            tokens.push(Token::Link {
                url: url.to_owned(),
                display: url.to_owned(),
            });
            i = end;
            continue;
        }

        if is_discord && let Some((end, tok)) = parse_mention(s, i) {
            tokens.push(Token::Mention(tok.to_owned()));
            i = end;
            continue;
        }

        if format_markdown && depth < MAX_DEPTH {
            if rest.starts_with("**") {
                if let Some(close) = find_closing_pair(s, i + 2, "**") {
                    let inner =
                        parse_inline_depth(&s[i + 2..close], format_markdown, is_discord, depth + 1);
                    tokens.push(Token::Bold(inner));
                    i = close + 2;
                } else {
                    push_text(&mut tokens, "**");
                    i += 2;
                }
                continue;
            }

            if rest.starts_with("~~") {
                if let Some(close) = find_closing_pair(s, i + 2, "~~") {
                    let inner =
                        parse_inline_depth(&s[i + 2..close], format_markdown, is_discord, depth + 1);
                    tokens.push(Token::Strike(inner));
                    i = close + 2;
                } else {
                    push_text(&mut tokens, "~~");
                    i += 2;
                }
                continue;
            }

            if let Some(delim) = rest.chars().next().filter(|&c| c == '*' || c == '_') {
                if let Some(close) = find_closing_italic(s, i, delim) {
                    let inner =
                        parse_inline_depth(&s[i + 1..close], format_markdown, is_discord, depth + 1);
                    tokens.push(Token::Italic(inner));
                    i = close + 1;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().expect("non-empty slice");
        let next = i + ch.len_utf8();
        push_text(&mut tokens, &s[i..next]);
        i = next;
    }
    tokens
}

pub(crate) fn parse_lines(
    text: &str,
    format_markdown: bool,
    is_discord: bool,
) -> Vec<Vec<Token>> {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            parse_inline_depth(line, format_markdown, is_discord, 0)
        })
        .collect()
}

fn render_token(token: Token) -> AnyView {
    match token {
        Token::Text(t) => t.into_any(),
        Token::Link { url, display } => view! {
            <a class="chat-inline-link" href=url target="_blank" rel="noopener noreferrer">{display}</a>
        }
        .into_any(),
        Token::Mention(tok) => view! {
            <span class="chat-mention">{tok}</span>
        }
        .into_any(),
        Token::Code(code) => view! {
            <code class="chat-inline-code">{code}</code>
        }
        .into_any(),
        Token::Bold(inner) => view! {
            <strong>{render_tokens(inner)}</strong>
        }
        .into_any(),
        Token::Italic(inner) => view! {
            <em>{render_tokens(inner)}</em>
        }
        .into_any(),
        Token::Strike(inner) => view! {
            <del>{render_tokens(inner)}</del>
        }
        .into_any(),
    }
}

fn render_tokens(tokens: Vec<Token>) -> AnyView {
    tokens.into_iter().map(render_token).collect_view().into_any()
}

/// Renders message text into Leptos views with linkification, Discord mentions, and inline Markdown.
pub fn render_message_text(text: &str, format_markdown: bool, is_discord: bool) -> AnyView {
    let lines = parse_lines(text, format_markdown, is_discord);
    let total = lines.len();
    lines
        .into_iter()
        .enumerate()
        .map(|(i, tokens)| {
            let line_view = render_tokens(tokens);
            if i + 1 < total {
                view! { {line_view}<br/> }.into_any()
            } else {
                line_view
            }
        })
        .collect_view()
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(text: &str, format_markdown: bool, is_discord: bool) -> Vec<Token> {
        let mut lines = parse_lines(text, format_markdown, is_discord);
        assert_eq!(lines.len(), 1);
        lines.pop().unwrap()
    }

    #[test]
    fn extracts_urls_and_strips_unmatched_trailing_punctuation() {
        assert_eq!(
            parse_one("See (https://example.com/a_(b)).", true, false),
            vec![
                Token::Text("See (".into()),
                Token::Link {
                    url: "https://example.com/a_(b)".into(),
                    display: "https://example.com/a_(b)".into(),
                },
                Token::Text(").".into()),
            ]
        );
        assert_eq!(
            parse_one("Plain http://localhost:3000/chat!", false, false),
            vec![
                Token::Text("Plain ".into()),
                Token::Link {
                    url: "http://localhost:3000/chat".into(),
                    display: "http://localhost:3000/chat".into(),
                },
                Token::Text("!".into()),
            ]
        );
    }

    #[test]
    fn formats_inline_code_bold_strike_and_italic() {
        assert_eq!(
            parse_one(
                "Run `caddy reload` with **zero** ~~downtime~~ and *care*.",
                true,
                false
            ),
            vec![
                Token::Text("Run ".into()),
                Token::Code("caddy reload".into()),
                Token::Text(" with ".into()),
                Token::Bold(vec![Token::Text("zero".into())]),
                Token::Text(" ".into()),
                Token::Strike(vec![Token::Text("downtime".into())]),
                Token::Text(" and ".into()),
                Token::Italic(vec![Token::Text("care".into())]),
                Token::Text(".".into()),
            ]
        );
    }

    #[test]
    fn preserves_snake_case_and_respects_markdown_toggle() {
        assert_eq!(
            parse_one("Set my_var_name and _this_ too", true, false),
            vec![
                Token::Text("Set my_var_name and ".into()),
                Token::Italic(vec![Token::Text("this".into())]),
                Token::Text(" too".into()),
            ]
        );
        assert_eq!(
            parse_one("**bold** `code` https://dar.local", false, false),
            vec![
                Token::Text("**bold** `code` ".into()),
                Token::Link {
                    url: "https://dar.local".into(),
                    display: "https://dar.local".into(),
                },
            ]
        );
    }

    #[test]
    fn styles_discord_mentions_only_after_start_or_whitespace() {
        assert_eq!(
            parse_one("@lina.k ping @anas. email@domain.com", true, true),
            vec![
                Token::Mention("@lina.k".into()),
                Token::Text(" ping ".into()),
                Token::Mention("@anas".into()),
                Token::Text(". email@domain.com".into()),
            ]
        );
        assert_eq!(
            parse_one("@anas", true, false),
            vec![Token::Text("@anas".into())]
        );
    }

    #[test]
    fn keeps_unclosed_delimiters_literal_and_splits_lines() {
        assert_eq!(
            parse_one("**unclosed *also `open ~~nope", true, false),
            vec![Token::Text("**unclosed *also `open ~~nope".into())]
        );
        assert_eq!(
            parse_lines("Are we still on for dinner?\nYes!", true, false),
            vec![
                vec![Token::Text("Are we still on for dinner?".into())],
                vec![Token::Text("Yes!".into())],
            ]
        );
    }
}
