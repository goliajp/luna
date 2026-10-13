//! Object constructors. Each `try_` constructor fails with [`Oom`] when the
//! allocation context refuses, leaving the heap as it was; the constructor
//! of the same name without `try_` ends the process instead, as the
//! standard library's containers do, for embedders that do not handle
//! memory errors.

use super::*;
use crate::runtime::mem::{LSlice, LVec, Oom, oom_abort};
use std::alloc::Layout;

impl Heap {
    /// A heap whose memory comes from `mem`, with a seed of its own.
    pub fn new_on(mem: crate::runtime::mem::MemOwner) -> Heap {
        Heap::with_mem(mem, make_seed())
    }

    /// The allocation context.
    pub fn mem_ctx(&self) -> &crate::runtime::mem::MemCtx {
        self.mem().ctx()
    }

    /// A block for one `T` from the allocation context.
    #[inline]
    fn alloc_block<T: GcObject>(&self) -> Result<*mut T, Oom> {
        match self.mem().ctx().alloc(Layout::new::<T>(), T::KIND) {
            Some(p) => Ok(p.cast::<T>().as_ptr()),
            None => Err(Oom(self.mem())),
        }
    }

    /// Drop the object `p` points at and give its block back.
    ///
    /// # Safety
    /// `p` is an object this heap allocated with [`Heap::alloc_block`],
    /// unlinked, which nothing uses afterwards.
    pub(super) unsafe fn free_block<T: GcObject>(&self, p: *mut T) {
        // SAFETY: the caller's contract: the object is initialized and its
        // block of this layout came from this heap's context
        unsafe {
            ptr::drop_in_place(p);
            self.mem().ctx().free(
                std::ptr::NonNull::new_unchecked(p.cast()),
                Layout::new::<T>(),
            );
        }
    }

    /// Put `obj` in a new block and under GC management. On failure `obj`
    /// is dropped.
    #[inline]
    pub(crate) fn try_adopt<T: GcObject>(&mut self, obj: T) -> Result<Gc<T>, Oom> {
        let p = self.alloc_block::<T>()?;
        // SAFETY: `p` is a fresh block for one `T`, linked nowhere yet, and a
        // `GcObject` starts with its header; once linked the heap owns the
        // object until a collect finds it unreachable
        unsafe {
            p.write(obj);
            self.link(p as *mut GcHeader);
        }
        self.bytes += std::mem::size_of::<T>();
        // SAFETY: as above
        Ok(unsafe { Gc::from_ptr(p) })
    }

    /// Allocate and adopt a fresh empty [`Table`].
    pub fn new_table(&mut self) -> Gc<Table> {
        self.try_new_table()
            .unwrap_or_else(|_| oom_abort(Layout::new::<Table>()))
    }

    /// [`Heap::new_table`], failing when memory runs out.
    #[inline]
    pub fn try_new_table(&mut self) -> Result<Gc<Table>, Oom> {
        // tables freed by earlier allocation bursts wait in the pool: one
        // is reset instead of allocating afresh (~30 ns a table)
        let p = if let Some(ptr) = self.table_pool.pop() {
            let t = ptr.as_ptr();
            // gc-verify: the pointer is alive again — drop it from the
            // freed log so read-time probes don't flag the recycled table.
            #[cfg(feature = "gc-verify")]
            {
                self.recently_freed.remove(&(t as usize));
                crate::runtime::gc_verify_probe::FREED
                    .with(|f| f.borrow_mut().remove(&(t as usize)));
            }
            // SAFETY: `t` came off `table_pool`, which holds only tables `free_obj` unlinked from every list and emptied of their interior allocations; the pool's pointer was the only one, and these writes restore the fields left behind
            unsafe {
                (*t).hdr = GcHeader::new(ObjTag::Table);
                (*t).inline_storage =
                    std::cell::UnsafeCell::new([0; crate::runtime::table::INLINE_U64S]);
                (*t).lastfree = 0;
            }
            t
        } else {
            let t = self.alloc_block::<Table>()?;
            // SAFETY: `t` is a fresh block for one table
            unsafe { t.write(Table::new(GcHeader::new(ObjTag::Table))) };
            t
        };
        // SAFETY: `p` is a fresh block or a pool table reset above, linked nowhere yet
        unsafe { self.link(p as *mut GcHeader) };
        self.bytes += std::mem::size_of::<Table>();
        // the Table is now at its final heap address; wire `array_ptr` to
        // point at the inline storage that lives inside it
        // SAFETY: `p` is the table linked just above, which the heap now manages; no other handle or reference to it exists yet
        unsafe {
            let g = Gc::from_ptr(p);
            let t = g.as_mut();
            t.init_array_ptr();
            t.hdr.sub = self.table_dialect as u8;
            t.alimit.set(0);
            t.lenhint.set(0);
            Ok(g)
        }
    }

