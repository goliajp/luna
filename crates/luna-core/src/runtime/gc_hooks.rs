//! The hooks of the builds only tests use: `gc-trace` (the accounting
//! trace compared with an instrumented PUC), `gc-verify` (checks of the
//! collector's invariants), `gc-stress` (a collection at every
//! checkpoint). Each hook is empty, or a constant, in the product build;
//! the code that calls them names no feature. What a hook does lives in
//! the modules only those builds compile (`gc_trace`, `heap::trace`,
//! `heap::verify`, ...).

use crate::runtime::heap::{GcHeader, Heap, Running};
use crate::runtime::puc::PucKind;
use crate::runtime::{LuaStr, Table, Value};

/// The accounting trace is written.
pub(crate) const TRACE: bool = cfg!(feature = "gc-trace");
/// The collector's invariants are checked.
pub(crate) const VERIFY: bool = cfg!(feature = "gc-verify");
/// Every checkpoint collects.
pub(crate) const STRESS: bool = cfg!(feature = "gc-stress");
/// Something watches each checkpoint: it is never only noted.
pub(crate) const CHECKPOINTS_WATCHED: bool = TRACE || VERIFY || STRESS;
/// The checks of debug builds, which gc-verify runs too.
pub(crate) const DEBUG_CHECKS: bool = cfg!(any(debug_assertions, feature = "gc-verify"));

/// What the test-only builds keep on the heap; nothing in the product.
#[derive(Default)]
pub(crate) struct HookState {
    #[cfg(feature = "gc-trace")]
    pub(crate) trace: crate::runtime::gc_trace::Trace,
    /// gc-verify: headers freed since the last collection began, for the
    /// read-time probes (exact only under quarantining allocators)
    #[cfg(feature = "gc-verify")]
    pub(crate) recently_freed: std::collections::HashSet<usize>,
}

/// gc-verify: a marking that, instead of marking a child, reports one the
/// sweep about to run would free while the object being traced holds it.
#[derive(Clone, Copy, Default)]
pub(crate) struct ChildCheck {
    #[cfg(feature = "gc-verify")]
    pub(crate) check: Option<(u8, *mut GcHeader, &'static str)>,
}

impl ChildCheck {
    /// Whether this marking only checks.
    #[inline(always)]
    pub(crate) fn active(&self) -> bool {
        #[cfg(feature = "gc-verify")]
        return self.check.is_some();
        #[cfg(not(feature = "gc-verify"))]
        false
    }

    /// The check of child `h`, when active: true when it was checked
    /// rather than marked.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    pub(crate) unsafe fn report(&self, h: *mut GcHeader) -> bool {
        #[cfg(feature = "gc-verify")]
        if let Some(check) = self.check {
            // SAFETY: the caller's contract
            unsafe { crate::runtime::heap::verify::dead_child(h, check) };
            return true;
        }
        let _ = h;
        false
    }
}

/// gc-trace: the lines held back while counting is set aside (see
/// `Heap::defer_begin`); nothing in the product.
#[derive(Default)]
pub(crate) struct HeldLines {
    #[cfg(feature = "gc-trace")]
    lines: Vec<String>,
}

/// gc-trace: the seed the instrumented PUC build takes from the
/// environment too.
#[inline(always)]
pub(crate) fn fixed_seed() -> Option<u32> {
    #[cfg(feature = "gc-trace")]
    return crate::runtime::gc_trace::fixed_seed();
    #[cfg(not(feature = "gc-trace"))]
    None
}

/// gc-trace: the line `luaC_step` logs before (`g`) and after (`G`) a step.
#[inline(always)]
pub(crate) fn hook_step<H: crate::runtime::puc::step::StepHost + ?Sized>(h: &mut H, after: bool) {
    #[cfg(feature = "gc-trace")]
    crate::runtime::puc::trace::trace_step(h, after);
    let _ = (h, after);
}

