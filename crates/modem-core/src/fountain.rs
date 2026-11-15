//! A fountain code for one-way channels: a file of `k` symbols becomes as
//! many symbols as the sender cares to send, and any `k` of them, whichever
//! arrived, give the file back (nearly always; one or two more always).
//!
//! The code is systematic: symbols `0..k` are the pieces of the file, so on a
//! clean channel nothing has to be solved. Every further symbol is a random
//! combination of all pieces over GF(256), its coefficients drawn from a
//! generator seeded with its number. The receiver solves the equations as
//! they come (Gaussian elimination): the work grows as k², which a few
//! hundred symbols keep small. See `docs/protocol.md`.

/// The polynomial of GF(256): x⁸ + x⁴ + x³ + x² + 1, that of Reed–Solomon codes.
const POLYNOMIAL: u16 = 0x11D;

/// Powers of 2 in GF(256), twice over so that sums of logarithms need no
/// reduction, and the logarithms.
const TABLES: ([u8; 512], [u8; 256]) = {
    let (mut exp, mut log) = ([0u8; 512], [0u8; 256]);
    let mut value: u16 = 1;
    let mut power = 0;
    while power < 255 {
        exp[power] = value as u8;
        exp[power + 255] = value as u8;
        log[value as usize] = power as u8;
        value <<= 1;
        if value & 0x100 != 0 {
            value ^= POLYNOMIAL;
        }
        power += 1;
    }
    (exp, log)
};

fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let (exp, log) = &TABLES;
    exp[usize::from(log[usize::from(a)]) + usize::from(log[usize::from(b)])]
}

fn inverse(a: u8) -> u8 {
    debug_assert_ne!(a, 0);
    let (exp, log) = &TABLES;
    exp[255 - usize::from(log[usize::from(a)])]
}

/// `target += factor · source`, byte by byte (addition in GF(256) is XOR).
fn add_scaled(target: &mut [u8], source: &[u8], factor: u8) {
    let row: [u8; 256] = std::array::from_fn(|x| mul(factor, x as u8));
    for (t, &s) in target.iter_mut().zip(source) {
        *t ^= row[usize::from(s)];
    }
}

/// Pieces of `T` bytes that `len` bytes take (one at least).
pub fn symbols_for(len: usize, size: usize) -> usize {
    len.div_ceil(size).max(1)
}

/// The coefficients of symbol `id` over the `k` pieces: the piece itself
/// below `k`, else the bytes of splitmix64 seeded with `id`, the least
/// significant first.
fn coefficients(id: u32, k: usize) -> Vec<u8> {
    if (id as usize) < k {
        let mut unit = vec![0; k];
        unit[id as usize] = 1;
        return unit;
    }
    let mut state = u64::from(id);
    let mut out = Vec::with_capacity(k + 7);
    while out.len() < k {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.extend(z.to_le_bytes());
    }
    out.truncate(k);
    out
}

/// The symbols of a file.
pub struct Encoder {
    size: usize,
    /// The file padded with zeros to whole pieces.
    pieces: Vec<u8>,
}

impl Encoder {
    /// `size`: bytes of a symbol, one at least.
    pub fn new(data: &[u8], size: usize) -> Self {
        let mut pieces = data.to_vec();
        pieces.resize(symbols_for(data.len(), size) * size, 0);
        Self { size, pieces }
    }

    /// Bytes of a symbol.
    pub fn symbol_bytes(&self) -> usize {
        self.size
    }

    /// `k`: how many symbols give the file back.
    pub fn pieces(&self) -> usize {
        self.pieces.len() / self.size
    }

    /// Symbol number `id`.
    pub fn symbol(&self, id: u32) -> Vec<u8> {
        let k = self.pieces();
        let mut symbol = vec![0; self.size];
        for (piece, factor) in self.pieces.chunks_exact(self.size).zip(coefficients(id, k)) {
            if factor != 0 {
                add_scaled(&mut symbol, piece, factor);
            }
        }
        symbol
    }
}

/// One equation: coefficients over the pieces and the bytes they give.
struct Row {
    coefficients: Vec<u8>,
    bytes: Vec<u8>,
}

impl Row {
    fn subtract(&mut self, other: &Row, factor: u8) {
        add_scaled(&mut self.coefficients, &other.coefficients, factor);
        add_scaled(&mut self.bytes, &other.bytes, factor);
    }

    fn scale(&mut self, factor: u8) {
        for value in self.coefficients.iter_mut().chain(&mut self.bytes) {
            *value = mul(*value, factor);
        }
    }
}

/// Collects symbols and solves for the pieces.
pub struct Decoder {
    size: usize,
    /// For each piece, the equation that has 1 there and 0 at every other
    /// piece that has an equation (reduced row echelon form).
    rows: Vec<Option<Row>>,
    rank: usize,
}

impl Decoder {
    /// For a file of `k` pieces of `size` bytes.
    pub fn new(k: usize, size: usize) -> Self {
        Self {
            size,
            rows: (0..k).map(|_| None).collect(),
            rank: 0,
        }
    }

    /// Takes symbol `id`; false when it told nothing new (or is not a symbol of this size).
    pub fn add(&mut self, id: u32, bytes: &[u8]) -> bool {
        let k = self.rows.len();
        if self.rank == k || bytes.len() != self.size {
            return false;
        }
        let mut row = Row {
            coefficients: coefficients(id, k),
            bytes: bytes.to_vec(),
        };
        for piece in 0..k {
            let factor = row.coefficients[piece];
            if factor != 0
                && let Some(known) = &self.rows[piece]
            {
                row.subtract(known, factor);
            }
        }
        let Some(piece) = row.coefficients.iter().position(|&c| c != 0) else {
            return false;
        };
        row.scale(inverse(row.coefficients[piece]));
        for other in self.rows.iter_mut().flatten() {
            let factor = other.coefficients[piece];
            if factor != 0 {
                other.subtract(&row, factor);
            }
        }
        self.rows[piece] = Some(row);
        self.rank += 1;
        true
    }

