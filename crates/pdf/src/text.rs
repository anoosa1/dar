//! Decoding of PDF text strings (ISO 32000-1 §7.9.2.2, PDF 2.0 §7.9.2.2).
//!
//! PDF text strings are encoded as UTF-16BE (with a `0xFE 0xFF` BOM), UTF-8 (with a
//! `0xEF 0xBB 0xBF` BOM, introduced in PDF 2.0), or `PDFDocEncoding`.

/// `PDFDocEncoding` mapping for bytes `0x18..=0x1F` (Annex D, Table D.2).
const PDF_DOC_18_1F: [char; 8] = [
    '\u{02D8}', // 0x18 BREVE
    '\u{02C7}', // 0x19 CARON
    '\u{02C6}', // 0x1A MODIFIER LETTER CIRCUMFLEX ACCENT
    '\u{02D9}', // 0x1B DOT ABOVE
    '\u{02DD}', // 0x1C DOUBLE ACUTE ACCENT
    '\u{02DB}', // 0x1D OGONEK
    '\u{02DA}', // 0x1E RING ABOVE
    '\u{02DC}', // 0x1F SMALL TILDE
];

/// `PDFDocEncoding` mapping for bytes `0x80..=0xA0` (Annex D, Table D.2).
const PDF_DOC_80_A0: [char; 33] = [
    '\u{2022}', // 0x80 BULLET
    '\u{2020}', // 0x81 DAGGER
    '\u{2021}', // 0x82 DOUBLE DAGGER
    '\u{2026}', // 0x83 HORIZONTAL ELLIPSIS
    '\u{2014}', // 0x84 EM DASH
    '\u{2013}', // 0x85 EN DASH
    '\u{0192}', // 0x86 LATIN SMALL LETTER F WITH HOOK
    '\u{2044}', // 0x87 FRACTION SLASH
    '\u{2039}', // 0x88 SINGLE LEFT-POINTING ANGLE QUOTATION MARK
    '\u{203A}', // 0x89 SINGLE RIGHT-POINTING ANGLE QUOTATION MARK
    '\u{2212}', // 0x8A MINUS SIGN
    '\u{2030}', // 0x8B PER MILLE SIGN
    '\u{201E}', // 0x8C DOUBLE LOW-9 QUOTATION MARK
    '\u{201C}', // 0x8D LEFT DOUBLE QUOTATION MARK
    '\u{201D}', // 0x8E RIGHT DOUBLE QUOTATION MARK
    '\u{2018}', // 0x8F LEFT SINGLE QUOTATION MARK
    '\u{2019}', // 0x90 RIGHT SINGLE QUOTATION MARK
    '\u{201A}', // 0x91 SINGLE LOW-9 QUOTATION MARK
    '\u{2122}', // 0x92 TRADE MARK SIGN
    '\u{FB01}', // 0x93 LATIN SMALL LIGATURE FI
    '\u{FB02}', // 0x94 LATIN SMALL LIGATURE FL
    '\u{0141}', // 0x95 LATIN CAPITAL LETTER L WITH STROKE
    '\u{0152}', // 0x96 LATIN CAPITAL LIGATURE OE
    '\u{0160}', // 0x97 LATIN CAPITAL LETTER S WITH CARON
    '\u{0178}', // 0x98 LATIN CAPITAL LETTER Y WITH DIAERESIS
    '\u{017D}', // 0x99 LATIN CAPITAL LETTER Z WITH CARON
    '\u{0131}', // 0x9A LATIN SMALL LETTER DOTLESS I
    '\u{0142}', // 0x9B LATIN SMALL LETTER L WITH STROKE
    '\u{0153}', // 0x9C LATIN SMALL LIGATURE OE
    '\u{0161}', // 0x9D LATIN SMALL LETTER S WITH CARON
    '\u{017E}', // 0x9E LATIN SMALL LETTER Z WITH CARON
    '\u{FFFD}', // 0x9F undefined
    '\u{20AC}', // 0xA0 EURO SIGN
];

/// Decodes raw bytes from a PDF text string into a Rust `String`.
pub(crate) fn decode_pdf_text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16be(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    decode_pdf_doc_encoding(bytes)
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();

    // Strip optional PDF language escape sequences (`0x001B <2 or 4 ASCII letters> 0x001B`).
    let mut filtered = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        if units[i] == 0x001B
            && let Some(end) = find_language_escape_end(&units[i + 1..])
        {
            i += 1 + end + 1;
            continue;
        }
        filtered.push(units[i]);
        i += 1;
    }

    char::decode_utf16(filtered)
        .map(|res| res.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn find_language_escape_end(after_esc: &[u16]) -> Option<usize> {
    // ISO 32000-1 §7.9.2.2: 2-letter language code optionally followed by 2-letter country code,
    // terminated by U+001B.
    for len in [2usize, 4usize] {
        if after_esc.len() > len
            && after_esc[len] == 0x001B
            && after_esc[..len]
                .iter()
                .all(|&u| u <= 0x7F && (u as u8).is_ascii_alphabetic())
        {
            return Some(len);
        }
    }
    None
}

fn decode_pdf_doc_encoding(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        let ch = match b {
            0x00..=0x17 => char::from(b),
            0x18..=0x1F => PDF_DOC_18_1F[usize::from(b - 0x18)],
            0x20..=0x7E => char::from(b),
            0x7F => char::REPLACEMENT_CHARACTER,
            0x80..=0xA0 => PDF_DOC_80_A0[usize::from(b - 0x80)],
            0xA1..=0xFF => char::from(b),
        };
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::decode_pdf_text_string;

    #[test]
    fn decodes_ascii_and_pdf_doc_encoding() {
        assert_eq!(decode_pdf_text_string(b"Hello, PDF!"), "Hello, PDF!");
        // 0x84 = em dash, 0xA0 = euro sign, 0xE9 = é, 0x8D/0x8E = smart double quotes
        let bytes = [b'A', 0x84, b'B', b' ', 0xA0, b'5', b' ', 0x8D, 0xE9, 0x8E];
        assert_eq!(
            decode_pdf_text_string(&bytes),
            "A\u{2014}B \u{20AC}5 \u{201C}\u{00E9}\u{201D}"
        );
    }

    #[test]
    fn decodes_utf16be_with_bom_and_surrogates() {
        // BOM + "Caf" + é (U+00E9) + " " + 🦀 (U+1F980 -> D83E DD80)
        let bytes = [
            0xFE, 0xFF, 0x00, b'C', 0x00, b'a', 0x00, b'f', 0x00, 0xE9, 0x00, b' ', 0xD8, 0x3E,
            0xDD, 0x80,
        ];
        assert_eq!(decode_pdf_text_string(&bytes), "Caf\u{00E9} \u{1F980}");
    }

    #[test]
    fn decodes_utf16be_strips_language_escape() {
        // BOM + ESC "en" ESC + "Hi"
        let bytes = [
            0xFE, 0xFF, 0x00, 0x1B, 0x00, b'e', 0x00, b'n', 0x00, 0x1B, 0x00, b'H', 0x00, b'i',
        ];
        assert_eq!(decode_pdf_text_string(&bytes), "Hi");
    }

    #[test]
    fn decodes_utf8_with_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("Résumé — 日本語".as_bytes());
        assert_eq!(decode_pdf_text_string(&bytes), "Résumé — 日本語");
    }
}
