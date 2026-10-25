//! The model's part of making an object: PUC's count gains what PUC
//! allocates for it, or the object is marked as one PUC has not made. One
//! impl of [`Born`] per type and way of making it, so each of the heap's
//! allocation paths reaches its own code, chosen when it is compiled.

use super::*;
use crate::runtime::Coro;
use crate::runtime::function::{LuaClosure, NativeClosure, Proto};
use crate::runtime::upvalue::Upvalue;

/// What PUC's count gains when the heap makes an object of this type in
/// the way `B` names.
pub(in crate::runtime) trait Born<B>: GcObject + Sized {
    /// What the heap learns back.
    type Out;
    fn born(heap: &mut Heap, g: Gc<Self>, how: B) -> Self::Out;
}

/// Made as PUC makes one of its kind.
pub(in crate::runtime) struct Made;
/// A closure of a prototype with this many upvalues; tells whether it
/// keeps 5.1's environment among them.
pub(in crate::runtime) struct OfProto<'a>(pub &'a Proto, pub usize);
/// A new string of this many bytes.
pub(in crate::runtime) struct NewStr(pub usize);
/// An interned string of this many bytes found again.
pub(in crate::runtime) struct FoundStr(pub usize);
/// A string of this many bytes the string sweep took off, found again.
pub(in crate::runtime) struct RemadeStr(pub usize);
/// 5.5's external string.
pub(in crate::runtime) struct ExternalStr;
/// A library function PUC makes with this many upvalues, made uncounted.
pub(in crate::runtime) struct PucFunction(pub usize);
/// PUC 5.1's C closure without upvalues, as the opening makes it.
pub(in crate::runtime) struct C0;
/// A function standing for PUC's objects `objs`, the first `counted` of
/// them counted already.
pub(in crate::runtime) struct StandingFor<'a> {
    pub objs: &'a [(PucKind, usize)],
    pub counted: usize,
}
/// A native function with `nup` upvalues; `fixed` while the opening
/// makes the library.
pub(in crate::runtime) struct NativeFunction {
    pub nup: usize,
    pub fixed: bool,
}

impl Heap {
    /// The heap made `g` in the way `how` names.
    #[inline(always)]
    pub(in crate::runtime) fn puc_born<T: Born<B>, B>(&mut self, g: Gc<T>, how: B) -> T::Out {
        T::born(self, g, how)
    }

    /// Mark the object behind `h` as one PUC has made (`false`) or not.
    #[inline(always)]
    fn set_counted(&mut self, h: Gc<GcHeader>, counted: bool) {
        // SAFETY: `h` is a handle, so its object is live; nothing borrows
        // the header while its per-type byte is written
        unsafe { (*h.as_ptr()).set_uncounted(!counted) };
    }
}

impl Born<Made> for Table {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<Table>, _: Made) {
        heap.acct_new_table(g.hdr());
    }
}

impl Born<Made> for Proto {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<Proto>, _: Made) {
        heap.acct_proto(g);
    }
}

impl Born<OfProto<'_>> for LuaClosure {
    type Out = bool;
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<LuaClosure>, how: OfProto<'_>) -> bool {
        let size = heap.closure_size(how.0, how.1);
        heap.acct_new(g.hdr(), PucKind::LuaClosure, size);
        heap.env51(how.0)
    }
}

impl Born<Made> for Upvalue {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<Upvalue>, _: Made) {
        let size = heap.puc_size(PucObj::Upval);
        heap.acct_new(g.hdr(), PucKind::Upval, size);
    }
}

impl Born<Made> for Userdata {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<Userdata>, _: Made) {
        heap.acct_new(g.hdr(), PucKind::Userdata, g.puc_size);
    }
}

impl Born<Made> for Coro {
    /// whether PUC counts it
    type Out = bool;
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<Coro>, _: Made) -> bool {
        let size = heap.puc_size(PucObj::Thread);
        heap.acct_new(g.hdr(), PucKind::Thread, size);
        heap.acct_thread_blocks(g.stack_size);
        heap.counting()
    }
}

impl Born<NewStr> for LuaStr {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<LuaStr>, how: NewStr) {
        heap.acct_new_string(g, how.0);
    }
}

impl Born<FoundStr> for LuaStr {
    type Out = ();
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<LuaStr>, how: FoundStr) {
        if g.hdr.is_uncounted() {
            heap.acct_promote(g, how.0);
        }
    }
}

impl Born<RemadeStr> for LuaStr {
    type Out = ();
    /// PUC makes it again, now or (found while nothing is counted) when
    /// the program first uses it.
    fn born(heap: &mut Heap, g: Gc<LuaStr>, how: RemadeStr) {
        if !heap.counting() {
            heap.set_counted(g.hdr(), false);
        } else {
            heap.acct_string(how.0);
            heap.strt_enter(g);
        }
    }
}

impl Born<ExternalStr> for LuaStr {
    type Out = ();
    fn born(heap: &mut Heap, g: Gc<LuaStr>, _: ExternalStr) {
        if !heap.counting() {
            heap.set_counted(g.hdr(), false);
        } else {
            heap.acct_obj(PucKind::LongStr, heap.puc_size(PucObj::ExternalStr));
        }
    }
}

impl Born<PucFunction> for NativeClosure {
    type Out = ();
    fn born(heap: &mut Heap, g: Gc<NativeClosure>, how: PucFunction) {
        if heap.counting() {
            heap.set_counted(g.hdr(), true);
            if heap.puc.rules.c_function_is_object(how.0) {
                heap.acct_obj(PucKind::CClosure, heap.puc_size(PucObj::CClosure(how.0)));
            }
        }
    }
}

impl Born<C0> for NativeClosure {
    type Out = ();
    fn born(heap: &mut Heap, g: Gc<NativeClosure>, _: C0) {
        if heap.counting() {
            heap.acct_obj(PucKind::CClosure, heap.puc_size(PucObj::CClosure(0)));
        } else {
            heap.set_counted(g.hdr(), false);
        }
    }
}

impl Born<StandingFor<'_>> for NativeClosure {
    /// whether it stands for them (`STANDS`)
    type Out = bool;
    fn born(heap: &mut Heap, g: Gc<NativeClosure>, how: StandingFor<'_>) -> bool {
        if !heap.counting() {
            return false;
        }
        for &(kind, size) in &how.objs[how.counted..] {
            heap.acct_obj(kind, size);
        }
        heap.stand_for(g.header(), how.objs.iter().map(|&(_, s)| s));
        true
    }
}

impl Born<NativeFunction> for NativeClosure {
    type Out = ();
    /// PUC 5.2+ makes a C function without upvalues a light value, and its
    /// library functions are such functions, whatever upvalues luna's keep
    /// (one with upvalues of PUC's is a [`PucFunction`]).
    #[inline(always)]
    fn born(heap: &mut Heap, g: Gc<NativeClosure>, how: NativeFunction) {
        if how.fixed {
            if how.nup > 0 {
                heap.set_counted(g.hdr(), false);
            }
        } else if heap.puc.rules.c_function_is_object(how.nup) {
            let size = heap.puc_size(PucObj::CClosure(how.nup));
            heap.acct_new(g.hdr(), PucKind::CClosure, size);
        } else {
            // a light value: no object a sweep of PUC's meets
            heap.set_counted(g.hdr(), false);
        }
    }
}
