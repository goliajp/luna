//! Taking up code a backend finished compiling in the background.

/// Puts the code a backend finished compiling in the background (see
/// `Proto::jit_next`) in place of `proto`'s entry, runnable on this thread
/// ([`crate::jit::code_fence`]). Whether it did.
#[inline]
pub(super) fn take_jit_next(proto: &crate::runtime::function::Proto) -> bool {
    use crate::runtime::function::JitProtoState;
    let Some(next) = proto.jit_next.take() else {
        return false;
    };
    let ready = next.load(std::sync::atomic::Ordering::Acquire);
    if ready == 0 {
        proto.jit_next.set(Some(next));
        return false;
    }
    if ready == crate::runtime::function::JIT_NEXT_NONE {
        return false;
    }
    crate::jit::code_fence();
    if let JitProtoState::Compiled {
        num_args,
        returns_one,
        arg_float_mask,
        arg_table_mask,
        ret_is_float,
        ret_is_table,
        ..
    } = proto.jit.get()
    {
        proto.jit.set(JitProtoState::Compiled {
            entry: ready as *const u8,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        });
    }
    true
}
