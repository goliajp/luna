//! The checks of the counts against the encoders, which only debug,
//! gc-trace and gc-verify builds make.

use super::*;

/// Debug, gc-trace and gc-verify builds check what the encoder emitted for
/// `p` against [`Layout::width`]; the product build does nothing.
#[inline(always)]
pub(in crate::vm::dump::puc_writer) fn hook_encoded(
    p: &Proto,
    d: Dialect,
    body: &super::super::asm::Body,
) {
    #[cfg(any(debug_assertions, feature = "gc-trace", feature = "gc-verify"))]
    super::check::check(p, d, body);
    let _ = (p, d, body);
}

/// Debug, gc-trace and gc-verify builds check that the encoder adds no
/// constant to code luna compiled; the product build does nothing.
#[inline(always)]
pub(crate) fn hook_compiled(proto: &Proto, version: LuaVersion) {
    #[cfg(any(debug_assertions, feature = "gc-trace", feature = "gc-verify"))]
    super::check::check_compiled(proto, version);
    let _ = (proto, version);
}
