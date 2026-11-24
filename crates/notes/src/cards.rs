//! Flashcards with spaced repetition, compatible with Obsidian's Spaced Repetition plugin.
//!
//! Supported card forms (outside YAML properties and fenced code blocks):
//! - Single-line: `Question::Answer` (one card) and `Question:::Answer` (reversible: two cards).
//! - Multi-line: question lines, a line `?` (or `??` for reversible), then answer lines until a
//!   blank line or heading.
//! - Logseq-style list item tagged `#card` whose nested lines are the answer.
//!
//! Schedules are stored right after the card as `<!--SR:!YYYY-MM-DD,interval,ease-->` (reversible
//! cards store two `!date,interval,ease` groups in one comment). Cards without a schedule comment
//! (or a reversible sibling with `interval == 0`) are new.

use std::fmt;
use std::ops::Range;

/// Starting ease factor (2.50, stored as integer percent as in Obsidian Spaced Repetition).
pub const INITIAL_EASE: u32 = 250;
/// Minimum ease factor (1.30).
pub const MIN_EASE: u32 = 130;
/// Maximum scheduling interval in days (100 years).
pub const MAX_INTERVAL: u32 = 36_525;

const HARD_FACTOR: f64 = 0.5;
const EASY_BONUS: f64 = 1.3;
const PLACEHOLDER_GROUP: &str = "!2000-01-01,0,250";

/// Proleptic Gregorian calendar date (`YYYY-MM-DD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Date {
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > 31 {
            return None;
        }
        let days = days_from_civil(year, month, day);
        let (y, m, d) = civil_from_days(days);
        (y == year && m == month && d == day).then_some(Self { year, month, day })
    }

    pub fn parse(value: &str) -> Option<Self> {
        let b = value.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        if !b[0..4]
            .iter()
            .chain(&b[5..7])
            .chain(&b[8..10])
            .all(u8::is_ascii_digit)
        {
            return None;
        }
        let year = value[0..4].parse::<i32>().ok()?;
        let month = value[5..7].parse::<u8>().ok()?;
        let day = value[8..10].parse::<u8>().ok()?;
        Self::new(year, month, day)
    }

    pub fn add_days(self, days: i32) -> Self {
        let total = days_from_civil(self.year, self.month, self.day) + i64::from(days);
        let (year, month, day) = civil_from_days(total);
        Self { year, month, day }
    }

    pub fn days_since(self, earlier: Self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
            - days_from_civil(earlier.year, earlier.month, earlier.day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Howard Hinnant's civil-to-days conversion (days since 1970-01-01).
fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = i64::from(y) - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's days-to-civil conversion.
fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y as i32, m as u8, d as u8)
}

/// Review response grade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Again,
    Hard,
    Good,
    Easy,
}

/// Persisted scheduling state for one card (`!YYYY-MM-DD,interval,ease`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub due: Date,
    pub interval: u32,
    pub ease: u32,
}

impl Schedule {
    pub fn is_due(self, today: Date) -> bool {
        self.due <= today
    }
}

/// Computes the next schedule using Obsidian Spaced Repetition's SM-2 variant (`osrCore`).
///
/// Formula (starting at `ease = 250`, minimum `ease = 130`, `lapsesIntervalChange = 0.5`,
/// `easyBonus = 1.3`, maximum interval `36525` days):
/// - A new card (`state == None`) starts with `interval = 1`, `ease = 250`, `delay = 0`.
/// - A reviewed card uses `delay = max(0, today - state.due)` (days overdue).
/// - `Again` (Reset): `ease = max(130, ease - 20)`, `interval = 1`.
/// - `Hard`: `ease = max(130, ease - 20)`, `interval = round((interval + delay / 4) * 0.5)`.
/// - `Good`: `ease` unchanged, `interval = round((interval + delay / 2) * (ease / 100))`.
/// - `Easy`: `ease = ease + 20`, `interval = round((interval + delay) * (ease / 100) * 1.3)`.
///
/// The resulting interval is clamped to `1..=36525` days and `due = today + interval`.
/// Random fuzzing is omitted so interval previews and persisted schedules stay deterministic.
pub fn schedule(state: Option<Schedule>, grade: Grade, today: Date) -> Schedule {
    let (prev_interval, prev_ease, delay) = match state {
        Some(prev) => (
            prev.interval.max(1) as f64,
            prev.ease.max(MIN_EASE),
            today.days_since(prev.due).max(0) as f64,
        ),
        None => (1.0, INITIAL_EASE, 0.0),
    };

    let (next_interval, next_ease) = match grade {
        Grade::Again => (1.0, prev_ease.saturating_sub(20).max(MIN_EASE)),
        Grade::Hard => {
            let ease = prev_ease.saturating_sub(20).max(MIN_EASE);
            let interval = ((prev_interval + delay / 4.0) * HARD_FACTOR).round();
            (interval, ease)
        }
        Grade::Good => {
            let interval = ((prev_interval + delay / 2.0) * (f64::from(prev_ease) / 100.0)).round();
            (interval, prev_ease)
        }
        Grade::Easy => {
            let ease = prev_ease.saturating_add(20);
            let interval =
                ((prev_interval + delay) * (f64::from(ease) / 100.0) * EASY_BONUS).round();
            (interval, ease)
        }
    };

    let interval = next_interval.clamp(1.0, MAX_INTERVAL as f64) as u32;
    Schedule {
        due: today.add_days(interval as i32),
        interval,
        ease: next_ease,
    }
}

