//! Injectable randomness, bit-compatible with Python's `random.Random`.
//!
//! The tests seed `Random(seed)` and assert on the resulting picks, so the
//! port keeps CPython's Mersenne Twister exactly: `init_by_array` seeding,
//! the 53-bit `random()` construction, and `choice` via `_randbelow` /
//! `getrandbits`. Callers take an `&mut dyn OutfitRng` so tests can supply a
//! fixed sequence instead.

use std::time::{SystemTime, UNIX_EPOCH};

/// The two draws the decision code needs from Python's `random.Random`.
pub trait OutfitRng: Send {
    /// `Random.random()` — uniform in `[0.0, 1.0)`.
    fn random(&mut self) -> f64;
    /// `Random.choice(seq)` as an index into a sequence of `len` items.
    /// Panics on `len == 0`, like Python's `IndexError`.
    fn choice_index(&mut self, len: usize) -> usize;
}

/// Forwarding impl so a boxed rng (the director's `Mutex<Box<dyn OutfitRng>>`)
/// can be passed wherever a `&mut dyn OutfitRng` is wanted.
impl<R: OutfitRng + ?Sized> OutfitRng for Box<R> {
    fn random(&mut self) -> f64 {
        (**self).random()
    }
    fn choice_index(&mut self, len: usize) -> usize {
        (**self).choice_index(len)
    }
}

/// CPython's Mersenne Twister (`_random.Random`), seeded the way
/// `random.Random(seed)` does for small integer seeds.
pub struct PyRandom {
    mt: [u32; N],
    index: usize,
}

const N: usize = 624;
const M: usize = 397;

impl PyRandom {
    /// `random.Random(seed)` for a non-negative integer seed.
    pub fn from_seed(seed: u64) -> Self {
        // CPython splits the seed into little-endian 32-bit words; all seeds
        // used here fit in a single word (and seed 0 is one zero word).
        Self::init_by_array(&[(seed & 0xffff_ffff) as u32])
    }

    /// `random.Random()` with entropy from the wall clock.
    pub fn from_entropy() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545_F491_4F6C_DD1D);
        Self::from_seed(nanos)
    }

    fn init_genrand(seed: u32) -> Self {
        let mut mt = [0u32; N];
        mt[0] = seed;
        for i in 1..N {
            let prev = mt[i - 1];
            mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        Self { mt, index: N }
    }

    fn init_by_array(key: &[u32]) -> Self {
        let mut this = Self::init_genrand(19_650_218);
        let key_len = key.len().max(1);
        let mut i = 1usize;
        let mut j = 0usize;
        for _ in 0..N.max(key_len) {
            let prev = this.mt[i - 1];
            let mul = (prev ^ (prev >> 30)).wrapping_mul(1_664_525);
            this.mt[i] = (this.mt[i] ^ mul)
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                this.mt[0] = this.mt[N - 1];
                i = 1;
            }
            if j >= key_len {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = this.mt[i - 1];
            let mul = (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941);
            this.mt[i] = (this.mt[i] ^ mul).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                this.mt[0] = this.mt[N - 1];
                i = 1;
            }
        }
        this.mt[0] = 0x8000_0000;
        this.index = N;
        this
    }

    /// `genrand_int32` / `genrand_res53`'s building block.
    fn genrand_u32(&mut self) -> u32 {
        if self.index >= N {
            for i in 0..N {
                let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % N] & 0x7fff_ffff);
                let mut x = y >> 1;
                if y & 1 == 1 {
                    x ^= 0x9908_b0df;
                }
                self.mt[i] = self.mt[(i + M) % N] ^ x;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `Random.getrandbits(k)` for `k <= 32`.
    fn getrandbits(&mut self, k: u32) -> u32 {
        debug_assert!((1..=32).contains(&k));
        let r = self.genrand_u32();
        if k < 32 { r >> (32 - k) } else { r }
    }
}

impl OutfitRng for PyRandom {
    /// `genrand_res53`: two 32-bit draws widened to 53 bits.
    fn random(&mut self) -> f64 {
        let a = f64::from(self.genrand_u32() >> 5);
        let b = f64::from(self.genrand_u32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `Random.choice` -> `Random._randbelow(len)` (getrandbits variant).
    fn choice_index(&mut self, len: usize) -> usize {
        assert!(len > 0, "Cannot choose from an empty sequence");
        let k = usize::BITS - len.leading_zeros(); // n.bit_length()
        loop {
            let r = self.getrandbits(k) as usize;
            if r < len {
                return r;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Well-known `random.Random(seed).random()` values from CPython - the
    /// cross-check that this MT port matches the Python sequences the tests
    /// are seeded with.
    #[test]
    fn py_random_matches_cpython_reference_values() {
        assert_eq!(PyRandom::from_seed(1).random(), 0.134_364_244_112_401_22);
        assert_eq!(PyRandom::from_seed(42).random(), 0.639_426_798_457_883_7);
        assert_eq!(PyRandom::from_seed(0).random(), 0.844_421_851_525_048_1);
    }
}
