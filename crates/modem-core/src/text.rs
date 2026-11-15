//! A text in fewer bytes than its UTF-8: a fixed Huffman code over the
//! characters of English and Russian chat, with a code of its own after a
//! Latin and after a Cyrillic letter. Any other character goes as an escape
//! and its code point. Short texts do not repeat enough for deflate; letters
//! of a known language do not need to. See `docs/protocol.md`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::OnceLock;

/// The symbol that ends a text.
const END: usize = 0;
/// The symbol before a character that has none of its own: 21 bits of its code point follow.
const ESCAPE: usize = 1;
/// Bits of an escaped code point.
const CODE_POINT_BITS: usize = 21;
/// The longest code the weights below give, bits.
const MAX_CODE_BITS: usize = 24;

/// The characters that have a code, with their weights after a Latin letter
/// and after a Cyrillic one: how many times in 100 000 characters of chat
/// in English, and of chat in Russian. Letter frequencies of the two
/// languages; a letter of the other script is rare. The code is part of the
/// wire format: a change here is a change of the protocol.
#[rustfmt::skip]
const CHARACTERS: &[(char, u32, u32)] = &[
    (' ', 17000, 15000), ('\n', 400, 400),
    ('a', 5882, 25), ('b', 929, 4), ('c', 2002, 8), ('d', 3060, 13), ('e', 9144, 38),
    ('f', 1606, 7), ('g', 1454, 6), ('h', 4385, 18), ('i', 5018, 21), ('j', 108, 1),
    ('k', 554, 2), ('l', 2902, 12), ('m', 1735, 7), ('n', 4860, 20), ('o', 5407, 23),
    ('p', 1390, 6), ('q', 72, 1), ('r', 4313, 18), ('s', 4558, 19), ('t', 6523, 27),
    ('u', 1987, 8), ('v', 706, 3), ('w', 1699, 7), ('x', 108, 1), ('y', 1418, 6),
    ('z', 50, 1),
    ('A', 200, 1), ('B', 80, 1), ('C', 100, 1), ('D', 80, 1), ('E', 60, 1), ('F', 60, 1),
    ('G', 60, 1), ('H', 120, 1), ('I', 350, 1), ('J', 30, 1), ('K', 30, 1), ('L', 60, 1),
    ('M', 100, 1), ('N', 80, 1), ('O', 100, 1), ('P', 80, 1), ('Q', 10, 1), ('R', 60, 1),
    ('S', 150, 1), ('T', 300, 1), ('U', 30, 1), ('V', 30, 1), ('W', 150, 1), ('X', 10, 1),
    ('Y', 60, 1), ('Z', 10, 1),
    ('а', 24, 6248), ('б', 5, 1240), ('в', 14, 3541), ('г', 5, 1326), ('д', 9, 2324),
    ('е', 25, 6591), ('ж', 3, 733), ('з', 5, 1287), ('и', 22, 5733), ('й', 4, 944),
    ('к', 10, 2722), ('л', 13, 3432), ('м', 10, 2504), ('н', 20, 5226), ('о', 33, 8557),
    ('п', 8, 2192), ('р', 14, 3689), ('с', 16, 4267), ('т', 19, 4883), ('у', 8, 2044),
    ('ф', 1, 203), ('х', 3, 757), ('ц', 1, 374), ('ч', 4, 1123), ('ш', 2, 569),
    ('щ', 1, 281), ('ъ', 1, 31), ('ы', 6, 1482), ('ь', 5, 1357), ('э', 1, 250),
    ('ю', 2, 499), ('я', 6, 1568), ('ё', 1, 100),
    ('А', 1, 80), ('Б', 1, 60), ('В', 1, 150), ('Г', 1, 60), ('Д', 1, 100), ('Е', 1, 30),
    ('Ж', 1, 20), ('З', 1, 40), ('И', 1, 80), ('Й', 1, 2), ('К', 1, 150), ('Л', 1, 40),
    ('М', 1, 100), ('Н', 1, 150), ('О', 1, 100), ('П', 1, 200), ('Р', 1, 60), ('С', 1, 180),
    ('Т', 1, 120), ('У', 1, 30), ('Ф', 1, 20), ('Х', 1, 30), ('Ц', 1, 10), ('Ч', 1, 50),
    ('Ш', 1, 20), ('Щ', 1, 5), ('Ъ', 1, 1), ('Ы', 1, 1), ('Ь', 1, 1), ('Э', 1, 80),
    ('Ю', 1, 10), ('Я', 1, 100), ('Ё', 1, 5),
    ('0', 150, 150), ('1', 150, 150), ('2', 120, 120), ('3', 90, 90), ('4', 80, 80),
    ('5', 90, 90), ('6', 70, 70), ('7', 70, 70), ('8', 70, 70), ('9', 80, 80),
    ('.', 900, 900), (',', 1000, 1500), ('!', 200, 250), ('?', 300, 350), ('-', 200, 300),
    (':', 150, 150), (';', 30, 30), ('\'', 250, 20), ('"', 100, 100), ('(', 50, 50),
    (')', 80, 80), ('/', 50, 50), ('+', 20, 20), ('=', 20, 20), ('@', 20, 10),
    ('#', 10, 10), ('%', 20, 20), ('&', 20, 5), ('*', 20, 20), ('_', 10, 10),
    ('<', 5, 5), ('>', 5, 5), ('[', 5, 5), (']', 5, 5), ('~', 5, 5), ('$', 10, 5),
    ('\t', 5, 5), ('—', 20, 100), ('–', 10, 30), ('…', 30, 50), ('«', 2, 50), ('»', 2, 50),
    ('’', 20, 5), ('“', 10, 5), ('”', 10, 5), ('№', 1, 10),
];
/// Weights of the end and of the escape: a message is some fifty characters,
/// and now and then has an emoji.
const END_WEIGHT: u32 = 2000;
const ESCAPE_WEIGHT: u32 = 300;

