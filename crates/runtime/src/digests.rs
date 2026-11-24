//! SHA-1 and MD5, streaming (RFC 3174, RFC 1321). Dar never names or trusts content by either:
//! WebDAV clients compare them (`files/checksums.ts`) and calendar subscriptions name events by
//! the SHA-1 of their UID. Written here rather than added as dependencies.

pub struct Sha1 {
    state: [u32; 5],
    buffer: Vec<u8>,
    length: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Sha1 {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            buffer: Vec::with_capacity(64),
            length: 0,
        }
    }
}

impl Sha1 {
    fn block(&mut self, chunk: &[u8]) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        if !self.buffer.is_empty() {
            let take = (64 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == 64 {
                let block = std::mem::take(&mut self.buffer);
                self.block(&block);
            }
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        self.buffer.extend_from_slice(data);
    }

    pub fn hex(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.buffer.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        let length = self.length;
        self.update(&tail);
        self.length = length;
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }
}

pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::default();
    h.update(data);
    h.hex()
}

pub struct Md5 {
    state: [u32; 4],
    buffer: Vec<u8>,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Md5 {
            state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476],
            buffer: Vec::with_capacity(64),
            length: 0,
        }
    }
}

const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn md5_k(i: usize) -> u32 {
    ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32
}

impl Md5 {
    fn block(&mut self, chunk: &[u8]) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(md5_k(i)).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(SHIFTS[i]));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        if !self.buffer.is_empty() {
            let take = (64 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == 64 {
                let block = std::mem::take(&mut self.buffer);
                self.block(&block);
            }
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        self.buffer.extend_from_slice(data);
    }

    pub fn hex(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.buffer.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        self.update(&tail);
        self.state
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let mut split = Sha1::default();
        for chunk in vec![b'a'; 1000].chunks(7) {
            split.update(chunk);
        }
        assert_eq!(split.hex(), sha1_hex(&[b'a'; 1000]));
        let md5 = |d: &[u8]| {
            let mut h = Md5::default();
            h.update(d);
            h.hex()
        };
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(md5(&[b'x'; 200]), {
            let mut h = Md5::default();
            for c in [b'x'; 200].chunks(13) {
                h.update(c);
            }
            h.hex()
        });
    }
}
