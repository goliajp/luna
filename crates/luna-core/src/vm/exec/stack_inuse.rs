//! What a thread's frames reserve of its stack (PUC `stackinuse`), and the
//! stack shrunk to it.

use super::stack_size;
use super::*;
use crate::vm::callstack::NativeAct;

/// PUC's `ci->top` of a thread's bottom `CallInfo`, in luna's numbering:
/// `LUA_MINSTACK` above its empty function slot.
const BASE_CI_TOP: u32 = stack_size::MIN_STACK;

/// PUC `stackinuse` of a thread with these frames, top and running natives:
/// one past the highest slot the top or a frame's reserved stack reaches,
/// in PUC's numbering.
fn thread_in_use(frames: &[CallFrame], top: u32, natives: &[NativeAct]) -> u32 {
    let mut lim = top.max(BASE_CI_TOP);
    for f in frames {
        lim = lim.max(match f {
            CallFrame::Lua(f) => f.base + u32::from(f.closure.proto.max_stack),
            CallFrame::Cont(c) => c.kind.ci_top(),
        });
    }
    // the host's protected-call levels luna makes are no frames of PUC's
    let host_level = |a: &&NativeAct| {
        use crate::vm::exec::native_call::NativeKind as K;
        a.nc.is_luna_only()
            && matches!(
                a.nc.kind,
                K::HostXpcall | K::HostXpcallInC | K::HostPcallInC | K::HostPcall
            )
    };
    for a in natives.iter().filter(|a| !host_level(a)) {
        lim = lim.max(a.ci_top);
    }
    lim + 2
}

/// What PUC keeps for a thread that its collector and a caught error
/// shrink: the stack's size, 5.1's `CallInfo` array, the later dialects'
/// `CallInfo`s besides the base one.
pub(super) struct ThreadSizes {
    pub(super) stack: u32,
    pub(super) frames51: u32,
    pub(super) ci: u32,
    /// whose blocks they are
    pub(super) thread: crate::runtime::puc::Thread,
}

/// Where a thread is shrunk: PUC's collector visiting it, or
/// `luaD_shrinkstack` after a protected call caught an error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Shrink {
    Collector,
    Caught,
    /// an emergency collection: 5.2's `sweepthread` frees the unused
    /// `CallInfo`s but leaves the stack; later dialects touch neither
    Emergency,
}

/// PUC shrinking a thread whose frames and top reach slot `inuse`, with
/// `levels` entries of 5.1's `CallInfo` array in use (the base one among
/// them) and `ci_used` later `CallInfo`s besides its base one: 5.1
/// `checkstacksizes`; 5.2's `sweepthread` and `luaD_shrinkstack`; 5.3 on
/// `luaD_shrinkstack` with its `luaE_shrinkCI` / `luaE_freeCI`.
pub(super) fn shrink_thread(
    heap: &mut crate::runtime::heap::Heap,
    v: LuaVersion,
    at: Shrink,
    inuse: u32,
    levels: u32,
    ci_used: u32,
    sz: &mut ThreadSizes,
) {
    let free_ci = |heap: &mut crate::runtime::heap::Heap, sz: &mut ThreadSizes, all: bool| {
        let unused = sz.ci - ci_used;
        // `luaE_shrinkCI` frees every other one of the unused
        let n = if all { unused } else { unused / 2 };
        heap.acct_ci(sz.thread, 0, n);
        sz.ci -= n;
    };
    if at == Shrink::Emergency {
        if v == LuaVersion::Lua52 {
            free_ci(heap, sz, true);
        }
        return;
    }
    let stack = |heap: &mut crate::runtime::heap::Heap, sz: &mut ThreadSizes| {
        heap.hook_shrink(sz.stack, inuse);
        let s = stack_size::shrink(v, sz.stack, inuse);
        heap.acct_stack(sz.thread, sz.stack, s);
        sz.stack = s;
    };
    match v {
        LuaVersion::Lua51 => {
            // `traversestack` runs when marking reaches the thread and again
            // when the atomic step remarks it (`grayagain`)
            for _ in 0..2 {
                if at != Shrink::Collector {
                    break;
                }
                heap.hook_shrink(sz.stack, inuse - 1);
                let (s, calls) = halve_51(sz.stack, inuse, sz.frames51, levels.saturating_sub(1));
                heap.acct_ci51(sz.thread, sz.frames51, calls);
                heap.acct_stack(sz.thread, sz.stack, s);
                (sz.stack, sz.frames51) = (s, calls);
            }
        }
        LuaVersion::Lua52 => {
            if at == Shrink::Collector {
                free_ci(heap, sz, true);
            }
            stack(heap, sz);
        }
        LuaVersion::Lua53 => {
            free_ci(heap, sz, sz.stack > stack_size::MAX_STACK);
            stack(heap, sz);
        }
        _ => {
            stack(heap, sz);
            free_ci(heap, sz, false);
        }
    }
}

