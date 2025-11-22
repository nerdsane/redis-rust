use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub struct DeterministicRng {
    rng: ChaCha8Rng,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        DeterministicRng {
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.rng.next_u64()
    }

    pub fn gen_range(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            return min;
        }
        min + (self.next_u64() % (max - min))
    }

    pub fn gen_bool(&mut self, probability: f64) -> bool {
        let val = self.next_u64() as f64 / u64::MAX as f64;
        val < probability
    }

    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for i in (1..slice.len()).rev() {
            let j = self.gen_range(0, (i + 1) as u64) as usize;
            slice.swap(i, j);
        }
    }
}

pub fn buggify(rng: &mut DeterministicRng) -> bool {
    rng.gen_bool(0.01)
}
