//! Chat (`chat/workspace.tsx`, `chat/voice.tsx`, `shared/chat.ts`, `shared/qr.ts`):
//! WhatsApp and Discord accounts, chats, servers, channels, voice calls and linking.

pub mod conversation;
pub mod panes;
pub mod voice;
pub mod workspace;

pub use workspace::ChatWorkspace;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

pub const BASE: &str = "/api/v1/chat";

pub const CHAT_LIMITS_ACCOUNTS: usize = 10;
pub const CHAT_LIMITS_NAME: usize = 64;
pub const CHAT_LIMITS_TEXT: usize = 65536;
pub const CHAT_LIMITS_DISCORD_TEXT: usize = 4000;

pub const CHAT_QUICK_REACTIONS: [&str; 6] = [
    "\u{1f44d}",
    "\u{2764}\u{fe0f}",
    "\u{1f602}",
    "\u{1f62e}",
    "\u{1f622}",
    "\u{1f64f}",
];

/// Mute choices as WhatsApp offers them; -1 is always.
pub const CHAT_MUTE_CHOICES: [(&str, i64); 3] = [
    ("8 hours", 8 * 3_600_000),
    ("1 week", 7 * 86_400_000),
    ("Always", -1),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatView {
    Chats,
    Unread,
    Groups,
    Archived,
}

impl ChatView {
    pub const ALL: [Self; 4] = [Self::Chats, Self::Unread, Self::Groups, Self::Archived];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chats => "chats",
            Self::Unread => "unread",
            Self::Groups => "groups",
            Self::Archived => "archived",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "unread" => Self::Unread,
            "groups" => Self::Groups,
            "archived" => Self::Archived,
            _ => Self::Chats,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Chats => "Chats",
            Self::Unread => "Unread",
            Self::Groups => "Groups",
            Self::Archived => "Archived",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Chats => "chat",
            Self::Unread => "unread",
            Self::Groups => "contacts",
            Self::Archived => "archive",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatCounts {
    pub chats: u32,
    pub unread: u32,
    pub groups: u32,
    pub archived: u32,
}

impl ChatCounts {
    pub fn for_view(&self, view: ChatView) -> u32 {
        match view {
            ChatView::Chats => self.chats,
            ChatView::Unread => self.unread,
            ChatView::Groups => self.groups,
            ChatView::Archived => self.archived,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatAccount {
    pub id: String,
    pub platform: String,
    pub name: String,
    pub jid: String,
    pub state: String,
    pub detail: String,
    pub created_at: f64,
    pub linked_at: f64,
    pub synced_at: f64,
    pub history_progress: u32,
    pub importing: bool,
    pub unread: u32,
    pub counts: ChatCounts,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatBridgeStatus {
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatOverview {
    pub accounts: Vec<ChatAccount>,
    pub bridge: ChatBridgeStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatContact {
    pub name: String,
    pub book: String,
    pub href: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatThread {
    pub id: String,
    pub account_id: String,
    pub jid: String,
    pub kind: String,
    pub name: String,
    pub at: f64,
    pub preview: String,
    pub preview_kind: String,
    pub preview_from_me: bool,
    pub preview_sender: String,
    pub preview_status: String,
    pub unread: u32,
    pub archived: bool,
    pub pinned: bool,
    pub muted_until: f64,
    pub whatsapp_name: String,
    pub contact: Option<ChatContact>,
    pub server_id: String,
    pub section: String,
    pub order: String,
    pub topic: String,
    pub mentions: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatServer {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub muted_until: f64,
    pub unread: bool,
    pub mentions: u32,
    pub channels: Vec<ChatThread>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatFriend {
    pub id: String,
    pub name: String,
    pub username: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceMember {
    pub id: String,
    pub name: String,
    pub muted: bool,
    pub deafened: bool,
    pub video: bool,
    pub streaming: bool,
}

pub type ChatVoiceServers = BTreeMap<String, BTreeMap<String, Vec<ChatVoiceMember>>>;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceCall {
    pub state: String,
    pub server_id: String,
    pub channel_id: String,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub can_speak: bool,
    pub e2ee: bool,
    pub protocol: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceEnded {
    pub channel_id: String,
    pub reason: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatLiveMessage {
    Hello {
        #[serde(default)]
        servers: ChatVoiceServers,
        #[serde(default)]
        call: Option<ChatVoiceCall>,
    },
    Voice {
        #[serde(default)]
        servers: ChatVoiceServers,
        #[serde(default)]
        complete: bool,
    },
    Call {
        #[serde(default)]
        call: Option<ChatVoiceCall>,
        #[serde(default)]
        ended: Option<ChatVoiceEnded>,
    },
    Owner {
        #[serde(default)]
        here: bool,
        #[serde(default)]
        elsewhere: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatThreadPage {
    pub pinned: Vec<ChatThread>,
    pub threads: Vec<ChatThread>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMedia {
    pub kind: String,
    pub mime: String,
    pub size: f64,
    pub seconds: u32,
    pub width: u32,
    pub height: u32,
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatReaction {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessage {
    pub id: String,
    pub message_id: String,
    pub sender: String,
    pub sender_name: String,
    pub from_me: bool,
    pub at: f64,
    pub kind: String,
    pub text: String,
    pub file_name: String,
    pub quoted_id: String,
    pub quoted_sender: String,
    pub quoted_text: String,
    pub status: String,
    pub edited: bool,
    pub deleted: bool,
    pub media: Option<ChatMedia>,
    pub reactions: Vec<ChatReaction>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessagePage {
    pub thread: ChatThread,
    pub messages: Vec<ChatMessage>,
    pub before: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatLinkState {
    pub account_id: String,
    pub state: String,
    pub qr: Option<String>,
    pub expires_at: Option<f64>,
    pub code: Option<String>,
    pub reason: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoAccount {
    pub id: String,
    pub name: String,
    pub state: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoServer {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoMedia {
    pub message_id: String,
    pub kind: String,
    pub mime: String,
    pub state: String,
    pub file_name: String,
    pub size: f64,
    pub at: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMember {
    pub jid: String,
    pub name: String,
    pub admin: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoGroup {
    pub name: String,
    pub topic: String,
    pub members: Vec<ChatMember>,
    pub created: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfo {
    pub thread: ChatThread,
    pub account: ChatInfoAccount,
    pub server: Option<ChatInfoServer>,
    pub messages: u64,
    pub complete: bool,
    pub media: Vec<ChatInfoMedia>,
    pub group: Option<ChatInfoGroup>,
    pub group_error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSetting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
}

pub fn platform_name(platform: &str) -> &'static str {
    if platform == "discord" {
        "Discord"
    } else {
        "WhatsApp"
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "Photo",
        "video" => "Video",
        "audio" => "Audio",
        "voice" => "Voice message",
        "document" => "Document",
        "sticker" => "Sticker",
        "location" => "Location",
        "contact" => "Contact",
        "poll" => "Poll",
        "unsupported" => "Message not supported here yet",
        _ => "",
    }
}

/// Fixed-width keys that sort newest first (`chatTimeKey`).
pub fn chat_time_key(at: f64) -> String {
    let clamped = at.max(0.0).floor() as i64;
    let inv = (9_999_999_999_999_i64 - clamped).max(0);
    format!("{inv:013}")
}

/// The name an attachment downloads and saves under (`localName`).
pub fn local_name(kind: &str, mime: &str, file_name: &str, at: f64, platform: &str) -> String {
    let given: String = file_name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_owned();
    if !given.is_empty() && given != "." && given != ".." {
        return given.chars().take(200).collect();
    }
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/3gpp" => "3gp",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "application/pdf" => "pdf",
        _ => "",
    };
    let label = match kind {
        "image" => "Photo",
        "sticker" => "Sticker",
        "video" => "Video",
        "audio" => "Audio",
        "voice" => "Voice message",
        "document" => "Document",
        _ => "Attachment",
    };
    let d = js_sys::Date::new(&JsValue::from_f64(at));
    let service = if platform == "discord" {
        "Discord"
    } else {
        "WhatsApp"
    };
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    format!(
        "{service} {label} {}-{:02}-{:02} at {:02}.{:02}.{:02}{suffix}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds()
    )
}

pub fn search_params(pairs: &[(&str, &str)]) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    for (k, v) in pairs {
        params.append(k, v);
    }
    params.to_string().as_string().unwrap_or_default()
}

// QR code encoder (byte mode, error correction M), ported from `next/shared/qr.ts`.
const ECC_PER_BLOCK: [usize; 41] = [
    0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
const BLOCK_COUNT: [usize; 41] = [
    0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
    25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];
const FORMAT_BITS_M: u32 = 0;

fn raw_modules(version: usize) -> usize {
    let mut result = (16 * version + 128) * version + 64;
    if version >= 2 {
        let align = version / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if version >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_codewords(version: usize) -> usize {
    raw_modules(version) / 8 - ECC_PER_BLOCK[version] * BLOCK_COUNT[version]
}

fn gf_multiply(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11d);
        z ^= (((y >> i) & 1) as u16) * (x as u16);
    }
    z as u8
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root: u8 = 1;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_multiply(root, 2);
    }
    result
}

fn rs_remainder(data: &[u8], div: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; div.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (slot, &coef) in result.iter_mut().zip(div.iter()) {
            *slot ^= gf_multiply(coef, factor);
        }
    }
    result
}

fn bit(value: u32, i: u32) -> bool {
    ((value >> i) & 1) != 0
}

pub fn qr_matrix(text: &str) -> Result<Vec<Vec<bool>>, String> {
    let bytes = text.as_bytes();
    let mut version = 1usize;
    while version <= 40 {
        let count_bits = if version < 10 { 8 } else { 16 };
        if 4 + count_bits + 8 * bytes.len() <= data_codewords(version) * 8 {
            break;
        }
        version += 1;
    }
    if version > 40 {
        return Err("The code is too long for a QR code.".into());
    }
    let mut bits: Vec<u8> = Vec::new();
    let push_bits = |bits: &mut Vec<u8>, value: usize, length: usize| {
        for i in (0..length).rev() {
            bits.push(((value >> i) & 1) as u8);
        }
    };
    push_bits(&mut bits, 4, 4);
    push_bits(&mut bits, bytes.len(), if version < 10 { 8 } else { 16 });
    for &b in bytes {
        push_bits(&mut bits, b as usize, 8);
    }
    let capacity = data_codewords(version) * 8;
    let n = 4.min(capacity - bits.len());
    push_bits(&mut bits, 0, n);
    let n = (8 - bits.len() % 8) % 8;
    push_bits(&mut bits, 0, n);
    let mut pad = 0xecusize;
    while bits.len() < capacity {
        push_bits(&mut bits, pad, 8);
        pad ^= 0xec ^ 0x11;
    }
    let mut data: Vec<u8> = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.chunks(8) {
        data.push(chunk.iter().fold(0u8, |a, &b| (a << 1) | b));
    }

    let blocks = BLOCK_COUNT[version];
    let ecc_length = ECC_PER_BLOCK[version];
    let raw = raw_modules(version) / 8;
    let short = blocks - raw % blocks;
    let short_length = raw / blocks;
    let div = rs_divisor(ecc_length);
    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(blocks);
    let mut k = 0usize;
    for i in 0..blocks {
        let len = short_length - ecc_length + usize::from(i >= short);
        let mut chunk = data[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&chunk, &div);
        if i < short {
            chunk.push(0);
        }
        chunk.extend_from_slice(&ecc);
        parts.push(chunk);
    }
    let mut codewords: Vec<u8> = Vec::new();
    let part_len = parts[0].len();
    for i in 0..part_len {
        for (j, part) in parts.iter().enumerate() {
            if i != short_length - ecc_length || j >= short {
                codewords.push(part[i]);
            }
        }
    }

    let size = version * 4 + 17;
    let mut modules = vec![vec![false; size]; size];
    let mut fixed = vec![vec![false; size]; size];
    let set = |modules: &mut Vec<Vec<bool>>,
               fixed: &mut Vec<Vec<bool>>,
               x: usize,
               y: usize,
               dark: bool| {
        modules[y][x] = dark;
        fixed[y][x] = true;
    };
    for i in 0..size {
        set(&mut modules, &mut fixed, 6, i, i % 2 == 0);
        set(&mut modules, &mut fixed, i, 6, i % 2 == 0);
    }
    for &(cx, cy) in &[
        (3isize, 3isize),
        (size as isize - 4, 3),
        (3, size as isize - 4),
    ] {
        for dy in -4..=4 {
            for dx in -4..=4 {
                let (x, y) = (cx + dx, cy + dy);
                let dist = dx.abs().max(dy.abs());
                if x >= 0 && x < size as isize && y >= 0 && y < size as isize {
                    set(
                        &mut modules,
                        &mut fixed,
                        x as usize,
                        y as usize,
                        dist != 2 && dist != 4,
                    );
                }
            }
        }
    }
    if version > 1 {
        let count = version / 7 + 2;
        let step = if version == 32 {
            26
        } else {
            (version * 4 + 4).div_ceil(count * 2 - 2) * 2
        };
        let mut positions = vec![6usize];
        let mut pos = size - 7;
        while positions.len() < count {
            positions.insert(1, pos);
            pos = pos.saturating_sub(step);
        }
        for i in 0..count {
            for j in 0..count {
                if (i == 0 && j == 0) || (i == 0 && j == count - 1) || (i == count - 1 && j == 0) {
                    continue;
                }
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let x = (positions[i] as isize + dx) as usize;
                        let y = (positions[j] as isize + dy) as usize;
                        set(&mut modules, &mut fixed, x, y, dx.abs().max(dy.abs()) != 1);
                    }
                }
            }
        }
    }

    let write_format = |modules: &mut Vec<Vec<bool>>, fixed: &mut Vec<Vec<bool>>, mask: u32| {
        let value = (FORMAT_BITS_M << 3) | mask;
        let mut rem = value;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let f = ((value << 10) | rem) ^ 0x5412;
        for i in 0..=5 {
            set(modules, fixed, 8, i, bit(f, i as u32));
        }
        set(modules, fixed, 8, 7, bit(f, 6));
        set(modules, fixed, 8, 8, bit(f, 7));
        set(modules, fixed, 7, 8, bit(f, 8));
        for i in 9..15 {
            set(modules, fixed, 14 - i, 8, bit(f, i as u32));
        }
        for i in 0..8 {
            set(modules, fixed, size - 1 - i, 8, bit(f, i as u32));
        }
        for i in 8..15 {
            set(modules, fixed, 8, size - 15 + i, bit(f, i as u32));
        }
        set(modules, fixed, 8, size - 8, true);
    };
    write_format(&mut modules, &mut fixed, 0);

    if version >= 7 {
        let mut rem = version as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1f25);
        }
        let v = ((version as u32) << 12) | rem;
        for i in 0..18usize {
            let a = size - 11 + i % 3;
            let b = i / 3;
            set(&mut modules, &mut fixed, a, b, bit(v, i as u32));
            set(&mut modules, &mut fixed, b, a, bit(v, i as u32));
        }
    }

    let mut bit_idx = 0usize;
    let mut right = size as isize - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = (right - j) as usize;
                let y = if ((right + 1) & 2) == 0 {
                    size - 1 - vert
                } else {
                    vert
                };
                if !fixed[y][x] && bit_idx < codewords.len() * 8 {
                    modules[y][x] = bit(codewords[bit_idx >> 3] as u32, (7 - (bit_idx & 7)) as u32);
                    bit_idx += 1;
                }
            }
        }
        right -= 2;
    }

    let mask_fn = |mask: u32, x: usize, y: usize| -> bool {
        match mask {
            0 => (x + y) % 2 == 0,
            1 => y % 2 == 0,
            2 => x % 3 == 0,
            3 => (x + y) % 3 == 0,
            4 => (x / 3 + y / 2) % 2 == 0,
            5 => (x * y) % 2 + (x * y) % 3 == 0,
            6 => ((x * y) % 2 + (x * y) % 3) % 2 == 0,
            _ => ((x + y) % 2 + (x * y) % 3) % 2 == 0,
        }
    };
    let apply_mask = |modules: &mut Vec<Vec<bool>>, fixed: &[Vec<bool>], mask: u32| {
        for y in 0..size {
            for x in 0..size {
                if !fixed[y][x] && mask_fn(mask, x, y) {
                    modules[y][x] = !modules[y][x];
                }
            }
        }
    };
    let penalty = |modules: &[Vec<bool>]| -> usize {
        let mut score = 0usize;
        let mut dark = 0usize;
        for a in 0..size {
            let (mut row_run, mut col_run) = (1usize, 1usize);
            for b in 1..size {
                if modules[a][b] == modules[a][b - 1] {
                    row_run += 1;
                    if row_run == 5 {
                        score += 3;
                    } else if row_run > 5 {
                        score += 1;
                    }
                } else {
                    row_run = 1;
                }
                if modules[b][a] == modules[b - 1][a] {
                    col_run += 1;
                    if col_run == 5 {
                        score += 3;
                    } else if col_run > 5 {
                        score += 1;
                    }
                } else {
                    col_run = 1;
                }
            }
        }
        for y in 0..size {
            for x in 0..size {
                if modules[y][x] {
                    dark += 1;
                }
                if x < size - 1
                    && y < size - 1
                    && modules[y][x] == modules[y][x + 1]
                    && modules[y][x] == modules[y + 1][x]
                    && modules[y][x] == modules[y + 1][x + 1]
                {
                    score += 3;
                }
            }
        }
        let total = size * size;
        let balance = (dark as isize * 20 - total as isize * 10).unsigned_abs() / total;
        score + balance * 10
    };

    let mut best = 0u32;
    let mut best_score = usize::MAX;
    for mask in 0..8u32 {
        apply_mask(&mut modules, &fixed, mask);
        write_format(&mut modules, &mut fixed, mask);
        let score = penalty(&modules);
        if score < best_score {
            best = mask;
            best_score = score;
        }
        apply_mask(&mut modules, &fixed, mask);
    }
    apply_mask(&mut modules, &fixed, best);
    write_format(&mut modules, &mut fixed, best);
    Ok(modules)
}

/// An SVG path of the dark modules, offset by a four-module quiet zone (`qrPath`).
pub fn qr_path(matrix: &[Vec<bool>]) -> (String, usize) {
    let mut d = String::new();
    for (y, row) in matrix.iter().enumerate() {
        for (x, &dark) in row.iter().enumerate() {
            if dark {
                let _ = write!(&mut d, "M{} {}h1v1h-1z", x + 4, y + 4);
            }
        }
    }
    (d, matrix.len() + 8)
}
