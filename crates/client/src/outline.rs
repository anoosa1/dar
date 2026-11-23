//! Outliner editing in the Markdown source, as Logseq and Obsidian do it: indent and outdent
//! list items, move lines up and down, and continue a list on Enter. Offsets are bytes; the
//! caller converts the textarea's UTF-16 selection.

use std::ops::Range;

/// The whole lines a selection touches.
fn line_span(text: &str, start: usize, end: usize) -> Range<usize> {
    let first = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let end = if end > start && text.as_bytes()[end - 1] == b'\n' { end - 1 } else { end };
    let last = text[end..].find('\n').map_or(text.len(), |i| end + i);
    first..last
}

/// A list item's marker (`- `, `* `, `+ `, `1. `, `1) `, with `[ ] ` after it), after indentation.
pub fn list_marker(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let mut marker = if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
        2
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 || !(rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
            return None;
        }
        digits + 2
    };
    for check in ["[ ] ", "[x] ", "[X] "] {
        if rest[marker..].starts_with(check) {
            marker += 4;
        }
    }
    Some((indent, &rest[..marker]))
}

/// Tab and Shift+Tab on list items: every selected line gains or loses one level.
/// `None` when the caret is not in a list (Tab then moves focus as usual).
pub fn indent(text: &str, start: usize, end: usize, outdent: bool) -> Option<(String, usize, usize)> {
    let span = line_span(text, start, end);
    let lines: Vec<&str> = text[span.clone()].split('\n').collect();
    if !lines.iter().any(|l| list_marker(l).is_some()) {
        return None;
    }
    let mut out = Vec::with_capacity(lines.len());
    let (mut shift_start, mut shift_total) = (0isize, 0isize);
    for (i, line) in lines.iter().enumerate() {
        let next = if outdent {
            if let Some(rest) = line.strip_prefix('\t') {
                rest.to_owned()
            } else {
                let spaces = line.len() - line.trim_start_matches(' ').len();
                line[spaces.min(4).min(if spaces >= 2 { spaces } else { 0 })..].to_owned()
            }
        } else if line.is_empty() {
            String::new()
        } else {
            format!("\t{line}")
        };
        let delta = next.len() as isize - line.len() as isize;
        if i == 0 {
            shift_start = delta;
        }
        shift_total += delta;
        out.push(next);
    }
    let replaced = out.join("\n");
    let result = format!("{}{}{}", &text[..span.start], replaced, &text[span.end..]);
    let new_start = (start as isize + shift_start).max(span.start as isize) as usize;
    let new_end = (end as isize + shift_total).max(new_start as isize) as usize;
    Some((result, new_start, new_end))
}

/// Alt+Shift+Up/Down: the selected lines swap with the line above or below.
pub fn move_lines(text: &str, start: usize, end: usize, up: bool) -> Option<(String, usize, usize)> {
    let span = line_span(text, start, end);
    let block = &text[span.clone()];
    if up {
        if span.start == 0 {
            return None;
        }
        let above_start = text[..span.start - 1].rfind('\n').map_or(0, |i| i + 1);
        let above = &text[above_start..span.start - 1];
        let result = format!("{}{block}\n{above}{}", &text[..above_start], &text[span.end..]);
        let moved = span.start - above_start;
        Some((result, start - moved, end - moved))
    } else {
        if span.end >= text.len() {
            return None;
        }
        let below_end = text[span.end + 1..].find('\n').map_or(text.len(), |i| span.end + 1 + i);
        let below = &text[span.end + 1..below_end];
        let result = format!("{}{below}\n{block}{}", &text[..span.start], &text[below_end..]);
        let moved = below.len() + 1;
        Some((result, start + moved, end + moved))
    }
}

/// Enter in a list item: the next line starts with the same marker (numbers count up,
/// checkboxes start open); Enter on an empty item ends the list.
pub fn continue_list(text: &str, caret: usize) -> Option<(String, usize)> {
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[caret..].find('\n').map_or(text.len(), |i| caret + i);
    let line = &text[line_start..line_end];
    let (indent, marker) = list_marker(line)?;
    if caret < line_start + indent + marker.len() {
        return None;
    }
    if line[indent + marker.len()..].trim().is_empty() {
        let result = format!("{}{}", &text[..line_start], &text[line_end..]);
        return Some((result, line_start));
    }
    let bare = marker.trim_end_matches("[ ] ").trim_end_matches("[x] ").trim_end_matches("[X] ");
    let mut next = match bare.trim_end().strip_suffix(['.', ')']) {
        Some(number) if number.bytes().all(|b| b.is_ascii_digit()) => {
            let close = &bare[number.len()..number.len() + 1];
            format!("{}{close} ", number.parse::<u64>().unwrap_or(0) + 1)
        }
        _ => bare.to_owned(),
    };
    if marker.len() > bare.len() {
        next.push_str("[ ] ");
    }
    let insert = format!("\n{}{next}", &line[..indent]);
    let result = format!("{}{insert}{}", &text[..caret], &text[caret..]);
    Some((result, caret + insert.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_list_items_only() {
        assert_eq!(indent("- a\n- b", 4, 4, false), Some(("- a\n\t- b".into(), 5, 5)));
        assert_eq!(indent("- a\n\t- b", 6, 6, true), Some(("- a\n- b".into(), 5, 5)));
        assert_eq!(indent("- a\n  - b", 7, 7, true), Some(("- a\n- b".into(), 5, 5)));
        assert_eq!(indent("plain", 2, 2, false), None);
        assert_eq!(indent("- a\n- b\n", 0, 7, false).map(|r| r.0), Some("\t- a\n\t- b\n".into()));
    }

    #[test]
    fn moves_lines() {
        assert_eq!(move_lines("one\ntwo\nthree", 5, 5, true), Some(("two\none\nthree".into(), 1, 1)));
        assert_eq!(move_lines("one\ntwo\nthree", 1, 1, false), Some(("two\none\nthree".into(), 5, 5)));
        assert_eq!(move_lines("one\ntwo", 1, 1, true), None);
        assert_eq!(move_lines("one\ntwo", 5, 5, false), None);
    }

    #[test]
    fn continues_lists() {
        assert_eq!(continue_list("- a", 3), Some(("- a\n- ".into(), 6)));
        assert_eq!(continue_list("\t- [x] done", 11), Some(("\t- [x] done\n\t- [ ] ".into(), 19)));
        assert_eq!(continue_list("9. nine", 7), Some(("9. nine\n10. ".into(), 12)));
        assert_eq!(continue_list("x\n- ", 4), Some(("x\n".into(), 2)));
        assert_eq!(continue_list("text", 4), None);
    }
}