/// Compact human-readable interval label for review buttons (`1d`, `3d`, `2mo`, `1.2y`).
pub fn format_interval(days: u32) -> String {
    if days < 30 {
        format!("{days}d")
    } else if days < 365 {
        let tenths = ((f64::from(days) / 30.0) * 10.0).round() as u32;
        if tenths % 10 == 0 {
            format!("{}mo", tenths / 10)
        } else {
            format!("{}.{}mo", tenths / 10, tenths % 10)
        }
    } else {
        let tenths = ((f64::from(days) / 365.0) * 10.0).round() as u32;
        if tenths % 10 == 0 {
            format!("{}y", tenths / 10)
        } else {
            format!("{}.{}y", tenths / 10, tenths % 10)
        }
    }
}

/// Syntax form a flashcard was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    SingleLine,
    SingleLineReversible,
    MultiLine,
    MultiLineReversible,
    ListCard,
}

/// A flashcard extracted from a Markdown note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Zero-based index of this card within the note.
    pub index: usize,
    pub kind: CardKind,
    /// Full byte range of the card in the source Markdown (including its schedule comment if any).
    pub range: Range<usize>,
    /// Byte range of an existing `<!--SR:...-->` comment, when present.
    pub comment_range: Option<Range<usize>>,
    /// Byte offset where a new `<!--SR:...-->` comment line is inserted when `comment_range` is `None`.
    pub insert_at: usize,
    /// Leading indentation for a newly inserted comment line (e.g. for indented `#card` items).
    pub insert_indent: String,
    /// Schedule group index within the comment (`0` normally; `0` or `1` for reversible cards).
    pub slot: u8,
    /// Total schedule groups in the comment (`1` normally; `2` for reversible cards).
    pub slots: u8,
    /// Zero-based starting line in the note.
    pub line: usize,
    pub question: String,
    pub answer: String,
    pub schedule: Option<Schedule>,
}

/// Resolves the deck name for a note: `#flashcards/<subdeck>` tag first, else folder path, else `"Notes"`.
pub fn deck(path: &str, tags: &[String]) -> String {
    const PREFIX: &[u8] = b"flashcards/";
    for tag in tags {
        let trimmed = tag.trim().trim_start_matches('#');
        if trimmed.len() > PREFIX.len()
            && trimmed.as_bytes()[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
        {
            let sub = trimmed[PREFIX.len()..].trim_matches('/');
            if !sub.is_empty() {
                return sub.to_owned();
            }
        }
    }
    if let Some(slash) = path.rfind('/') {
        let folder = path[..slash].trim_matches('/');
        if !folder.is_empty() {
            return folder.to_owned();
        }
    }
    "Notes".to_owned()
}

struct SourceLine<'a> {
    number: usize,
    start: usize,
    content_end: usize,
    raw: &'a str,
}

fn collect_lines(text: &str, body: usize) -> Vec<SourceLine<'_>> {
    let base_line = text[..body].bytes().filter(|&b| b == b'\n').count();
    let mut lines = Vec::new();
    let mut offset = body;
    for (idx, chunk) in text[body..].split_inclusive('\n').enumerate() {
        let end = offset + chunk.len();
        let raw = chunk.strip_suffix('\n').unwrap_or(chunk);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let content_end = offset + raw.len();
        lines.push(SourceLine {
            number: base_line + idx,
            start: offset,
            content_end,
            raw,
        });
        offset = end;
    }
    lines
}

fn parse_fence_open(raw: &str) -> Option<(u8, usize)> {
    let trimmed = raw.trim_start_matches(' ');
    if raw.len() - trimmed.len() > 3 {
        return None;
    }
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count < 3 {
        return None;
    }
    if first == b'`' && trimmed[count..].contains('`') {
        return None;
    }
    Some((first, count))
}

fn is_fence_close(raw: &str, fence: (u8, usize)) -> bool {
    let trimmed = raw.trim_start_matches(' ');
    if raw.len() - trimmed.len() > 3 {
        return false;
    }
    let count = trimmed.bytes().take_while(|&b| b == fence.0).count();
    count >= fence.1 && trimmed[count..].trim().is_empty()
}

fn is_heading(raw: &str) -> bool {
    let trimmed = raw.trim_start_matches(' ');
    if raw.len() - trimmed.len() > 3 {
        return false;
    }
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && trimmed[hashes..].starts_with([' ', '\t'])
}

