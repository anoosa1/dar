//! Line diffs shared by Folder history and Git (`diff.tsx`). Display only: nothing here writes content.

use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Removed,
    Added,
}

impl DiffKind {
    pub fn prefix(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Removed => '-',
            Self::Added => '+',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Minimal line diff. Large middles fall back to replacement to bound browser work.
pub fn line_diff(old: &[&str], next: &[&str]) -> Vec<DiffLine> {
    let mut start = 0usize;
    let mut end_old = old.len();
    let mut end_new = next.len();
    while start < end_old && start < end_new && old[start] == next[start] {
        start += 1;
    }
    while end_old > start && end_new > start && old[end_old - 1] == next[end_new - 1] {
        end_old -= 1;
        end_new -= 1;
    }
    let a = &old[start..end_old];
    let b = &next[start..end_new];
    let mut middle = Vec::new();
    if a.len().saturating_mul(b.len()) > 2_000_000 {
        for &text in a {
            middle.push(DiffLine {
                kind: DiffKind::Removed,
                text: text.to_owned(),
            });
        }
        for &text in b {
            middle.push(DiffLine {
                kind: DiffKind::Added,
                text: text.to_owned(),
            });
        }
    } else {
        let width = b.len() + 1;
        let mut table = vec![0u32; (a.len() + 1) * width];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                table[i * width + j] = if a[i] == b[j] {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                middle.push(DiffLine {
                    kind: DiffKind::Context,
                    text: a[i].to_owned(),
                });
                i += 1;
                j += 1;
            } else if i < a.len()
                && (j == b.len() || table[(i + 1) * width + j] >= table[i * width + j + 1])
            {
                middle.push(DiffLine {
                    kind: DiffKind::Removed,
                    text: a[i].to_owned(),
                });
                i += 1;
            } else {
                middle.push(DiffLine {
                    kind: DiffKind::Added,
                    text: b[j].to_owned(),
                });
                j += 1;
            }
        }
    }
    let mut out = Vec::with_capacity(start + middle.len() + (old.len() - end_old));
    for &text in &old[..start] {
        out.push(DiffLine {
            kind: DiffKind::Context,
            text: text.to_owned(),
        });
    }
    out.extend(middle);
    for &text in &old[end_old..] {
        out.push(DiffLine {
            kind: DiffKind::Context,
            text: text.to_owned(),
        });
    }
    out
}

/// Splits text into lines; the final newline ends the last line rather than starting another.
pub fn text_lines(value: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = value.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Unified hunks with three context lines after the given header lines.
pub fn unified_hunks(old: &[&str], next: &[&str], head: &[&str]) -> String {
    let lines = line_diff(old, next);
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != DiffKind::Context).then_some(index))
        .collect();
    if changed.is_empty() {
        return head.join("\n");
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let from = index.saturating_sub(3);
        let to = (index + 4).min(lines.len());
        if let Some(last) = ranges.last_mut()
            && from <= last.1
        {
            last.1 = to;
        } else {
            ranges.push((from, to));
        }
    }
    let mut output: Vec<String> = head.iter().map(|s| (*s).to_owned()).collect();
    let (mut old_line, mut new_line, mut cursor) = (1usize, 1usize, 0usize);
    for (from, to) in ranges {
        while cursor < from {
            if lines[cursor].kind != DiffKind::Added {
                old_line += 1;
            }
            if lines[cursor].kind != DiffKind::Removed {
                new_line += 1;
            }
            cursor += 1;
        }
        let slice = &lines[from..to];
        let old_count = slice
            .iter()
            .filter(|line| line.kind != DiffKind::Added)
            .count();
        let new_count = slice
            .iter()
            .filter(|line| line.kind != DiffKind::Removed)
            .count();
        let old_start = if old_count > 0 {
            old_line
        } else {
            old_line.saturating_sub(1)
        };
        let new_start = if new_count > 0 {
            new_line
        } else {
            new_line.saturating_sub(1)
        };
        output.push(format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
        ));
        for line in slice {
            output.push(format!("{}{}", line.kind.prefix(), line.text));
        }
    }
    output.join("\n")
}

pub fn count_lines(old: &[&str], next: &[&str]) -> (usize, usize) {
    let lines = line_diff(old, next);
    let added = lines.iter().filter(|l| l.kind == DiffKind::Added).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Removed).count();
    (added, removed)
}

