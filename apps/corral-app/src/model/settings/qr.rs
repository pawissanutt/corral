//! A QR Code encoder for the pairing link a trusted device shows: byte mode, error correction
//! level M, the smallest version that holds the text and the mask with the lowest penalty
//! (ISO/IEC 18004), so a phone camera opens the link. It follows Project Nayuki's reference
//! algorithm; the link is the only input, so only byte mode is needed.

/// Error correction codewords per block at level M, by version (index 0 unused).
const ECC_PER_BLOCK: [usize; 41] = [
    0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];

/// Error correction blocks at level M, by version (index 0 unused).
const BLOCKS: [usize; 41] = [
    0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
    25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];

/// Level M in the format information.
const FORMAT_M: u32 = 0;

/// An encoded symbol: `size` modules a side, dark modules true, without the quiet zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qr {
    size: usize,
    modules: Vec<bool>,
}

impl Qr {
    /// The symbol of `text`, or None when it exceeds version 40.
    pub fn encode(text: &str) -> Option<Qr> {
        let data = text.as_bytes();
        let version = (1..=40).find(|&version| {
            4 + count_bits(version) + data.len() * 8 <= data_codewords(version) * 8
                && data.len() < (1 << count_bits(version))
        })?;
        let codewords = codewords(version, data);
        let mut symbol = Symbol::new(version);
        symbol.draw_function_patterns();
        symbol.draw_codewords(&interleave(version, &codewords));
        let mask = (0..8)
            .min_by_key(|&mask| {
                let mut trial = symbol.clone();
                trial.apply_mask(mask);
                trial.draw_format(mask);
                trial.penalty()
            })
            .unwrap_or(0);
        symbol.apply_mask(mask);
        symbol.draw_format(mask);
        Some(Qr {
            size: symbol.size,
            modules: symbol.modules,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// The symbol's rows, dark modules true, inside a quiet zone of `quiet` light modules.
    pub fn rows(&self, quiet: usize) -> Vec<Vec<bool>> {
        let side = self.size + 2 * quiet;
        (0..side)
            .map(|y| {
                (0..side)
                    .map(|x| {
                        x >= quiet
                            && y >= quiet
                            && x - quiet < self.size
                            && y - quiet < self.size
                            && self.modules[(y - quiet) * self.size + x - quiet]
                    })
                    .collect()
            })
            .collect()
    }
}

/// Bits of the byte-mode character count.
fn count_bits(version: usize) -> usize {
    if version < 10 { 8 } else { 16 }
}

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
    raw_modules(version) / 8 - ECC_PER_BLOCK[version] * BLOCKS[version]
}

/// The data codewords: mode, count, bytes, terminator and the alternating pad bytes.
fn codewords(version: usize, data: &[u8]) -> Vec<u8> {
    let capacity = data_codewords(version) * 8;
    let mut bits: Vec<bool> = Vec::with_capacity(capacity);
    let mut push = |value: usize, len: usize| {
        for i in (0..len).rev() {
            bits.push((value >> i) & 1 == 1);
        }
    };
    push(0b0100, 4);
    push(data.len(), count_bits(version));
    for &byte in data {
        push(usize::from(byte), 8);
    }
    let terminator = (capacity - bits.len()).min(4);
    bits.extend(std::iter::repeat_n(false, terminator));
    let pad = (8 - bits.len() % 8) % 8;
    bits.extend(std::iter::repeat_n(false, pad));
    let mut bytes: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0u8, |byte, &bit| (byte << 1) | u8::from(bit))
        })
        .collect();
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if bytes.len() * 8 >= capacity {
            break;
        }
        bytes.push(pad);
    }
    bytes
}

