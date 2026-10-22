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
        builtin: Builtin,
        puc_nup: usize,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let g = self.uncounted(|h| h.try_new_builtin(f, upvals, builtin, false));
        let g = g?;
        let h = g.as_ptr() as *mut GcHeader;
        self.set_native_puc_nup(g, puc_nup);
        if self.counting() {
            puc::set_uncounted(h, false);
            if puc_nup > 0 || self.dialect() == crate::runtime::dialect::Dialect::L51 {
                self.acct_obj(PucKind::CClosure, self.puc_size(PucObj::CClosure(puc_nup)));
            }
        }
        Ok(g)
    }

    /// PUC's light C function `f` (5.2 on, no upvalues), as the opening
    /// makes it: fixed, uncounted, standing for no object of PUC's.
    pub(crate) fn try_new_light(
        &mut self,
        f: NativeFn,
        builtin: Builtin,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let mut hdr = GcHeader::native(&[]);
        hdr.aux = self.take_id();
        // `set_native_puc_nup(g, 0)`: a function of no upvalues of PUC's
        hdr.sub = 1;
        let obj = NativeClosure {
            hdr,
            f,
            upvals: crate::runtime::mem::LSlice::empty(self.mem()),
            is_async: false,
            kind: crate::vm::exec::native_call::NativeKind::of(builtin),
            builtin,
        };
        let p = self.slab_block()?;
        // SAFETY: `p` is a free slot of a slab block, for one native, which
        // goes straight onto the fixed list
        unsafe {
            p.write(obj);
            let h = p as *mut GcHeader;
            (*h).cls |= SLAB;
            (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
            (*h).next = self.fixed;
            self.fixed = h;
            self.live += 1;
            Ok(Gc::from_ptr(p))
        }
    }

    /// PUC 5.1's C closure of `f` without upvalues, as the opening makes
    /// it: counted at PUC's size (luna's made while nothing is counted
    /// stays uncounted).
    pub(crate) fn try_new_c0(
        &mut self,
        f: NativeFn,
        builtin: Builtin,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let mut hdr = GcHeader::native(&[]);
        hdr.aux = self.take_id();
        hdr.sub = 1;
        let obj = NativeClosure {
            hdr,
            f,
            upvals: crate::runtime::mem::LSlice::empty(self.mem()),
            is_async: false,
            kind: crate::vm::exec::native_call::NativeKind::of(builtin),
            builtin,
        };
        let p = self.slab_block()?;
        // SAFETY: `p` is a free slot of a slab block, for one native
        let h = unsafe {
            p.write(obj);
            let h = p as *mut GcHeader;
            (*h).cls |= SLAB;
            self.link(h);
            h
        };
        if self.counting() {
            self.acct_obj(PucKind::CClosure, self.puc_size(PucObj::CClosure(0)));
        } else {
            puc::set_uncounted(h, true);
        }
        // SAFETY: as above
        Ok(unsafe { Gc::from_ptr(p) })
    }

    /// Native `g` stands for a C closure of PUC's with `n` upvalues (from
    /// 5.2 on none: a light function), whatever upvalues luna gives it.
    pub(crate) fn set_native_puc_nup(&mut self, g: Gc<NativeClosure>, n: usize) {
        let h = g.as_ptr() as *mut GcHeader;
        debug_assert!(n + 1 < usize::from(puc::ENV51));
        // SAFETY: `g` is a live native; only its per-type byte is written
        unsafe { (*h).sub = (*h).sub & UNCOUNTED | (n as u8 + 1) };
    }

    /// [`Heap::try_new_native`] for a library function standing for the
    /// objects of PUC's in `objs`, made in that order and freed with it
    /// (5.4's `gmatch`: a state userdata and a C closure).
    pub(crate) fn try_new_native_stand_in(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        objs: &[(PucKind, usize)],
        counted: usize,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let g = self.uncounted(|h| h.try_new_builtin(f, upvals, Builtin::None, false));
        let g = g?;
        if self.counting() {
            for &(kind, size) in &objs[counted..] {
                self.acct_obj(kind, size);
            }
            let h = g.as_ptr() as *mut GcHeader;
            self.stand_for(h, objs.iter().map(|&(_, s)| s));
            // SAFETY: `h` heads the native just made; only its class byte
            unsafe { (*h).cls |= STANDS };
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
        let obj = NativeClosure {
            hdr,
            f,
            upvals,
            is_async,
            kind,
            builtin,
        };
        // made in a slab block; a fixed one goes straight onto the fixed list
        let p = self.slab_block()?;
        // SAFETY: `p` is a free slot of a slab block, for one native
        let g = unsafe {
            p.write(obj);
            let h = p as *mut GcHeader;
            (*h).cls |= SLAB;
            if fix {
                (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
                (*h).next = self.fixed;
                self.fixed = h;
                self.live += 1;
            } else {
                self.link(h);
            }
            Gc::from_ptr(p)
        };
        if fix {
            return Ok(g);
        }
        // PUC 5.2+ makes a C function without upvalues a light value, and
        // its library functions are such functions, whatever upvalues
        // luna's keep (one with upvalues of PUC's is made by
        // `try_new_native_puc`)
        if self.fix_natives {
            if nup > 0 {
                puc::set_uncounted(g.as_ptr() as *mut GcHeader, true);
            }
        } else if nup > 0 || self.dialect() == crate::runtime::dialect::Dialect::L51 {
            let size = self.puc_size(PucObj::CClosure(nup));
            self.acct_new(g.as_ptr() as *mut GcHeader, PucKind::CClosure, size);
        }
        Ok(g)
    }
}
