//! The array part's slab: allocation, release and the tag / payload views.

use super::*;

impl Table {
    /// Set `array_ptr` to the inline storage's stable heap
    /// address. Called by `Heap::new_table` once the Table is at its
    /// final location.
    #[inline]
    pub(crate) fn init_array_ptr(&mut self) {
        self.array_ptr = self.inline_storage.get() as *mut u8;
    }

    /// Freshly-derived base pointer for the array part. Rust-side
    /// accessors MUST use this instead of the cached `array_ptr`
    /// field when the backing is inline: a pointer into `*self`
    /// cached across `&mut self` boundaries is invalidated by every
    /// function-entry retag under Stacked Borrows — `&mut` retags
    /// ignore `UnsafeCell` (only `&` retags respect it), so the
    /// cached tag dies on the next method call (Miri:
    /// `retag ... tag does not exist in the borrow stack`). Deriving
    /// through `UnsafeCell::get()` at each use gives a fresh
    /// SharedReadWrite tag valid for reads AND writes even from
    /// `&self`. The slab case keeps the cached pointer: its tag
    /// lives on the heap allocation, outside `*self`, untouched by
    /// entry retags. `array_ptr` itself stays maintained for the
    /// JIT, whose emitted code loads the field directly (no Rust
    /// borrows involved).
    #[inline(always)]
    pub(super) fn array_base(&self) -> *mut u8 {
        if self.asize <= INLINE_ASIZE {
            self.inline_storage.get() as *mut u8
        } else {
            self.array_ptr
        }
    }

    /// Read view onto the array-part tag bytes. Trails
    /// the avals portion in the active backing (inline or slab).
    #[inline(always)]
    pub(crate) fn atags(&self) -> &[u8] {
        let n = self.asize as usize;
        if n == 0 {
            return &[];
        }
        // SAFETY: `array_ptr` always points to a buffer with `n`
        // RawVal slots followed by `n` u8 tag bytes (either
        // `inline_storage` of `INLINE_U64S` u64s, or a `slab` of
        // `asize + ceil(asize/8)` u64s). The tag bytes start at byte
        // offset `n * 8` from the buffer base.
        unsafe {
            let ptr = self.array_base().add(n * 8);
            std::slice::from_raw_parts(ptr, n)
        }
    }

    #[inline(always)]
    pub(crate) fn atags_mut(&mut self) -> &mut [u8] {
        let n = self.asize as usize;
        if n == 0 {
            return &mut [];
        }
        // SAFETY: `array_ptr` was allocated by `Heap::init_array_ptr` with `array_cap` slots; the table holds it for its lifetime and the heap is single-threaded so no concurrent writers exist.
        unsafe {
            let ptr = self.array_base().add(n * 8);
            std::slice::from_raw_parts_mut(ptr, n)
        }
    }

    /// Read view onto the array-part payload slots. Sits
    /// at the start of the active backing (u64-aligned, identical size
    /// and layout to `RawVal`).
    #[inline(always)]
    pub(crate) fn avals(&self) -> &[RawVal] {
        let n = self.asize as usize;
        if n == 0 {
            return &[];
        }
        // SAFETY: inline_storage / slab both store u64s, so the cast
        // to `*const RawVal` is alignment-safe (RawVal size = 8,
        // align = 8). The buffer holds at least `n` such slots.
        unsafe { std::slice::from_raw_parts(self.array_base() as *const RawVal, n) }
    }

    #[inline(always)]
    pub(crate) fn avals_mut(&mut self) -> &mut [RawVal] {
        let n = self.asize as usize;
        if n == 0 {
            return &mut [];
        }
        // SAFETY: `array_ptr` was allocated by `Heap::init_array_ptr` with `array_cap` slots; the table holds it for its lifetime and the heap is single-threaded so no concurrent writers exist.
        unsafe { std::slice::from_raw_parts_mut(self.array_base() as *mut RawVal, n) }
    }

    /// Layout of the external `[avals: asize × 8 bytes][atags: asize
    /// bytes]` slab, rounded up to whole u64s. Only used when
    /// `asize > INLINE_ASIZE`.
    pub(super) fn slab_layout(asize: usize) -> std::alloc::Layout {
        std::alloc::Layout::array::<u64>(asize + asize.div_ceil(8))
            .expect("array part within MAX_ASIZE")
    }

    /// Allocate a zeroed slab (avals = `RawVal::NIL` aka `0`; atags =
    /// `raw::NIL` aka `0`) for `asize > INLINE_ASIZE` slots.
    pub(super) fn alloc_slab(asize: usize) -> *mut u8 {
        let layout = Self::slab_layout(asize);
        // SAFETY: the layout is non-zero-sized (asize > INLINE_ASIZE > 0)
        let p = unsafe { std::alloc::alloc_zeroed(layout) };
        if p.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        p
    }

    /// Free the slab behind `ptr`, which `alloc_slab(asize)` returned.
    ///
    /// # Safety
    /// `ptr` came from `alloc_slab(asize)` with this same `asize` and is
    /// not used afterwards.
    pub(super) unsafe fn free_slab(ptr: *mut u8, asize: usize) {
        // SAFETY: per the contract, same pointer and layout as the allocation
        unsafe { std::alloc::dealloc(ptr, Self::slab_layout(asize)) }
    }

    /// Free the array part's slab, if it has one. The table must not read
    /// its array part again before setting up a new one.
    pub(super) fn free_array_slab(&mut self) {
        if self.asize > INLINE_ASIZE {
            // SAFETY: an array part larger than the inline storage lives in
            // a slab from `alloc_slab(asize)`, owned by this table
            unsafe { Self::free_slab(self.array_ptr, self.asize as usize) };
        }
    }

    /// Release the array part and leave an empty one on the inline storage
    /// (the pool recycles a freed table this way).
    pub(crate) fn drop_array_part(&mut self) {
        self.free_array_slab();
        self.asize = 0;
        self.acount = 0;
        self.aprefix = 0;
        self.array_ptr = self.inline_storage.get() as *mut u8;
    }
}
