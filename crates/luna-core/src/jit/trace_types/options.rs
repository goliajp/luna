//! The options the trace lowerer compiles with.

use super::*;

/// Compile-time options for the trace lowerer.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    /// When `true`, the trace's clean-close path emits a back-edge
    /// jump to its own body-loop block instead of returning
    /// `head_pc` to the caller — so the JIT'd code runs in a tight
    /// native loop until a cmp side-exit fires. The dispatcher's
    /// per-entry marshal cost amortizes across however many
    /// iterations the trace runs before diverging.
    ///
    /// Internal-loop traces require at least one exit edge
    /// (`Lt / Le / Eq` cmp or `Op::ForLoop`) AND no `Op::Call`
    /// truncation — otherwise the trace would run forever. The
    /// lowerer auto-downgrades to one-shot when neither condition
    /// holds, so callers can safely set this `true` for any
    /// record.
    ///
    /// Defaults to `false` in `try_compile_trace` (one-shot, the
    /// shape unit tests assume) and `true` in
    /// `try_compile_trace_with_options` when callers explicitly
    /// want the dispatcher fast path.
    pub internal_loop: bool,
    /// Lua dialect — `true` for 5.1 / 5.2 / 5.3, `false` for
    /// 5.4 / 5.5. The numeric `for` op (`Op::ForLoop`) has a
    /// different layout pre-5.3 (the slot at `R[A+1]` is the raw
    /// `limit` Value, not a remaining-iteration count). Only the
    /// 5.4+ Int count form is lowered; pre-5.3 traces bail
    /// and stay on the interp side.
    pub pre53: bool,
    /// Emit AOT-relocatable IR.
    ///
    /// When `false` (the JIT default), interned-string-key arguments
    /// to `luna_jit_*_field` helpers are baked as
    /// `iconst(I64, key_str.as_ptr() as i64)` — the live runtime
    /// pointer, valid because the lowered mcode runs in the same
    /// process / `Vm` / `StringTable` as the recorder.
    ///
    /// When `true` (the AOT path), the lowerer instead routes each
    /// key through a writable data slot named
    /// `__luna_aot_strkey_slot_<hex>` (8 bytes, zero-initialised at
    /// link time) and emits a load through that symbol. A sibling
    /// read-only object `__luna_aot_strkey_bytes_<hex>` carries the
    /// UTF-8 bytes; the deploy-side startup hook walks every
    /// `_bytes_<hex>` symbol, interns the bytes into the deploy
    /// `Vm`'s `StringTable`, and writes the resulting
    /// `Gc<LuaStr>::as_ptr()` into the matching `_slot_<hex>` before
    /// any AOT trace dispatches. JIT path is unaffected — same
    /// `iconst` it always emitted.
    pub aot: bool,
    /// Which code generator compiles the trace.
    pub tier: TraceTier,
    /// With [`TraceTier::Auto`]: the loop iterations plus entries after
    /// which the trace is compiled again by the optimizing tier (`0`:
    /// never).
    pub tier_up_at: u32,
    /// The dialect of the Vm the trace runs in, when known: inline table
    /// code then follows only that dialect's length rules (5.4 keeps a
    /// length limit array indexing can move, 5.5 a length hint). `None`
    /// compiles code that serves every dialect.
    pub dialect: Option<crate::version::LuaVersion>,
}
