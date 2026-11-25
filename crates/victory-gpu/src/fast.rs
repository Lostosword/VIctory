//! A hasher for tables keyed by our own pointers, addresses and sizes: a
//! multiply and a rotate for each word. The standard one resists made-up
//! keys, which these never are, and the recording threads ask such tables
//! some hundred thousand times a frame.

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub(crate) struct Fast(u64);

impl Fast {
    fn word(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

impl Hasher for Fast {
    fn write(&mut self, bytes: &[u8]) {
        let mut words = bytes.chunks_exact(8);
        for w in &mut words {
            self.word(u64::from_le_bytes(w.try_into().unwrap_or_default()));
        }
        let rest = words.remainder();
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last) ^ (rest.len() as u64) << 56);
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.word(u64::from(n));
    }
    fn write_u32(&mut self, n: u32) {
        self.word(u64::from(n));
    }
    fn write_u64(&mut self, n: u64) {
        self.word(n);
    }
    fn write_usize(&mut self, n: usize) {
        self.word(n as u64);
    }
    fn finish(&self) -> u64 {
        // The tables take the low bits for the place and the high ones for
        // the tag: both must depend on every word.
        self.0 ^ (self.0 >> 29)
    }
}

pub(crate) type FastMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<Fast>>;
pub(crate) type FastSet<K> = std::collections::HashSet<K, BuildHasherDefault<Fast>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_hold_what_was_put_in() {
        let mut map: FastMap<(u64, u32, u64), u32> = FastMap::default();
        for i in 0..10_000u64 {
            map.insert((i * 256, i as u32 & 3, i << 40), i as u32);
        }
        assert_eq!(map.len(), 10_000);
        for i in 0..10_000u64 {
            assert_eq!(map.get(&(i * 256, i as u32 & 3, i << 40)), Some(&(i as u32)));
        }
        let mut set: FastSet<[u8; 13]> = FastSet::default();
        for i in 0..255u8 {
            assert!(set.insert([i; 13]));
            assert!(!set.insert([i; 13]));
        }
    }
}
