//! Debug records of a compiled function: its upvalues and local variables.

/// Where a closure's upvalue is captured from, relative to the *enclosing*
/// function (PUC Upvaldesc).
#[derive(Clone, Debug)]
pub struct UpvalDesc {
    /// captured from the enclosing frame's registers (true) or from the
    /// enclosing closure's own upvalues (false)
    pub in_stack: bool,
    /// Index in the enclosing frame's register file (when `in_stack`) or
    /// in the enclosing closure's upvalue array (otherwise).
    pub index: u8,
    /// variable name, for error messages and debug info
    pub name: Box<str>,
    /// the captured variable is `<const>` (5.5): assignment through this
    /// upvalue is a compile-time error
    pub read_only: bool,
}

/// Debug record for a local variable: its name and the pc range over which it
/// occupies register `reg`. Used to name registers in error messages and
/// debug.getinfo (PUC LocVar).
#[derive(Clone, Debug)]
pub struct LocVar {
    /// Local-variable name.
    pub name: Box<str>,
    /// Register holding the variable while in scope.
    pub reg: u32,
    /// First pc where the variable is live.
    pub start_pc: u32,
    /// Pc one past the last where the variable is live.
    pub end_pc: u32,
}
