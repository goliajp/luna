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
        self.set_native_puc_nup(g, puc_nup);
        if self.puc.silent == 0 {
            acct::set_uncounted(h, false);
            if puc_nup > 0 || self.table_dialect == crate::runtime::table::Dialect::L51 {
                self.acct_obj(PucKind::CClosure, self.sizes.c_closure(puc_nup));
            }
        }
        Ok(g)
    }

    /// Native `g` stands for a C closure of PUC's with `n` upvalues (from
    /// 5.2 on none: a light function), whatever upvalues luna gives it.
    pub(crate) fn set_native_puc_nup(&mut self, g: Gc<NativeClosure>, n: usize) {
        let h = g.as_ptr() as *mut GcHeader;
        debug_assert!(n + 1 < usize::from(acct::ENV51));
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
        self.puc.silent += 1;
        let g = self.try_new_builtin(f, upvals, Builtin::None, false);
        self.puc.silent -= 1;
        let g = g?;
        if self.puc.silent == 0 {
            for &(kind, size) in &objs[counted..] {
                self.acct_obj(kind, size);
            }
            let h = g.as_ptr() as *mut GcHeader;
            self.puc.stand_ins.extend(objs.iter().map(|&(_, s)| (h, s)));
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
        if fix
            && self.puc.libs_opened
            && let Some(g) = self.light_native(f, builtin)
        {
            return Ok(g);
        }
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
                acct::set_uncounted(g.as_ptr() as *mut GcHeader, true);
            }
        } else if nup > 0 || self.table_dialect == crate::runtime::table::Dialect::L51 {
            let size = self.sizes.c_closure(nup);
            self.acct_new(g.as_ptr() as *mut GcHeader, PucKind::CClosure, size);
        }
        Ok(g)
    }
}

impl Heap {
    /// The library function over `f` made before (PUC's light C function
    /// of `f`: one value however often a library pushes it).
    fn light_native(&self, f: NativeFn, builtin: Builtin) -> Option<Gc<NativeClosure>> {
        let mut h = self.fixed;
        // SAFETY: the links of `fixed` lead to live objects, each read
        // through its tag's type
        unsafe {
            while !h.is_null() {
                if (*h).tag == ObjTag::Native {
                    let n = &*(h as *const NativeClosure);
                    if std::ptr::fn_addr_eq(n.f, f) && n.builtin == builtin && n.upvals.is_empty() {
                        return Some(Gc::from_ptr(h as *mut NativeClosure));
                    }
                }
                h = (*h).next;
            }
        }
        None
    }

    /// Library functions made fixed (PUC's light C functions) that stand
    /// for C closures of PUC's: objects again, back on `all` in the order
    /// given, and ones the count leaves to what they stand for. One not
    /// on the fixed list is left as it is.
    pub(crate) fn unfix_natives(&mut self, hs: &[*mut GcHeader]) {
        // SAFETY: each of `hs` is a live native and the links of `fixed`
        // lead to live objects; only links, class and flag bytes are written
        unsafe {
            // the ones to take carry a class bit no fixed native has for the
            // walk, which clears it on each it takes; one with a colour is
            // not fixed (a fixed object has none) and is not looked for
            let mut left = 0;
            for &h in hs {
                if (*h).flags & COLOR_BITS == 0 {
                    (*h).cls |= STANDS;
                    left += 1;
                }
            }
            let mut link: *mut *mut GcHeader = &mut self.fixed;
            while left > 0 && !(*link).is_null() {
                let h = *link;
                if (*h).cls & STANDS != 0 {
                    *link = (*h).next;
                    (*h).cls &= !STANDS;
                    left -= 1;
                } else {
                    link = &mut (*h).next;
                }
            }
            // one with a colour, or whose class bit the walk left set, was
            // not on `fixed`, and stays where it is
            for &h in hs {
                if (*h).flags & COLOR_BITS != 0 {
                    continue;
                }
                if (*h).cls & STANDS != 0 {
                    (*h).cls &= !STANDS;
                    continue;
                }
                (*h).next = self.all;
                (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
                self.all = h;
                acct::set_uncounted(h, true);
            }
        }
    }
}
