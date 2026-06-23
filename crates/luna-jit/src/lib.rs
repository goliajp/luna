#![warn(missing_docs)]
//! luna — a Lua runtime in pure Rust with a Cranelift-backed JIT.
//!
//! Primary dialect: Lua 5.5 (tracks official upstream).
//! Compat modes: Lua 5.1 / 5.2 / 5.3 / 5.4.
//!
//! This crate is `luna-core` plus:
//! - the Cranelift-backed method + trace JIT (`jit_backend`);
//! - the `lua.h`-compatible C ABI (`capi`);
//! - the `luna` CLI bin.
//!
//! Pure-Rust embedders that don't want the JIT (and don't want
//! Cranelift in their dependency tree) can depend on `luna-core`
//! directly — its API surface is a subset of this crate's.
//!
//! # Embedding contract
//!
//! ```no_run
//! use luna_jit::{Vm, LuaVersion};
//! // JIT-equipped Vm, fully-loaded stdlib:
//! let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
//! vm.eval("return 1 + 2").unwrap();
//! ```
//!
//! Sandbox embedders should call `vm.set_jit_enabled(false)` and
//! `vm.set_bytecode_loading(false)` before running untrusted
//! scripts. See `luna-core` rustdoc for the full caveat list.

// v1.1 A1 Session C — re-export everything from luna-core so existing
// `use luna_jit::vm::Vm`, `use luna_jit::version::LuaVersion`,
// `use luna_jit::runtime::Value`, etc. paths continue to resolve. The
// JIT-bearing surface (CraneliftBackend, the `luna_jit_*` helpers,
// `enter_jit`, `cache_lookup_or_compile`, `try_compile_trace_with_options`)
// hangs off `luna_jit::jit_backend` / the unified `luna_jit::jit`
// re-export below.
pub use luna_core::*;

pub mod capi;
pub mod jit_backend;
pub mod lua_facade;

pub use lua_facade::{IntoLuaArgs, Lua, LuaFunction, LuaRoot, LuaSandboxBuilder, LuaTable};


/// Unified `jit` namespace — combines luna-core's trait surface +
/// pure types with luna's Cranelift-backed implementations. Existing
/// `use luna_jit::jit::TraceRecord`, `use luna_jit::jit::CraneliftBackend`,
/// `use luna_jit::jit::cache_lookup_or_compile`, etc. resolve through
/// this module.
pub mod jit {
    pub use luna_core::jit::*;
    pub use crate::jit_backend::{
        CraneliftBackend, cache_lookup_or_compile, enter_jit, try_compile_int_chunk,
    };
    // `cache_clear` + `cache_entry_count` are #[cfg(test)] in the
    // jit_backend, so they aren't re-exportable here (they don't
    // exist in non-test builds). Tests reach them via
    // `luna_jit::jit::cache_clear` only when both crates compile under
    // `--test`, which luna's test binaries do today.
    #[cfg(test)]
    pub use crate::jit_backend::{cache_clear, cache_entry_count};
    /// `luna_core::jit::trace` (the types) merged with
    /// `luna_jit::jit_backend::trace` (the codegen entry points). Old
    /// `crate::jit::trace::TraceRecord` paths in user code keep
    /// working; new code can `use luna_jit::jit::trace::try_compile_trace_with_options`.
    pub mod trace {
        pub use luna_core::jit::trace_types::*;
        pub use crate::jit_backend::trace::{
            last_compile_checkpoint, try_compile_trace, try_compile_trace_with_options,
        };
    }
}

/// v1.1 A1 Session C — build a JIT-equipped minimal Vm. Equivalent
/// to `luna_core::vm::Vm::new_minimal(version)` followed by
/// `install_default_jit(&mut vm)`. Use this as the v1.0
/// drop-in replacement for `Vm::new_minimal` callers who want the
/// Cranelift backend.
pub fn new_minimal_with_jit(version: version::LuaVersion) -> vm::Vm {
    let mut vm = vm::Vm::new_minimal(version);
    install_default_jit(&mut vm);
    vm
}

/// v1.1 A1 Session C — build a JIT-equipped, fully-loaded Vm.
/// Equivalent to `new_minimal_with_jit(version)` followed by
/// `vm.open_all_libs()`. Use this as the v1.0 drop-in replacement
/// for `luna_jit::Vm::new` callers.
pub fn new_with_jit(version: version::LuaVersion) -> vm::Vm {
    let mut vm = new_minimal_with_jit(version);
    vm.open_all_libs();
    vm
}

/// v1.1 A1 Session C — install the default Cranelift backend on an
/// already-constructed Vm. Idempotent on a JIT-equipped Vm; useful
/// for re-arming a Vm previously running under `install_null_jit`.
///
/// Equivalent to v1.0's `Vm::install_default_jit`, lifted to a free
/// fn because the trait orphan rule prevents adding inherent methods
/// to `luna_core::vm::Vm` from this crate. The `VmExt` extension
/// trait below restores the dotted-method form.
pub fn install_default_jit(vm: &mut vm::Vm) {
    vm.install_jit_backend(
        jit_backend::CraneliftBackend,
        jit_backend::CraneliftBackend,
    );
}

/// Extension trait that exposes the JIT-installing constructors and
/// the `install_default_jit` shim as methods on `luna_core::vm::Vm`.
/// Bring it into scope with `use luna_jit::VmExt;` to write
/// `vm.install_default_jit()` (matches v1.0 syntax) instead of
/// `luna_jit::install_default_jit(&mut vm)`.
pub trait VmExt {
    /// See [`install_default_jit`].
    fn install_default_jit(&mut self);
}

impl VmExt for vm::Vm {
    fn install_default_jit(&mut self) {
        install_default_jit(self);
    }
}

// LuaVersion is also a common entry point — re-export at the top
// level so `use luna_jit::LuaVersion` works without an explicit
// `luna_jit::version::LuaVersion` path.
pub use version::LuaVersion;
pub use vm::Vm;
