//! ChaCha20 (RFC 8439), and the random generator built on it.
//!
//! ChaCha20 turns a 256-bit key, a 32-bit block counter and a 96-bit
//! nonce into 64 bytes that look random to anyone without the key. It is
//! twenty rounds of adding, rotating and XOR-ing 32-bit words - nothing
//! a CPU needs special instructions for, which is why it is what Linux,
//! OpenBSD and others use for their random number generators.
//!
//! **The generator** (`Generator`) keeps one key and never hands it out:
//!
//! ```text
//!  stir(sample)   key = first 32 bytes of ChaCha20(key, 0, nonce = sample, count)
//!  split()        one block of ChaCha20(key, 0, nonce = pool, count):
//!                    bytes  0..32 -> the generator's next key
//!                    bytes 32..64 -> a key for this request only
//!  a request      ChaCha20(request key, 0, 1, 2, ..., nonce 0) -> the bytes asked for
//! ```
//!
//! Every request first replaces the generator's key ("fast key
//! erasure"): whoever reads the kernel's memory afterwards cannot work
//! out what was handed out before. And the key a request uses is its
//! own, so the bytes can be made with no lock held.
//!
//! What goes in (`stir`, `add`) is anything that differs from machine to
//! machine and moment to moment: the CPU's random instruction where
//! there is one, the counter's low bits at each timer tick, how long a
//! loop took. None of it has to be good on its own: ChaCha20 with the
//! old key mixes each sample in, so the key is at least as unpredictable
//! as everything that went into it.
//!
//! This is a design in the style of OpenBSD's arc4random and Linux's
//! later random.c, written to learn from. Nobody has reviewed it as a
//! cryptographer would.

/// "expand 32-byte k", the constant every ChaCha20 block starts with.
const CONSTANT: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// One ChaCha20 block: 16 words, to be written out little-endian.
pub fn block(key: &[u32; 8], counter: u32, nonce: [u32; 3]) -> [u32; 16] {
    let mut start = [0u32; 16];
    start[..4].copy_from_slice(&CONSTANT);
    start[4..12].copy_from_slice(key);
    start[12] = counter;
    start[13..].copy_from_slice(&nonce);
    let mut s = start;
    for _ in 0..10 {
        // A column round, then a diagonal round.
        quarter_round(&mut s, 0, 4, 8, 12);
        quarter_round(&mut s, 1, 5, 9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7, 8, 13);
        quarter_round(&mut s, 3, 4, 9, 14);
    }
    for (word, first) in s.iter_mut().zip(start) {
        *word = word.wrapping_add(first);
    }
    s
}

/// Fills `out` with the ChaCha20 stream of `key` (nonce 0), starting at
/// block `first`. Returns the next block's number.
pub fn stream(key: &[u32; 8], first: u32, out: &mut [u8]) -> u32 {
    let mut counter = first;
    for chunk in out.chunks_mut(64) {
        let words = block(key, counter, [0; 3]);
        for (bytes, word) in chunk.chunks_mut(4).zip(words) {
            bytes.copy_from_slice(&word.to_le_bytes()[..bytes.len()]);
        }
        counter = counter.wrapping_add(1);
    }
    counter
}

/// The generator's state. The kernel keeps one behind a lock.
pub struct Generator {
    key: [u32; 8],
    /// How many times the key was changed: a nonce word, so that no two
    /// blocks made from the same key share a nonce.
    count: u64,
    /// Samples taken in since the key last changed: what they are
    /// matters only to the next change.
    samples: u64,
}

impl Generator {
    pub const fn new() -> Generator {
        Generator { key: [0; 8], count: 0, samples: 0 }
    }

    /// Mixes `sample` into the key at once.
    pub fn stir(&mut self, sample: u64) {
        let words = block(&self.key, 0, [sample as u32, (sample >> 32) as u32, self.count as u32]);
        self.key.copy_from_slice(&words[..8]);
        self.count = self.count.wrapping_add(1);
        self.samples += 1;
    }

