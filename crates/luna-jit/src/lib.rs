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

// v1.3 UD3 — re-export the derive macro + impl-block attr macro so
// embedders writing `use luna_jit::LuaUserdata;` get both the trait
// (from the `pub use luna_core::*;` above) and the derive (here).
// Rust allows the same path to resolve to both a trait and a derive
// — they live in different namespaces (mirrors serde's
// `pub use serde_derive::Serialize;` pattern).
pub use luna_jit_derive::{LuaUserdata, lua_userdata_methods};

pub mod capi;
pub mod jit_backend;
pub mod lua_facade;

/// v2.0 Track TL — pure-read inspection accessors over a live `Vm`.
/// Re-exports [`luna_core::vm::inspect`] so the `luna-tools`
/// binaries (`luna-heap-dump`, `luna-trace-inspect`,
/// `luna-profile`) can `use luna_jit::inspect::*` without a
/// separate `luna-core` direct dep.
pub mod inspect {
    pub use luna_core::vm::inspect::*;
}

pub use lua_facade::{IntoLuaArgs, Lua, LuaFunction, LuaRoot, LuaSandboxBuilder, LuaTable};

/// Unified `jit` namespace — combines luna-core's trait surface +
/// pure types with luna's Cranelift-backed implementations. Existing
/// `use luna_jit::jit::TraceRecord`, `use luna_jit::jit::CraneliftBackend`,
/// `use luna_jit::jit::cache_lookup_or_compile`, etc. resolve through
/// this module.
pub mod jit {
    pub use crate::jit_backend::{
        CraneliftBackend, cache_lookup_or_compile, enter_jit, try_compile_int_chunk,
    };
    pub use luna_core::jit::*;
    // v2.0 Track J sub-step J-B — `cache_clear` + `cache_entry_count`
    // are no longer `#[cfg(test)]` (the per-Vm storage migration made
    // them harmless probes). Re-exported unconditionally so the J-B
    // integration test + any embedder can probe a Vm's JIT cache size
    // / reset it without a downcast.
    pub use crate::jit_backend::{cache_clear, cache_entry_count};

    /// v2.0 Track J sub-step J-A — `Send` wrapper newtype for
    /// `cranelift_jit::JITModule`. Exposed `#[doc(hidden)]` so
    /// integration tests under `crates/luna-jit/tests/` can run the
    /// static-`Send` assertion + cross-thread smoke without a
    /// `pub(crate)` carve-out. **Not a stable embedder API** — J-B
    /// consumes this internally and the type will move to
    /// `Vm.VmJitStorage` when the field migration lands.
    #[doc(hidden)]
    pub use crate::jit_backend::SendJitModule as __SendJitModule_for_j_a_test;
    /// `luna_core::jit::trace` (the types) merged with
    /// `luna_jit::jit_backend::trace` (the codegen entry points). Old
    /// `crate::jit::trace::TraceRecord` paths in user code keep
    /// working; new code can `use luna_jit::jit::trace::try_compile_trace_with_options`.
    pub mod trace {
        pub use crate::jit_backend::trace::{
            base_var_scaffold_declared_count, last_compile_checkpoint,
            reset_base_var_scaffold_declared_count, try_compile_trace,
            try_compile_trace_with_options,
        };
        pub use luna_core::jit::trace_types::*;
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
///
/// # v2.1 Phase 1K.D.4 — `LUNA_JIT_BACKEND` env-var override
///
/// The chosen backend is normally Cranelift. Set the
/// `LUNA_JIT_BACKEND` env var to override at Vm-construction time:
///
/// | Value | Behaviour |
/// |---|---|
/// | unset / `cranelift` | Cranelift (default). |
/// | `llvm` (with `--features llvm-jit`) | LLVM 18 backend. |
/// | `llvm` (feature OFF) | `panic!` with a rebuild hint. |
/// | any other value | `panic!` listing the accepted values. |
///
/// The env var is read **once per `install_default_jit` call**; the
/// selection is then frozen into the Vm. Switching at runtime is
/// out of scope (see `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md`
/// § 4.4).
pub fn install_default_jit(vm: &mut vm::Vm) {
    let backend = std::env::var("LUNA_JIT_BACKEND").unwrap_or_default();
    match backend.as_str() {
        "" | "cranelift" => install_cranelift_jit(vm),
        "llvm" => install_llvm_jit(vm),
        other => panic!(
            "LUNA_JIT_BACKEND={other:?} not recognised; expected one of \
             {{unset, \"cranelift\", \"llvm\"}}.",
        ),
    }
}

/// v2.1 Phase 1K.D.4 — concrete Cranelift backend installer. Always
/// available; this is the install path `install_default_jit` lands
/// on when `LUNA_JIT_BACKEND` is unset or `cranelift`.
fn install_cranelift_jit(vm: &mut vm::Vm) {
    vm.install_jit_backend(jit_backend::CraneliftBackend, jit_backend::CraneliftBackend);
    // v2.0 Track J sub-step J-B — pair the CraneliftBackend trait
    // impls with a fresh CraneliftJitStorage so the trait impls'
    // `_storage` param downcasts to the right concrete type once
    // Phases D/E/F start using it.
    vm.install_jit_storage(jit_backend::storage::CraneliftJitStorage::default());
}

/// v2.1 Phase 1K.D.4 — concrete LLVM backend installer. Only
/// compiled when `luna-jit` is built with `--features llvm-jit`.
/// Selected at runtime via `LUNA_JIT_BACKEND=llvm`.
#[cfg(feature = "llvm-jit")]
fn install_llvm_jit(vm: &mut vm::Vm) {
    vm.install_jit_backend(luna_jit_llvm::LlvmBackend, luna_jit_llvm::LlvmBackend);
    vm.install_jit_storage(luna_jit_llvm::LlvmJitStorage);
}

/// v2.1 Phase 1K.D.4 — `LUNA_JIT_BACKEND=llvm` was requested but
/// the build does not include the LLVM backend. Panic with a
/// rebuild hint rather than silently falling back to Cranelift
/// (silent fallback would mask the configuration error and lead
/// to confusing bench / test results).
#[cfg(not(feature = "llvm-jit"))]
fn install_llvm_jit(_vm: &mut vm::Vm) {
    panic!(
        "LUNA_JIT_BACKEND=llvm requested but the `llvm-jit` cargo \
         feature is OFF in this build of luna-jit. Rebuild with \
         `cargo build -p luna-jit --features llvm-jit` (or add the \
         feature to your Cargo.toml's luna-jit dep). See \
         `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md` § 4.3."
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