impl Vm {
    /// [`thread_in_use`] of the running thread.
    pub(super) fn stack_in_use(&self) -> u32 {
        let host = if self.current.is_none() {
            self.g.host_ci_top
        } else {
            0
        };
        // a native that runs has its own `L->top`
        let top = self.native_top().unwrap_or(self.top);
        thread_in_use(
            &self.frames,
            top.max(host).max(self.g.jit_top),
            &self.running_natives[self.natives_base..],
        )
    }

    /// PUC shrinking the running thread at `at`, its stack reserved up to
    /// `inuse`.
    pub(super) fn shrink_running(&mut self, at: Shrink, inuse: u32) {
        // PUC shrinks a caught error with the protected call's level still
        // current; luna has popped the continuation that held it
        self.shrink_levels(at, inuse, u32::from(at == Shrink::Caught));
    }

    /// `luaD_pcall` catching an error, the level it is made from current in
    /// luna as in PUC (a load's, a finalizer's).
    pub(crate) fn shrink_caught_error(&mut self) {
        let inuse = self.stack_in_use();
        self.shrink_levels(Shrink::Caught, inuse, 0);
    }

    fn shrink_levels(&mut self, at: Shrink, inuse: u32, extra: u32) {
        let mut sz = ThreadSizes {
            stack: self.stack_size,
            frames51: self.g.frame_size,
            ci: self.g.ci_alloc,
            thread: self.running_blocks(),
        };
        let (levels, ci_used) = (self.frames_in_use() + extra, self.ci_in_use() + extra);
        shrink_thread(
            &mut self.heap,
            self.version,
            at,
            inuse,
            levels,
            ci_used,
            &mut sz,
        );
        self.set_stack_size(sz.stack);
        self.g.frame_size = sz.frames51;
        self.g.ci_alloc = sz.ci;
        self.g.ci_hint = 0;
    }

    /// PUC's collector visiting the running thread and the main thread it
    /// suspended (`traversethread` → `luaD_shrinkstack`; 5.1
    /// `checkstacksizes`, which halves the `CallInfo` array too). The
    /// suspended coroutines are visited after marking
    /// ([`Vm::collector_shrinks_threads`]).
    pub(super) fn collector_shrinks_stacks(&mut self) {
        let inuse = self.stack_in_use();
        self.shrink_running(Shrink::Collector, inuse);
        self.collector_shrinks_main(Shrink::Collector);
    }

    /// An emergency collection visiting every thread (see
    /// [`Shrink::Emergency`]).
    pub(crate) fn emergency_shrinks_threads(&mut self) {
        let inuse = self.stack_in_use();
        self.shrink_levels(Shrink::Emergency, inuse, 0);
        self.collector_shrinks_main(Shrink::Emergency);
        let threads = self.heap.take_threads();
        for &co in &threads {
            if !self.current.is_some_and(|c| c.ptr_eq(co)) {
                self.collector_shrinks_coro(co, Shrink::Emergency);
            }
        }
        self.heap.put_back_threads(threads);
    }

