//! YAML properties at the top of a note (the subset Obsidian's property editor writes):
//! `key: value`, `key: [a, b]` and `key:` followed by `  - item` lines. Anything else is kept
//! as text.

#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Text(String),
    List(Vec<String>),
}

impl Prop {
    /// Items of a list, or a text split at commas (`tags: a, b`).
    pub fn items(&self) -> Vec<String> {
        match self {
            Prop::List(items) => items.clone(),
            Prop::Text(text) => text
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
    pub fn display(&self) -> String {
        match self {
            Prop::Text(text) => text.clone(),
            Prop::List(items) => items.join(", "),
        }
    }
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_owned();
        }
    }
    v.to_owned()
}

/// The properties and the offset where the body starts, when the note opens with `---`.
pub fn front_matter(text: &str) -> Option<(Vec<(String, Prop)>, usize)> {
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let rest = &text[bom..];
    let first = rest.find('\n')?;
    if rest[..first].trim_end() != "---" {
        return None;
    }
    let mut at = bom + first + 1;
    let mut lines = Vec::new();
    let mut end = None;
    for line in text[at..].split_inclusive('\n') {
        let next = at + line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        if content == "---" || content == "..." {
            end = Some(next);
            break;
        }
        lines.push(content);
        at = next;
    }
    let end = end?;
    let mut properties: Vec<(String, Prop)> = Vec::new();
    for line in lines {
        if let Some(item) = line
            .trim_start()
            .strip_prefix("- ")
            .or_else(|| (line.trim() == "-").then_some(""))
            && line.starts_with(char::is_whitespace)
            && let Some((_, value)) = properties.last_mut()
        {
            match value {
                Prop::List(items) => items.push(unquote(item)),
                Prop::Text(text) if text.is_empty() => *value = Prop::List(vec![unquote(item)]),
                _ => {}
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let prop = if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            Prop::List(
                inner
                    .split(',')
                    .map(unquote)
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        } else {
            Prop::Text(unquote(value))
        };
        properties.push((key.trim().to_owned(), prop));
    }
    Some((properties, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_properties() {
        let text = "---\ntitle: \"Trip\"\ntags: [a, 'b']\naliases:\n  - One\n  - Two\nempty:\nrating: 4\n---\nBody";
        let (props, body) = front_matter(text).unwrap();
        assert_eq!(&text[body..], "Body");
        assert_eq!(
            props,
            [
                ("title".into(), Prop::Text("Trip".into())),
                ("tags".into(), Prop::List(vec!["a".into(), "b".into()])),
                (
                    "aliases".into(),
                    Prop::List(vec!["One".into(), "Two".into()])
                ),
                ("empty".into(), Prop::Text(String::new())),
                ("rating".into(), Prop::Text("4".into())),
            ]
        );
        assert_eq!(front_matter("# No properties\n---\n"), None);
        assert_eq!(front_matter("---\nunclosed: yes\n"), None);
    }
}