fn is_hr(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.len() < 3 {
        return false;
    }
    for ch in ['-', '*', '_'] {
        if trimmed.chars().all(|c| c == ch || c == ' ')
            && trimmed.chars().filter(|&c| c == ch).count() >= 3
        {
            return true;
        }
    }
    false
}

/// Parses a `<!--SR:!YYYY-MM-DD,interval,ease...-->` comment inside `slice`.
fn parse_sr_groups(inner: &str) -> Option<Vec<Option<Schedule>>> {
    let trimmed = inner.trim();
    let rest = trimmed.strip_prefix('!')?;
    let mut out = Vec::new();
    for part in rest.split('!') {
        let mut fields = part.split(',');
        let date_str = fields.next()?;
        let interval_str = fields.next()?;
        let ease_str = fields.next()?;
        if fields.next().is_some() {
            return None;
        }
        let due = Date::parse(date_str)?;
        let interval = interval_str.parse::<u32>().ok()?;
        let ease = ease_str.parse::<u32>().ok()?;
        if interval == 0 {
            out.push(None);
        } else {
            out.push(Some(Schedule {
                due,
                interval,
                ease,
            }));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Matches a line whose only non-whitespace content is a `<!--SR:...-->` comment.
fn parse_standalone_sr(line: &SourceLine<'_>) -> Option<(Range<usize>, Vec<Option<Schedule>>)> {
    let leading = line.raw.len() - line.raw.trim_start().len();
    let trimmed = line.raw.trim();
    let inner = trimmed.strip_prefix("<!--SR:")?.strip_suffix("-->")?;
    let groups = parse_sr_groups(inner)?;
    let start = line.start + leading;
    let end = start + trimmed.len();
    Some((start..end, groups))
}

/// Matches a trailing inline `<!--SR:...-->` comment at the end of a line.
fn parse_trailing_sr(
    line: &SourceLine<'_>,
) -> Option<(Range<usize>, Vec<Option<Schedule>>, usize)> {
    let rtrimmed = line.raw.trim_end();
    if !rtrimmed.ends_with("-->") {
        return None;
    }
    let open = rtrimmed.rfind("<!--SR:")?;
    let comment = &rtrimmed[open..];
    let inner = comment.strip_prefix("<!--SR:")?.strip_suffix("-->")?;
    let groups = parse_sr_groups(inner)?;
    let start = line.start + open;
    let end = start + comment.len();
    Some((start..end, groups, open))
}

fn format_sr_comment(slots: &[Option<Schedule>]) -> String {
    let mut out = String::from("<!--SR:");
    for slot in slots {
        match slot {
            Some(s) => out.push_str(&format!("!{},{},{}", s.due, s.interval, s.ease)),
            None => out.push_str(PLACEHOLDER_GROUP),
        }
    }
    out.push_str("-->");
    out
}

/// Finds the first `:::` (preferred) or `::` outside inline code spans and `%%...%%` comments.
fn find_colon_separator(line: &str) -> Option<(usize, usize, bool)> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut first_double: Option<(usize, usize)> = None;

    while i < len {
        // Skip inline code spans (`...`, ``...``, etc.).
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let mut j = i + ticks;
            let mut closed = None;
            while j < len {
                if bytes[j] == b'`' {
                    let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                    if run == ticks {
                        closed = Some(j + run);
                        break;
                    }
                    j += run;
                } else {
                    j += 1;
                }
            }
            if let Some(end) = closed {
                i = end;
            } else {
                i += ticks;
            }
            continue;
        }
        // Skip inline Obsidian comments `%%...%%`.
        if i + 1 < len && bytes[i] == b'%' && bytes[i + 1] == b'%' {
            if let Some(rel) = line[i + 2..].find("%%") {
                i = i + 2 + rel + 2;
                continue;
            }
        }
        if bytes[i] == b':' {
            let run = bytes[i..].iter().take_while(|&&b| b == b':').count();
            if run == 3 {
                return Some((i, i + 3, true));
            }
            if run == 2 && first_double.is_none() {
                first_double = Some((i, i + 2));
            }
            i += run;
            continue;
        }
        i += 1;
    }

    first_double.map(|(s, e)| (s, e, false))
}

fn indent_width(raw: &str) -> (&str, usize) {
    let mut cols = 0usize;
    let mut end = 0usize;
    for (i, ch) in raw.char_indices() {
        match ch {
            ' ' => {
                cols += 1;
                end = i + 1;
            }
            '\t' => {
                cols += 4;
                end = i + 1;
            }
            _ => break,
        }
    }
    (&raw[..end], cols)
}

/// Parses a Markdown list item line, returning `(indent_slice, indent_cols, item_text)`.
fn parse_list_item(raw: &str) -> Option<(&str, usize, &str)> {
    let (indent, cols) = indent_width(raw);
    let rest = &raw[indent.len()..];
    if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        if let Some(text) = after.strip_prefix([' ', '\t']) {
            return Some((indent, cols, text));
        }
        return None;
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        if let Some(after) = rest[digits..].strip_prefix(['.', ')'])
            && let Some(text) = after.strip_prefix([' ', '\t'])
        {
            return Some((indent, cols, text));
        }
    }
    None
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// Removes a standalone `#card` tag (outside inline code), returning `Some(cleaned)` when found.
fn strip_card_tag(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut found_range: Option<Range<usize>> = None;

    while i < len {
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let mut j = i + ticks;
            let mut closed = None;
            while j < len {
                if bytes[j] == b'`' {
                    let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                    if run == ticks {
                        closed = Some(j + run);
                        break;
                    }
                    j += run;
                } else {
                    j += 1;
                }
            }
            i = closed.unwrap_or(i + ticks);
            continue;
        }
        if bytes[i] == b'#' {
            let before_ok = text[..i]
                .chars()
                .next_back()
                .is_none_or(|c| c.is_whitespace() || "([{,;\"'".contains(c));
            if before_ok
                && i + 5 <= len
                && bytes[i + 1..i + 5].eq_ignore_ascii_case(b"card")
                && text[i + 5..].chars().next().is_none_or(|c| !is_tag_char(c))
            {
                found_range = Some(i..i + 5);
                break;
            }
        }
        let ch = text[i..].chars().next()?;
        i += ch.len_utf8();
    }

    let range = found_range?;
    let before = text[..range.start].trim_end();
    let after = text[range.end..].trim_start();
    let joined = if before.is_empty() {
        after.to_owned()
    } else if after.is_empty() {
        before.to_owned()
    } else {
        format!("{before} {after}")
    };
    Some(joined)
}