    /// Give the pooled tables' blocks back to the allocation context (an
    /// emergency collection does, so they can serve other allocations), and
    /// the gray stack's block, which the emergency collection's roots may
    /// have grown past what the collector otherwise needs.
    pub(crate) fn release_pool(&mut self) {
        for &ptr in self.table_pool.take().iter() {
            // SAFETY: the pool holds unlinked tables whose interiors were
            // freed when they were pooled; taking the pool made this the
            // only pointer to each
            unsafe { self.free_block(ptr.as_ptr()) };
        }
        if self.gray.is_empty() && !self.gray_overflow {
            self.gray = LVec::new(self.mem());
        }
    }

    /// Adopt an empty table and pre-allocate `asize` NIL slots in the array
    /// part: the array reaches its final size in one allocation rather than
    /// O(log N) doubling rounds. Sizes past the array part's hard ceiling
    /// (`2^27`) give an empty table, which the interpreter grows as needed.
    pub fn new_table_sized(&mut self, asize: usize) -> Gc<Table> {
        self.try_new_table_sized(asize)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Table>()))
    }

    /// [`Heap::new_table_sized`], failing when memory runs out.
    pub fn try_new_table_sized(
        &mut self,
        asize: usize,
    ) -> Result<Gc<Table>, crate::runtime::table::TableError> {
        const MAX_ASIZE_HINT: usize = 1 << 27;
        let g = self.try_new_table()?;
        let clamped = asize.min(MAX_ASIZE_HINT);
        if clamped > 0 {
            // SAFETY: the freshly adopted table has no live borrow
            // anywhere else; we hold the only `Gc<Table>` handle.
            unsafe { g.as_mut() }.resize(self, clamped, 0)?;
        }
        Ok(g)
    }

    /// A table sized by `NewTable`'s operands, as the interpreter makes
    /// it (see `table::new_table_sizes`); `None` for sizes past a table's
    /// limit. Public for the JIT crates.
    #[doc(hidden)]
    pub fn new_table_presized(&mut self, b: u32, c: u32, k: bool) -> Option<Gc<Table>> {
        self.try_new_table_presized(b, c, k)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Table>()))
    }

    /// [`Heap::new_table_presized`], failing when memory runs out.
    #[doc(hidden)]
    pub fn try_new_table_presized(
        &mut self,
        b: u32,
        c: u32,
        k: bool,
    ) -> Result<Option<Gc<Table>>, crate::runtime::table::TableError> {
        let Some((asize, hsize)) = crate::runtime::table::new_table_sizes(b, c, k) else {
            return Ok(None);
        };
        let g = self.try_new_table()?;
        if asize != 0 || hsize != 0 {
            // SAFETY: the table was made just above; this is its only handle
            unsafe { g.as_mut() }.resize(self, asize, hsize)?;
        }
        Ok(Some(g))
    }

    /// Adopt a compiler-built prototype (its `hdr` must carry ObjTag::Proto).
    pub fn adopt_proto(&mut self, proto: Proto) -> Gc<Proto> {
        self.try_adopt_proto(proto)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Proto>()))
    }

    /// [`Heap::adopt_proto`], failing when memory runs out.
    pub fn try_adopt_proto(&mut self, proto: Proto) -> Result<Gc<Proto>, Oom> {
        debug_assert!(proto.hdr.tag == ObjTag::Proto);
        self.try_adopt(proto)
    }

    /// Back-compat constructor for callers that already
    /// built a `Box<[Gc<Upvalue>]>`: [`Heap::new_closure_inline`] copies
    /// the handles, and the box is freed after.
    pub fn new_closure(&mut self, proto: Gc<Proto>, upvals: Box<[Gc<Upvalue>]>) -> Gc<LuaClosure> {
        self.new_closure_inline(proto, &upvals)
    }

    /// Hot-path constructor for the `Op::Closure` handler.
    /// Takes a slice (typically backed by a stack array) so the caller
    /// doesn't allocate a Vec/Box just to hand it over. Upvals are
    /// copied into `inline_storage` for small closures, or into a
    /// block from the allocation context for the rare overflow case.
    pub fn new_closure_inline(
        &mut self,
        proto: Gc<Proto>,
        upvals: &[Gc<Upvalue>],
    ) -> Gc<LuaClosure> {
        self.try_new_closure(proto, upvals)
            .unwrap_or_else(|_| oom_abort(Layout::new::<LuaClosure>()))
    }

    /// [`Heap::new_closure_inline`], failing when memory runs out.
    #[inline]
    pub fn try_new_closure(
        &mut self,
        proto: Gc<Proto>,
        upvals: &[Gc<Upvalue>],
    ) -> Result<Gc<LuaClosure>, Oom> {
        use crate::runtime::function::INLINE_UPVALS_N;
        let n = upvals.len();
        let store = if n > INLINE_UPVALS_N {
            Some(LSlice::from_slice(self.mem(), upvals)?)
        } else {
            None
        };
        let p = self.alloc_block::<LuaClosure>()?;
        // SAFETY: `p` is a fresh block for one closure, linked nowhere yet;
        // the storage is filled at its final address so `upvals_ptr` can
        // point into it, and the inline array is written through its cell
        // (see the field doc)
        let g = unsafe {
            p.write(LuaClosure {
                hdr: GcHeader::new(ObjTag::Closure),
                proto,
                code: proto.code.as_ptr(),
                consts: proto.consts.as_ptr(),
                upvals_ptr: std::ptr::null_mut(),
                upvals_len: n as u32,
                inline_storage: std::cell::UnsafeCell::new(
                    [std::mem::MaybeUninit::<Gc<Upvalue>>::uninit(); INLINE_UPVALS_N],
                ),
            });
            let c = &mut *p;
            match store {
                Some(s) => c.set_overflow(s),
                None => {
                    for (i, &uv) in upvals.iter().enumerate() {
                        (*c.inline_storage.get())[i] = std::mem::MaybeUninit::new(uv);
                    }
                }
            }
            self.link(p as *mut GcHeader);
            Gc::from_ptr(p)
        };
        self.bytes += std::mem::size_of::<LuaClosure>();
        // SAFETY: `g` is the closure linked just above; no other handle or reference to it exists yet
        unsafe { g.as_mut() }.init_upvals_ptr();
        Ok(g)
    }

    /// The items of `v` in a block of exactly their number from the
    /// context, or the failure to allocate it.
    pub(crate) fn try_block_of<T>(
        &self,
        v: impl ExactSizeIterator<Item = T>,
    ) -> Result<LSlice<T>, crate::runtime::mem::Oom> {
        LSlice::collect_exact(self.mem(), v)
    }

    /// The items of `v` in a block of exactly their number from the
    /// context.
    pub(crate) fn block_of<T>(&self, v: impl ExactSizeIterator<Item = T>) -> LSlice<T> {
        let n = v.len();
        LSlice::collect_exact(self.mem(), v)
            .unwrap_or_else(|_| oom_abort(Layout::array::<T>(n).unwrap_or(Layout::new::<T>())))
    }

    /// Allocate a fresh [`Upvalue`] cell in the given `state` (open / closed).
    pub fn new_upvalue(&mut self, state: UpvalState) -> Gc<Upvalue> {
        self.try_new_upvalue(state)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Upvalue>()))
    }

    /// [`Heap::new_upvalue`], failing when memory runs out.
    #[inline]
    pub fn try_new_upvalue(&mut self, state: UpvalState) -> Result<Gc<Upvalue>, Oom> {
        self.try_adopt(Upvalue {
            hdr: GcHeader::new(ObjTag::Upvalue),
            state,
        })
    }

    /// Create a fresh suspended coroutine wrapping `body`. The new thread
    /// inherits the creator's globals table; a `setfenv(0, env)` inside it
    /// will retune that copy without affecting the creator.
    pub fn new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Gc<crate::runtime::Coro> {
        self.try_new_coro(body, globals)
            .unwrap_or_else(|_| oom_abort(Layout::new::<crate::runtime::Coro>()))
    }

    /// [`Heap::new_coro`], failing when memory runs out.
    pub fn try_new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Result<Gc<crate::runtime::Coro>, Oom> {
        let mem = self.mem();
        self.try_adopt(crate::runtime::Coro {
            hdr: GcHeader::new(ObjTag::Coro),
            status: crate::runtime::CoroStatus::Suspended,
            body,
            started: false,
            resumer: None,
            resume_at: None,
            error_value: None,
            error_status: crate::runtime::coroutine::ErrorStatus::Run,
            error_traceback: None,
            error_levels: None,
            natives: 0..0,
            stack: LVec::new(mem),
            frames: LVec::new(mem),
            open_upvals: LVec::new(mem),
            tbc: LVec::new(mem),
            top: 0,
            meta_conts: 0,
            stale_frames: 0,
            stack_size: 0,
            frame_size: 0,
            hook: crate::vm::exec::HookState::default(),
            globals,
            host_stack: LVec::new(mem),
            host_state: None,
        })
    }

    /// Create a userdata (an io file handle — luna's only userdata) with no
    /// metatable yet; the io library installs the shared `FILE*` metatable.
    pub fn new_userdata(&mut self, payload: UserdataPayload, writable: bool) -> Gc<Userdata> {
        self.try_new_userdata(payload, writable)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Userdata>()))
    }

    /// [`Heap::new_userdata`], failing when memory runs out.
    pub fn try_new_userdata(
        &mut self,
        payload: UserdataPayload,
        writable: bool,
    ) -> Result<Gc<Userdata>, Oom> {
        self.try_adopt(Userdata::new(
            GcHeader::new(ObjTag::Userdata),
            payload,
            writable,
        ))
    }
}