/// Which code a character is written with: that of the script of the last letter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Latin,
    Cyrillic,
}

impl Context {
    /// The script of a letter that has a code; None for anything else.
    fn of(c: char) -> Option<Context> {
        if c.is_ascii_alphabetic() {
            Some(Context::Latin)
        } else if matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё') {
            Some(Context::Cyrillic)
        } else {
            None
        }
    }

    /// The context after `c`: a letter sets it, anything else keeps it.
    fn after(self, c: char) -> Context {
        Context::of(c).unwrap_or(self)
    }
}

/// A canonical prefix code (as in deflate): codes of each length are
/// consecutive and follow the order of the symbols.
struct Code {
    /// Length of the code of each symbol, bits.
    lengths: Vec<usize>,
    /// The code of each symbol.
    codes: Vec<u32>,
    /// How many symbols have a code of each length.
    counts: [usize; MAX_CODE_BITS + 1],
    /// The symbols by the length of their code, then by number.
    sorted: Vec<usize>,
}

impl Code {
    /// Huffman's algorithm over the weights: the two lightest trees are
    /// joined first, the one made earlier first among equal weights.
    fn new(weights: &[u32]) -> Code {
        let leaves = weights.len();
        let mut parent = vec![0usize; 2 * leaves - 1];
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = weights
            .iter()
            .enumerate()
            .map(|(symbol, &weight)| Reverse((u64::from(weight), symbol)))
            .collect();
        let mut next = leaves;
        while let (Some(Reverse((a, first))), Some(Reverse((b, second)))) = (heap.pop(), heap.pop())
        {
            parent[first] = next;
            parent[second] = next;
            heap.push(Reverse((a + b, next)));
            next += 1;
        }
        let root = next - 1;
        let lengths: Vec<usize> = (0..leaves)
            .map(|symbol| {
                let (mut node, mut depth) = (symbol, 0);
                while node != root {
                    node = parent[node];
                    depth += 1;
                }
                depth
            })
            .collect();
        let mut counts = [0; MAX_CODE_BITS + 1];
        for &length in &lengths {
            counts[length] += 1;
        }
        let mut sorted: Vec<usize> = (0..leaves).collect();
        sorted.sort_by_key(|&symbol| (lengths[symbol], symbol));
        let mut codes = vec![0; leaves];
        let (mut code, mut length) = (0u32, 0);
        for &symbol in &sorted {
            code <<= lengths[symbol] - length;
            length = lengths[symbol];
            codes[symbol] = code;
            code += 1;
        }
        Code {
            lengths,
            codes,
            counts,
            sorted,
        }
    }

