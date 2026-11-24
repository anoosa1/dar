//! Key definitions for `Input.dispatchKeyEvent` (US layout), as Playwright's keyboard sends them.

pub struct Key {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    /// The character typed, if any (none for Escape, arrows and keys pressed with Control/Meta).
    pub text: Option<String>,
    pub location: u32,
}

pub const ALT: u32 = 1;
pub const CONTROL: u32 = 2;
pub const META: u32 = 4;
pub const SHIFT: u32 = 8;

pub fn modifier(name: &str) -> Option<u32> {
    match name {
        "Alt" => Some(ALT),
        "Control" | "ControlOrMeta" => Some(CONTROL),
        "Meta" => Some(META),
        "Shift" => Some(SHIFT),
        _ => None,
    }
}

pub fn key(name: &str, shift: bool) -> Option<Key> {
    let simple = |key: &str, code: &str, key_code: u32, text: Option<&str>| Key {
        key: key.into(),
        code: code.into(),
        key_code,
        text: text.map(Into::into),
        location: 0,
    };
    Some(match name {
        "Enter" => simple("Enter", "Enter", 13, Some("\r")),
        "Escape" => simple("Escape", "Escape", 27, None),
        "Tab" => simple("Tab", "Tab", 9, None),
        "Backspace" => simple("Backspace", "Backspace", 8, None),
        "Delete" => simple("Delete", "Delete", 46, None),
        "Space" | " " => simple(" ", "Space", 32, Some(" ")),
        "ArrowLeft" => simple("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowUp" => simple("ArrowUp", "ArrowUp", 38, None),
        "ArrowRight" => simple("ArrowRight", "ArrowRight", 39, None),
        "ArrowDown" => simple("ArrowDown", "ArrowDown", 40, None),
        "Home" => simple("Home", "Home", 36, None),
        "End" => simple("End", "End", 35, None),
        "PageUp" => simple("PageUp", "PageUp", 33, None),
        "PageDown" => simple("PageDown", "PageDown", 34, None),
        "Insert" => simple("Insert", "Insert", 45, None),
        "Shift" => Key {
            location: 1,
            ..simple("Shift", "ShiftLeft", 16, None)
        },
        "Control" | "ControlOrMeta" => Key {
            location: 1,
            ..simple("Control", "ControlLeft", 17, None)
        },
        "Alt" => Key {
            location: 1,
            ..simple("Alt", "AltLeft", 18, None)
        },
        "Meta" => Key {
            location: 1,
            ..simple("Meta", "MetaLeft", 91, None)
        },
        f if f.len() >= 2
            && f.starts_with('F')
            && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: u32 = f[1..].parse().unwrap();
            simple(f, f, 111 + n, None)
        }
        single if single.chars().count() == 1 => {
            let c = single.chars().next().unwrap();
            return Some(character(c, shift));
        }
        _ => return None,
    })
}

/// A printable character: letters and digits map to their key codes, others to their US keys.
pub fn character(c: char, shift: bool) -> Key {
    let (code, key_code, shifted) = match c {
        'a'..='z' => (
            format!("Key{}", c.to_ascii_uppercase()),
            c.to_ascii_uppercase() as u32,
            false,
        ),
        'A'..='Z' => (format!("Key{c}"), c as u32, true),
        '0'..='9' => (format!("Digit{c}"), c as u32, false),
        ' ' => ("Space".into(), 32, false),
        '-' => ("Minus".into(), 189, false),
        '_' => ("Minus".into(), 189, true),
        '=' => ("Equal".into(), 187, false),
        '+' => ("Equal".into(), 187, true),
        ',' => ("Comma".into(), 188, false),
        '<' => ("Comma".into(), 188, true),
        '.' => ("Period".into(), 190, false),
        '>' => ("Period".into(), 190, true),
        '/' => ("Slash".into(), 191, false),
        '?' => ("Slash".into(), 191, true),
        ';' => ("Semicolon".into(), 186, false),
        ':' => ("Semicolon".into(), 186, true),
        '\'' => ("Quote".into(), 222, false),
        '"' => ("Quote".into(), 222, true),
        '[' => ("BracketLeft".into(), 219, false),
        '{' => ("BracketLeft".into(), 219, true),
        ']' => ("BracketRight".into(), 221, false),
        '}' => ("BracketRight".into(), 221, true),
        '\\' => ("Backslash".into(), 220, false),
        '|' => ("Backslash".into(), 220, true),
        '`' => ("Backquote".into(), 192, false),
        '~' => ("Backquote".into(), 192, true),
        '!' => ("Digit1".into(), 49, true),
        '@' => ("Digit2".into(), 50, true),
        '#' => ("Digit3".into(), 51, true),
        '$' => ("Digit4".into(), 52, true),
        '%' => ("Digit5".into(), 53, true),
        '^' => ("Digit6".into(), 54, true),
        '&' => ("Digit7".into(), 55, true),
        '*' => ("Digit8".into(), 56, true),
        '(' => ("Digit9".into(), 57, true),
        ')' => ("Digit0".into(), 48, true),
        _ => (String::new(), 0, false),
    };
    let typed = if shift && c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    };
    let _ = shifted;
    Key {
        key: typed.to_string(),
        code,
        key_code,
        text: Some(typed.to_string()),
        location: 0,
    }
}