fn dedent_lines(lines: &[&str]) -> String {
    let min_cols = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_width(l).1)
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let mut cols = 0usize;
            let mut cut = 0usize;
            for (i, ch) in line.char_indices() {
                if cols >= min_cols {
                    break;
                }
                match ch {
                    ' ' => {
                        cols += 1;
                        cut = i + 1;
                    }
                    '\t' => {
                        cols += 4;
                        cut = i + 1;
                    }
                    _ => break,
                }
            }
            &line[cut..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds all flashcards in `text`, skipping YAML properties and fenced code blocks.
pub fn find(text: &str) -> Vec<Card> {
    let body = crate::front_matter(text).map_or(0, |(_, b)| b);
    let lines = collect_lines(text, body);
    let mut cards: Vec<Card> = Vec::new();
    let mut pending_q: Vec<usize> = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i].raw;

        // Track fenced code blocks.
        if let Some(f) = fence {
            if is_fence_close(raw, f) {
                fence = None;
            }
            pending_q.clear();
            i += 1;
            continue;
        }
        if let Some(f) = parse_fence_open(raw) {
            fence = Some(f);
            pending_q.clear();
            i += 1;
            continue;
        }

        // Track multi-line `%%...%%` Obsidian comments.
        if in_comment {
            if raw.contains("%%") {
                in_comment = false;
            }
            pending_q.clear();
            i += 1;
            continue;
        }
        let trimmed = raw.trim();
        if let Some(after_open) = trimmed.strip_prefix("%%") {
            if !after_open.contains("%%") {
                in_comment = true;
            }
            pending_q.clear();
            i += 1;
            continue;
        }

        // Blank lines or orphan schedule comments break multi-line questions.
        if trimmed.is_empty() || parse_standalone_sr(&lines[i]).is_some() {
            pending_q.clear();
            i += 1;
            continue;
        }

        // (b) Multi-line card: question lines, `?` (or `??`), answer lines until blank line/heading.
        if (trimmed == "?" || trimmed == "??") && !pending_q.is_empty() {
            let reversible = trimmed == "??";
            let mut ans_indices: Vec<usize> = Vec::new();
            let mut inner_fence: Option<(u8, usize)> = None;
            let mut j = i + 1;
            while j < lines.len() {
                let ans_raw = lines[j].raw;
                if let Some(f) = inner_fence {
                    ans_indices.push(j);
                    if is_fence_close(ans_raw, f) {
                        inner_fence = None;
                    }
                    j += 1;
                    continue;
                }
                if ans_raw.trim().is_empty()
                    || parse_standalone_sr(&lines[j]).is_some()
                    || is_heading(ans_raw)
                {
                    break;
                }
                if let Some(f) = parse_fence_open(ans_raw) {
                    inner_fence = Some(f);
                }
                ans_indices.push(j);
                j += 1;
            }

            if !ans_indices.is_empty() {
                let last_ans = *ans_indices.last().expect("non-empty");
                let mut comment_range: Option<Range<usize>> = None;
                let mut groups: Vec<Option<Schedule>> = Vec::new();
                let mut end_offset = lines[last_ans].content_end;
                let mut next_i = j;
                let mut last_line_slice = lines[last_ans].raw;

                if let Some((cr, parsed, cut)) = parse_trailing_sr(&lines[last_ans]) {
                    comment_range = Some(cr);
                    groups = parsed;
                    last_line_slice = lines[last_ans].raw[..cut].trim_end();
                } else if j < lines.len()
                    && let Some((cr, parsed)) = parse_standalone_sr(&lines[j])
                {
                    end_offset = cr.end;
                    comment_range = Some(cr);
                    groups = parsed;
                    next_i = j + 1;
                }

                let question = pending_q
                    .iter()
                    .map(|&idx| lines[idx].raw)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_owned();
                let mut ans_parts: Vec<&str> = ans_indices[..ans_indices.len() - 1]
                    .iter()
                    .map(|&idx| lines[idx].raw)
                    .collect();
                if !last_line_slice.trim().is_empty() || ans_parts.is_empty() {
                    ans_parts.push(last_line_slice);
                }
                let answer = ans_parts.join("\n").trim().to_owned();

                if !question.is_empty() && !answer.is_empty() {
                    let first_q = pending_q[0];
                    let card_range = lines[first_q].start..end_offset;
                    let insert_at = lines[last_ans].content_end;
                    let line_no = lines[first_q].number;
                    let slots = if reversible { 2 } else { 1 };
                    let kind = if reversible {
                        CardKind::MultiLineReversible
                    } else {
                        CardKind::MultiLine
                    };

                    cards.push(Card {
                        index: cards.len(),
                        kind,
                        range: card_range.clone(),
                        comment_range: comment_range.clone(),
                        insert_at,
                        insert_indent: String::new(),
                        slot: 0,
                        slots,
                        line: line_no,
                        question: question.clone(),
                        answer: answer.clone(),
                        schedule: groups.first().copied().flatten(),
                    });
                    if reversible {
                        cards.push(Card {
                            index: cards.len(),
                            kind,
                            range: card_range,
                            comment_range,
                            insert_at,
                            insert_indent: String::new(),
                            slot: 1,
                            slots: 2,
                            line: line_no,
                            question: answer,
                            answer: question,
                            schedule: groups.get(1).copied().flatten(),
                        });
                    }
                    pending_q.clear();
                    i = next_i;
                    continue;
                }
            }
            pending_q.clear();
            i += 1;
            continue;
        }

        // (c) Logseq-style list item tagged `#card` whose nested items are the answer.
        if let Some((parent_indent, parent_cols, item_text)) = parse_list_item(raw)
            && let Some(cleaned_q) = strip_card_tag(item_text)
        {
            let mut ans_indices: Vec<usize> = Vec::new();
            let mut inner_fence: Option<(u8, usize)> = None;
            let mut j = i + 1;
            while j < lines.len() {
                let child_raw = lines[j].raw;
                if let Some(f) = inner_fence {
                    ans_indices.push(j);
                    if is_fence_close(child_raw, f) {
                        inner_fence = None;
                    }
                    j += 1;
                    continue;
                }
                if child_raw.trim().is_empty() || parse_standalone_sr(&lines[j]).is_some() {
                    break;
                }
                let (_, child_cols) = indent_width(child_raw);
                if child_cols <= parent_cols {
                    break;
                }
                if let Some(f) = parse_fence_open(child_raw.trim_start()) {
                    inner_fence = Some(f);
                }
                ans_indices.push(j);
                j += 1;
            }

            if !ans_indices.is_empty() {
                let last_ans = *ans_indices.last().expect("non-empty");
                let mut comment_range: Option<Range<usize>> = None;
                let mut groups: Vec<Option<Schedule>> = Vec::new();
                let mut end_offset = lines[last_ans].content_end;
                let mut next_i = j;
                let mut last_line_slice = lines[last_ans].raw;

                if let Some((cr, parsed, cut)) = parse_trailing_sr(&lines[last_ans]) {
                    comment_range = Some(cr);
                    groups = parsed;
                    last_line_slice = lines[last_ans].raw[..cut].trim_end();
                } else if j < lines.len()
                    && let Some((cr, parsed)) = parse_standalone_sr(&lines[j])
                {
                    end_offset = cr.end;
                    comment_range = Some(cr);
                    groups = parsed;
                    next_i = j + 1;
                }

                let question = cleaned_q.trim().to_owned();
                let mut ans_raws: Vec<&str> = ans_indices[..ans_indices.len() - 1]
                    .iter()
                    .map(|&idx| lines[idx].raw)
                    .collect();
                if !last_line_slice.trim().is_empty() || ans_raws.is_empty() {
                    ans_raws.push(last_line_slice);
                }
                let answer = dedent_lines(&ans_raws).trim().to_owned();

                if !question.is_empty() && !answer.is_empty() {
                    cards.push(Card {
                        index: cards.len(),
                        kind: CardKind::ListCard,
                        range: lines[i].start..end_offset,
                        comment_range,
                        insert_at: lines[last_ans].content_end,
                        insert_indent: parent_indent.to_owned(),
                        slot: 0,
                        slots: 1,
                        line: lines[i].number,
                        question,
                        answer,
                        schedule: groups.first().copied().flatten(),
                    });
                    pending_q.clear();
                    i = next_i;
                    continue;
                }
            }
        }

        // (a) Single-line card: `Question::Answer` or `Question:::Answer`.
        if !is_heading(raw) && !is_hr(raw) {
            let (body_slice, inline_sr) = match parse_trailing_sr(&lines[i]) {
                Some((cr, groups, cut)) => (&raw[..cut], Some((cr, groups))),
                None => (raw, None),
            };
            if let Some((sep_start, sep_end, reversible)) = find_colon_separator(body_slice) {
                let question = body_slice[..sep_start].trim().to_owned();
                let answer = body_slice[sep_end..].trim().to_owned();
                if !question.is_empty() && !answer.is_empty() {
                    let mut comment_range: Option<Range<usize>> = None;
                    let mut groups: Vec<Option<Schedule>> = Vec::new();
                    let mut end_offset = lines[i].content_end;
                    let mut next_i = i + 1;

                    if let Some((cr, parsed)) = inline_sr {
                        comment_range = Some(cr);
                        groups = parsed;
                    } else if i + 1 < lines.len()
                        && let Some((cr, parsed)) = parse_standalone_sr(&lines[i + 1])
                    {
                        end_offset = cr.end;
                        comment_range = Some(cr);
                        groups = parsed;
                        next_i = i + 2;
                    }

                    let card_range = lines[i].start..end_offset;
                    let insert_at = lines[i].content_end;
                    let line_no = lines[i].number;
                    let slots = if reversible { 2 } else { 1 };
                    let kind = if reversible {
                        CardKind::SingleLineReversible
                    } else {
                        CardKind::SingleLine
                    };

                    cards.push(Card {
                        index: cards.len(),
                        kind,
                        range: card_range.clone(),
                        comment_range: comment_range.clone(),
                        insert_at,
                        insert_indent: String::new(),
                        slot: 0,
                        slots,
                        line: line_no,
                        question: question.clone(),
                        answer: answer.clone(),
                        schedule: groups.first().copied().flatten(),
                    });
                    if reversible {
                        cards.push(Card {
                            index: cards.len(),
                            kind,
                            range: card_range,
                            comment_range,
                            insert_at,
                            insert_indent: String::new(),
                            slot: 1,
                            slots: 2,
                            line: line_no,
                            question: answer,
                            answer: question,
                            schedule: groups.get(1).copied().flatten(),
                        });
                    }
                    pending_q.clear();
                    i = next_i;
                    continue;
                }
            }
        }

        if is_heading(raw) || is_hr(raw) {
            pending_q.clear();
        } else {
            pending_q.push(i);
        }
        i += 1;
    }

    cards
}

