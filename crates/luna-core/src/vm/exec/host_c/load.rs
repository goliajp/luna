//! Loading and dumping chunks, the collector's controls and warnings,
//! for the C API.

use super::*;
use crate::vm::lib_gc::{self, PARAM_ORDER, Param};

/// Why a C API load failed: a memory error (`LUA_ERRMEM`) or another one
/// (`LUA_ERRSYNTAX`), with the error object.
#[doc(hidden)]
pub enum HostLoadError {
    /// the memory error
    Memory(Value),
    /// a syntax or binary chunk error
    Syntax(Value),
}

impl Vm {
    /// Whether `prefix`, the start of a binary chunk a `lua_load` reader
    /// delivered, already holds every byte PUC's undumper reads: PUC stops
    /// calling the reader there.
    pub(crate) fn host_binary_chunk_complete(&mut self, prefix: &[u8]) -> bool {
        !crate::vm::dump::truncated(
            prefix,
            &mut self.heap,
            self.version,
            self.puc_bytecode_loading,
        )
    }

    /// Compile or undump `src` as the chunk `chunkname` (PUC `f_parser`):
    /// the function and the number of upvalues its prototype declares, or
    /// the error.
    pub(crate) fn host_load_chunk(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError> {
        match self.load_named(src, chunkname, None) {
            Ok(cl) => {
                let n = cl.proto.upvals.len();
                Ok((cl, n))
            }
            Err(e) => Err(self.host_load_error(&e, chunkname)),
        }
    }

    /// The error a load of `chunkname` failed with: the memory error, or
    /// the message of any other.
    fn host_load_error(
        &mut self,
        e: &crate::frontend::error::SyntaxError,
        chunkname: &[u8],
    ) -> HostLoadError {
        if e.is_memory() {
            HostLoadError::Memory(self.mem_err().0)
        } else {
            HostLoadError::Syntax(self.load_error_value(e, chunkname))
        }
    }

    /// `Vm::host_load_chunk` for a text chunk a reader handed over
    /// while it was parsed (see `Vm::text_load`).
    pub(crate) fn host_load_parsed(
        &mut self,
        parsed: crate::vm::exec::ParsedText,
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError> {
        match self.load_parsed(parsed, chunkname, None) {
            Ok(cl) => {
                let n = cl.proto.upvals.len();
                Ok((cl, n))
            }
            Err(e) => Err(self.host_load_error(&e, chunkname)),
        }
    }

    /// Set the first upvalue of a function `lua_load` just made, which no
    /// other code holds yet, to `v` (PUC `lua_load`: the globals), in the
    /// upvalue the load made. Never fails.
    pub(crate) fn host_set_first_upvalue(
        &mut self,
        cl: Gc<LuaClosure>,
        v: Value,
    ) -> Result<(), LuaError> {
        if let Some(&uv) = cl.upvals().first() {
            // SAFETY: a load makes each upvalue of its function closed and
            // its own, and only the caller holds the function; the borrow
            // covers one store
            unsafe { uv.as_mut() }.set_closed(v);
            self.heap.barrier_forward(uv, v);
        }
        Ok(())
    }

    /// The binary chunk `lua_dump` writes for `f`: PUC bytecode of the
    /// dialect, as `string.dump` writes it, or luna's own format for a
    /// function the dialect's instruction set cannot hold (as one block),
    /// with the size of each block PUC's dumper writes on its own; `None`
    /// for a value that is not a Lua function.
    pub(crate) fn host_dump(&self, f: Value, strip: bool) -> Option<(Vec<u8>, Vec<usize>)> {
        let Value::Closure(cl) = f else {
            return None;
        };
        let v = self.version;
        if !v.is_macro_lua()
            && let Ok(b) = crate::vm::dump::dump_puc_blocks(&cl.proto, strip, v)
        {
            return Some(b);
        }
        let b = crate::vm::dump::dump(&cl.proto, strip, v);
        let n = b.len();
        Some((b, vec![n]))
    }

    /// Start the collector in incremental mode, as a state made by
    /// `lua_newstate` is (the stand-alone interpreter switches 5.4 and 5.5
    /// to generational mode, which is a fresh Vm's mode).
    pub(crate) fn host_gc_start_incremental(&mut self) {
        self.gc_switch_mode("incremental");
    }

    /// Turn the default warning function on (5.5's `luaL_newstate`).
    pub(crate) fn host_warn_on(&mut self) {
        self.warn_state = WarnState::On;
    }

    /// PUC `lua_warning`: one piece of a warning, to the warning function.
    pub(crate) fn host_warning(&mut self, msg: &[u8], to_cont: bool) -> Result<(), LuaError> {
        self.emit_warn(msg, to_cont)
    }

    /// PUC `lua_gc` with the option `what` of the dialect's numbering and
    /// its arguments (`args`: those the option takes, in order, the rest
    /// zero), mapped onto luna's collector the way `collectgarbage` is.
    /// 5.1 to 5.3 raise a finalizer's error from a full collection.
    pub(crate) fn host_gc(&mut self, what: i32, args: [i64; 3]) -> Result<i32, LuaError> {
        use LuaVersion::*;
        let v = self.version;
        // 5.4+ refuse every option while a finalizer runs
        if v >= Lua54 && self.gc_finalizing {
            return Ok(-1);
        }
        let op = match (v, what) {
            (_, 0..=5) => what,
            (Lua51 | Lua52 | Lua53 | Lua54 | MacroLua, 6) => GC_SETPAUSE,
            (Lua51 | Lua52 | Lua53 | Lua54 | MacroLua, 7) => GC_SETSTEPMUL,
            (Lua52, 8) => GC_SETMAJORINC,
            (Lua52 | Lua53 | Lua54 | MacroLua, 9) | (Lua55, 6) => GC_ISRUNNING,
            (Lua52 | Lua54 | MacroLua, 10) | (Lua55, 7) => GC_GEN,
            (Lua52 | Lua54 | MacroLua, 11) | (Lua55, 8) => GC_INC,
            (Lua55, 9) => GC_PARAM,
            _ => return Ok(-1),
        };
        let a = args[0] as i32;
        Ok(match op {
            0 => {
                self.heap.gc_set_stopped(true);
                0
            }
            1 => {
                self.heap.gc_set_stopped(false);
                0
            }
            2 => {
                if v <= Lua53 {
                    self.collect_garbage_propagating()?;
                } else {
                    self.collect_garbage();
                }
                0
            }
            3 => (self.gc_count_bytes() >> 10) as i32,
            4 => (self.gc_count_bytes() & 0x3ff) as i32,
            5 => {
                // 5.5 takes a `size_t`; one that does not fit a signed
                // count is a basic step
                let n = if v >= Lua55 {
                    args[0].max(0)
                } else {
                    i64::from(a)
                };
                i32::from(lib_gc::step(self, n)?)
            }
            GC_SETPAUSE => self.host_gc_set(Param::Pause, a),
            GC_SETSTEPMUL => self.host_gc_set(Param::StepMul, a),
            GC_SETMAJORINC => self.host_gc_set(Param::MajorInc, a),
            GC_ISRUNNING => i32::from(!self.heap.gc_is_stopped() && !self.gc_finalizing),
            GC_GEN | GC_INC => {
                let mode = if op == GC_GEN {
                    "generational"
                } else {
                    "incremental"
                };
                if matches!(v, Lua54 | MacroLua) {
                    let vals = args.map(|x| x as i32);
                    lib_gc::set_mode_params(self, mode, &vals);
                }
                let prev = self.gc_switch_mode(mode);
                let (gen_code, inc_code) = if v >= Lua55 { (7, 8) } else { (10, 11) };
                match (v, prev) {
                    (Lua52, _) => 0,
                    (_, "generational") => gen_code,
                    _ => inc_code,
                }
            }
            _ => {
                let Some(&param) = usize::try_from(a).ok().and_then(|i| PARAM_ORDER.get(i)) else {
                    return Ok(-1);
                };
                let prev = self.heap.gc_params().get(param) as i32;
                let value = args[1] as i32;
                if value >= 0 {
                    self.heap.gc_params_mut().set(param, value);
                    lib_gc::sync_pacing(self);
                }
                prev
            }
        })
    }

    /// Set a pacing parameter from `lua_gc` and return its previous value.
    fn host_gc_set(&mut self, param: Param, v: i32) -> i32 {
        let prev = self.heap.gc_params().get(param) as i32;
        self.heap.gc_params_mut().set(param, v);
        lib_gc::sync_pacing(self);
        prev
    }
}

// `host_gc`'s options past the five every dialect numbers alike
const GC_SETPAUSE: i32 = 100;
const GC_SETSTEPMUL: i32 = 101;
const GC_SETMAJORINC: i32 = 102;
const GC_ISRUNNING: i32 = 103;
const GC_GEN: i32 = 104;
const GC_INC: i32 = 105;
const GC_PARAM: i32 = 106;
