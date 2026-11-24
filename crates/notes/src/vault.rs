//! Which note a link names, as Obsidian resolves it. Paths are relative to the Notes folder and
//! keep their extension (`Projects/Plan.md`).

use std::collections::HashMap;

use crate::Link;

pub struct Vault {
    paths: Vec<String>,
    by_path: HashMap<String, usize>,
    by_stem: HashMap<String, Vec<usize>>,
}

fn without_ext(path: &str) -> &str {
    let lower = path.to_lowercase();
    if lower.ends_with(".markdown") {
        &path[..path.len() - 9]
    } else if lower.ends_with(".md") {
        &path[..path.len() - 3]
    } else {
        path
    }
}

fn key(path: &str) -> String {
    without_ext(path.trim().trim_start_matches("./").trim_start_matches('/')).to_lowercase()
}

fn folder(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

fn stem(path: &str) -> &str {
    let name = path.rfind('/').map_or(path, |i| &path[i + 1..]);
    without_ext(name)
}

/// True when a target names a note rather than another file (`photo.png`, `paper.pdf`).
pub fn is_note_target(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    match name.rsplit_once('.') {
        None => true,
        Some((_, ext)) => {
            let ext = ext.to_lowercase();
            ext == "md"
                || ext == "markdown"
                || ext.is_empty()
                || ext.len() > 5
                || !ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
    }
}

pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
            && let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("0"), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_owned())
}

fn percent_encode(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '#' => out.push_str("%23"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '%' => out.push_str("%25"),
            _ => out.push(c),
        }
    }
    out
}

/// The relative, encoded path a Markdown link from `source` uses for `target`.
pub fn md_target(source: &str, target: &str) -> String {
    let from: Vec<&str> = folder(source)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to: Vec<&str> = target.split('/').collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(a, b)| a == b)
        .count()
        .min(to.len().saturating_sub(1));
    let mut parts: Vec<&str> = std::iter::repeat_n("..", from.len() - common).collect();
    parts.extend(&to[common..]);
    percent_encode(&parts.join("/"))
}

