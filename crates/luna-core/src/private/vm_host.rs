//! What luna's own crates (the C API, the CLI, tests) ask of a Vm
//! beyond its documented interface.

use crate::jit::CompiledTrace;
use crate::runtime::ErrorStatus;
use crate::runtime::LuaClosure;
use crate::runtime::Value;
use crate::runtime::heap::Gc;
use crate::vm::LuaError;
use crate::vm::exec::SpecialErrors;
use crate::vm::exec::TextLoad;

/// The internals of `Vm` luna's other crates use.
pub trait VmHostInternals {
    /// `luaL_traceback(L, L, msg, level)` (5.2 on) as the standalone
    /// interpreter's message handler calls it: the string PUC makes.
    fn traceback_string(&mut self, msg: Option<&[u8]>, level: i64) -> Result<Value, LuaError>;
    /// lua.c's message for an error object that is no string (5.2 on):
    /// `lua_pushfstring(L, "(error object is a %s value)", luaL_typename(L, 1))`,
    /// made as each dialect's `lua_pushfstring` makes it.
    fn error_object_message(&mut self, e: Value) -> Result<Gc<crate::runtime::LuaStr>, LuaError>;

    /// How many errors have taken a status of their own so far; see
    /// `Vm::error_status`.
    fn special_errors(&self) -> SpecialErrors;
    /// The status of error `e`, raised after `before` was taken: an error
    /// the vm raised itself with a status of its own carries that status's
    /// message, so a message the program raised with the same text, and no
    /// such error raised meanwhile, stays `Run`.
    fn error_status(&self, e: Value, before: SpecialErrors) -> ErrorStatus;

