//! The traces and functions an [`super::Engine`] holds, and dropping the
//! oldest of them beyond its capacity.

use crate::jit_backend::chunk_share::{ChunkImage, ChunkKey};
use crate::jit_backend::trace::image::{Settings, TraceImage};
use crate::jit_backend::trace::share_failures::Failure;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

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
    /// Recordings that failed to compile, by their head.
    pub(crate) failures: HashMap<HeadKey, Vec<Failure>>,
    /// Images in the order they came, for dropping the oldest.
    order: VecDeque<Held>,
    pub(super) bytes: usize,
    pub(super) capacity: usize,
}

/// An image in [`TraceCache::order`].
#[derive(Clone, Copy)]
enum Held {
    Trace(HeadKey, u64),
    Chunk(ChunkKey, u64),
    /// The failures of one head, all of them.
    Failures(HeadKey),
}

impl TraceCache {
    /// A cache holding nothing, of about `capacity` bytes.
    pub(super) fn new(capacity: usize) -> TraceCache {
        TraceCache {
            by_head: HashMap::new(),
            children: HashMap::new(),
            chunks: HashMap::new(),
            failures: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            capacity,
        }
    }

    pub(crate) fn insert(&mut self, key: HeadKey, img: Arc<TraceImage>) {
        self.bytes += img.size;
        if let Some((_, _, parent)) = img.side_parent {
            self.children.entry(parent).or_default().push(img.clone());
        }
        self.order.push_back(Held::Trace(key, img.id));
        self.by_head.entry(key).or_default().push(img);
        self.evict();
    }

    /// Keeps `f`, or counts it once more when the same failure is here;
    /// a head keeps at most `cap` different failures.
    pub(crate) fn insert_failure(&mut self, key: HeadKey, f: Failure, cap: usize) {
        let list = self.failures.entry(key).or_default();
        if let Some(g) = list.iter_mut().find(|g| g.same(&f)) {
            g.again();
            return;
        }
        if list.len() >= cap {
            return;
        }
        if list.is_empty() {
            self.order.push_back(Held::Failures(key));
        }
        self.bytes += f.size();
        list.push(f);
        self.evict();
    }

    pub(crate) fn insert_chunk(&mut self, key: ChunkKey, img: Arc<ChunkImage>) {
        self.bytes += img.size;
        self.order.push_back(Held::Chunk(key, img.id));
        self.chunks.entry(key).or_default().push(img);
        self.evict();
    }

    pub(super) fn evict(&mut self) {
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
                Some(Held::Failures(key)) => {
                    if let Some(list) = self.failures.remove(&key) {
                        self.bytes -= list.iter().map(Failure::size).sum::<usize>();
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
