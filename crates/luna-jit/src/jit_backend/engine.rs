//! Compiled code shared between Vms: [`Engine`].

use super::chunk_share::{ChunkImage, ChunkKey};
use super::trace::image::{Settings, TraceImage};
use std::collections::{HashMap, VecDeque};
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
    next_id: AtomicU64,
    pub(crate) traces: Mutex<TraceCache>,
}

/// Where a trace starts, among code of every content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HeadKey {
    pub(crate) settings: Settings,
    pub(crate) content: u64,
    pub(crate) head_pc: u32,
}

pub(crate) struct TraceCache {
    pub(crate) by_head: HashMap<HeadKey, Vec<Arc<TraceImage>>>,
    /// Side traces by the id of their parent's image.
    pub(crate) children: HashMap<u64, Vec<Arc<TraceImage>>>,
    /// The method JIT's functions.
    pub(crate) chunks: HashMap<ChunkKey, Vec<Arc<ChunkImage>>>,
    /// Images in the order they came, for dropping the oldest.
    order: VecDeque<Held>,
    bytes: usize,
    capacity: usize,
}

/// An image in [`TraceCache::order`].
#[derive(Clone, Copy)]
enum Held {
    Trace(HeadKey, u64),
    Chunk(ChunkKey, u64),
}

/// [`Engine::set_capacity_bytes`] until set.
const DEFAULT_CAPACITY: usize = 64 << 20;

impl TraceCache {
    pub(crate) fn insert(&mut self, key: HeadKey, img: Arc<TraceImage>) {
        self.bytes += img.size;
        if let Some((_, _, parent)) = img.side_parent {
            self.children.entry(parent).or_default().push(img.clone());
        }
        self.order.push_back(Held::Trace(key, img.id));
        self.by_head.entry(key).or_default().push(img);
        self.evict();
    }

    pub(crate) fn insert_chunk(&mut self, key: ChunkKey, img: Arc<ChunkImage>) {
        self.bytes += img.size;
        self.order.push_back(Held::Chunk(key, img.id));
        self.chunks.entry(key).or_default().push(img);
        self.evict();
    }

    fn evict(&mut self) {
        while self.bytes > self.capacity {
            let (key, id) = match self.order.pop_front() {
                None => break,
                Some(Held::Chunk(key, id)) => {
                    if let Some(list) = self.chunks.get_mut(&key)
                        && let Some(i) = list.iter().position(|x| x.id == id)
                    {
                        self.bytes -= list.remove(i).size;
                        if list.is_empty() {
                            self.chunks.remove(&key);
                        }
                    }
                    continue;
                }
                Some(Held::Trace(key, id)) => (key, id),
            };
            let Some(list) = self.by_head.get_mut(&key) else {
                continue;
            };
            let Some(i) = list.iter().position(|x| x.id == id) else {
                continue;
            };
            let img = list.remove(i);
            if list.is_empty() {
                self.by_head.remove(&key);
            }
            self.children.remove(&id);
            if let Some((_, _, parent)) = img.side_parent
                && let Some(c) = self.children.get_mut(&parent)
            {
                c.retain(|x| x.id != id);
            }
            self.bytes -= img.size;
        }
    }

    /// Counts what the optimizing tier added to an image already here.
    pub(crate) fn grew(&mut self, by: usize) {
        self.bytes += by;
        self.evict();
    }
}

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
        let seed = luna_core::runtime::Heap::new().seed();
        Engine(Arc::new(Shared {
            seed,
            next_id: AtomicU64::new(1),
            traces: Mutex::new(TraceCache {
                by_head: HashMap::new(),
                children: HashMap::new(),
                chunks: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
                capacity: DEFAULT_CAPACITY,
            }),
        }))
    }

    /// A JIT-equipped Vm with the standard libraries, sharing this engine's
    /// code (as [`crate::new_with_jit`] otherwise).
    pub fn new_vm(&self, version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
        let mut vm = self.new_minimal_vm(version);
        vm.open_all_libs();
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

    pub(crate) fn next_id(&self) -> u64 {
        self.0.next_id.fetch_add(1, Ordering::Relaxed)
    }
}