    /// How many samples have gone in so far.
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// A key for one request, after mixing in `pool` (whatever was
    /// gathered since the last request); the generator's own key is
    /// replaced in the same step.
    pub fn split(&mut self, pool: u64) -> [u32; 8] {
        let words = block(&self.key, 0, [pool as u32, (pool >> 32) as u32, self.count as u32 ^ 0x8000_0000]);
        self.key.copy_from_slice(&words[..8]);
        self.count = self.count.wrapping_add(1);
        let mut request = [0u32; 8];
        request.copy_from_slice(&words[8..]);
        request
    }
}

impl Default for Generator {
    fn default() -> Generator {
        Generator::new()
    }
}

/// Mixes `sample` into a pool word cheaply (a rotate and an XOR): what
/// an interrupt can afford. The pool is taken into the key at the next
/// request.
pub const fn pool_add(pool: u64, sample: u64) -> u64 {
    pool.rotate_left(13) ^ sample
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// RFC 8439, section 2.3.2: key 00 01 .. 1f, counter 1, nonce
    /// 00 00 00 09 00 00 00 4a 00 00 00 00.
    #[test]
    fn rfc_8439_block() {
        let key: [u32; 8] = core::array::from_fn(|i| u32::from_le_bytes([4 * i as u8, 4 * i as u8 + 1, 4 * i as u8 + 2, 4 * i as u8 + 3]));
        let nonce = [0x0900_0000, 0x4a00_0000, 0];
        let expected = hex(
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e",
        );
        assert_eq!(bytes(&block(&key, 1, nonce)), expected);
    }

    /// RFC 8439, appendix A.1, test vector 1: the all-zero key, counter
    /// 0, nonce 0.
    #[test]
    fn rfc_8439_zero_key() {
        let expected = hex(
            "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7\
             da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586",
        );
        assert_eq!(bytes(&block(&[0; 8], 0, [0; 3])), expected);
    }

    #[test]
    fn stream_is_blocks_in_a_row_and_stops_mid_block() {
        let key = [7u32; 8];
        let mut out = [0u8; 150];
        assert_eq!(stream(&key, 5, &mut out), 8);
        let mut expected = Vec::new();
        for counter in 5..8 {
            expected.extend(bytes(&block(&key, counter, [0; 3])));
        }
        assert_eq!(&out[..], &expected[..150]);
    }

    #[test]
    fn every_split_gives_a_new_key_and_changes_the_generator() {
        let mut g = Generator::new();
        g.stir(42);
        let a = g.split(0);
        let b = g.split(0);
        assert_ne!(a, b, "the same pool twice still gives two keys");
        let mut h = Generator::new();
        h.stir(43);
        assert_ne!(h.split(0), a, "a different sample, a different key");
    }

    #[test]
    fn the_same_samples_give_the_same_keys() {
        // Deterministic: everything unpredictable comes from the samples.
        let (mut g, mut h) = (Generator::new(), Generator::new());
        for sample in [1, 2, 3] {
            g.stir(sample);
            h.stir(sample);
        }
        assert_eq!(g.split(9), h.split(9));
        assert_eq!(g.samples(), 3);
    }

    #[test]
    fn output_bits_are_balanced() {
        // Not a proof of anything; a check that nothing is stuck.
        let mut g = Generator::new();
        g.stir(0x1234_5678_9abc_def0);
        let key = g.split(0);
        let mut out = vec![0u8; 1 << 16];
        stream(&key, 0, &mut out);
        let ones: u32 = out.iter().map(|b| b.count_ones()).sum();
        let bits = out.len() as f64 * 8.0;
        assert!((ones as f64 / bits - 0.5).abs() < 0.005, "{ones} of {bits}");
    }

    #[test]
    fn pool_add_keeps_every_sample() {
        assert_ne!(pool_add(pool_add(0, 1), 2), pool_add(pool_add(0, 2), 1));
    }
}