    /// The next symbol from the bits, or None when they end first.
    fn decode(&self, bits: &mut Bits) -> Option<usize> {
        let (mut code, mut first, mut index) = (0usize, 0usize, 0usize);
        for &count in &self.counts[1..] {
            code |= bits.next()?;
            if code - first < count {
                return Some(self.sorted[index + code - first]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// The two codes, the symbols being the end, the escape, then `CHARACTERS`.
fn codes() -> &'static [Code; 2] {
    static CODES: OnceLock<[Code; 2]> = OnceLock::new();
    CODES.get_or_init(|| {
        let weights = |pick: fn(&(char, u32, u32)) -> u32| -> Vec<u32> {
            [END_WEIGHT, ESCAPE_WEIGHT]
                .into_iter()
                .chain(CHARACTERS.iter().map(pick))
                .collect()
        };
        [
            Code::new(&weights(|&(_, latin, _)| latin)),
            Code::new(&weights(|&(_, _, cyrillic)| cyrillic)),
        ]
    })
}

fn code(context: Context) -> &'static Code {
    &codes()[usize::from(context == Context::Cyrillic)]
}

/// Bits written most significant first.
#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    used: usize,
}

impl Writer {
    fn put(&mut self, value: u32, bits: usize) {
        for bit in (0..bits).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let byte = self.bytes.last_mut().expect("a byte was pushed");
            *byte |= (((value >> bit) & 1) as u8) << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn next(&mut self) -> Option<usize> {
        let byte = self.bytes.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(usize::from(bit))
    }

    fn take(&mut self, bits: usize) -> Option<u32> {
        (0..bits).try_fold(0u32, |value, _| Some((value << 1) | self.next()? as u32))
    }
}

/// The text packed: one bit for the context of its first letter (1:
/// Cyrillic), the code of every character in the context of the letter
/// before it, the end, zero bits up to a whole byte.
pub fn pack(text: &str) -> Vec<u8> {
    let mut context = text.chars().find_map(Context::of).unwrap_or(Context::Latin);
    let mut out = Writer::default();
    out.put(u32::from(context == Context::Cyrillic), 1);
    for c in text.chars() {
        let symbol = CHARACTERS.iter().position(|&(known, ..)| known == c);
        let code = code(context);
        match symbol {
            Some(index) => out.put(code.codes[index + 2], code.lengths[index + 2]),
            None => {
                out.put(code.codes[ESCAPE], code.lengths[ESCAPE]);
                out.put(u32::from(c), CODE_POINT_BITS);
            }
        }
        context = context.after(c);
    }
    let code = code(context);
    out.put(code.codes[END], code.lengths[END]);
    out.bytes
}

/// The text back, or None when the bytes are not a packed text.
pub fn unpack(bytes: &[u8]) -> Option<String> {
    let mut bits = Bits { bytes, at: 0 };
    let mut context = if bits.next()? == 1 {
        Context::Cyrillic
    } else {
        Context::Latin
    };
    let mut text = String::new();
    loop {
        let c = match code(context).decode(&mut bits)? {
            END => return Some(text),
            ESCAPE => char::from_u32(bits.take(CODE_POINT_BITS)?)?,
            symbol => CHARACTERS[symbol - 2].0,
        };
        text.push(c);
        context = context.after(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> usize {
        let packed = pack(text);
        assert_eq!(unpack(&packed).as_deref(), Some(text));
        packed.len()
    }

    #[test]
    fn every_text_comes_back() {
        for text in [
            "",
            "hello",
            "Привет! Как слышно? Приём.",
            "Mixed: модем over the air, 62.5 бит/с — ok?",
            "emoji 👋🏽 and 漢字 and \r\n\u{0}",
            "«Ёлки», ЁЖ, ё-моё № 5…",
            "\t[x] <a href=\"#\">50% & ~$3</a> @user_name *",
        ] {
            round_trip(text);
        }
        let every: String = CHARACTERS.iter().map(|&(c, ..)| c).collect();
        round_trip(&every);
    }

    #[test]
    fn chat_in_english_and_russian_takes_far_fewer_bytes() {
        let english = "How do you hear me? I am at the window, over.";
        let russian = "Как слышно? Я у окна, приём.";
        assert!(
            round_trip(english) * 100 <= english.len() * 65,
            "{}",
            pack(english).len()
        );
        assert!(
            round_trip(russian) * 100 <= russian.len() * 45,
            "{}",
            pack(russian).len()
        );
    }

    #[test]
    fn the_first_letter_chooses_the_first_code() {
        assert_eq!(pack("Да")[0] >> 7, 1);
        assert_eq!(pack("1, 2: yes")[0] >> 7, 0);
        assert_eq!(pack("1, 2: да")[0] >> 7, 1);
    }

    #[test]
    fn the_codes_are_complete_prefix_codes() {
        for code in codes() {
            // Kraft's equality: every bit pattern begins exactly one code
            let sum: f64 = code.lengths.iter().map(|&l| 0.5f64.powi(l as i32)).sum();
            assert!((sum - 1.0).abs() < 1e-12, "{sum}");
            assert!(
                code.lengths
                    .iter()
                    .all(|&l| (1..=MAX_CODE_BITS).contains(&l))
            );
        }
        // the characters are all different
        let mut seen: Vec<char> = CHARACTERS.iter().map(|&(c, ..)| c).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CHARACTERS.len());
    }

    #[test]
    fn bytes_that_are_not_a_packed_text_give_nothing_or_a_text() {
        assert_eq!(unpack(&[]), None);
        // ends before the end symbol
        let packed = pack("a longer text");
        assert_eq!(unpack(&packed[..packed.len() / 2]), None);
        // any bytes: no panic
        let mut seed = 1u32;
        for len in 0..200 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = unpack(&bytes);
        }
    }
}
