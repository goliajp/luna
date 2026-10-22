//! The accounting trace's hooks (`gc-trace`): the lines written where
//! luna counts, the refusals of the instrumented PUC's allocation
//! function, the trace's switches.

use super::*;
use crate::runtime::heap::{Heap, Running};
use crate::runtime::puc::PucKind;
use crate::runtime::{LuaStr, Table, Value};

impl Heap {
    /// gc-trace: a line of the accounting trace.
    #[inline(always)]
    pub(crate) fn trace_line(&mut self, a: std::fmt::Arguments<'_>) {
        #[cfg(feature = "gc-trace")]
        self.hooks.trace.line(a);
        #[cfg(not(feature = "gc-trace"))]
        let _ = a;
    }

    /// gc-trace: an object of PUC's `kind` and `size` bytes counted.
    #[inline(always)]
    pub(crate) fn hook_alloc(&mut self, kind: PucKind, size: usize) {
        #[cfg(feature = "gc-trace")]
        {
            let tag = crate::runtime::gc_trace::puc_tag(kind, self.puc.dialect);
            self.hooks.trace.line(format_args!("a {tag} {size}"));
        }
        #[cfg(not(feature = "gc-trace"))]
        let _ = (kind, size);
    }

    /// A counted block going from `old` to `new` bytes: gc-verify checks
    /// the count does not go below 0, gc-trace writes it.
    #[inline(always)]
    pub(crate) fn hook_block(&mut self, old: usize, new: usize) {
        #[cfg(feature = "gc-verify")]
        assert!(
            self.puc.bytes + new >= old,
            "[gc-verify] a block of {old} bytes freed from a count of {}",
            self.puc.bytes
        );
        #[cfg(feature = "gc-trace")]
        self.hooks.trace.block(old, new);
        let _ = (old, new);
    }

    /// A block PUC allocates through the allocation function but leaves out
    /// of `totalbytes` (a `luaL_Buffer` box's contents): only the trace sees
    /// it.
    #[inline(always)]
    pub(crate) fn trace_raw(&mut self, old: usize, new: usize) {
        #[cfg(feature = "gc-trace")]
        if self.counting() {
            self.hooks.trace.raw = true;
            self.hooks.trace.block(old, new);
            self.hooks.trace.raw = false;
        }
        let _ = (old, new);
    }

    /// gc-trace (`LUNA_GC_WHERE`): a string freed.
    #[inline(always)]
    pub(crate) fn hook_string_freed(&mut self, s: *mut LuaStr) {
        #[cfg(feature = "gc-trace")]
        self.trace_string(b"s", s);
        let _ = s;
    }

    /// gc-trace (`LUNA_GC_WHERE`): a string made.
    #[inline(always)]
    pub(crate) fn hook_string_made(&mut self, s: *mut LuaStr) {
        #[cfg(feature = "gc-trace")]
        self.trace_string(b"s+", s);
        let _ = s;
    }

    /// gc-trace (`LUNA_GC_KEYS`): key `k` enters table `t`'s hash part.
    #[inline(always)]
    pub(crate) fn hook_new_key(&mut self, t: &Table, k: Value) {
        #[cfg(feature = "gc-trace")]
        self.trace_new_key(t, k);
        let _ = (t, k);
    }

