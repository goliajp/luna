//! The LLVM backend's method JIT: a hot function runs Cranelift's code at
//! once, as with the Cranelift backend, and LLVM's once LLVM, on the
//! compile thread, has compiled it (LLVM takes milliseconds per function,
//! Cranelift a fraction of one).
//!
//! The compile thread stores LLVM's entry in a cell the function's proto
//! holds (`Proto::jit_next`); the Vm puts it in place of Cranelift's at the
//! next call into the function from the interpreter. Calls the code makes
//! to itself go straight to its own body, so a call already running
//! finishes in Cranelift's code.

use super::*;
use luna_core::jit::JitStorage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The functions of one Vm LLVM compiles in the background.
#[derive(Default)]
pub(crate) struct Chunks {
    /// By cache key: the cell the compile thread stores LLVM's entry in.
    cells: std::collections::HashMap<u64, Arc<AtomicUsize>>,
    /// The code LLVM compiled, kept mapped for the Vm's lifetime.
    code: Arc<Mutex<Vec<luna_jit_llvm::EnginePair>>>,
}

/// [`IntChunkCompiler::try_compile`] for the LLVM backend: LLVM's code
/// after `llvm_after` of Cranelift's, or at once when `None` (Cranelift's
/// when LLVM's method JIT does not take the function).
pub(super) fn try_compile(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
    llvm_after: Option<std::time::Duration>,
) -> CompileResult {
    let llvm_now = |storage: &mut dyn JitStorage| {
        let Some(llvm) = storage::from_storage(storage)
            .ok()
            .and_then(|cs| cs.llvm.as_mut())
        else {
            return CompileResult::Skipped;
        };
        luna_jit_llvm::LlvmBackend.try_compile(llvm, proto, pre53, float_only)
    };
    let Some(llvm_after) = llvm_after else {
        return match llvm_now(storage) {
            CompileResult::Skipped => cranelift(storage, proto, pre53, float_only),
            r => r,
        };
    };
    let Some((
        entry,
        num_args,
        returns_one,
        arg_float_mask,
        arg_table_mask,
        ret_is_float,
        ret_is_table,
    )) = cache_lookup_or_compile(storage, proto, pre53, float_only)
    else {
        // a shape only LLVM's method JIT takes: compiled at once
        return llvm_now(storage);
    };
    let cranelift = CompileResult::Compiled {
        entry,
        num_args,
        returns_one,
        arg_float_mask,
        arg_table_mask,
        ret_is_float,
        ret_is_table,
    };
    // LLVM's code passes integers only and returns an integer or nothing
    let plain = arg_float_mask == 0 && arg_table_mask == 0 && !ret_is_float && !ret_is_table;
    let Some(job) = luna_jit_llvm::ChunkJob::of(&proto)
        .filter(|j| plain && j.num_args() == num_args && j.returns_one() == returns_one)
    else {
        return cranelift;
    };
    let key = chunk_cache::proto_cache_key(&proto, pre53, float_only);
    let Ok(cs) = storage::from_storage(storage) else {
        return cranelift;
    };
    if let Some(cell) = cs.llvm_chunks.cells.get(&key) {
        let ready = cell.load(Ordering::Acquire);
        if ready != 0 {
            luna_core::jit::code_fence();
            return CompileResult::Compiled {
                entry: ready as *const u8,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            };
        }
        proto.jit_next.set(Some(cell.clone()));
        return cranelift;
    }
    let cell = Arc::new(AtomicUsize::new(0));
    let (to, code) = (cell.clone(), cs.llvm_chunks.code.clone());
    let ticket = super::llvm_thread::submit(
        Box::new(move || {
            if let Some(c) = job.compile() {
                let entry = c.entry;
                code.lock()
                    .expect("a job never panics holding the code list")
                    .push(c.pair);
                // the execution engine has done the cache maintenance; the
                // thread that takes the entry runs `code_fence`
                to.store(entry, Ordering::Release);
            }
        }),
        std::time::Instant::now() + llvm_after,
    );
    cs.llvm_tickets.push(ticket);
    proto.jit_next.set(Some(cell.clone()));
    cs.llvm_chunks.cells.insert(key, cell);
    cranelift
}

/// The Cranelift backend's method JIT.
fn cranelift(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
) -> CompileResult {
    CraneliftBackend.try_compile(storage, proto, pre53, float_only)
}