    /// The collector visiting the main thread a coroutine suspended.
    pub(super) fn collector_shrinks_main(&mut self, at: Shrink) {
        let v = self.version;
        let (host_frames, host_ci_top) = (self.g.host_frames, self.g.host_ci_top);
        let natives = &self.running_natives;
        if let Some(m) = &mut self.main_ctx {
            let top = m.top.max(host_ci_top);
            let inuse = thread_in_use(&m.frames, top, &natives[m.natives.clone()]);
            let n = (m.frames.len() + m.natives.len()) as u32 + host_frames - m.meta_conts;
            let mut sz = ThreadSizes {
                stack: m.stack_size,
                frames51: m.frame_size,
                ci: m.ci_alloc,
                thread: crate::runtime::puc::Thread::Main,
            };
            shrink_thread(&mut self.heap, v, at, inuse, n, n - m.host_conts, &mut sz);
            (m.stack_size, m.frame_size, m.ci_alloc) = (sz.stack, sz.frames51, sz.ci);
        }
    }

    /// The collector visiting each coroutine marked live that is not
    /// running: the ones a marking just kept ([`Heap::threads`]).
    pub(super) fn collector_shrinks_threads(&mut self) {
        let threads = self.heap.take_threads();
        for &co in &threads {
            if self.current.is_some_and(|c| c.ptr_eq(co)) {
                continue;
            }
            self.collector_shrinks_coro(co, Shrink::Collector);
        }
        self.heap.put_back_threads(threads);
    }

    /// The collector visiting `co`, a coroutine that is not running.
    pub(super) fn collector_shrinks_coro(
        &mut self,
        co: crate::runtime::heap::Gc<crate::runtime::Coro>,
        at: Shrink,
    ) {
        let v = self.version;
        // SAFETY: `co` survived the marking just done, so it is allocated; it is not the running thread, so its context is saved in it and nothing else refers into it here
        let m = unsafe { co.as_mut() };
        // only a thread waiting on one it resumed has natives running
        let normal = m.status == crate::runtime::CoroStatus::Normal;
        let natives = if normal {
            &self.running_natives[m.natives.clone()]
        } else {
            &[]
        };
        // a thread a C function suspended keeps that function's level
        let suspended = m.status == crate::runtime::CoroStatus::Suspended;
        let yielded = if suspended { m.yield_ci_top } else { 0 };
        let inuse = thread_in_use(&m.frames, m.top.max(yielded), natives);
        let n = (m.frames.len() + natives.len()) as u32 + 1 + u32::from(yielded > 0) - m.meta_conts;
        let mut sz = ThreadSizes {
            stack: m.stack_size,
            frames51: m.frame_size,
            ci: m.ci_alloc,
            thread: crate::runtime::puc::Thread::Coro(co),
        };
        shrink_thread(&mut self.heap, v, at, inuse, n, n - 1, &mut sz);
        (m.stack_size, m.frame_size, m.ci_alloc) = (sz.stack, sz.frames51, sz.ci);
    }
}

/// PUC 5.1 `checkstacksizes`: the `CallInfo` array of `calls` entries,
/// `used` of them in use, and the stack of size `s`, `inuse` of it used,
/// each halved when less than a quarter is used; neither is touched while
/// the array is past the call limit (an overflow is being handled).
fn halve_51(s: u32, inuse: u32, calls: u32, used: u32) -> (u32, u32) {
    if calls > MAX_CALLS_51 {
        return (s, calls);
    }
    let calls = stack_size::halve_calls_51(calls, used, MAX_CALLS_51, BASIC_FRAME_SIZE_51);
    // `traversestack` measures to the highest slot, not one past it
    (stack_size::halve_51(s, inuse - 1), calls)
}
