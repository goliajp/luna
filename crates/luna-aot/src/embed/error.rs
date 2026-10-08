//! The AOT pipeline's errors.

use std::io;

/// Errors surfaced by the AOT pipeline. Variants intentionally carry
/// the upstream message verbatim so the CLI can pass it through to
/// `stderr` without re-formatting (and so future structured-error
/// consumers can match on the variant tag).
#[derive(Debug)]
pub enum AotError {
    /// Reading the Lua source file failed (missing, permission, ...).
    Io(io::Error),
    /// Parser or compiler rejected the source (PUC-style line/message).
    Syntax(String),
    /// Object-file emission failed (unsupported target triple,
    /// internal `object`-crate error).
    Object(String),
    /// Linker (`cc` / user-supplied driver) failed. Carries the
    /// linker's stderr verbatim so users can diagnose toolchain
    /// issues without re-running.
    Link(String),
    /// The target triple isn't supported. The scaffold path rejects
    /// anything other than the host triple;
    /// [`compile_and_link`](super::compile_and_link) rejects triples
    /// [`TargetSpec::from_triple`](super::TargetSpec::from_triple) can't
    /// describe.
    UnsupportedTarget(String),
}

impl std::fmt::Display for AotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AotError::Io(e) => write!(f, "io error: {e}"),
            AotError::Syntax(msg) => write!(f, "syntax error: {msg}"),
            AotError::Object(msg) => write!(f, "object-file emission failed: {msg}"),
            AotError::Link(msg) => write!(f, "linker failed: {msg}"),
            AotError::UnsupportedTarget(t) => {
                write!(f, "unsupported target triple in scaffold session: {t}")
            }
        }
    }
}

impl std::error::Error for AotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AotError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for AotError {
    fn from(e: io::Error) -> Self {
        AotError::Io(e)
    }
}
