//! A prototype's JIT cache state.

/// What a backend stores in [`super::Proto::jit_next`] when it compiled no code
/// after all.
pub const JIT_NEXT_NONE: usize = usize::MAX;

/// Per-Proto JIT cache state. Copy so it fits a plain
/// `Cell` on the dispatch hot path (no `RefCell` borrow check); the
/// fn pointer's mmap is kept alive by `Vm.jit_handles`.
#[derive(Clone, Copy, Debug)]
pub enum JitProtoState {
    /// Compilation hasn't been attempted yet.
    Untried,
    /// Compilation was attempted and the body fell outside the whitelist;
    /// subsequent calls skip the attempt.
    Failed,
    /// Native code is installed and callable through the recorded entry.
    Compiled {
        /// Raw mmap'd code address. Transmute to the
        /// `unsafe extern "C" fn(i64, …) -> i64` shape matching
        /// `num_args` at the call site.
        entry: *const u8,
        /// 0..=MAX_JIT_ARITY. Picks the transmute target.
        num_args: u8,
        /// True when the Lua chunk terminates with `Return1` (single
        /// observable return value). False means the chunk only
        /// side-effects + `Return0` — host gets an empty `Vec<Value>`
        /// from `Vm::call_value`, an interpreter `Op::Call` gets
        /// zero results pushed (PUC nresults handling).
        returns_one: bool,
        /// Per-arg Float bit. Bit `i = 1` ↔ arg slot `i`
        /// is f64 (passed as i64 bit-pattern across the ABI, bitcast
        /// inside the JIT). Bit `i = 0` ↔ Int. Bits ≥ MAX_JIT_ARITY
        /// are zero.
        arg_float_mask: u8,
        /// Per-arg Table bit. Bit `i = 1` ↔ arg slot `i`
        /// is `Gc<Table>` raw ptr (passed as the i64 pointer value
        /// directly, since `Gc<Table>` is `NonNull<Table>` =
        /// pointer-shaped). Mutually exclusive with `arg_float_mask`
        /// for the same bit. Required so `try_jit_call_op`'s arg
        /// marshalling can accept `Value::Table(t)` and pack
        /// `t.as_ptr() as i64`; without it a Table arg would fall
        /// into the dispatcher's default-deny match arm and the
        /// callee couldn't be reached via JIT.
        arg_table_mask: u8,
        /// True iff the chunk's `Return1` value is f64.
        /// Dispatcher wraps `r` as `Value::Float(f64::from_bits(r))`
        /// vs `Value::Int(r)` accordingly. Meaningful only when
        /// `returns_one == true`.
        ret_is_float: bool,
        /// True iff the chunk's `Return1` value is a
        /// `Gc<Table>` ptr. Mutually exclusive with `ret_is_float`.
        /// Dispatcher wraps `r` as
        /// `Value::Table(Gc::from_ptr(r as *mut Table))`.
        ret_is_table: bool,
    },
}

// Cell<JitProtoState> stores raw pointers; explicit Send + Sync
// negative: keep these on a single-threaded runtime. The Vm itself
// already is !Send (Heap holds raw GcHeader pointers), so we don't
// need any auto-trait gymnastics — this comment exists so a future
// audit doesn't try to flip the trait without thinking.