#[component]
pub fn DiffStat(added: usize, removed: usize) -> impl IntoView {
    view! {
        <span class="diff-stat">
            <span class="diff-added">{format!("+{added}")}</span>
            <span class="diff-removed">{format!("\u{2212}{removed}")}</span>
        </span>
    }
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let old_digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if old_digits == 0 {
        return None;
    }
    let old_line: usize = rest[..old_digits].parse().ok()?;
    let after_old = &rest[old_digits..];
    let after_count = if let Some(comma) = after_old.strip_prefix(',') {
        let count_digits = comma.bytes().take_while(u8::is_ascii_digit).count();
        if count_digits == 0 {
            return None;
        }
        &comma[count_digits..]
    } else {
        after_old
    };
    let plus = after_count.strip_prefix(" +")?;
    let new_digits = plus.bytes().take_while(u8::is_ascii_digit).count();
    if new_digits == 0 {
        return None;
    }
    let new_line: usize = plus[..new_digits].parse().ok()?;
    Some((old_line, new_line))
}

/// Display layout for unified or side-by-side split diffs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DiffMode {
    #[default]
    Unified,
    Split,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitCell {
    pub line_num: String,
    pub kind: &'static str,
    pub sign: &'static str,
    pub text: String,
}

impl SplitCell {
    fn empty() -> Self {
        Self {
            line_num: String::new(),
            kind: "empty",
            sign: "",
            text: " ".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitRow {
    Hunk(String),
    Pair { left: SplitCell, right: SplitCell },
}

fn flush_split_runs(
    out: &mut Vec<SplitRow>,
    removed: &mut Vec<(String, String)>,
    added: &mut Vec<(String, String)>,
) {
    let len = removed.len().max(added.len());
    for i in 0..len {
        let left = match removed.get(i) {
            Some((num, text)) => SplitCell {
                line_num: num.clone(),
                kind: "removed",
                sign: "-",
                text: text.clone(),
            },
            None => SplitCell::empty(),
        };
        let right = match added.get(i) {
            Some((num, text)) => SplitCell {
                line_num: num.clone(),
                kind: "added",
                sign: "+",
                text: text.clone(),
            },
            None => SplitCell::empty(),
        };
        out.push(SplitRow::Pair { left, right });
    }
    removed.clear();
    added.clear();
}

/// Parses a unified diff patch into aligned side-by-side split rows.
pub fn parse_split_rows(source: &str) -> Vec<SplitRow> {
    let mut out = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut in_hunk = false;
    let mut removed: Vec<(String, String)> = Vec::new();
    let mut added: Vec<(String, String)> = Vec::new();

    for line in source
        .split('\n')
        .filter(|l| !l.starts_with("--- ") && !l.starts_with("+++ "))
    {
        if line.starts_with("@@") {
            flush_split_runs(&mut out, &mut removed, &mut added);
            let (o, n) = parse_hunk_header(line).unwrap_or((1, 1));
            old_line = o;
            new_line = n;
            in_hunk = true;
            let display = if line.is_empty() { " " } else { line };
            out.push(SplitRow::Hunk(display.to_owned()));
            continue;
        }
        if !in_hunk {
            if !line.is_empty() {
                out.push(SplitRow::Hunk(line.to_owned()));
            }
            continue;
        }
        let body = line
            .chars()
            .next()
            .map(|c| &line[c.len_utf8()..])
            .unwrap_or("");
        let display = if body.is_empty() {
            " ".to_owned()
        } else {
            body.to_owned()
        };
        if line.starts_with('-') {
            removed.push((old_line.to_string(), display));
            old_line += 1;
        } else if line.starts_with('+') {
            added.push((new_line.to_string(), display));
            new_line += 1;
        } else {
            flush_split_runs(&mut out, &mut removed, &mut added);
            let (o_num, n_num) = if line.starts_with(' ') {
                let pair = (old_line.to_string(), new_line.to_string());
                old_line += 1;
                new_line += 1;
                pair
            } else {
                (String::new(), String::new())
            };
            out.push(SplitRow::Pair {
                left: SplitCell {
                    line_num: o_num,
                    kind: "",
                    sign: " ",
                    text: display.clone(),
                },
                right: SplitCell {
                    line_num: n_num,
                    kind: "",
                    sign: " ",
                    text: display,
                },
            });
        }
    }
    flush_split_runs(&mut out, &mut removed, &mut added);
    out
}

fn parse_unified_rows(source: &str) -> Vec<(String, String, String, String)> {
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut in_hunk = false;
    source
        .split('\n')
        .filter(|line| !line.starts_with("--- ") && !line.starts_with("+++ "))
        .map(|line| {
            let mut old_number = String::new();
            let mut new_number = String::new();
            let mut kind = "";
            if line.starts_with("@@") {
                let (o, n) = parse_hunk_header(line).unwrap_or((1, 1));
                old_line = o;
                new_line = n;
                in_hunk = true;
                kind = "hunk";
            } else if in_hunk {
                if line.starts_with('-') {
                    kind = "removed";
                    old_number = old_line.to_string();
                    old_line += 1;
                } else if line.starts_with('+') {
                    kind = "added";
                    new_number = new_line.to_string();
                    new_line += 1;
                } else if line.starts_with(' ') {
                    old_number = old_line.to_string();
                    new_number = new_line.to_string();
                    old_line += 1;
                    new_line += 1;
                }
            }
            let body = if kind == "hunk" || !in_hunk {
                line
            } else {
                line.chars()
                    .next()
                    .map(|c| &line[c.len_utf8()..])
                    .unwrap_or("")
            };
            let display = if body.is_empty() {
                " ".to_owned()
            } else {
                body.to_owned()
            };
            (
                format!("code-diff-line {kind}"),
                old_number,
                new_number,
                display,
            )
        })
        .collect()
}

/// Renders a unified or side-by-side split patch, preserving hunk boundaries.
#[component]
pub fn Diff(
    #[prop(into)] source: String,
    #[prop(into)] wrap: Signal<bool>,
    #[prop(optional, into)] label: Option<String>,
    #[prop(optional, into)] mode: Option<Signal<DiffMode>>,
) -> impl IntoView {
    let label = label.unwrap_or_else(|| "File diff".to_owned());
    let source = StoredValue::new(source);
    let active_mode = move || mode.map(|m| m.get()).unwrap_or_default();

    view! {
        <div
            class=move || if wrap.get() { "code-diff code-diff-wrap" } else { "code-diff" }
            role="region"
            aria-label=label
            tabindex="0"
        >
            {move || match active_mode() {
                DiffMode::Unified => {
                    let rows = source.with_value(|s| parse_unified_rows(s));
                    rows.into_iter().map(|(class, old_num, new_num, text)| {
                        view! {
                            <div class=class>
                                <span class="code-diff-number" aria-hidden="true">{old_num}</span>
                                <span class="code-diff-number" aria-hidden="true">{new_num}</span>
                                <code class="code-diff-text">{text}</code>
                            </div>
                        }
                    }).collect_view().into_any()
                }
                DiffMode::Split => {
                    let rows = source.with_value(|s| parse_split_rows(s));
                    view! {
                        <div class="code-diff-split">
                            {rows.into_iter().map(|row| match row {
                                SplitRow::Hunk(header) => view! {
                                    <div class="code-diff-split-hunk">{header}</div>
                                }.into_any(),
                                SplitRow::Pair { left, right } => view! {
                                    <div class=format!("code-diff-split-cell {}", left.kind)>
                                        <span class="code-diff-number" aria-hidden="true">{left.line_num}</span>
                                        <span class="code-diff-sign" aria-hidden="true">{left.sign}</span>
                                        <code class="code-diff-text">{left.text}</code>
                                    </div>
                                    <div class=format!("code-diff-split-cell {}", right.kind)>
                                        <span class="code-diff-number" aria-hidden="true">{right.line_num}</span>
                                        <span class="code-diff-sign" aria-hidden="true">{right.sign}</span>
                                        <code class="code-diff-text">{right.text}</code>
                                    </div>
                                }.into_any(),
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_hunks_matches_js_client() {
        let a = text_lines("# Project\n\nFirst line\n");
        let b = text_lines("# Project\n\nSecond line\n");
        let patch = unified_hunks(&a, &b, &[]);
        assert_eq!(
            patch,
            "@@ -1,3 +1,3 @@\n # Project\n \n-First line\n+Second line"
        );
        assert_eq!(count_lines(&a, &b), (1, 1));
    }

    #[test]
    fn parse_hunk_headers() {
        assert_eq!(
            parse_hunk_header("@@ -10,4 +12,6 @@ fn main()"),
            Some((10, 12))
        );
        assert_eq!(parse_hunk_header("@@ -0,0 +1,3 @@"), Some((0, 1)));
        assert_eq!(parse_hunk_header("@@ -5 +7 @@"), Some((5, 7)));
    }

    #[test]
    fn parse_split_rows_aligns_changes() {
        let patch = "@@ -1,3 +1,4 @@\n ctx\n-old\n+new1\n+new2\n end";
        let rows = parse_split_rows(patch);
        assert_eq!(rows.len(), 5);
        assert!(matches!(&rows[0], SplitRow::Hunk(h) if h == "@@ -1,3 +1,4 @@"));
        assert!(matches!(
            &rows[1],
            SplitRow::Pair { left, right }
                if left.line_num == "1" && right.line_num == "1" && left.text == "ctx"
        ));
        assert!(matches!(
            &rows[2],
            SplitRow::Pair { left, right }
                if left.kind == "removed" && left.line_num == "2" && left.text == "old"
                && right.kind == "added" && right.line_num == "2" && right.text == "new1"
        ));
        assert!(matches!(
            &rows[3],
            SplitRow::Pair { left, right }
                if left.kind == "empty" && right.kind == "added" && right.line_num == "3" && right.text == "new2"
        ));
        assert!(matches!(
            &rows[4],
            SplitRow::Pair { left, right }
                if left.line_num == "3" && right.line_num == "4" && left.text == "end"
        ));
    }
}