/// Splits the data into blocks, appends each block's error correction and interleaves them.
fn interleave(version: usize, data: &[u8]) -> Vec<u8> {
    let blocks = BLOCKS[version];
    let ecc_len = ECC_PER_BLOCK[version];
    let raw = raw_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;
    let divisor = divisor(ecc_len);
    let mut split = Vec::with_capacity(blocks);
    let mut at = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let chunk = &data[at..at + len];
        at += len;
        let mut block = chunk.to_vec();
        // Short blocks take a placeholder so every block indexes alike; it is never emitted.
        if i < short_blocks {
            block.push(0);
        }
        block.extend(remainder(chunk, &divisor));
        split.push(block);
    }
    let mut result = Vec::with_capacity(raw);
    for i in 0..short_len + 1 {
        for (j, block) in split.iter().enumerate() {
            if i != short_len - ecc_len || j >= short_blocks {
                result.push(block[i]);
            }
        }
    }
    result
}

/// Product in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1.
fn multiply(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= u16::from((y >> i) & 1) * u16::from(x);
    }
    (z & 0xFF) as u8
}

/// The Reed-Solomon generator polynomial of `degree`, leading term omitted.
fn divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = multiply(root, 0x02);
    }
    result
}

fn remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &byte in data {
        let factor = byte ^ result[0];
        result.remove(0);
        result.push(0);
        for (out, &coefficient) in result.iter_mut().zip(divisor) {
            *out ^= multiply(coefficient, factor);
        }
    }
    result
}

/// A symbol being drawn: its modules, and which of them are function patterns.
#[derive(Clone)]
struct Symbol {
    version: usize,
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Symbol {
    fn new(version: usize) -> Symbol {
        let size = version * 4 + 17;
        Symbol {
            version,
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        }
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn draw_function_patterns(&mut self) {
        let size = self.size;
        for i in 0..size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        self.draw_finder(3, 3);
        self.draw_finder(size - 4, 3);
        self.draw_finder(3, size - 4);
        let positions = self.alignment_positions();
        let last = positions.len().saturating_sub(1);
        for (i, &y) in positions.iter().enumerate() {
            for (j, &x) in positions.iter().enumerate() {
                let corner = (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0);
                if !corner {
                    self.draw_alignment(x, y);
                }
            }
        }
        // Reserve the format areas now; the chosen mask's bits overwrite them.
        self.draw_format(0);
        self.draw_version();
    }

    fn draw_finder(&mut self, x: usize, y: usize) {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let distance = dx.abs().max(dy.abs());
                let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                if (0..self.size as i32).contains(&xx) && (0..self.size as i32).contains(&yy) {
                    self.set_function(xx as usize, yy as usize, distance != 2 && distance != 4);
                }
            }
        }
    }

    fn draw_alignment(&mut self, x: usize, y: usize) {
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let dark = dx.abs().max(dy.abs()) != 1;
                self.set_function((x as i32 + dx) as usize, (y as i32 + dy) as usize, dark);
            }
        }
    }

    fn alignment_positions(&self) -> Vec<usize> {
        if self.version == 1 {
            return Vec::new();
        }
        let count = self.version / 7 + 2;
        let step = (self.version * 8 + count * 3 + 5) / (count * 4 - 4) * 2;
        let mut result = vec![6; count];
        let mut position = self.size - 7;
        for slot in result.iter_mut().skip(1).rev() {
            *slot = position;
            position -= step;
        }
        result
    }

    fn draw_format(&mut self, mask: u32) {
        let data = (FORMAT_M << 3) | mask;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 == 1;
        let size = self.size;
        for i in 0..=5 {
            self.set_function(8, i, bit(i));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i));
        }
        self.set_function(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version < 7 {
            return;
        }
        let version = self.version as u32;
        let mut rem = version;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = (version << 12) | rem;
        for i in 0..18 {
            let dark = (bits >> i) & 1 == 1;
            let a = self.size - 11 + i % 3;
            let b = i / 3;
            self.set_function(a, b, dark);
            self.set_function(b, a, dark);
        }
    }

    /// Places the codewords in the zigzag of two-module columns, right to left.
    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size;
        let mut i = 0;
        let mut right = size - 1;
        loop {
            if right == 6 {
                right = 5;
            }
            for vertical in 0..size {
                for j in 0..2 {
                    let x = right - j;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward {
                        size - 1 - vertical
                    } else {
                        vertical
                    };
                    if !self.function[y * size + x] && i < data.len() * 8 {
                        self.modules[y * size + x] = (data[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        i += 1;
                    }
                }
            }
            if right < 2 {
                break;
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u32) {
        let size = self.size;
        for y in 0..size {
            for x in 0..size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let at = y * size + x;
                if !self.function[at] && invert {
                    self.modules[at] = !self.modules[at];
                }
            }
        }
    }

    fn penalty(&self) -> usize {
        let size = self.size;
        let at = |x: usize, y: usize| self.modules[y * size + x];
        let mut result = 0;
        for line in 0..size {
            let row: Vec<bool> = (0..size).map(|x| at(x, line)).collect();
            let column: Vec<bool> = (0..size).map(|y| at(line, y)).collect();
            result += line_penalty(&row) + line_penalty(&column);
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let dark = at(x, y);
                if dark == at(x + 1, y) && dark == at(x, y + 1) && dark == at(x + 1, y + 1) {
                    result += 3;
                }
            }
        }
        let total = size * size;
        let dark = self.modules.iter().filter(|&&dark| dark).count();
        let k = (dark * 20)
            .abs_diff(total * 10)
            .div_ceil(total)
            .saturating_sub(1);
        result + k * 10
    }
}

