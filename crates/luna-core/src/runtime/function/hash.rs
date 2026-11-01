//! A prototype's stable hash, which names it across processes.

use super::Proto;
use crate::runtime::fnv::FnvHash128;
use crate::runtime::value::Value;

impl Proto {
    /// Stable 128-bit hash over a
    /// Proto's identity-defining bytes. Two `Proto`s whose Lua source +
    /// dialect compile to the same bytecode hash to the same digest;
    /// distinct sources hash distinct. The digest is stable across
    /// `dump` / `undump` round-trips and across separate process runs,
    /// so an AOT pipeline (which fingerprints protos at compile time)
    /// and the deploy `Vm` (which fingerprints the same protos after
    /// undumping the embedded bytecode) agree on which `(Proto, pc)`
    /// site a precompiled trace targets.
    ///
    /// # What's fed into the hash
    ///
    /// - `code`: the raw u32 packed words, in order.
    /// - `consts`: per entry, a one-byte discriminant + payload bytes
    ///   (Int/Float as raw 8-byte LE; Str as `[len_u32_le | bytes]`;
    ///   Nil/Bool as discriminant alone). Heap-pointer variants in
    ///   `Value` (Table / Closure / Native / Coro / Userdata /
    ///   LightUserdata) never appear in a Proto's constant table —
    ///   constants are restricted to nil / bool / number / string by
    ///   the Lua compiler — so a `debug_assert!` catches the contract
    ///   if a future refactor changes that.
    /// - `upvals`: per descriptor, `in_stack` byte + `index` byte +
    ///   `read_only` byte + name bytes (length-prefixed u32 LE).
    /// - `num_params`, `is_vararg`, `max_stack`: single-byte each.
    ///
    /// # What's NOT fed in
    ///
    /// - Nested `protos`: each nested Proto has its own `stable_hash`;
    ///   parent identity is determined by its own immediate bytes only.
    ///   Callers that need a "whole tree" identity should hash the
    ///   roots they care about.
    /// - `lines`, `locvars`, `source`, `line_defined`,
    ///   `last_line_defined`: debug metadata. A `.lua` source edited
    ///   to add a comment shouldn't invalidate AOT traces — bytecode
    ///   is the identity, not the editor cursor.
    /// - JIT cache fields (`jit`, `traces`, `trace_hot_count`, …),
    ///   `cache`, `has_vararg_table_pseudo`, `has_compat_vararg_arg`,
    ///   `env_upval_idx`: runtime-only state derived from the
    ///   load-bearing fields above.
    ///
    /// # Algorithm
    ///
    /// Hand-rolled FNV-1a-128 (no third-party deps — `luna-core` 0-dep
    /// contract is hard). The standard 128-bit constants:
    ///
    /// - offset basis = `0x6c62272e07bb014262b821756295c58d`
    /// - prime        = `0x0000000001000000000000000000013b`
    ///
    /// Collision resistance suffices for AOT proto ID — collisions
    /// would manifest as a precompiled trace dispatched against the
    /// wrong Proto, but the dispatcher's existing guards (entry_tags
    /// match, head_pc match, register types match) would deopt to
    /// interp on a mismatch rather than corrupt state.
    pub fn stable_hash(&self) -> [u8; 16] {
        let mut h = FnvHash128::new();
        // 1. Bytecode words — `Inst` is `repr(transparent)` over u32;
        //    feed the raw little-endian bytes so the hash matches
        //    cross-platform (luna only targets little-endian platforms
        //    today, but the explicit LE serialization future-proofs).
        for inst in self.code.iter() {
            h.update(&inst.0.to_le_bytes());
        }
        // 2. Constants — discriminant + payload. Keep the discriminant
        //    byte values stable: bumping the `Value` enum order would
        //    invalidate AOT cache files, but that's the same constraint
        //    as `Value::tag_byte` already imposes.
        for c in self.consts.iter() {
            match c {
                Value::Nil => h.update(&[0u8]),
                Value::Bool(b) => {
                    h.update(&[1u8, *b as u8]);
                }
                Value::Int(i) => {
                    h.update(&[2u8]);
                    h.update(&i.to_le_bytes());
                }
                Value::Float(f) => {
                    // Hash the bit pattern so +0.0 / -0.0 don't
                    // collide and NaNs are stable across runs.
                    h.update(&[3u8]);
                    h.update(&f.to_bits().to_le_bytes());
                }
                Value::Str(s) => {
                    h.update(&[4u8]);
                    let bytes = s.as_bytes();
                    h.update(&(bytes.len() as u32).to_le_bytes());
                    h.update(bytes);
                }
                // Heap-pointer constants are not produced by the Lua
                // compiler. A debug_assert keeps the contract honest
                // without paying a runtime cost in release.
                Value::Table(_)
                | Value::Closure(_)
                | Value::Native(_)
                | Value::Coro(_)
                | Value::Userdata(_)
                | Value::LightUserdata(_) => {
                    debug_assert!(
                        false,
                        "Proto::stable_hash: unexpected heap-pointer constant \
                         (kind={}); luna's compiler only emits nil/bool/number/string \
                         constants",
                        c.type_name()
                    );
                    // Fall-through default: treat as a NUL byte. Won't
                    // happen in practice (compiler invariant), so the
                    // exact behaviour doesn't matter.
                    h.update(&[255u8]);
                }
            }
        }
        // 3. Upvalue descriptors — `name` bytes affect debug.getinfo
        //    only, but they're cheap and bytecode-equivalent compiles
        //    always produce equal names, so include them.
        for u in self.upvals.iter() {
            h.update(&[u.in_stack as u8, u.index, u.read_only as u8]);
            let name_bytes = u.name.as_bytes();
            h.update(&(name_bytes.len() as u32).to_le_bytes());
            h.update(name_bytes);
        }
        // 4. Signature bytes.
        h.update(&[self.num_params, self.is_vararg as u8, self.max_stack]);
        h.finish()
    }
}
