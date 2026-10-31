//! Which natives the call path runs itself instead of calling their
//! function, fixed when the closure is made.

/// See [`crate::runtime::NativeClosure::kind`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NativeKind {
    Plain,
    Async,
    Pcall,
    Xpcall,
    /// `Vm::call_value_with_handler`: not a level of the stack
    HostXpcall,
    /// `Vm::call_value_with_handler_in_c`: a level of the stack
    HostXpcallInC,
    /// `Vm::call_value_in_c`: a level of the stack, no message handler
    HostPcallInC,
    /// the C API's `lua_pcallk` without a message handler: not a level of
    /// the stack
    HostPcall,
    Pairs,
}

impl NativeKind {
    /// The kind of a synchronous native that is `builtin`.
    pub(crate) fn of(builtin: crate::runtime::Builtin) -> NativeKind {
        use crate::runtime::Builtin as B;
        match builtin {
            B::Pcall => NativeKind::Pcall,
            B::Xpcall => NativeKind::Xpcall,
            B::HostXpcall => NativeKind::HostXpcall,
            B::HostXpcallInC => NativeKind::HostXpcallInC,
            B::HostPcallInC => NativeKind::HostPcallInC,
            B::HostPcall => NativeKind::HostPcall,
            B::Pairs => NativeKind::Pairs,
            _ => NativeKind::Plain,
        }
    }
}
