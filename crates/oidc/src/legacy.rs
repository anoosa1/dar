//! Cookies the TypeScript stack's oidc-provider signed, so its browser sessions continue after
//! an instance moves to the Rust stack. That library signs with keygrip: a `<name>.sig` cookie
//! holding the URL-safe base64 HMAC-SHA1 of `<name>=<value>` under the provider's cookie key.
//! SHA-1 is written out here (it only checks those existing signatures; Dar's own cookies use
//! HMAC-SHA256) rather than adding a crate.

use base64::Engine;
use subtle::ConstantTimeEq;

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.iter().map(|b| b ^ byte).collect::<Vec<u8>>();
    let mut inner = pad(0x36);
    inner.extend_from_slice(data);
    let mut outer = pad(0x5c);
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// keygrip's signature of `data`: URL-safe base64 without padding.
fn keygrip(key: &[u8], data: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hmac_sha1(key, data.as_bytes()))
}

/// Whether `signature` (the `<name>.sig` cookie) signs `<name>=<value>` under `key`.
pub(crate) fn valid(key: &[u8], name: &str, value: &str, signature: &str) -> bool {
    keygrip(key, &format!("{name}={value}")).as_bytes().ct_eq(signature.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_the_standard_vectors() {
        let hex = |d: [u8; 20]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(sha1(&[b'a'; 1000])), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn keygrip_signatures_match_the_typescript_stack() {
        // node -e 'console.log(require("crypto").createHmac("sha1","dar-cookie-key").update("dar_oidc=abc").digest("base64url"))'
        assert!(valid(b"dar-cookie-key", "dar_oidc", "abc", KEYGRIP_VECTOR));
        assert!(!valid(b"dar-cookie-key", "dar_oidc", "abd", KEYGRIP_VECTOR));
        assert!(!valid(b"another-key", "dar_oidc", "abc", KEYGRIP_VECTOR));
    }

    const KEYGRIP_VECTOR: &str = "hfEn094-TrLJk8SJLEaJ229x2U0";
}
