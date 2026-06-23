//! Runtime errors: a Lua error is an arbitrary Lua value (usually a
//! string). Propagated as `Result<_, LuaError>` through the interpreter;
//! `pcall` catches it at the native boundary. Traceback capture lands with
//! the debug interfaces (P05).

use crate::runtime::Value;
use crate::runtime::table::TableError;
use std::fmt;

/// A Lua error: an arbitrary Lua value (almost always a string) plus
/// classification metadata recorded on the [`Vm`](crate::vm::Vm) side.
///
/// `LuaError` itself is `Copy` (16 bytes) — embedders matching on the
/// error value do so via `e.0`. Richer error context (kind, source,
/// traceback) lives on the Vm and is accessed through:
///
/// - [`Vm::error_text`](crate::vm::Vm::error_text) — formatted message
/// - [`Vm::error_kind`](crate::vm::Vm::error_kind) — [`LuaErrorKind`] classification
/// - [`Vm::error_source`](crate::vm::Vm::error_source) — `(source_name, line)` of the most recent error
/// - [`Vm::take_error_traceback`](crate::vm::Vm::take_error_traceback) — formatted traceback
///
/// ```
/// use luna_core::vm::{Vm, LuaError, LuaErrorKind};
/// use luna_core::version::LuaVersion;
///
/// let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
/// let err: LuaError = vm.eval("syntax error here").unwrap_err();
/// // The error value is a string (PUC convention):
/// assert!(err.0.try_as_str().is_some());
/// // Vm-side metadata records the classification:
/// assert_eq!(vm.error_kind(), LuaErrorKind::Syntax);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct LuaError(pub Value);

impl LuaError {
    /// Construct a `LuaError` carrying `Value::Nil` (the cheap default
    /// for trait conversions that don't have a `&mut Vm` to intern a
    /// rich message with). Callers with a Vm typically use
    /// [`LuaError::message`].
    pub fn nil() -> LuaError {
        LuaError(Value::Nil)
    }

    /// Construct a `LuaError` from a `Value` directly.
    pub fn new(v: Value) -> LuaError {
        LuaError(v)
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::Str(s) => match std::str::from_utf8(s.as_bytes()) {
                Ok(t) => f.write_str(t),
                Err(_) => write!(f, "{}", String::from_utf8_lossy(s.as_bytes())),
            },
            Value::Nil => f.write_str("(nil error)"),
            other => write!(f, "(error object is a {} value)", other.type_name()),
        }
    }
}

impl std::error::Error for LuaError {
    /// Lua errors do not carry a chained Rust-side `source` — the
    /// causal chain (`__index` failure → metamethod error → top-level
    /// failure) is captured in the `traceback` accessible via
    /// `Vm::take_error_traceback`.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl From<TableError> for LuaError {
    /// Lift a `TableError` into a `LuaError` carrying `Value::Nil`.
    /// Heap-free so it composes with `?` in functions that don't have
    /// a `&mut Vm`. The classification is later refined when the error
    /// crosses a Vm boundary (caller can set
    /// [`Vm::set_error_kind`](crate::vm::Vm::set_error_kind) if needed).
    fn from(_: TableError) -> LuaError {
        LuaError(Value::Nil)
    }
}

/// Classification of the most recent error raised on a Vm.
///
/// Embedders switch on this to decide whether to retry (`InstrBudget`,
/// `MemoryCap`), report (`Runtime`, `Syntax`), or treat as fatal
/// (`Native`, `OutOfMemory`). Default is [`LuaErrorKind::Runtime`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LuaErrorKind {
    /// Generic Lua runtime error (`error(...)`, type errors, missing
    /// global, etc.). The default classification.
    #[default]
    Runtime,
    /// Source did not parse (lexer or parser rejected the input).
    Syntax,
    /// `vm.set_instr_budget(Some(N))` budget exhausted mid-call.
    InstrBudget,
    /// `vm.set_memory_cap(Some(N))` cap exceeded during allocation.
    MemoryCap,
    /// A `NativeFn` callback returned `Err(LuaError)` (host-side error).
    Native,
    /// Allocation failed (typically only on cap exhaustion in luna).
    OutOfMemory,
    /// `error` raised at a type boundary (e.g. arithmetic on a table).
    Type,
}

impl fmt::Display for LuaErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            LuaErrorKind::Runtime => "runtime",
            LuaErrorKind::Syntax => "syntax",
            LuaErrorKind::InstrBudget => "instr-budget",
            LuaErrorKind::MemoryCap => "memory-cap",
            LuaErrorKind::Native => "native",
            LuaErrorKind::OutOfMemory => "out-of-memory",
            LuaErrorKind::Type => "type",
        };
        f.write_str(s)
    }
}
