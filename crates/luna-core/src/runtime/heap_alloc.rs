//! Object constructors and string interning.

use super::*;

impl Heap {
    /// Allocate and adopt a fresh empty [`Table`].
    pub fn new_table(&mut self) -> Gc<Table> {
        // table_pool fast path. When btrees-
        // style alloc bursts have left freed Tables in the pool, pop a
        // recycled one and reset its fields instead of mallocing fresh.
        // Saves ~30ns per alloc (malloc roundtrip elided).
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
                // Reset to fresh-Table state. Box-owned slab/nodes/
                // metatable were already cleared in `free_obj` before
                // pool push, so we only reset stack-resident fields here.
                (*t).hdr = GcHeader::new(ObjTag::Table);
                (*t).inline_storage =
                    std::cell::UnsafeCell::new([0; crate::runtime::table::INLINE_U64S]);
                (*t).lastfree = 0;
            }
            t
        } else {
            Box::into_raw(Box::new(Table::new(GcHeader::new(ObjTag::Table))))
        };
        // Link + bytes accounting (same as adopt path).
        // SAFETY: `p` is a fresh box or a pool table reset above, linked nowhere yet
        unsafe { self.link(p as *mut GcHeader) };
        self.bytes += std::mem::size_of::<Table>();
        // the Table is now at its final heap address; wire
        // `array_ptr` to point at the inline storage that lives inside
        // the boxed Table.
        // SAFETY: `p` is the table linked just above, which the heap now manages; no other handle or reference to it exists yet
        unsafe {
            let g = Gc::from_ptr(p);
            g.as_mut().init_array_ptr();
            g
        }
    }

    /// Adopt an empty table and pre-allocate `asize`
    /// NIL slots in the array part. Equivalent to `new_table()`
    /// followed by `set_int(N, Nil)` worth of `rehash`es, except
    /// the array reaches its final size in one allocation rather
    /// than O(log N) doubling rounds.
    ///
    /// `asize == 0` is identical to `new_table()`. Larger sizes
    /// are clamped at the array part's hard ceiling
    /// `MAX_ASIZE = 2^27`; requests beyond that fall back to the
    /// empty table, which the interpreter would have grown
    /// gracefully via rehash anyway.
    pub fn new_table_sized(&mut self, asize: usize) -> Gc<Table> {
        const MAX_ASIZE_HINT: usize = 1 << 27;
        let g = self.new_table();
        let clamped = asize.min(MAX_ASIZE_HINT);
        if clamped > 0 {
            // SAFETY: the freshly adopted table has no live borrow
            // anywhere else; we hold the only `Gc<Table>` handle.
            unsafe { g.as_mut() }.resize(self, clamped, 0);
        }
        g
    }

    /// Adopt a compiler-built prototype (its `hdr` must carry ObjTag::Proto).
    pub fn adopt_proto(&mut self, proto: Proto) -> Gc<Proto> {
        debug_assert!(proto.hdr.tag == ObjTag::Proto);
        self.adopt(Box::new(proto))
    }

    /// Back-compat constructor for callers that already
    /// built a `Box<[Gc<Upvalue>]>`. Internally re-routes through
    /// `new_closure_inline` so small-upval cases also pick the
    /// inline path (the input Box is freed after the copy).
    pub fn new_closure(&mut self, proto: Gc<Proto>, upvals: Box<[Gc<Upvalue>]>) -> Gc<LuaClosure> {
        use crate::runtime::function::INLINE_UPVALS_N;
        let n = upvals.len();
        if n <= INLINE_UPVALS_N {
            let g = self.new_closure_inline(proto, &upvals);
            drop(upvals);
            g
        } else {
            // large closure: the input Box becomes its storage, no copy
            self.adopt_closure_with(proto, n as u32, |c| c.set_overflow(upvals))
        }
    }

    /// Hot-path constructor for the `Op::Closure` handler.
    /// Takes a slice (typically backed by a stack array) so the caller
    /// doesn't allocate a Vec/Box just to hand it over. Upvals are
    /// copied into `inline_storage` for small closures, or into a
    /// freshly-allocated `Box<[..]>` for the rare overflow case.
    pub fn new_closure_inline(
        &mut self,
        proto: Gc<Proto>,
        upvals: &[Gc<Upvalue>],
    ) -> Gc<LuaClosure> {
        use crate::runtime::function::INLINE_UPVALS_N;
        let n = upvals.len();
        self.adopt_closure_with(proto, n as u32, |c| {
            if n <= INLINE_UPVALS_N {
                for (i, &uv) in upvals.iter().enumerate() {
                    // SAFETY: exclusive &mut c inside the constructor;
                    // write through the cell so no direct borrow of
                    // the inline array is formed (see the field doc).
                    unsafe {
                        (*c.inline_storage.get())[i] = std::mem::MaybeUninit::new(uv);
                    }
                }
            } else {
                c.set_overflow(upvals.to_vec().into_boxed_slice());
            }
        })
    }

    pub(super) fn adopt_closure_with<F: FnOnce(&mut LuaClosure)>(
        &mut self,
        proto: Gc<Proto>,
        upvals_len: u32,
        fill: F,
    ) -> Gc<LuaClosure> {
        let mut boxed = Box::new(LuaClosure {
            hdr: GcHeader::new(ObjTag::Closure),
            proto,
            code: proto.code.as_ptr(),
            consts: proto.consts.as_ptr(),
            upvals_ptr: std::ptr::null_mut(),
            upvals_len,
            inline_storage: std::cell::UnsafeCell::new(
                [std::mem::MaybeUninit::<Gc<Upvalue>>::uninit();
                    crate::runtime::function::INLINE_UPVALS_N],
            ),
        });
        // Box is heap-stable now — populate storage at the final
        // address so `upvals_ptr` will be valid.
        fill(&mut boxed);
        let g = self.adopt(boxed);
        // SAFETY: `g` is the closure `adopt` just returned; no other handle or reference to it exists yet
        unsafe { g.as_mut() }.init_upvals_ptr();
        g
    }

    /// Allocate a [`NativeClosure`] wrapping host function `f` with the
    /// given captured upvalues.
    pub fn new_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        let fix = self.fix_natives && upvals.is_empty();
        let g = self.adopt(Box::new(NativeClosure {
            hdr: GcHeader::native(&upvals),
            f,
            upvals,
            is_async: false,
            kind: crate::vm::exec::native_call::NativeKind::of(f),
        }));
        if fix {
            // SAFETY: `adopt` just linked `g` at the head of `all`. PUC `luaC_fix`:
            // onto `fixed`, gray, so marking, barriers and weak tables skip it
            unsafe {
                let h = g.as_ptr() as *mut GcHeader;
                self.all = (*h).next;
                (*h).next = self.fixed;
                (*h).flags &= !COLOR_BITS;
                self.fixed = h;
            }
        }
        g
    }

    /// Like [`Heap::new_native`] but tags the
    /// closure with `is_async = true`. The dispatcher's native-call
    /// path then transmutes `f` to `AsyncNativeFn` and routes through
    /// the cooperative-yield path. The caller is responsible for
    /// having transmuted the `AsyncNativeFn` pointer to `NativeFn`
    /// shape (both are `fn` pointers of the same size); see
    /// [`crate::vm::async_drive`] for the helper that does this.
    pub fn new_async_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        self.adopt(Box::new(NativeClosure {
            hdr: GcHeader::native(&upvals),
            f,
            upvals,
            is_async: true,
            kind: crate::vm::exec::native_call::NativeKind::Async,
        }))
    }

    /// Allocate a fresh [`Upvalue`] cell in the given `state` (open / closed).
    pub fn new_upvalue(&mut self, state: UpvalState) -> Gc<Upvalue> {
        self.adopt(Box::new(Upvalue {
            hdr: GcHeader::new(ObjTag::Upvalue),
            state,
        }))
    }

    /// Create a fresh suspended coroutine wrapping `body`. The new thread
    /// inherits the creator's globals table; a `setfenv(0, env)` inside it
    /// will retune that copy without affecting the creator.
    pub fn new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Gc<crate::runtime::Coro> {
        self.adopt(Box::new(crate::runtime::Coro {
            hdr: GcHeader::new(ObjTag::Coro),
            status: crate::runtime::CoroStatus::Suspended,
            body,
            started: false,
            resumer: None,
            resume_at: None,
            error_value: None,
            error_traceback: None,
            error_levels: None,
            natives: 0..0,
            stack: Vec::new(),
            frames: Vec::new(),
            open_upvals: Vec::new(),
            tbc: Vec::new(),
            top: 0,
            pcall_depth: 0,
            hook: crate::vm::exec::HookState::default(),
            globals,
        }))
    }

    /// Create a userdata (an io file handle — luna's only userdata) with no
    /// metatable yet; the io library installs the shared `FILE*` metatable.
    pub fn new_userdata(&mut self, payload: UserdataPayload, writable: bool) -> Gc<Userdata> {
        self.adopt(Box::new(Userdata::new(
            GcHeader::new(ObjTag::Userdata),
            payload,
            writable,
        )))
    }

    /// Create (or find) a string. Short strings (≤ 40 bytes) are interned.
    pub fn intern(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        if bytes.len() <= string::MAX_SHORT_LEN {
            let (p, is_new) = self.strings.intern(bytes, self.seed);
            if is_new {
                // SAFETY: `StringTable::intern` just allocated `p` and put it only in its own bucket chain, which does not link objects
                unsafe { self.link(p as *mut GcHeader) };
                self.bytes += string::alloc_size(bytes.len());
            } else {
                // PUC `luaS_new` resurrect guard (lstring.c).
                // The bucket-chain is walked open-loop without consulting GC
                // color; during incremental sweep an existing entry may be
                // dead-white (in `sweep_cur`, scheduled for `free_obj`). If we
                // hand its pointer back, the budget-paced sweep frees it out
                // from under the mutator and the next bucket walk dereferences
                // a libc-recycled slot — the symptom recorded in
                // `0x800002a80000002d` deep in `StringTable::intern`).
                //
                // Flip the white bits to `current_white` so the upcoming sweep
                // skips it (PUC `changewhite`). Black / not-white objects are
                // already safe and untouched.
                // SAFETY: `p` came from `StringTable::intern` and is a valid
                // `LuaStr` header (its bucket chain is consistent under our
                // single-threaded heap).
                unsafe {
                    let f = (*(p as *mut GcHeader)).flags;
                    if is_white(f) && (f & self.current_white) == 0 {
                        (*(p as *mut GcHeader)).flags = (f & !WHITE_BITS) | self.current_white;
                    }
                }
            }
            // SAFETY: `p` is an interned string the heap manages: new and linked above, or found in the table and kept from this cycle's sweep by the recoloring above
            unsafe { Gc::from_ptr(p) }
        } else {
            let p = string::alloc_long(bytes, self.seed);
            // SAFETY: `alloc_long` just allocated `p`, linked nowhere yet
            unsafe { self.link(p as *mut GcHeader) };
            self.bytes += string::alloc_size(bytes.len());
            // SAFETY: `p` is the string linked just above
            unsafe { Gc::from_ptr(p) }
        }
    }
}
