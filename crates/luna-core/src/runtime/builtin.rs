//! Which library function a native closure is.

/// The library functions whose identity the call path, an error message or
/// compiled code depends on, tagged on the closure when it is created.
/// Natives are never told apart by their function's address: Rust
/// promises neither that a function has a single address nor that two
/// functions have two (identical bodies may be merged into one).
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Builtin {
    /// any other native, an embedder's included
    None,
    Pcall,
    Xpcall,
    /// `Vm::call_value_with_handler`'s protected call
    HostXpcall,
    /// `Vm::call_value_with_handler_in_c`'s protected call
    HostXpcallInC,
    /// `Vm::call_value_in_c`'s protected call
    HostPcallInC,
    /// the C API's `lua_pcallk` without a message handler
    HostPcall,
    Pairs,
    Error,
    Tostring,
    /// the iterator `ipairs` returns
    IpairsIter,
    MathSin,
    MathCos,
    MathTan,
    MathAsin,
    MathAcos,
    MathAtan,
    MathExp,
    MathLog,
    MathSqrt,
    MathFloor,
    MathCeil,
    MathMax,
    MathMin,
    MathFmod,
    StringSub,
    /// a C function the C API wraps
    CFunction,
}
