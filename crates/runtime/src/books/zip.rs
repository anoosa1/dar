//! Read-only ZIP archive reader for EPUB and CBZ blobs (`next/runtime/books/zip.ts`).
//! Only stored (0) and deflated (8) entries are read, with bounded entry counts and sizes;
//! entry names are kept verbatim and looked up by exact match so no entry can escape the archive.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;

use flate2::read::DeflateDecoder;

#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: i32 = 0o100000;
#[cfg(not(target_arch = "aarch64"))]
const O_NOFOLLOW: i32 = 0o400000;

pub const DEFAULT_MAX_ENTRIES: usize = 20_000;

#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub compressed: u64,
    pub size: u64,
    pub offset: u64,
    pub encrypted: bool,
}

#[derive(Clone, Debug)]
pub struct ZipError(pub String);

impl ZipError {
    fn new(message: &str) -> Self {
        Self(message.to_owned())
    }
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ZipError {}

pub struct Zip {
    file: Arc<File>,
    entries: Vec<ZipEntry>,
    by_name: HashMap<String, usize>,
}

impl Zip {
    /// Opens an immutable ZIP blob without following symlinks and parses its central directory.
    pub async fn open(path: &Path, max_entries: usize) -> Result<Arc<Self>, ZipError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_sync(&path, max_entries))
            .await
            .map_err(|_| ZipError::new("Damaged archive."))?
    }

    pub fn open_sync(path: &Path, max_entries: usize) -> Result<Arc<Self>, ZipError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
            .map_err(|_| ZipError::new("Damaged archive."))?;
        let entries = central_directory(&file, max_entries)?;
        let mut by_name = HashMap::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            by_name.insert(entry.name.clone(), index);
        }
        Ok(Arc::new(Self { file: Arc::new(file), entries, by_name }))
    }

    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&ZipEntry> {
        self.by_name.get(name).map(|&index| &self.entries[index])
    }

    /// Reads an entry's bytes, refusing when larger than `max` once inflated.
    pub async fn read(self: &Arc<Self>, entry: &ZipEntry, max: usize) -> Result<Vec<u8>, ZipError> {
        let file = self.file.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || read_entry_sync(&file, &entry, max))
            .await
            .map_err(|_| ZipError::new("Damaged entry."))?
    }
}

fn read_at(file: &File, buffer: &mut [u8], position: u64) -> Result<(), ZipError> {
    file.read_exact_at(buffer, position).map_err(|_| ZipError::new("Damaged archive."))
}

fn u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn read_entry_sync(file: &File, entry: &ZipEntry, max: usize) -> Result<Vec<u8>, ZipError> {
    if entry.encrypted {
        return Err(ZipError::new("Encrypted entry."));
    }
    let max_u64 = max as u64;
    if entry.size > max_u64 || entry.compressed > max_u64.saturating_mul(2).saturating_add(1024) {
        return Err(ZipError::new("Entry too large."));
    }
    let mut local = [0u8; 30];
    read_at(file, &mut local, entry.offset)?;
    if u32_le(&local, 0) != 0x0403_4b50 {
        return Err(ZipError::new("Damaged archive."));
    }
    let name_len = u16_le(&local, 26) as u64;
    let extra_len = u16_le(&local, 28) as u64;
    let start = entry
        .offset
        .checked_add(30)
        .and_then(|v| v.checked_add(name_len))
        .and_then(|v| v.checked_add(extra_len))
        .ok_or_else(|| ZipError::new("Damaged archive."))?;
    let compressed_len = usize::try_from(entry.compressed).map_err(|_| ZipError::new("Entry too large."))?;
    let mut data = vec![0u8; compressed_len];
    read_at(file, &mut data, start)?;
    if entry.method == 0 {
        if data.len() as u64 != entry.size {
            return Err(ZipError::new("Damaged entry."));
        }
        return Ok(data);
    }
    if entry.method != 8 {
        return Err(ZipError::new("Unsupported compression."));
    }
    let decoder = DeflateDecoder::new(&data[..]);
    let mut out = Vec::with_capacity((entry.size as usize).min(64 * 1024));
    let limit = entry.size.saturating_add(1);
    decoder.take(limit).read_to_end(&mut out).map_err(|_| ZipError::new("Damaged entry."))?;
    if out.len() as u64 != entry.size {
        return Err(ZipError::new("Damaged entry."));
    }
    Ok(out)
}