/// gc-trace: the end of a full collection.
#[inline(always)]
pub(crate) fn hook_full_end<H: crate::runtime::puc::step::StepHost + ?Sized>(h: &mut H) {
    #[cfg(feature = "gc-trace")]
    crate::runtime::puc::trace::trace_full_end(h);
    let _ = h;
}

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

    /// The object `h` is freed: gc-verify notes it for the read probes.
    #[inline(always)]
    pub(crate) fn hook_freed(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-verify")]
        self.note_freed(h);
        let _ = h;
    }

    /// The freed table `t` is made again from the pool: no longer freed.
    #[inline(always)]
    pub(crate) fn hook_reborn(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-verify")]
        self.forget_freed(h);
        let _ = h;
    }

    /// gc-verify: `h`, about to be held by `what`, is not one the sweep
    /// under way frees.
    #[inline(always)]
    pub(crate) fn check_not_dying(&self, h: *const GcHeader, what: &str) {
        #[cfg(feature = "gc-verify")]
        self.verify_not_dying(h, what);
        let _ = (h, what);
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

    /// gc-trace: the instrumented PUC refuses to allocate a long string of
    /// `len` bytes here.
    #[inline(always)]
    pub(crate) fn refuse_string_now(&mut self, len: usize) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.refuse_string(len);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = len;
            false
        }
    }

    /// gc-trace: the instrumented PUC refuses to resize table `t` (one of
    /// PUC's) from `old_a` array slots to `new_a` and `new_h` nodes.
    #[inline(always)]
    pub(crate) fn refuse_table_now(
        &mut self,
        t: &Table,
        old_a: usize,
        new_a: usize,
        new_h: usize,
    ) -> bool {
        #[cfg(feature = "gc-trace")]
        return !self.is_own_table(t) && self.refuse_table(old_a, new_a, new_h);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = (t, old_a, new_a, new_h);
            false
        }
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

    /// gc-verify: after the atomic step, the tricolor invariant, black
    /// objects' children, the string count; the slow bits (debug builds
    /// too).
    #[inline(always)]
    pub(crate) fn check_after_atomic(&self, ctx: &'static str) {
        #[cfg(feature = "gc-verify")]
        {
            self.verify_tricolor(ctx);
            self.verify_black_children(ctx);
            self.verify_string_count(ctx);
        }
        self.check_slow_bits(ctx);
    }

    /// gc-verify: nothing reachable was freed (after a sweep, or a step of
    /// one).
    #[inline(always)]
    pub(crate) fn check_no_dangling(&self, ctx: &str) {
        #[cfg(feature = "gc-verify")]
        self.verify_no_dangling(ctx);
        let _ = ctx;
    }

    /// The slow bits agree with the colours (debug builds and gc-verify).
    #[inline(always)]
    pub(crate) fn check_slow_bits(&self, ctx: &str) {
        #[cfg(any(debug_assertions, feature = "gc-verify"))]
        self.verify_slow_bits(ctx);
        let _ = ctx;
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

    /// gc-trace: a growth the instrumented PUC refused waits already.
    #[inline(always)]
    pub(crate) fn refusal_waits(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.refused();
        #[cfg(not(feature = "gc-trace"))]
        false
    }

    /// gc-trace: the bytes counted since a growth the instrumented PUC
    /// refused, when it refused one since `waited` was read.
    #[inline(always)]
    pub(crate) fn take_new_refusal(&mut self, waited: bool) -> Option<i64> {
        #[cfg(feature = "gc-trace")]
        if !waited && self.hooks.trace.refused() {
            return Some(self.hooks.trace.take_refused().1);
        }
        let _ = waited;
        None
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

    /// gc-trace: the stack top an emergency collection answering a refusal
    /// takes, where the VM cannot tell it.
    #[inline(always)]
    pub(crate) fn trace_alloc_top(&self) -> Option<u32> {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.alloc_top;
        #[cfg(not(feature = "gc-trace"))]
        None
    }

    /// gc-trace: the instrumented PUC refuses a box's block growing from
    /// `old` to `new` bytes.
    #[inline(always)]
    pub(crate) fn refuse_box_now(&mut self, old: usize, new: usize) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.refuse_now(old, new);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = (old, new);
            false
        }
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

impl Table {
    /// gc-verify: the key a hash lookup compares, and every node key it
    /// meets, are live.
    #[inline(always)]
    pub(crate) fn hook_find_node(&self, k: Value) {
        #[cfg(feature = "gc-verify")]
        self.verify_find_node_keys(k);
        let _ = k;
    }
}