    /// Independent symbols so far; the file is there at `k`.
    pub fn rank(&self) -> usize {
        self.rank
    }

    /// The pieces, once every one is known: the file and the zeros after it.
    pub fn decoded(&self) -> Option<Vec<u8>> {
        if self.rank < self.rows.len() {
            return None;
        }
        Some(
            self.rows
                .iter()
                .flatten()
                .flat_map(|row| row.bytes.iter().copied())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + i / 7) as u8).collect()
    }

    /// A random number below `n`.
    fn below(seed: &mut u32, n: u32) -> u32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) % n
    }

    #[test]
    fn arithmetic_of_gf_256() {
        for a in 1..=255u8 {
            assert_eq!(mul(a, inverse(a)), 1, "{a}");
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(a, 0), 0);
        }
        // x · x⁷ = x⁸ = x⁴ + x³ + x² + 1
        assert_eq!(mul(2, 0x80), 0x1D);
        for (a, b, c) in [(3u8, 7u8, 200u8), (91, 13, 255), (128, 128, 2)] {
            assert_eq!(mul(a, b ^ c), mul(a, b) ^ mul(a, c));
            assert_eq!(mul(a, mul(b, c)), mul(mul(a, b), c));
        }
    }

    #[test]
    fn the_first_symbols_are_the_pieces_of_the_file() {
        let file = data(100);
        let encoder = Encoder::new(&file, 30);
        assert_eq!(encoder.pieces(), 4);
        assert_eq!(encoder.symbol(0), file[..30]);
        assert_eq!(encoder.symbol(3)[..10], file[90..]);
        assert_eq!(encoder.symbol(3)[10..], [0; 20]);
        let mut decoder = Decoder::new(4, 30);
        for id in 0..4 {
            assert!(decoder.add(id, &encoder.symbol(id)));
        }
        assert_eq!(decoder.decoded().unwrap()[..100], file);
    }

    #[test]
    fn the_rest_are_combinations_of_all_of_them() {
        let encoder = Encoder::new(&data(100), 30);
        let repair = encoder.symbol(4);
        assert_eq!(repair.len(), 30);
        let mut expected = vec![0; 30];
        for (piece, factor) in coefficients(4, 4).into_iter().enumerate() {
            add_scaled(&mut expected, &encoder.symbol(piece as u32), factor);
        }
        assert_eq!(repair, expected);
        // the coefficients are those of splitmix64 seeded with the number
        assert_eq!(coefficients(4, 3), [0xCA, 0x8A, 0x33]);
    }

    #[test]
    fn any_k_symbols_nearly_always_give_the_file_back() {
        let file = data(2000);
        let encoder = Encoder::new(&file, 50);
        let k = encoder.pieces();
        let mut seed = 9;
        let mut extra = Vec::new();
        for _ in 0..200 {
            // a third of the symbols are lost on the way
            let mut decoder = Decoder::new(k, 50);
            let (mut id, mut received) = (0, 0);
            while decoder.decoded().is_none() {
                if below(&mut seed, 3) > 0 {
                    decoder.add(id, &encoder.symbol(id));
                    received += 1;
                }
                id += 1;
            }
            assert_eq!(decoder.decoded().unwrap()[..2000], file);
            extra.push(received - k);
        }
        // a symbol more than the pieces now and then, never three
        let exact = extra.iter().filter(|&&e| e == 0).count();
        assert!(exact >= 190, "{extra:?}");
        assert!(extra.iter().all(|&e| e <= 2), "{extra:?}");
    }

    #[test]
    fn repair_symbols_alone_will_do() {
        let file = data(300);
        let encoder = Encoder::new(&file, 16);
        let k = encoder.pieces();
        let mut decoder = Decoder::new(k, 16);
        let mut id = k as u32 + 1000;
        while decoder.decoded().is_none() {
            decoder.add(id, &encoder.symbol(id));
            id += 1;
        }
        assert!(id as usize - 1000 - k <= k + 2);
        assert_eq!(decoder.decoded().unwrap()[..300], file);
    }

    #[test]
    fn a_symbol_that_tells_nothing_new_is_said_so() {
        let encoder = Encoder::new(&data(64), 16);
        let mut decoder = Decoder::new(4, 16);
        assert!(decoder.add(1, &encoder.symbol(1)));
        assert!(!decoder.add(1, &encoder.symbol(1)));
        // of the wrong size
        assert!(!decoder.add(2, &[0; 15]));
        assert_eq!(decoder.rank(), 1);
        for id in [0, 2, 3] {
            assert!(decoder.add(id, &encoder.symbol(id)));
        }
        // nothing more is taken once the file is there
        assert!(!decoder.add(9, &encoder.symbol(9)));
        assert_eq!(decoder.decoded().unwrap(), data(64));
    }

    #[test]
    fn a_file_of_one_piece_and_an_empty_one() {
        for len in [0, 1, 7] {
            let file = data(len);
            let encoder = Encoder::new(&file, 8);
            assert_eq!(encoder.pieces(), 1);
            let mut decoder = Decoder::new(1, 8);
            // a repair symbol of one piece is the piece times a number
            decoder.add(5, &encoder.symbol(5));
            assert_eq!(decoder.decoded().unwrap()[..len], file);
        }
    }
}