impl Vault {
    pub fn new<S: Into<String>>(paths: impl IntoIterator<Item = S>) -> Self {
        let paths: Vec<String> = paths.into_iter().map(Into::into).collect();
        let mut by_path = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, path) in paths.iter().enumerate() {
            by_path.entry(key(path)).or_insert(index);
            by_stem
                .entry(stem(path).to_lowercase())
                .or_default()
                .push(index);
        }
        Self {
            paths,
            by_path,
            by_stem,
        }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.by_path.get(&key(path)).copied()
    }

    /// The note a link from `source` names; an empty wikilink target (`[[#Heading]]`) is the source.
    pub fn resolve(&self, source: &str, link: &Link) -> Option<usize> {
        if link.wiki {
            self.resolve_wiki(source, &link.target)
        } else {
            self.resolve_path(source, &link.target)
        }
    }

    pub fn resolve_wiki(&self, source: &str, target: &str) -> Option<usize> {
        let target = target.trim();
        if target.is_empty() {
            return self.find(source);
        }
        if !is_note_target(target) {
            return None;
        }
        self.resolve_name(source, target)
    }

    /// A file named by `[[name]]` or `[[folder/name]]`, whatever its kind (attachments).
    pub fn resolve_name(&self, source: &str, target: &str) -> Option<usize> {
        let wanted = key(target);
        let candidates: Vec<usize> = if wanted.contains('/') {
            if let Some(index) = self.by_path.get(&wanted) {
                return Some(*index);
            }
            let suffix = format!("/{wanted}");
            (0..self.paths.len())
                .filter(|i| key(&self.paths[*i]).ends_with(&suffix))
                .collect()
        } else {
            self.by_stem.get(&wanted).cloned().unwrap_or_default()
        };
        let here = folder(source).to_lowercase();
        candidates.into_iter().min_by(|a, b| {
            let (pa, pb) = (&self.paths[*a], &self.paths[*b]);
            (folder(pa).to_lowercase() != here)
                .cmp(&(folder(pb).to_lowercase() != here))
                .then(pa.len().cmp(&pb.len()))
                .then(pa.cmp(pb))
        })
    }

    /// A Markdown link's path: relative to the source's folder, else from the Notes folder.
    pub fn resolve_path(&self, source: &str, target: &str) -> Option<usize> {
        let mut parts: Vec<&str> = folder(source)
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if target.starts_with('/') {
            parts.clear();
        }
        for part in target.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                other => parts.push(other),
            }
        }
        self.find(&parts.join("/")).or_else(|| self.find(target))
    }

    /// The shortest wikilink target naming `index`: its name, or its path when the name is shared.
    pub fn wiki_target(&self, index: usize) -> String {
        let path = &self.paths[index];
        let name = stem(path);
        if self
            .by_stem
            .get(&name.to_lowercase())
            .is_some_and(|all| all.len() == 1)
        {
            name.to_owned()
        } else {
            without_ext(path).to_owned()
        }
    }

    /// The target a link should use after `index` moved: same style as `link` (name, path or Markdown path).
    pub fn retarget(&self, source: &str, link: &Link, index: usize) -> String {
        if !link.wiki {
            return md_target(source, &self.paths[index]);
        }
        let path = &self.paths[index];
        let written = link.target.trim();
        let keep_ext = written.to_lowercase().ends_with(".md");
        let base = if written.contains('/') {
            without_ext(path).to_owned()
        } else {
            self.wiki_target(index)
        };
        if keep_ext && !base.to_lowercase().ends_with(".md") {
            format!("{base}.md")
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{facts, rewrite};

    fn vault() -> Vault {
        Vault::new([
            "Plan.md",
            "Work/Plan.md",
            "Work/Deep/Notes.md",
            "Ideas.md",
            "Trips/Paris 2026.md",
        ])
    }

    #[test]
    fn resolves_like_obsidian() {
        let v = vault();
        assert_eq!(v.resolve_wiki("Ideas.md", "Plan"), Some(0));
        assert_eq!(
            v.resolve_wiki("Work/Deep/Notes.md", "plan"),
            Some(0),
            "shortest path wins outside the folder"
        );
        assert_eq!(
            v.resolve_wiki("Work/Other.md", "Plan"),
            Some(1),
            "the same folder wins"
        );
        assert_eq!(v.resolve_wiki("Ideas.md", "Work/Plan"), Some(1));
        assert_eq!(v.resolve_wiki("Ideas.md", "Deep/Notes"), Some(2));
        assert_eq!(v.resolve_wiki("Ideas.md", ""), Some(3));
        assert_eq!(v.resolve_wiki("Ideas.md", "photo.png"), None);
        let files = Vault::new(["attachments/photo.png", "Work/photo.png"]);
        assert_eq!(files.resolve_name("Work/Plan.md", "photo.png"), Some(1));
        assert_eq!(
            files.resolve_name("attachments/Note.md", "photo.png"),
            Some(0)
        );
        assert_eq!(
            files.resolve_path("Ideas.md", "attachments/photo.png"),
            Some(0)
        );
        assert_eq!(v.resolve_wiki("Ideas.md", "Missing"), None);
        assert_eq!(v.resolve_path("Work/Deep/Notes.md", "../Plan.md"), Some(1));
        assert_eq!(v.resolve_path("Ideas.md", "Trips/Paris 2026.md"), Some(4));
        assert_eq!(
            md_target("Work/Deep/Notes.md", "Trips/Paris 2026.md"),
            "../../Trips/Paris%202026.md"
        );
        assert_eq!(md_target("Work/A.md", "Work/B.md"), "B.md");
        assert_eq!(v.wiki_target(0), "Plan");
        assert_eq!(v.wiki_target(1), "Work/Plan");
        assert_eq!(v.wiki_target(3), "Ideas");
        assert!(is_note_target("Version 1.2 plan"));
        assert!(!is_note_target("paper.pdf"));
    }

    #[test]
    fn renames_keep_link_style() {
        let old = vault();
        let new = Vault::new([
            "Plan.md",
            "Work/Plan.md",
            "Work/Deep/Notes.md",
            "Thoughts.md",
            "Trips/Paris 2026.md",
        ]);
        let source = "Work/Deep/Notes.md";
        let text = "[[Ideas]] [[ideas#Top|x]] [[Ideas.md]] [i](../../Ideas.md) [[Plan]]";
        let next = rewrite(text, |link| {
            let hit = old.resolve(source, link)?;
            (hit == 3).then(|| new.retarget(source, link, hit))
        })
        .unwrap();
        assert_eq!(
            next,
            "[[Thoughts]] [[Thoughts#Top|x]] [[Thoughts.md]] [i](../../Thoughts.md) [[Plan]]"
        );
        assert_eq!(facts(&next).links.len(), 5);
    }
}