    /// gc-trace (`LUNA_GC_PROP`): the object `h` traversed with work `w`.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    pub(crate) unsafe fn hook_prop(&mut self, h: *mut GcHeader, w: i64, run: Running) {
        #[cfg(feature = "gc-trace")]
        // SAFETY: the caller's contract
        unsafe {
            self.trace_prop(h, w, run)
        };
        let _ = (h, w, run);
    }

    /// gc-trace (`LUNA_GC_PROP`): how many objects PUC's list holds as the sweep
    /// finds it.
    #[inline(always)]
    pub(crate) fn hook_listed(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.trace_listed();
    }

    /// The sweep goes over `h`, `dead` or not: gc-trace (`LUNA_GC_SWEEP`)
    /// writes it; gc-verify checks its class byte says whether PUC lists
    /// it (`strt`: the string table holds strings).
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    pub(crate) unsafe fn hook_swept(&mut self, h: *mut GcHeader, dead: bool, strt: bool) {
        // SAFETY: the caller's contract
        #[cfg(feature = "gc-trace")]
        unsafe {
            self.trace_swept(h, dead)
        };
        #[cfg(feature = "gc-verify")]
        // SAFETY: the caller's contract
        unsafe {
            self.verify_class_byte(h, strt)
        };
        let _ = (h, dead, strt);
    }

    /// gc-trace: what the trace writes from now is held back.
    #[inline(always)]
    pub(crate) fn hook_hold_lines(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.hooks.trace.hold();
    }

    /// gc-trace: the lines held back since [`Heap::hook_hold_lines`].
    #[inline(always)]
    pub(crate) fn hook_release_lines(&mut self) -> HeldLines {
        HeldLines {
            #[cfg(feature = "gc-trace")]
            lines: self.hooks.trace.release(),
        }
    }

    /// gc-trace: lines held back written now.
    #[inline(always)]
    pub(crate) fn hook_put_lines(&mut self, held: HeldLines) {
        #[cfg(feature = "gc-trace")]
        self.hooks.trace.put(held.lines);
        let _ = held;
    }

    /// gc-trace (`LUNA_GC_SWEEP`): the string sweep frees string `h`.
    #[inline(always)]
    pub(crate) fn hook_string_swept(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-trace")]
        self.trace_string_swept(h);
        let _ = h;
    }

    /// gc-trace: checkpoints are shown as made in a C function and in a
    /// call hook of PUC's as `(in_c, call_hook)` says; the context before.
    #[inline(always)]
    pub(crate) fn trace_context(&mut self, ctx: (bool, bool)) -> (bool, bool) {
        #[cfg(feature = "gc-trace")]
        {
            let t = &mut self.hooks.trace;
            let was = (t.in_c, t.call_hook);
            (t.in_c, t.call_hook) = ctx;
            return was;
        }
        #[cfg(not(feature = "gc-trace"))]
        ctx
    }

    /// gc-trace: a thread of `stack` slots shrunk with `inuse` in use.
    #[inline(always)]
    pub(crate) fn hook_shrink(&mut self, stack: u32, inuse: u32) {
        #[cfg(feature = "gc-trace")]
        self.trace_shrink(stack, inuse);
        let _ = (stack, inuse);
    }

    /// gc-trace: a mark named `name` where a test cuts the comparison with
    /// PUC's (`LUNA_GC_EXIT_AT=<name>` stops the program there).
    #[inline(always)]
    pub(crate) fn hook_mark(&mut self, name: &str) {
        #[cfg(feature = "gc-trace")]
        {
            self.hooks.trace.line(format_args!("M {name}"));
            if std::env::var("LUNA_GC_EXIT_AT").is_ok_and(|at| at == name) {
                self.hooks.trace.flush();
                std::process::exit(0);
            }
        }
        let _ = name;
    }

    /// gc-trace: the collector's mode, which the steps' lines show.
    #[inline(always)]
    pub(crate) fn hook_mode(&mut self, generational: bool) {
        #[cfg(feature = "gc-trace")]
        {
            self.hooks.trace.generational = generational;
        }
        let _ = generational;
    }

    /// gc-trace: the trace written out (a host may end the process without
    /// dropping the state).
    #[inline(always)]
    pub(crate) fn hook_flush(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.hooks.trace.flush();
    }

    /// No automatic step runs (gc-trace with `LUNA_GC_NOAUTO` set).
    #[inline(always)]
    pub(crate) fn no_auto(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.no_auto;
        #[cfg(not(feature = "gc-trace"))]
        false
    }

    /// No collection runs (gc-trace with `LUNA_GC_NOGC` set).
    #[inline(always)]
    pub(crate) fn no_gc(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.no_gc;
        #[cfg(not(feature = "gc-trace"))]
        false
    }
}
