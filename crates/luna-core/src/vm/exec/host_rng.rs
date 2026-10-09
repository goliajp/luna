//! The Vm's random generator (xoshiro256**, PUC 5.4's) and the clock the
//! os library reads.

use super::*;

impl Vm {
    /// xoshiro256** next.
    pub(crate) fn rng_next(&mut self) -> u64 {
        let s = &mut self.rng;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Seed the RNG via splitmix64 expansion (PUC randseed shape).
    pub(crate) fn rng_seed(&mut self, a: u64, b: u64) {
        // PUC setseed: state = [n1, 0xff, n2, 0] (0xff avoids an all-zero
        // state), then 16 discards to spread the seed. Matches PUC's exact
        // sequence so the low-level conformance test passes.
        self.rng = [a, 0xff, b, 0];
        for _ in 0..16 {
            self.rng_next();
        }
    }

    /// Wall-clock since VM creation (os.clock approximation).
    pub(crate) fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Entropy for math.randomseed() with no arguments.
    pub(crate) fn rng_auto_seed(&mut self) -> (i64, i64) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let addr = &self.rng as *const _ as u64;
        (t as i64, addr as i64)
    }
}
