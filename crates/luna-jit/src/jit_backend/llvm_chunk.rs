//! The LLVM backend's method JIT: a hot function runs Cranelift's code at
//! once, as with the Cranelift backend, and LLVM's once LLVM, on the
//! compile thread, has compiled it (LLVM takes milliseconds per function,
//! Cranelift a fraction of one).
//!
//! The dispatcher keeps the entry it was given, so the function is given a
//! small entry of its own that calls through a cell: Cranelift's code first,
//! LLVM's after the compile thread writes it in. Calls the code makes to
//! itself go straight to its own body.

use super::*;
use luna_core::jit::JitStorage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The functions of one Vm that run through a cell, by cache key.
#[derive(Default)]
pub(crate) struct Chunks {
    entries: std::collections::HashMap<u64, CompileResult>,
    /// The cells the entries call through.
    cells: Vec<Arc<AtomicUsize>>,
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
    if let Some(&hit) = cs.llvm_chunks.entries.get(&key) {
        return hit;
    }
    let cell = Arc::new(AtomicUsize::new(entry as usize));
    let Some(handle) = cell_entry(&cell, num_args, returns_one) else {
        return cranelift;
    };
    let result = CompileResult::Compiled {
        entry: handle.entry_raw,
        num_args,
        returns_one,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    };
    let (to, code) = (cell.clone(), cs.llvm_chunks.code.clone());
    let ticket = super::llvm_thread::submit(
        Box::new(move || {
            if let Some(c) = job.compile() {
                code.lock()
                    .expect("a job never panics holding the code list")
                    .push(c.pair);
                to.store(c.entry, Ordering::Release);
            }
        }),
        std::time::Instant::now() + llvm_after,
    );
    cs.llvm_tickets.push(ticket);
    cs.cache_handles.push(handle);
    cs.llvm_chunks.cells.push(cell);
    cs.llvm_chunks.entries.insert(key, result);
    result
}

/// A function of `num_args` integer arguments that calls the function
/// whose address `cell` holds with them and returns its result.
fn cell_entry(cell: &Arc<AtomicUsize>, num_args: u8, returns_one: bool) -> Option<JitHandle> {
    let mut module =
        send_jit_module::UnpublishedModule::new(chunk_module::build_jit_module_with_helpers()?);
    let mut sig = module.make_signature();
    for _ in 0..num_args {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let fn_id = module
        .declare_function("luna_jit_llvm_cell_entry", Linkage::Local, &sig)
        .ok()?;
    let mut ctx = module.make_context();
    ctx.func.signature = sig.clone();
    let mut fbc = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let start = b.create_block();
    b.append_block_params_for_function_params(start);
    b.switch_to_block(start);
    b.seal_block(start);
    let args = b.block_params(start).to_vec();
    let at = b
        .ins()
        .iconst(types::I64, Arc::as_ptr(cell) as *const AtomicUsize as i64);
    let target = b.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        at,
        0,
    );
    let sref = b.import_signature(sig);
    let call = b.ins().call_indirect(sref, target, &args);
    let r = b.inst_results(call)[0];
    b.ins().return_(&[r]);
    b.finalize(module.target_config());
    module.define_function(fn_id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let ptr = module.get_finalized_function(fn_id);
    Some(JitHandle {
        _module: module.publish(),
        entry_raw: ptr,
        num_args,
        returns_one,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    })
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