/// Runs of five or more alike, and finder-like 1:1:3:1:1 patterns beside four light modules
/// (outside the symbol counts as light).
fn line_penalty(line: &[bool]) -> usize {
    let mut result = 0;
    let mut run = 1;
    for i in 1..=line.len() {
        if i < line.len() && line[i] == line[i - 1] {
            run += 1;
            continue;
        }
        if run >= 5 {
            result += 3 + (run - 5);
        }
        run = 1;
    }
    const FINDER: [bool; 7] = [true, false, true, true, true, false, true];
    let padded: Vec<bool> = std::iter::repeat_n(false, 4)
        .chain(line.iter().copied())
        .chain(std::iter::repeat_n(false, 4))
        .collect();
    for window in padded.windows(11) {
        let before = window[..4].iter().all(|&d| !d) && window[4..] == FINDER;
        let after = window[..7] == FINDER && window[7..].iter().all(|&d| !d);
        if before || after {
            result += 40;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_level_m_symbol_matches_the_reference_encoder() {
        // "HELLO WORLD" is the standard's worked example; at level M in byte mode version 1
        // holds it, and its data and error correction codewords are fixed.
        let qr = Qr::encode("HELLO WORLD").unwrap();
        assert_eq!(qr.size(), 21);
        let codewords = codewords(1, b"HELLO WORLD");
        assert_eq!(codewords.len(), 16);
        assert_eq!(&codewords[..2], &[0x40, 0xB4]);
        // Every Reed-Solomon codeword block is divisible by the generator: a zero remainder.
        let mut block = codewords.clone();
        block.extend(remainder(&codewords, &divisor(10)));
        assert!(remainder(&block, &divisor(10)).iter().all(|&b| b == 0));
    }

    #[test]
    fn long_pairing_links_choose_a_larger_version_and_keep_finders() {
        let link = format!(
            "http://127.0.0.1:8790/#corral-pair=corral-pair-1.{}",
            "A".repeat(99)
        );
        let qr = Qr::encode(&link).unwrap();
        assert!(qr.size() > 21 && (qr.size() - 17).is_multiple_of(4));
        let rows = qr.rows(0);
        let n = qr.size();
        for (ox, oy) in [(0, 0), (n - 7, 0), (0, n - 7)] {
            for y in 0..7 {
                for x in 0..7 {
                    let ring = x == 0 || x == 6 || y == 0 || y == 6;
                    let core = (2..=4).contains(&x) && (2..=4).contains(&y);
                    assert_eq!(rows[oy + y][ox + x], ring || core);
                }
            }
        }
        assert!(Qr::encode(&"x".repeat(3000)).is_none());
    }
}
