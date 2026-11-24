//! Minimal standard PKZIP writer for multi-file and folder downloads in Files.
//! Uses `flate2` (already in `dar-runtime`) with UTF-8 filenames and standard IEEE CRC32.

use std::io::{self, Write};
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::DeflateEncoder;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn already_compressed(path: &str) -> bool {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "zip" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "png" | "jpg" | "jpeg"
            | "webp" | "avif" | "gif" | "mp4" | "webm" | "mov" | "mkv" | "mp3" | "m4a" | "ogg"
            | "opus" | "aac" | "pdf"
    )
}

/// Builds an in-memory ZIP archive from `(archive_path, blob_file_path)` pairs.
pub fn build_zip(entries: &[(String, PathBuf)]) -> io::Result<Vec<u8>> {
    struct Central {
        name: Vec<u8>,
        method: u16,
        crc: u32,
        comp_size: u32,
        uncomp_size: u32,
        local_offset: u32,
    }

    let mut out = Vec::new();
    let mut central = Vec::with_capacity(entries.len());

    for (raw_name, file_path) in entries {
        let name = raw_name.trim_start_matches('/').as_bytes().to_vec();
        let data = std::fs::read(file_path)?;
        let uncomp_size = u32::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file too large for zip"))?;
        let crc = crc32(&data);

        let (method, payload) = if data.len() < 64 || already_compressed(raw_name) {
            (0u16, data)
        } else {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
            enc.write_all(&data)?;
            let deflated = enc.finish()?;
            if deflated.len() < data.len() {
                (8u16, deflated)
            } else {
                (0u16, data)
            }
        };
        let comp_size = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "entry too large for zip"))?;
        let local_offset = u32::try_from(out.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "archive too large"))?;
        let name_len = u16::try_from(name.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "filename too long"))?;

        // Local file header (PK\x03\x04), version 2.0, UTF-8 flag 0x0800.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        out.extend_from_slice(&uncomp_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&name);
        out.extend_from_slice(&payload);

        central.push(Central {
            name,
            method,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
        });
    }

    let cd_offset = u32::try_from(out.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "archive too large"))?;
    for c in &central {
        let name_len = c.name.len() as u16;
        // Central directory header (PK\x01\x02)
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8
        out.extend_from_slice(&c.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x5421u16.to_le_bytes());
        out.extend_from_slice(&c.crc.to_le_bytes());
        out.extend_from_slice(&c.comp_size.to_le_bytes());
        out.extend_from_slice(&c.uncomp_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        out.extend_from_slice(&0u32.to_le_bytes()); // external attr
        out.extend_from_slice(&c.local_offset.to_le_bytes());
        out.extend_from_slice(&c.name);
    }
    let cd_size = u32::try_from(out.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "archive too large"))?
        - cd_offset;
    let count = u16::try_from(central.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many files"))?;

    // End of central directory (PK\x05\x06)
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    #[test]
    fn builds_valid_zip_with_stored_and_deflated_entries() {
        let dir = std::env::temp_dir().join(format!("dar-zip-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let long = "Dar personal cloud file archive test line.\n".repeat(40);
        std::fs::write(&a, b"hi").unwrap();
        std::fs::write(&b, long.as_bytes()).unwrap();

        let bytes = build_zip(&[
            ("a.txt".into(), a.clone()),
            ("nested/b.txt".into(), b.clone()),
        ])
        .unwrap();
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        // Verify EOCD at end of buffer with 2 entries.
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]), 2);
        // Verify deflated entry round-trips.
        let second_offset = bytes
            .windows(4)
            .enumerate()
            .skip(1)
            .find(|(_, w)| *w == b"PK\x03\x04")
            .unwrap()
            .0;
        let method = u16::from_le_bytes([bytes[second_offset + 8], bytes[second_offset + 9]]);
        assert_eq!(method, 8);
        let comp_len = u32::from_le_bytes(
            bytes[second_offset + 18..second_offset + 22]
                .try_into()
                .unwrap(),
        ) as usize;
        let name_len = u16::from_le_bytes([bytes[second_offset + 26], bytes[second_offset + 27]])
            as usize;
        let payload_start = second_offset + 30 + name_len;
        let mut dec = DeflateDecoder::new(&bytes[payload_start..payload_start + comp_len]);
        let mut unpacked = String::new();
        dec.read_to_string(&mut unpacked).unwrap();
        assert_eq!(unpacked, long);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
