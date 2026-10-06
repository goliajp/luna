//! The per-`Vm` cache of compiled chunks, keyed by a hash of the proto's
//! bytecode and the inputs the lowerer reads.

use super::*;

/// cross-`Vm` JIT cache. Look up the proto by a hash of its
/// bytecode + structural ABI fields; on miss, compile through
/// `try_compile_int_chunk` and store the result. Compiled mmap
/// pages live in the cache's `JITModule` so they outlast any single
/// `Vm`. Returns the 7-tuple `(entry_raw, num_args, returns_one,
/// arg_float_mask, arg_table_mask, ret_is_float, ret_is_table)` on
/// success (whether served from cache or freshly compiled), or
/// `None` when the proto's body falls outside the cumulative
/// whitelist.
///
/// `pre53` distinguishes dialects whose `ForPrep` / `ForLoop`
/// use the pre-5.3 `R[A] -= step + jmp` form (Lua 5.1 / 5.2 / 5.3)
/// from the 5.4+ count form (Lua 5.4 / 5.5). The same source loaded
/// in dialects on opposite sides of that split needs distinct
/// native code; this bit partitions the cache. For chunks that
/// don't touch `for` loops the bit is still hashed — same-source
/// 5.5 vs 5.5 still share; same-source 5.5 vs 5.1 don't.
///
/// `arg_table_mask` is the per-arg `Gc<Table>` indicator and
/// `ret_is_table` is true ↔ Return1 yields a `Gc<Table>` ptr.
pub fn cache_lookup_or_compile(
    storage: &mut dyn luna_core::jit::JitStorage,
    proto: luna_core::runtime::Gc<Proto>,
    pre53: bool,
    float_only: bool,
) -> Option<(*const u8, u8, bool, u8, u8, bool, bool)> {
    let key = proto_cache_key(&proto, pre53, float_only);
    // cache lookups read from the per-`Vm` `storage.cache` field.
    //
    // `from_storage` returns `Result`; on
    // `StorageMismatch` (Vm.jit.storage isn't a CraneliftJitStorage)
    // skip JIT entirely. The dispatcher already treats `None` as
    // "this Proto stays on interp", so graceful skip = no JIT for
    // this Vm, no SIGABRT across any C-ABI boundary.
    let cs = storage::from_storage(storage).ok()?;
    let cached = cs.cache.get(&key).copied();
    if let Some(hit) = cached {
        return match hit {
            CacheEntry::Failed => None,
            CacheEntry::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            } => Some((
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            )),
        };
    }
    if let Some((entry, m)) = chunk_share::adopt(cs, &proto, pre53, float_only) {
        cs.cache.insert(
            key,
            CacheEntry::Compiled {
                entry,
                num_args: m.num_args,
                returns_one: m.returns_one,
                arg_float_mask: m.arg_float_mask,
                arg_table_mask: m.arg_table_mask,
                ret_is_float: m.ret_is_float,
                ret_is_table: m.ret_is_table,
            },
        );
        return Some((
            entry,
            m.num_args,
            m.returns_one,
            m.arg_float_mask,
            m.arg_table_mask,
            m.ret_is_float,
            m.ret_is_table,
        ));
    }
    let capture = cs.engine.is_some();
    let entry = match chunk_share::compile(proto, pre53, float_only, capture) {
        Some((handle, image)) => {
            if let Some(make) = image {
                chunk_share::publish(cs, &proto, pre53, float_only, make);
            }
            let raw = handle.entry_raw();
            let num_args = handle.num_args();
            let returns_one = handle.returns_one();
            let arg_float_mask = handle.arg_float_mask();
            let arg_table_mask = handle.arg_table_mask();
            let ret_is_float = handle.ret_is_float();
            let ret_is_table = handle.ret_is_table();
            // the JITModule the
            // handle owns holds the mmap. Park the handle on the
            // per-`Vm` storage so the entry_raw pointer stays valid
            // for the lifetime of this `Vm`. Append-only.
            //
            // `from_storage` is `Result`-shaped. The `.ok()?` short-circuit above already verified
            // the storage was a `CraneliftJitStorage`, so on a sane
            // call this branch is unreachable. Guard with `match`
            // for honesty: on the impossible Err arm the compiled
            // `handle` drops (its `JITModule` releases the mmap) and
            // we return None — no leaked code page, no crash.
            match storage::from_storage(storage) {
                Ok(cs) => cs.cache_handles.push(handle),
                Err(_) => return None,
            }
            CacheEntry::Compiled {
                entry: raw,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            }
        }
        None => CacheEntry::Failed,
    };
    // same `from_storage` is-Result rationale as
    // above; on the impossible Err branch we drop the freshly built
    // `entry` (it was `Copy`, no resource loss) and skip the cache
    // insert.
    storage::from_storage(storage)
        .ok()?
        .cache
        .insert(key, entry);
    match entry {
        CacheEntry::Failed => None,
        CacheEntry::Compiled {
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        } => Some((
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        )),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CacheEntry {
    Failed,
    Compiled {
        entry: *const u8,
        num_args: u8,
        returns_one: bool,
        arg_float_mask: u8,
        arg_table_mask: u8,
        ret_is_float: bool,
        ret_is_table: bool,
    },
}

/// Introspection (test-only): number of *Compiled* entries in
/// the given Vm's JIT cache (Failed cache slots are excluded so test
/// assertions over "compiled exactly once" don't drift when the
/// outer chunk's bail also occupies a slot).
///
/// Takes `&Vm` since the cache is per-`Vm`. Public so integration
/// tests (external binaries, not cfg(test) from this crate's POV) can
/// probe per-`Vm` cache size without a downcast. Harmless utility for
/// any embedder.
pub fn cache_entry_count(vm: &luna_core::vm::Vm) -> usize {
    let storage = vm.jit.storage.as_ref().as_any();
    let cs = storage
        .downcast_ref::<storage::CraneliftJitStorage>()
        .expect("vm storage not CraneliftJitStorage");
    cs.cache
        .values()
        .filter(|e| matches!(e, CacheEntry::Compiled { .. }))
        .count()
}

/// Functions of the method JIT the Vm installed from its engine (see
/// [`crate::Engine`]) instead of compiling them.
pub fn chunk_adopted_count(vm: &luna_core::vm::Vm) -> u64 {
    vm.jit
        .storage
        .as_ref()
        .as_any()
        .downcast_ref::<storage::CraneliftJitStorage>()
        .map_or(0, |cs| cs.chunks_adopted)
}

/// Introspection (test-only): empty the Vm's JIT cache. Used
/// between tests that want to measure first-compile vs cache-hit
/// behaviour in isolation.
///
/// Takes `&mut Vm` since the cache is per-`Vm`. Public for the same
/// reason as [`cache_entry_count`].
pub fn cache_clear(vm: &mut luna_core::vm::Vm) {
    let storage = vm.jit.storage.as_mut().as_any_mut();
    if let Some(cs) = storage.downcast_mut::<storage::CraneliftJitStorage>() {
        // the handles stay: functions already compiled keep calling
        // their code until the Vm drops
        cs.cache.clear();
    }
}

/// Stable cache key. The `proto.code` bytes + `num_params` +
/// `upvals.len()` + `max_stack` + every `consts[i]` that the lowerer
/// might read + the `pre53` dialect bit cover every input the
/// lowerer reads; two protos with identical bytecode AND identical
/// constants AND matching dialect share native code.
///
/// Constants are hashed because two protos with identical
/// `LoadK k0 + Return1` shape but different `consts[0]` values
/// (e.g. `return 1+0.5` → Float(1.5) vs `return 0/0` → Float(NaN))
/// would otherwise collide and the second chunk would return the
/// first's compiled constant.
///
/// The dialect bit is hashed because a `for i = 1, N do … end` chunk
/// compiles to a different shape in Lua 5.3 (pre-decrement + jmp
/// form) vs Lua 5.4/5.5 (count form). Mixing them in one cache
/// slot would either crash or compute the wrong sum.
pub(super) fn proto_cache_key(proto: &Proto, pre53: bool, float_only: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for inst in proto.code.iter() {
        inst.0.hash(&mut h);
    }
    for c in proto.consts.iter() {
        match c {
            luna_core::runtime::Value::Int(i) => {
                0u8.hash(&mut h);
                i.hash(&mut h);
            }
            luna_core::runtime::Value::Float(f) => {
                1u8.hash(&mut h);
                f.to_bits().hash(&mut h);
            }
            // string consts participate in the cache key via
            // their byte contents, not just their discriminant.
            // Two protos with identical bytecode but different
            // `GetField` k-operand strings (e.g. `math.sin` vs
            // `math.cos` — `GetField a=5 b=5 c=2` in both, but
            // `consts[2]` resolves to "sin" or "cos") would otherwise
            // collide and serve the wrong libm call from cache.
            luna_core::runtime::Value::Str(s) => {
                3u8.hash(&mut h);
                s.as_bytes().hash(&mut h);
            }
            // Other non-Int/Float consts still hash by discriminant so
            // unrelated protos stay distinct without paying for full
            // structural hashing of types we never inspect.
            other => {
                2u8.hash(&mut h);
                std::mem::discriminant(other).hash(&mut h);
            }
        }
    }
    proto.num_params.hash(&mut h);
    proto.upvals.len().hash(&mut h);
    proto.max_stack.hash(&mut h);
    pre53.hash(&mut h);
    float_only.hash(&mut h);
    h.finish()
}
