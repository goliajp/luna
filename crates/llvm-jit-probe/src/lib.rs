//! v2.1 Phase 1K.B — LLVM JIT toolchain validation harness.
//!
//! This crate is a standalone, `publish = false` validation harness for
//! the LLVM 18 + `inkwell` JIT toolchain selected by Phase 1K.A
//! (see `.dev/rfcs/v2.1-phase-1k-a-llvm-jit-selection.md`). Its sole
//! purpose is to prove the toolchain links and JIT-compiles a trivial
//! `add(i64, i64) -> i64` IR on the dev host before Phase 1K.C touches
//! any production luna code.
//!
//! The crate intentionally has no dependency on `luna-core`, `luna-jit`,
//! or `luna-aot`. The 0-third-party-dep contract on `luna-core` is
//! unaffected.

/// Phase 1K.B.2 scaffold marker. Replaced by real JIT entry points in
/// Phase 1K.B.4.
pub fn version() -> &'static str {
    "llvm-jit-probe v2.1 Phase 1K.B scaffold"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_version_string() {
        assert!(version().contains("1K.B"));
    }
}