/// Rewrites only `card`'s `<!--SR:...-->` schedule comment in `text`, leaving everything else untouched.
pub fn rewrite(text: &str, card: &Card, new_state: Schedule) -> String {
    let slot_count = usize::from(card.slots.max(1));
    let slot_index = usize::from(card.slot).min(slot_count - 1);

    if let Some(ref range) = card.comment_range
        && range.start <= range.end
        && range.end <= text.len()
    {
        let existing = &text[range.clone()];
        let mut slots = existing
            .strip_prefix("<!--SR:")
            .and_then(|s| s.strip_suffix("-->"))
            .and_then(parse_sr_groups)
            .unwrap_or_default();
        if slots.len() < slot_count {
            slots.resize(slot_count, None);
        }
        slots[slot_index] = Some(new_state);
        let replacement = format_sr_comment(&slots);
        let mut out = String::with_capacity(text.len() + replacement.len());
        out.push_str(&text[..range.start]);
        out.push_str(&replacement);
        out.push_str(&text[range.end..]);
        return out;
    }

    let mut slots = vec![None; slot_count];
    slots[slot_index] = Some(new_state);
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let insertion = format!("{eol}{}{}", card.insert_indent, format_sr_comment(&slots));
    let at = card.insert_at.min(text.len());
    let mut out = String::with_capacity(text.len() + insertion.len());
    out.push_str(&text[..at]);
    out.push_str(&insertion);
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).expect("date")
    }

    #[test]
    fn date_arithmetic_and_validation() {
        assert_eq!(d("2024-02-28").add_days(1), d("2024-02-29"));
        assert_eq!(d("2024-02-28").add_days(2), d("2024-03-01"));
        assert_eq!(d("2023-02-28").add_days(1), d("2023-03-01"));
        assert_eq!(d("2026-12-31").add_days(1), d("2027-01-01"));
        assert_eq!(d("2026-11-26").days_since(d("2026-11-23")), 3);
        assert_eq!(Date::parse("2023-02-29"), None);
        assert_eq!(Date::parse("2026-04-31"), None);
        assert_eq!(Date::parse("2026-13-01"), None);
        assert_eq!(Date::parse("not-a-date"), None);
        assert_eq!(d("2026-11-23").to_string(), "2026-11-23");
    }

    #[test]
    fn sm2_schedule_new_and_reviewed() {
        let today = d("2026-11-23");
        // New card (initial ease 250, interval 1).
        assert_eq!(
            schedule(None, Grade::Again, today),
            Schedule {
                due: d("2026-11-24"),
                interval: 1,
                ease: 230
            }
        );
        assert_eq!(
            schedule(None, Grade::Hard, today),
            Schedule {
                due: d("2026-11-24"),
                interval: 1,
                ease: 230
            }
        );
        assert_eq!(
            schedule(None, Grade::Good, today),
            Schedule {
                due: d("2026-11-26"),
                interval: 3,
                ease: 250
            }
        );
        assert_eq!(
            schedule(None, Grade::Easy, today),
            Schedule {
                due: d("2026-11-27"),
                interval: 4,
                ease: 270
            }
        );

        // Reviewed on time (delay = 0).
        let prev = Schedule {
            due: today,
            interval: 10,
            ease: 250,
        };
        assert_eq!(
            schedule(Some(prev), Grade::Again, today),
            Schedule {
                due: d("2026-11-24"),
                interval: 1,
                ease: 230
            }
        );
        assert_eq!(
            schedule(Some(prev), Grade::Hard, today),
            Schedule {
                due: d("2026-11-28"),
                interval: 5,
                ease: 230
            }
        );
        assert_eq!(
            schedule(Some(prev), Grade::Good, today),
            Schedule {
                due: d("2026-12-18"),
                interval: 25,
                ease: 250
            }
        );
        assert_eq!(
            schedule(Some(prev), Grade::Easy, today),
            Schedule {
                due: d("2026-12-28"),
                interval: 35,
                ease: 270
            }
        );

        // Overdue by 4 days (delay bonus applies to Hard/Good/Easy).
        let overdue = Schedule {
            due: d("2026-11-19"),
            interval: 10,
            ease: 250,
        };
        assert_eq!(schedule(Some(overdue), Grade::Hard, today).interval, 6);
        assert_eq!(schedule(Some(overdue), Grade::Good, today).interval, 30);
        assert_eq!(schedule(Some(overdue), Grade::Easy, today).interval, 49);

        // Ease never drops below MIN_EASE (130).
        let low = Schedule {
            due: today,
            interval: 4,
            ease: 140,
        };
        assert_eq!(schedule(Some(low), Grade::Again, today).ease, 130);
        assert_eq!(schedule(Some(low), Grade::Hard, today).ease, 130);
    }

    #[test]
    fn finds_all_card_styles_and_skips_code_and_properties() {
        let note = "\
---
title: Study::Guide
tags: [flashcards/bio]
---
# Study

Regular text with `std::vec::Vec` is not a card.

```rust
let fake = \"Question::Answer\";
Question
?
Answer
- Fake #card
  - Nope
```

%%
Hidden::Card
%%

Capital of France::Paris
<!--SR:!2026-11-20,5,250-->

Water:::H2O

What is the powerhouse
of the cell?
?
Mitochondria.
Produces ATP.

- What is Rust's ownership rule? #card
  - Each value has a single owner.
  - Dropped when owner leaves scope.
<!--SR:!2026-11-25,3,250-->
";

        let cards = find(note);
        assert_eq!(cards.len(), 5);

        assert_eq!(cards[0].kind, CardKind::SingleLine);
        assert_eq!(cards[0].question, "Capital of France");
        assert_eq!(cards[0].answer, "Paris");
        assert_eq!(
            cards[0].schedule,
            Some(Schedule {
                due: d("2026-11-20"),
                interval: 5,
                ease: 250
            })
        );
        assert_eq!(
            &note[cards[0].comment_range.clone().unwrap()],
            "<!--SR:!2026-11-20,5,250-->"
        );

        assert_eq!(cards[1].kind, CardKind::SingleLineReversible);
        assert_eq!(cards[1].question, "Water");
        assert_eq!(cards[1].answer, "H2O");
        assert_eq!(cards[1].slot, 0);
        assert_eq!(cards[1].slots, 2);
        assert_eq!(cards[1].schedule, None);

        assert_eq!(cards[2].kind, CardKind::SingleLineReversible);
        assert_eq!(cards[2].question, "H2O");
        assert_eq!(cards[2].answer, "Water");
        assert_eq!(cards[2].slot, 1);
        assert_eq!(cards[2].slots, 2);
        assert_eq!(cards[2].schedule, None);

        assert_eq!(cards[3].kind, CardKind::MultiLine);
        assert_eq!(cards[3].question, "What is the powerhouse\nof the cell?");
        assert_eq!(cards[3].answer, "Mitochondria.\nProduces ATP.");
        assert_eq!(cards[3].schedule, None);

        assert_eq!(cards[4].kind, CardKind::ListCard);
        assert_eq!(cards[4].question, "What is Rust's ownership rule?");
        assert_eq!(
            cards[4].answer,
            "- Each value has a single owner.\n- Dropped when owner leaves scope."
        );
        assert_eq!(
            cards[4].schedule,
            Some(Schedule {
                due: d("2026-11-25"),
                interval: 3,
                ease: 250
            })
        );
    }

    #[test]
    fn rewrites_only_target_card_comment() {
        let note = "Intro line.\n\nA::B\n\nC:::D\n\n- List Q #card\n  - List A\n\nFooter.\n";
        let cards = find(note);
        assert_eq!(cards.len(), 4);

        // Insert comment on single-line card.
        let s1 = Schedule {
            due: d("2026-11-26"),
            interval: 3,
            ease: 250,
        };
        let step1 = rewrite(note, &cards[0], s1);
        assert_eq!(
            step1,
            "Intro line.\n\nA::B\n<!--SR:!2026-11-26,3,250-->\n\nC:::D\n\n- List Q #card\n  - List A\n\nFooter.\n"
        );

        // Update existing single-line comment in place.
        let cards1 = find(&step1);
        let s1_next = Schedule {
            due: d("2026-12-04"),
            interval: 8,
            ease: 250,
        };
        let step2 = rewrite(&step1, &cards1[0], s1_next);
        assert_eq!(
            step2,
            "Intro line.\n\nA::B\n<!--SR:!2026-12-04,8,250-->\n\nC:::D\n\n- List Q #card\n  - List A\n\nFooter.\n"
        );

        // Schedule forward side of reversible card, then reverse side; both groups live in one comment.
        let cards2 = find(&step2);
        let rev_fwd = Schedule {
            due: d("2026-11-26"),
            interval: 3,
            ease: 250,
        };
        let step3 = rewrite(&step2, &cards2[1], rev_fwd);
        assert!(step3.contains("C:::D\n<!--SR:!2026-11-26,3,250!2000-01-01,0,250-->\n"));

        let cards3 = find(&step3);
        assert_eq!(cards3[1].schedule, Some(rev_fwd));
        assert_eq!(cards3[2].schedule, None);

        let rev_back = Schedule {
            due: d("2026-11-24"),
            interval: 1,
            ease: 230,
        };
        let step4 = rewrite(&step3, &cards3[2], rev_back);
        assert!(step4.contains("C:::D\n<!--SR:!2026-11-26,3,250!2026-11-24,1,230-->\n"));
        assert!(step4.contains("A::B\n<!--SR:!2026-12-04,8,250-->\n"));
        assert!(step4.ends_with("- List Q #card\n  - List A\n\nFooter.\n"));

        // Schedule Logseq `#card`.
        let cards4 = find(&step4);
        assert_eq!(cards4[1].schedule, Some(rev_fwd));
        assert_eq!(cards4[2].schedule, Some(rev_back));
        let step5 = rewrite(&step4, &cards4[3], s1);
        assert!(
            step5.ends_with("- List Q #card\n  - List A\n<!--SR:!2026-11-26,3,250-->\n\nFooter.\n")
        );
    }

    #[test]
    fn inline_comment_and_crlf_preserved() {
        let note = "Q::A <!--SR:!2026-11-20,2,250-->\r\nNext::Line\r\n";
        let cards = find(note);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].answer, "A");
        assert_eq!(
            cards[0].schedule,
            Some(Schedule {
                due: d("2026-11-20"),
                interval: 2,
                ease: 250
            })
        );

        let updated = rewrite(
            note,
            &cards[0],
            Schedule {
                due: d("2026-11-28"),
                interval: 5,
                ease: 250,
            },
        );
        assert_eq!(
            updated,
            "Q::A <!--SR:!2026-11-28,5,250-->\r\nNext::Line\r\n"
        );

        let cards2 = find(&updated);
        let updated2 = rewrite(
            &updated,
            &cards2[1],
            Schedule {
                due: d("2026-11-26"),
                interval: 3,
                ease: 250,
            },
        );
        assert_eq!(
            updated2,
            "Q::A <!--SR:!2026-11-28,5,250-->\r\nNext::Line\r\n<!--SR:!2026-11-26,3,250-->\r\n"
        );
    }

    #[test]
    fn deck_resolution() {
        assert_eq!(
            deck("Languages/Spanish.md", &["flashcards/vocab".into()]),
            "vocab"
        );
        assert_eq!(
            deck("Languages/Spanish.md", &["Flashcards/ES/Verbs".into()]),
            "ES/Verbs"
        );
        assert_eq!(deck("Languages/Spanish.md", &["study".into()]), "Languages");
        assert_eq!(deck("Science/Bio/Cells.md", &[]), "Science/Bio");
        assert_eq!(deck("Loose.md", &[]), "Notes");
    }

    #[test]
    fn format_intervals() {
        assert_eq!(format_interval(1), "1d");
        assert_eq!(format_interval(14), "14d");
        assert_eq!(format_interval(30), "1mo");
        assert_eq!(format_interval(45), "1.5mo");
        assert_eq!(format_interval(365), "1y");
        assert_eq!(format_interval(548), "1.5y");
    }
}
