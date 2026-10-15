//! Making native functions: what luna keeps of each, and what PUC's C
//! closure for it counts.

use super::*;
use crate::runtime::Builtin;
use crate::runtime::mem::Oom;
use crate::runtime::value::NativeFn;

impl Heap {
    /// A [`NativeClosure`] calling host function `f` over `upvals`.
    pub fn new_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Gc<NativeClosure> {
        self.new_builtin(f, &upvals, Builtin::None)
    }

    /// [`Heap::new_native`] copying the upvalues from a slice.
    pub fn new_native_from(&mut self, f: NativeFn, upvals: &[Value]) -> Gc<NativeClosure> {
        self.new_builtin(f, upvals, Builtin::None)
    }

    /// A native that is the library function `builtin`.
    pub(crate) fn new_builtin(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        builtin: Builtin,
    ) -> Gc<NativeClosure> {
        self.try_new_builtin(f, upvals, builtin, false)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// [`Heap::new_native`] with `is_async` set: `f` is an `AsyncNativeFn`
    /// the caller transmuted to `NativeFn` ([`crate::vm::async_drive`]).
    pub fn new_async_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Gc<NativeClosure> {
        self.try_new_builtin(f, &upvals, Builtin::None, true)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// A native function over `f` with a copy of `upvals`, async when
    /// `is_async`, failing when memory runs out.
    pub fn try_new_native(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        is_async: bool,
    ) -> Result<Gc<NativeClosure>, Oom> {
        self.try_new_builtin(f, upvals, Builtin::None, is_async)
    }

    /// [`Heap::try_new_native`] for a library function PUC makes with
    /// `puc_nup` upvalues where luna keeps more: counted at PUC's size.
    pub(crate) fn try_new_native_puc(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        puc_nup: usize,
    ) -> Result<Gc<NativeClosure>, Oom> {
        self.puc.silent += 1;
        let g = self.try_new_builtin(f, upvals, Builtin::None, false);
        self.puc.silent -= 1;
        let g = g?;
        let h = g.as_ptr() as *mut GcHeader;
        if self.puc.silent == 0 {
            acct::set_uncounted(h, false);
            // SAFETY: `g` was just made; only its per-type byte is written
            unsafe { (*h).sub = (*h).sub & UNCOUNTED | (puc_nup as u8 + 1) };
            if puc_nup > 0 || self.table_dialect == crate::runtime::table::Dialect::L51 {
                self.acct_obj(PucKind::CClosure, self.sizes.c_closure(puc_nup));
            }
        }
        Ok(g)
    }

    /// [`Heap::try_new_native`] for a library function standing for the
    /// objects of PUC's in `objs`, made in that order and freed with it
    /// (5.4's `gmatch`: a state userdata and a C closure).
    pub(crate) fn try_new_native_stand_in(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        objs: &[(PucKind, usize)],
    ) -> Result<Gc<NativeClosure>, Oom> {
        self.puc.silent += 1;
        let g = self.try_new_builtin(f, upvals, Builtin::None, false);
        self.puc.silent -= 1;
        let g = g?;
        if self.puc.silent == 0 {
            for &(kind, size) in objs {
                self.acct_obj(kind, size);
            }
            let sizes = objs.iter().map(|&(_, s)| s).collect();
            self.puc
                .stand_ins
                .push((g.as_ptr() as *mut GcHeader, sizes));
        }
        Ok(g)
    }

    /// [`Heap::try_new_native`] for the library function `builtin`.
    pub(crate) fn try_new_builtin(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        builtin: Builtin,
        is_async: bool,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let fix = self.fix_natives && upvals.is_empty() && !is_async;
        let mut hdr = GcHeader::native(upvals);
        hdr.aux = self.take_id();
        let kind = if is_async {
            crate::vm::exec::native_call::NativeKind::Async
        } else {
            crate::vm::exec::native_call::NativeKind::of(builtin)
        };
        let nup = upvals.len();
        let upvals = LSlice::from_slice(self.mem(), upvals)?;
        let g = self.try_adopt(NativeClosure {
            hdr,
            f,
            upvals,
            is_async,
            kind,
            builtin,
        })?;
        // PUC 5.2+ makes a C function without upvalues a light value
        if nup > 0 || self.table_dialect == crate::runtime::table::Dialect::L51 {
            let size = self.sizes.c_closure(nup);
            self.acct_new(g.as_ptr() as *mut GcHeader, PucKind::CClosure, size);
        }
        if fix {
            // SAFETY: `try_adopt` just linked `g` at the head of `all`
            unsafe { self.fix_newest(g.as_ptr() as *mut GcHeader) };
        }
        Ok(g)
    }
}
