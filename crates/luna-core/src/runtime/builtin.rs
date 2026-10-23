//! Which library function a native closure is.

/// The library functions whose identity the call path, an error message or
/// compiled code depends on, tagged on the closure when it is created.
/// Natives are never told apart by their function's address: Rust
/// promises neither that a function has a single address nor that two
/// functions have two (identical bodies may be merged into one).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Builtin {
    /// any other native, an embedder's included
    None,
    /// `pcall`
    Pcall,
    /// `xpcall`
    Xpcall,
    /// `Vm::call_value_with_handler`'s protected call
    HostXpcall,
    /// `Vm::call_value_with_handler_in_c`'s protected call
    HostXpcallInC,
    /// `Vm::call_value_in_c`'s protected call
    HostPcallInC,
    /// the C API's `lua_pcallk` without a message handler
    HostPcall,
    /// `pairs`
    Pairs,
    /// `error`
    Error,
    /// `tostring`
    Tostring,
    /// the iterator `ipairs` returns
    IpairsIter,
    /// `math.sin`
    MathSin,
    /// `math.cos`
    MathCos,
    /// `math.tan`
    MathTan,
    /// `math.asin`
    MathAsin,
    /// `math.acos`
    MathAcos,
    /// `math.atan`
    MathAtan,
    /// `math.exp`
    MathExp,
    /// `math.log`
    MathLog,
    /// `math.sqrt`
    MathSqrt,
    /// `math.floor`
    MathFloor,
    /// `math.ceil`
    MathCeil,
    /// `math.max`
    MathMax,
    /// `math.min`
    MathMin,
    /// `math.fmod`
    MathFmod,
    /// `string.sub`
    StringSub,
    /// a C function the C API wraps
    CFunction,
}
