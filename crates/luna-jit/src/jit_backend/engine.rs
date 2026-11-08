//! Compiled code shared between Vms: [`Engine`].

use luna_core::__private::VmHostInternals;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Compiled code shared by the Vms built through it.
///
/// A trace one of these Vms compiles is kept here, and another of them
/// that reaches code of the same content installs it instead of recording
/// and compiling it again: it copies the machine code into its own code
/// memory and writes its own addresses (strings, function prototypes) over
/// the places that hold the first Vm's. So no Vm runs code another Vm
/// owns, and dropping a Vm frees its code as it does without an engine.
///
/// The Vms of one engine hash strings with the engine's seed (chosen at
/// random when the engine is made): traces read table keys where the
/// recording found them, which depends on the seed.
///
/// `Engine` is `Clone` (all clones share the same code), `Send` and `Sync`:
/// Vms on different threads may share one.
///
/// ```
/// use luna_jit::{Engine, version::LuaVersion};
/// let engine = Engine::new();
/// for _ in 0..2 {
///     let mut vm = engine.new_vm(LuaVersion::Lua54);
///     vm.eval("local s = 0 for i = 1, 1000 do s = s + i end return s")
///         .unwrap();
/// }
/// ```
#[derive(Clone)]
pub struct Engine(pub(crate) Arc<Shared>);

pub(crate) struct Shared {
    seed: u32,
    /// The engine one Vm keeps for itself (see [`Engine::private`]).
    private: bool,
    next_id: AtomicU64,
    pub(crate) traces: Mutex<TraceCache>,
}

mod trace_cache;
pub(crate) use trace_cache::{HeadKey, TraceCache};

/// [`Engine::set_capacity_bytes`] until set.
const DEFAULT_CAPACITY: usize = 64 << 20;

/// What a Vm's own engine holds at most: the traces of the code it loads
/// again, beside the copies it runs.
const PRIVATE_CAPACITY: usize = 8 << 20;

impl Default for Engine {
    fn default() -> Engine {
        Engine::new()
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("traces", &self.trace_count())
            .field("bytes", &self.bytes())
            .finish()
    }
}

impl Engine {
    /// An engine holding no code yet, with a random hash seed.
    pub fn new() -> Engine {
        Engine::with(luna_core::__private::fresh_seed(), false)
    }

    /// The engine a Vm built without one keeps for itself: code of the same
    /// content loaded again into that Vm installs the traces an earlier load
    /// compiled. Its Vm hashes strings with its own seed, so the seed here
    /// is not used.
    pub(crate) fn private() -> Engine {
        Engine::with(0, true)
    }

    fn with(seed: u32, private: bool) -> Engine {
        Engine(Arc::new(Shared {
            seed,
            private,
            next_id: AtomicU64::new(1),
            traces: Mutex::new(TraceCache::new(if private {
                PRIVATE_CAPACITY
            } else {
                DEFAULT_CAPACITY
            })),
        }))
    }

    /// A JIT-equipped Vm with the standard libraries, sharing this engine's
    /// code (as [`crate::new_with_jit`] otherwise).
    pub fn new_vm(&self, version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
        let mut vm = self.new_minimal_vm(version);
        // running out of memory ends the process, as in `Vm::new`
        if vm.open_all_libs().is_err() {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<luna_core::runtime::Value>());
        }
        vm
    }

    /// A JIT-equipped Vm without the standard libraries, sharing this
    /// engine's code (as [`crate::new_minimal_with_jit`] otherwise). The
    /// JIT is Cranelift's whatever `LUNA_JIT_BACKEND` says.
    pub fn new_minimal_vm(&self, version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
        let mut vm = luna_core::vm::Vm::new_minimal_with_hash_seed(version, self.0.seed);
        vm.install_jit_backend(super::CraneliftBackend, super::CraneliftBackend);
        vm.install_jit_storage(super::storage::CraneliftJitStorage::with_engine(
            self.clone(),
            version,
        ));
        vm.enable_trace_sharing();
        vm
    }

    /// The string hash seed of this engine's Vms.
    pub fn hash_seed(&self) -> u32 {
        self.0.seed
    }

    /// Keep at most about `n` bytes of shared code and its data, dropping
    /// the oldest traces beyond that (64 MiB until set). Vms keep the
    /// traces they already installed.
    pub fn set_capacity_bytes(&self, n: usize) {
        let mut c = self.cache();
        c.capacity = n;
        c.evict();
    }

    /// Traces held.
    pub fn trace_count(&self) -> usize {
        let c = self.cache();
        c.by_head.values().map(Vec::len).sum()
    }

    /// Functions of the method JIT held.
    pub fn function_count(&self) -> usize {
        let c = self.cache();
        c.chunks.values().map(Vec::len).sum()
    }

    /// Bytes held, roughly: code and the data that goes with it.
    pub fn bytes(&self) -> usize {
        self.cache().bytes
    }

    pub(crate) fn cache(&self) -> std::sync::MutexGuard<'_, TraceCache> {
        self.0
            .traces
            .lock()
            .expect("no thread panics holding the cache")
    }

    /// Whether this is a Vm's own engine: no other Vm reads what it keeps.
    pub(crate) fn is_private(&self) -> bool {
        self.0.private
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.0.next_id.fetch_add(1, Ordering::Relaxed)
    }
}