    fn upval_get(&self, cl: Gc<LuaClosure>, idx: u32) -> Value;
    /// Write a mark into the accounting trace (`gc-trace`), where a test
    /// cuts the comparison with PUC's.
    fn trace_mark(&mut self, name: &str);
    /// `Vm::load_file` of standard input holding `src`, for a test that
    /// runs a script as `lua -` does.
    fn load_stdin_text(&mut self, src: Vec<u8>) -> Result<Value, LuaError>;
    /// `Vm::call_value_with_handler` made from inside a C function of the
    /// host's, as lua.c's `docall` runs inside `pmain`: that function is one
    /// C level below `f`, which `debug.getinfo` finds and a traceback ends
    /// with (`[C]: in ?`, 5.1 `[C]: ?`).
    fn call_value_with_handler_in_c(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError>;
    /// `f(args)` in protected mode with no message handler, made from inside
    /// a C function of the host's: PUC `lua_pcall(L, n, r, 0)` inside a C
    /// function, as lua.c's `l_print` calls `print` from `pmain`. That
    /// function is one C level below `f`, as for
    /// `Vm::call_value_with_handler_in_c`.
    fn call_value_in_c(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError>;
    /// Start loading a text chunk that a reader hands over piece by piece
    /// (`lua_load`, `load` with a function): what the parse needs from the
    /// vm, so that the reader can use the vm while the parse runs. `None`
    /// for MacroLua, whose macro pass needs the whole source first.
    fn text_load(&mut self) -> Option<TextLoad>;
    /// `lua_load` with a reader starts: the check made before it reads,
    /// once for the load whichever way it goes on; 5.2 on, the thread is
    /// not yieldable while the chunk is parsed (`incnny` in
    /// `luaD_protectedparser`).
    fn host_load_begin(&mut self) -> Result<(), LuaError>;
    /// The parse of the `lua_load` `Vm::host_load_begin` began has ended.
    fn host_load_end(&mut self);
    /// A native that is the library function `builtin`, collecting in an
    /// emergency when memory runs out.
    fn builtin(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
        builtin: crate::runtime::Builtin,
    ) -> Result<Value, LuaError>;
    /// [`VmHostInternals::builtin`] standing for a C function of PUC's with
    /// `puc_nup` upvalues.
    fn builtin_puc(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
        builtin: crate::runtime::Builtin,
        puc_nup: usize,
    ) -> Result<Value, LuaError>;
    /// `luaL_checkstack(L, n, msg)` in lua.c's `pmain` with `values` values
    /// on its stack (above its function at slot 0): `pmain`'s level
    /// reserves that room from then on.
    fn host_check_stack(&mut self, values: u32, n: u32, msg: &str) -> Result<(), LuaError>;
    /// Before recording a trace, ask the trace compiler for one other Vms
    /// compiled for code of the same content, and remember the chunks
    /// loaded from now on so such traces find the functions they inlined.
    /// The JIT crate's shared engine turns this on for the Vms it builds.
    fn enable_trace_sharing(&mut self);
    /// Whether the interpreter at `pc` enters trace `t`: one headed there
    /// that is dispatchable, or that closes by down-recursion (unless
    /// `downrec_blocked`, set for one entry after a forced exit so the
    /// interpreter runs the op at the head). Never a side trace: it runs
    /// only from its parent's exit, so its parent's function is running or
    /// inlined by a live trace, which tiering the side trace up reads.
    fn trace_admits(t: &CompiledTrace, pc: u32, downrec_blocked: bool) -> bool;
}

impl VmHostInternals for crate::vm::Vm {
    #[inline]
    fn traceback_string(&mut self, msg: Option<&[u8]>, level: i64) -> Result<Value, LuaError> {
        crate::vm::Vm::traceback_string(self, msg, level)
    }
    fn error_object_message(&mut self, e: Value) -> Result<Gc<crate::runtime::LuaStr>, LuaError> {
        let ty = e.type_name().as_bytes();
        let fmt = b"(error object is a %s value)";
        let args = [crate::runtime::puc::fmt::FmtArg::S(ty)];
        self.pushfstring(fmt, &args)
    }
    #[inline]
    fn special_errors(&self) -> SpecialErrors {
        crate::vm::Vm::special_errors(self)
    }
    #[inline]
    fn error_status(&self, e: Value, before: SpecialErrors) -> ErrorStatus {
        crate::vm::Vm::error_status(self, e, before)
    }
    #[inline]
    fn upval_get(&self, cl: Gc<LuaClosure>, idx: u32) -> Value {
        crate::vm::Vm::upval_get(self, cl, idx)
    }
    #[inline]
    fn trace_mark(&mut self, name: &str) {
        crate::vm::Vm::trace_mark(self, name)
    }
    #[inline]
    fn load_stdin_text(&mut self, src: Vec<u8>) -> Result<Value, LuaError> {
        crate::vm::Vm::load_stdin_text(self, src)
    }
    #[inline]
    fn call_value_with_handler_in_c(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        crate::vm::Vm::call_value_with_handler_in_c(self, f, args, msgh)
    }
    #[inline]
    fn call_value_in_c(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        crate::vm::Vm::call_value_in_c(self, f, args)
    }
    #[inline]
    fn text_load(&mut self) -> Option<TextLoad> {
        crate::vm::Vm::text_load(self)
    }
    #[inline]
    fn host_load_begin(&mut self) -> Result<(), LuaError> {
        crate::vm::Vm::host_load_begin(self)
    }
    #[inline]
    fn host_load_end(&mut self) {
        crate::vm::Vm::host_load_end(self)
    }
    #[inline]
    fn builtin(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
        builtin: crate::runtime::Builtin,
    ) -> Result<Value, LuaError> {
        crate::vm::Vm::builtin(self, f, upvals, builtin)
    }
    #[inline]
    fn builtin_puc(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
        builtin: crate::runtime::Builtin,
        puc_nup: usize,
    ) -> Result<Value, LuaError> {
        crate::vm::Vm::builtin_puc(self, f, upvals, builtin, puc_nup)
    }
    #[inline]
    fn host_check_stack(&mut self, values: u32, n: u32, msg: &str) -> Result<(), LuaError> {
        crate::vm::Vm::host_check_stack(self, values, n, msg)
    }
    #[inline]
    fn enable_trace_sharing(&mut self) {
        crate::vm::Vm::enable_trace_sharing(self)
    }
    #[inline]
    fn trace_admits(t: &CompiledTrace, pc: u32, downrec_blocked: bool) -> bool {
        crate::vm::Vm::trace_admits(t, pc, downrec_blocked)
    }
}
