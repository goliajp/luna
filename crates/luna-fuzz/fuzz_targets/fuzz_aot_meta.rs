//! v2.0 Track CV fuzz harness — AOT trace meta blob decoder.
//!
//! Feeds arbitrary bytes into `luna_core::jit::aot_meta::decode_meta_blob`.
//! The decoder reads bytes from `.luna_trace_meta` section in AOT-built
//! binaries; corrupted / adversarial bytes there must NOT panic the
//! deploy walker — the contract is `Err(reason)` + skip-and-log.
//!
//! Run:
//!     cargo +nightly fuzz run fuzz_aot_meta

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::jit::aot_meta;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    // decode_meta_blob: Result<DecodedMeta, &'static str>. Both arms
    // are valid outcomes. A panic / OOB / signed-overflow trap is the
    // real bug we are hunting.
    let _ = aot_meta::decode_meta_blob(data);

    // Companion: per-byte unpack functions. These should never panic
    // regardless of input byte value. unpack_exit_tag / unpack_tag_res_kind
    // are pure u8 -> Option<Tag> mappers but exposed here so any future
    // additions stay covered.
    for &b in data.iter() {
        let _ = aot_meta::unpack_exit_tag(b);
        let _ = aot_meta::unpack_tag_res_kind(b);
    }
});
