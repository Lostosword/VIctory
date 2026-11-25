//! A fast 128-bit content fingerprint, for telling whether a block of
//! memory changed since it was last looked at (texture memory on hosts
//! that cannot track writes). Not a cryptographic hash: nothing here
//! defends against chosen inputs, only against a changed block passing for
//! the same one, which needs a 1 in 2^64 accident per lane pair.
//!
//! Four independent lanes of 64-bit multiply-rotate mixing over 32-byte
//! blocks: plain integer code with no vector instructions, so it runs at
//! memory speed under a binary translator too (a vectorised cryptographic
//! hash ran at 0.7 GB/s under Rosetta 2, less than a tenth of this). Every
//! step of a lane is a bijection of the lane for a given input word, so
//! two blocks that differ in a single word never collide.

const PRIMES: [u64; 4] = [
    0x9e37_79b1_85eb_ca87,
    0xc2b2_ae3d_27d4_eb4f,
    0x1656_67b1_9e37_79f9,
    0x85eb_ca77_c2b2_ae63,
];

#[inline(always)]
fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

#[inline(always)]
fn round(lane: u64, input: u64, prime: u64) -> u64 {
    (lane ^ input).wrapping_mul(prime).rotate_left(29)
}

fn avalanche(mut x: u64) -> u64 {
    x ^= x >> 32;
    x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
    x ^= x >> 32;
    x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
    x ^ x >> 32
}

/// The fingerprint of `bytes`.
pub fn fingerprint(bytes: &[u8]) -> [u64; 2] {
    let mut lanes = [PRIMES[1], PRIMES[2], PRIMES[3], PRIMES[0]];
    let mut blocks = bytes.chunks_exact(32);
    for block in &mut blocks {
        lanes[0] = round(lanes[0], word(block), PRIMES[0]);
        lanes[1] = round(lanes[1], word(&block[8..]), PRIMES[1]);
        lanes[2] = round(lanes[2], word(&block[16..]), PRIMES[2]);
        lanes[3] = round(lanes[3], word(&block[24..]), PRIMES[3]);
    }
    // The tail (under 32 bytes), zero-padded; the length tells paddings apart.
    let rest = blocks.remainder();
    let mut tail = [0u8; 32];
    tail[..rest.len()].copy_from_slice(rest);
    for (i, lane) in lanes.iter_mut().enumerate() {
        *lane = round(*lane, word(&tail[8 * i..]), PRIMES[i]);
    }
    let len = bytes.len() as u64;
    // Each half takes every lane, through different bijections.
    let a = avalanche(lanes[0] ^ len)
        .wrapping_add(avalanche(lanes[1].rotate_left(17)))
        .wrapping_add(avalanche(lanes[2] ^ PRIMES[0]).rotate_left(31))
        .wrapping_add(avalanche(lanes[3].wrapping_add(PRIMES[1])).rotate_left(47));
    let b = avalanche(lanes[0].rotate_left(23) ^ PRIMES[2])
        .wrapping_add(avalanche(lanes[1] ^ len.rotate_left(32)).rotate_left(13))
        .wrapping_add(avalanche(lanes[2].wrapping_add(PRIMES[3])).rotate_left(37))
        .wrapping_add(avalanche(lanes[3] ^ PRIMES[2]).rotate_left(53));
    [avalanche(a), avalanche(b ^ a.rotate_left(32))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, mut seed: u64) -> Vec<u8> {
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (seed >> 56) as u8
            })
            .collect()
    }

    #[test]
    fn equal_blocks_agree_and_any_single_change_shows() {
        let data = noise(4099, 1);
        let base = fingerprint(&data);
        assert_eq!(base, fingerprint(&data.clone()));
        // Every byte position, a one-bit and an all-bits change.
        for at in 0..data.len() {
            for flip in [1u8, 0x80, 0xff] {
                let mut changed = data.clone();
                changed[at] ^= flip;
                assert_ne!(fingerprint(&changed), base, "byte {at} ^ {flip:#x}");
            }
        }
    }

    #[test]
    fn length_and_padding_show() {
        // Zeros of every length up to a few blocks are all different, as
        // are a block and itself with zeros appended.
        let mut seen = std::collections::HashSet::new();
        for n in 0..200 {
            assert!(seen.insert(fingerprint(&vec![0u8; n])), "{n} zeros");
        }
        let data = noise(100, 2);
        let mut longer = data.clone();
        longer.push(0);
        assert_ne!(fingerprint(&data), fingerprint(&longer));
        assert_ne!(fingerprint(&[]), fingerprint(&[0]));
    }

    #[test]
    fn swapped_and_moved_words_show() {
        let data = noise(256, 3);
        let base = fingerprint(&data);
        // Two words of the same lane exchanged; two blocks exchanged; a
        // word moved to another lane.
        let mut a = data.clone();
        a.copy_within(32..40, 0);
        a[32..40].copy_from_slice(&data[0..8]);
        assert_ne!(fingerprint(&a), base);
        let mut b = data.clone();
        b.copy_within(64..96, 0);
        b[64..96].copy_from_slice(&data[0..32]);
        assert_ne!(fingerprint(&b), base);
        let mut c = data.clone();
        c.copy_within(8..16, 0);
        c[8..16].copy_from_slice(&data[0..8]);
        assert_ne!(fingerprint(&c), base);
    }

    #[test]
    fn output_bits_spread() {
        // One input bit flipped changes about half the output bits.
        let data = noise(1024, 4);
        let base = fingerprint(&data);
        let (mut total, mut least) = (0u32, 128u32);
        for bit in (0..8 * data.len()).step_by(7) {
            let mut changed = data.clone();
            changed[bit / 8] ^= 1 << (bit % 8);
            let f = fingerprint(&changed);
            let differ = (f[0] ^ base[0]).count_ones() + (f[1] ^ base[1]).count_ones();
            total += differ;
            least = least.min(differ);
        }
        let n = (8 * data.len()).div_ceil(7) as u32;
        let mean = total / n;
        assert!((56..=72).contains(&mean), "mean {mean} of 128 bits");
        assert!(least >= 30, "least {least}");
    }

    #[test]
    fn many_similar_blocks_do_not_collide() {
        // Counters in each word position of a fixed block: 40,000 distinct
        // blocks, as many distinct fingerprints, in each half too.
        let data = noise(64, 5);
        let (mut whole, mut first, mut second) = (
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        );
        let mut n = 0;
        for position in 0..8 {
            for i in 0..5000u64 {
                let mut block = data.clone();
                block[8 * position..8 * position + 8].copy_from_slice(&i.to_le_bytes());
                let f = fingerprint(&block);
                whole.insert(f);
                first.insert(f[0]);
                second.insert(f[1]);
                n += 1;
            }
        }
        assert_eq!((whole.len(), first.len(), second.len()), (n, n, n));
    }
}
