//! The calling convention: System V on Unix, Win64 on Windows.

#[cfg(not(windows))]
mod sysv;
#[cfg(not(windows))]
pub(super) use sysv::*;
#[cfg(windows)]
mod win64;
#[cfg(windows)]
pub(super) use win64::*;
