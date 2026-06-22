//! Sandbox builder (B1, Phase 2 P2-A) — replaces the 5-line manual
//! setter sequence with a fluent builder.
//!
//! Conservative defaults (per `.dev/rfcs/v1.1-rfc-b-ergo.md` §B1):
//!
//! - **No stdlib opened** — the embedder explicitly opts in to each
//!   safe-by-default subset (`open_base`, `open_math`, `open_string`,
//!   `open_table`, `open_coroutine`). `os` / `io` / `debug` / `package`
//!   are intentionally not exposed on the builder; trusted embedders
//!   call `vm.open_io()` etc. directly on the built Vm.
//! - **Bytecode loading off** — `string.dump` / `load(bytecode)` are
//!   rejected. Reverse with `allow_bytecode_loading()` for trusted hosts.
//! - **No instruction or memory budget** — embedders set explicit caps
//!   via `with_instr_budget` / `with_memory_cap`.
//! - **JIT default follows the crate** — `luna-core` builds default to
//!   `NullJitBackend` (interpreter only); `luna` builds default to
//!   `CraneliftBackend` via `Vm::new_minimal_with_jit`. The builder
//!   itself does not flip the JIT switch.
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua54)
//!     .open_base()
//!     .open_math()
//!     .with_instr_budget(1_000_000)
//!     .build();
//!
//! let r = vm.eval("return 1 + 2").unwrap();
//! assert_eq!(r.len(), 1);
//! ```

use crate::version::LuaVersion;
use crate::vm::exec::Vm;

/// Fluent builder for a sandboxed `Vm`. Construct via [`Vm::sandbox`].
///
/// Methods take `self` (consume) and return `Self` (chain). Terminal
/// method is [`SandboxBuilder::build`].
#[derive(Debug, Clone, Copy)]
pub struct SandboxBuilder {
    version: LuaVersion,
    base: bool,
    math: bool,
    string: bool,
    table: bool,
    coroutine: bool,
    instr_budget: Option<i64>,
    memory_cap: Option<usize>,
    bytecode_loading: bool,
}

impl SandboxBuilder {
    /// Create a builder with conservative defaults.
    pub(crate) fn new(version: LuaVersion) -> Self {
        SandboxBuilder {
            version,
            base: false,
            math: false,
            string: false,
            table: false,
            coroutine: false,
            instr_budget: None,
            memory_cap: None,
            bytecode_loading: false,
        }
    }

    /// Open the `base` library (`print`, `pcall`, `type`, etc.).
    pub fn open_base(mut self) -> Self {
        self.base = true;
        self
    }

    /// Open the `math` library.
    pub fn open_math(mut self) -> Self {
        self.math = true;
        self
    }

    /// Open the `string` library (no `string.dump` exposure unless
    /// bytecode loading is also allowed).
    pub fn open_string(mut self) -> Self {
        self.string = true;
        self
    }

    /// Open the `table` library.
    pub fn open_table(mut self) -> Self {
        self.table = true;
        self
    }

    /// Open the `coroutine` library.
    pub fn open_coroutine(mut self) -> Self {
        self.coroutine = true;
        self
    }

    /// Cap dispatched instructions to `n` per `call_value` invocation.
    /// Beyond `n`, the Vm raises a catchable "instruction budget
    /// exceeded" error so the embedder can yield control.
    pub fn with_instr_budget(mut self, n: i64) -> Self {
        self.instr_budget = Some(n);
        self
    }

    /// Cap heap allocation to `n` bytes (approximate — see
    /// crate-level docs on `heap.bytes()` accuracy).
    pub fn with_memory_cap(mut self, n: usize) -> Self {
        self.memory_cap = Some(n);
        self
    }

    /// Allow `load(bytecode)` / `string.dump`. Untrusted scripts
    /// should NOT have this; precompiled chunks bypass the parser's
    /// safety gates.
    pub fn allow_bytecode_loading(mut self) -> Self {
        self.bytecode_loading = true;
        self
    }

    /// Finalize and return the `Vm`.
    pub fn build(self) -> Vm {
        let mut vm = Vm::new_minimal(self.version);
        if self.base {
            vm.open_base();
        }
        if self.math {
            vm.open_math();
        }
        if self.string {
            vm.open_string();
        }
        if self.table {
            vm.open_table();
        }
        if self.coroutine {
            vm.open_coroutine();
        }
        vm.set_instr_budget(self.instr_budget);
        if let Some(cap) = self.memory_cap {
            vm.set_memory_cap(Some(cap));
        }
        vm.set_bytecode_loading(self.bytecode_loading);
        vm
    }
}

impl Vm {
    /// Start a [`SandboxBuilder`] for a fresh, sandboxed Vm. See
    /// [`SandboxBuilder`] for defaults.
    pub fn sandbox(version: LuaVersion) -> SandboxBuilder {
        SandboxBuilder::new(version)
    }
}