fn central_directory(file: &File, max_entries: usize) -> Result<Vec<ZipEntry>, ZipError> {
    let meta = file.metadata().map_err(|_| ZipError::new("Damaged archive."))?;
    if !meta.is_file() {
        return Err(ZipError::new("Not a ZIP archive."));
    }
    let size = meta.len();
    if size < 22 {
        return Err(ZipError::new("Not a ZIP archive."));
    }
    let tail_len = size.min(65_557) as usize;
    let mut tail = vec![0u8; tail_len];
    read_at(file, &mut tail, size - tail_len as u64)?;

    let mut end = None;
    for i in (0..=tail.len() - 22).rev() {
        if u32_le(&tail, i) == 0x0605_4b50 {
            end = Some(i);
            break;
        }
    }
    let end = end.ok_or_else(|| ZipError::new("Not a ZIP archive."))?;

    let mut count = u16_le(&tail, end + 10) as u64;
    let mut length = u32_le(&tail, end + 12) as u64;
    let mut offset = u32_le(&tail, end + 16) as u64;

    // ZIP64: the locator just before the end record points at the 64-bit end record.
    if count == 0xffff || length == 0xffff_ffff || offset == 0xffff_ffff {
        if end < 20 || u32_le(&tail, end - 20) != 0x0706_4b50 {
            return Err(ZipError::new("Damaged archive."));
        }
        let mut record = [0u8; 56];
        let eocd64_offset = u64_le(&tail, end - 12);
        read_at(file, &mut record, eocd64_offset)?;
        if u32_le(&record, 0) != 0x0606_4b50 {
            return Err(ZipError::new("Damaged archive."));
        }
        count = u64_le(&record, 32);
        length = u64_le(&record, 40);
        offset = u64_le(&record, 48);
    }

    if count > max_entries as u64 {
        return Err(ZipError::new("Too many entries."));
    }
    if offset.checked_add(length).is_none_or(|dir_end| dir_end > size) || length > 64 * 1024 * 1024 {
        return Err(ZipError::new("Damaged archive."));
    }

    let mut directory = vec![0u8; length as usize];
    read_at(file, &mut directory, offset)?;

    let mut entries = Vec::with_capacity((count as usize).min(1024));
    let mut at = 0usize;
    while (entries.len() as u64) < count {
        if at + 46 > directory.len() || u32_le(&directory, at) != 0x0201_4b50 {
            return Err(ZipError::new("Damaged archive."));
        }
        let flags = u16_le(&directory, at + 8);
        let method = u16_le(&directory, at + 10);
        let mut compressed = u32_le(&directory, at + 20) as u64;
        let mut uncompressed = u32_le(&directory, at + 24) as u64;
        let name_len = u16_le(&directory, at + 28) as usize;
        let extra_len = u16_le(&directory, at + 30) as usize;
        let comment_len = u16_le(&directory, at + 32) as usize;
        let mut local = u32_le(&directory, at + 42) as u64;

        let name_start = at + 46;
        let Some(name_end) = name_start.checked_add(name_len) else {
            return Err(ZipError::new("Damaged archive."));
        };
        let Some(extra_end) = name_end.checked_add(extra_len) else {
            return Err(ZipError::new("Damaged archive."));
        };
        let Some(next_at) = extra_end.checked_add(comment_len) else {
            return Err(ZipError::new("Damaged archive."));
        };
        if next_at > directory.len() {
            return Err(ZipError::new("Damaged archive."));
        }

        let name_bytes = &directory[name_start..name_end];
        let extra = &directory[name_end..extra_end];

        // ZIP64 extra field: only the fields marked 0xffffffff are present, in this order.
        let mut x = 0usize;
        while x + 4 <= extra.len() {
            let id = u16_le(extra, x);
            let len = u16_le(extra, x + 2) as usize;
            let Some(field_end) = x.checked_add(4).and_then(|v| v.checked_add(len)) else {
                return Err(ZipError::new("Damaged archive."));
            };
            if field_end > extra.len() {
                break;
            }
            if id == 1 {
                let mut y = x + 4;
                if uncompressed == 0xffff_ffff {
                    if y + 8 > field_end {
                        return Err(ZipError::new("Damaged archive."));
                    }
                    uncompressed = u64_le(extra, y);
                    y += 8;
                }
                if compressed == 0xffff_ffff {
                    if y + 8 > field_end {
                        return Err(ZipError::new("Damaged archive."));
                    }
                    compressed = u64_le(extra, y);
                    y += 8;
                }
                if local == 0xffff_ffff {
                    if y + 8 > field_end {
                        return Err(ZipError::new("Damaged archive."));
                    }
                    local = u64_le(extra, y);
                }
            }
            x = field_end;
        }

        let name = if flags & 0x800 != 0 {
            String::from_utf8_lossy(name_bytes).into_owned()
        } else {
            name_bytes.iter().map(|&b| b as char).collect()
        };

        if local.checked_add(30).is_none_or(|e| e > size) || compressed > size {
            return Err(ZipError::new("Damaged archive."));
        }

        entries.push(ZipEntry {
            name,
            method,
            compressed,
            size: uncompressed,
            offset: local,
            encrypted: (flags & 1) != 0,
        });
        at = next_at;
    }

    Ok(entries)
}
