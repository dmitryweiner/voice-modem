//! Messages sealed by a shared password: a station that knows it reads them
//! and can tell they were not changed on the way; any other station hears
//! only that something is said.
//!
//! The password becomes a key with Argon2id, slow and memory-hungry on
//! purpose, so that guessing passwords costs the same for everyone. Each
//! message is sealed with ChaCha20-Poly1305 under a nonce of its own: eight
//! random bytes the application gives, then a counter. The salt is fixed —
//! both stations must come to the same key from the password alone — so a
//! password is only as good as it is long. See `docs/protocol.md`.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// Bytes a sealed message has more than the message: the nonce and the tag.
pub const SEAL_OVERHEAD: usize = NONCE_BYTES + TAG_BYTES;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
/// Argon2id: 64 MiB and three passes, the second choice of RFC 9106, in
/// one lane: WebAssembly runs on one thread. The salt is the same for every
/// station, so whatever guessing costs, it costs once for all of them.
const MEMORY_KIB: u32 = 64 * 1024;
const PASSES: u32 = 3;
const SALT: &[u8] = b"voice-modem/password/v0";

/// The key of a password.
fn key(password: &str) -> Key {
    let params = Params::new(MEMORY_KIB, PASSES, 1, Some(32)).expect("valid parameters");
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), SALT, &mut key)
        .expect("a 32-byte key from any password");
    Key::from(key)
}

pub struct Seal {
    cipher: ChaCha20Poly1305,
    /// The first eight bytes of every nonce; random, from the application.
    prefix: u64,
    counter: u32,
}

impl Seal {
    /// The seal of a password. `entropy` must be random — the same password
    /// with the same entropy twice would use nonces twice. Takes a fraction
    /// of a second: that is what makes guessing slow.
    pub fn new(password: &str, entropy: u64) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(&key(password)),
            prefix: entropy,
            counter: 0,
        }
    }

    /// The message sealed: the nonce, the message encrypted, the tag. `sender`
    /// is the address of the station that sends it, sealed in as well.
    pub fn seal(&mut self, sender: u16, message: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_BYTES];
        nonce[..8].copy_from_slice(&self.prefix.to_be_bytes());
        nonce[8..].copy_from_slice(&self.counter.to_be_bytes());
        self.counter = self.counter.wrapping_add(1);
        if self.counter == 0 {
            // Four billion messages: on with other first bytes (splitmix64).
            let mut z = self.prefix.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            self.prefix = z ^ (z >> 31);
        }
        let sealed = self
            .cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: message,
                    aad: &sender.to_be_bytes(),
                },
            )
            .expect("a message of any length a frame takes");
        [&nonce[..], &sealed].concat()
    }

    /// The message, if `sealed` was sealed by `sender` with this password
    /// and nothing in it changed.
    pub fn open(&self, sender: u16, sealed: &[u8]) -> Option<Vec<u8>> {
        let (nonce, rest) = sealed.split_first_chunk::<NONCE_BYTES>()?;
        self.cipher
            .decrypt(
                &Nonce::from(*nonce),
                Payload {
                    msg: rest,
                    aad: &sender.to_be_bytes(),
                },
            )
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_station_with_the_same_password_opens_what_another_sealed() {
        let mut alice = Seal::new("correct horse battery staple", 1);
        let bob = Seal::new("correct horse battery staple", 2);
        let sealed = alice.seal(7, b"meet at the bridge");
        assert_eq!(sealed.len(), 18 + SEAL_OVERHEAD);
        assert_ne!(
            &sealed[NONCE_BYTES..NONCE_BYTES + 18],
            b"meet at the bridge"
        );
        assert_eq!(
            bob.open(7, &sealed).as_deref(),
            Some(&b"meet at the bridge"[..])
        );
        assert_eq!(bob.open(7, &alice.seal(7, b"")).as_deref(), Some(&[][..]));
    }

    #[test]
    fn nothing_opens_with_another_password_or_after_a_change() {
        let mut alice = Seal::new("password one", 1);
        let sealed = alice.seal(7, b"meet at the bridge");
        assert_eq!(Seal::new("password two", 1).open(7, &sealed), None);
        let opener = Seal::new("password one", 3);
        // any bit changed: in the nonce, the text or the tag
        for bit in [0, 100, 8 * sealed.len() - 1] {
            let mut changed = sealed.clone();
            changed[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(opener.open(7, &changed), None, "bit {bit}");
        }
        // passed off as another station's
        assert_eq!(opener.open(8, &sealed), None);
        // too short to hold a nonce and a tag
        assert_eq!(opener.open(7, &sealed[..NONCE_BYTES + 3]), None);
        assert_eq!(opener.open(7, &[]), None);
    }

    #[test]
    fn every_message_has_a_nonce_of_its_own() {
        let mut first = Seal::new("p", 0x0102_0304_0506_0708);
        let a = first.seal(1, b"same");
        let b = first.seal(1, b"same");
        assert_eq!(a[..12], [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0]);
        assert_eq!(b[..12], [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 1]);
        assert_ne!(a, b);
        // another session: other random bytes
        let mut second = Seal::new("p", 99);
        assert_ne!(second.seal(1, b"same")[..8], a[..8]);
        // after four billion messages the first bytes change
        first.counter = u32::MAX;
        let last = first.seal(1, b"x");
        let next = first.seal(1, b"x");
        assert_eq!(last[..8], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_ne!(next[..8], last[..8]);
        assert_eq!(next[8..12], [0, 0, 0, 0]);
    }
}
