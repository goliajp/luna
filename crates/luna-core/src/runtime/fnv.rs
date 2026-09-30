//! FNV-1a-128, for fingerprinting protos without a hash crate.

/// Hand-rolled FNV-1a-128 state.
/// Used by [`crate::runtime::function::Proto::stable_hash`] to fingerprint a Proto without
/// pulling a third-party hash crate (`luna-core` 0-dep contract).
///
/// FNV-1a is not cryptographic; collision-resistance suffices for the
/// AOT proto-ID use case because a collision would surface as a
/// trace-vs-proto mismatch and the dispatcher's existing tag/shape
/// guards would deopt to interp rather than corrupt state.
pub(crate) struct FnvHash128 {
    state: u128,
}

impl FnvHash128 {
    /// Standard FNV-1a-128 offset basis.
    const OFFSET_BASIS: u128 = 0x6c62272e07bb014262b821756295c58d;
    /// Standard FNV-1a-128 prime.
    const PRIME: u128 = 0x0000000001000000000000000000013b;

    pub(crate) fn new() -> Self {
        FnvHash128 {
            state: Self::OFFSET_BASIS,
        }
    }

    /// Absorb `bytes` into the running hash. FNV-1a: per byte, XOR
    /// into the low octet of state, then multiply by the prime (wrap).
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        let mut s = self.state;
        for &b in bytes {
            s ^= b as u128;
            s = s.wrapping_mul(Self::PRIME);
        }
        self.state = s;
    }

    /// Finalise to 16 big-endian bytes (network order — stable across
    /// platforms; the LE/BE choice is cosmetic since the only consumer
    /// is byte-equality, but BE matches the canonical FNV-1a-128
    /// reference output if anyone cross-checks).
    pub(crate) fn finish(self) -> [u8; 16] {
        self.state.to_be_bytes()
    }
}
