//! One optimized function compiled from IR another crate builds: how
//! luna-jit's trace tier uses this backend.

use crate::codegen::finalize_with;
use crate::storage::EnginePair;
use inkwell::OptimizationLevel;
use inkwell::attributes::AttributeLoc;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};

/// The name of the function [`compile_function`] returns the address of.
pub const ENTRY: &str = "luna_jit_llvm_entry";

/// Builds a module with `build`, optimizes it for the host CPU and
/// compiles it. `build` must define a function named [`ENTRY`]; its
/// address comes back with the pair that keeps the code mapped (park it
/// with [`crate::LlvmJitStorage::park_engine`]). `Err` names the step that
/// failed.
#[doc(hidden)]
pub fn compile_function(
    build: impl for<'c> FnOnce(&'c Context, &Module<'c>) -> Result<(), &'static str>,
) -> Result<(*const u8, EnginePair), &'static str> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: the `Context` lives in a box whose address does not change
    // when the box moves. The module made from `ctx` is a local declared
    // after `ctx_box`, so it drops first on every early return;
    // `finalize_with` moves the box into the `EnginePair`, which drops the
    // engine before the context
    let ctx: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };
    let module = ctx.create_module("luna_jit_llvm_fn");
    let tm = host_machine().ok_or("llvm:target-machine")?;
    module.set_triple(&tm.get_triple());
    module.set_data_layout(&tm.get_target_data().get_data_layout());
    build(ctx, &module)?;
    let cpu = TargetMachine::get_host_cpu_name();
    let features = TargetMachine::get_host_cpu_features();
    let cpu = cpu.to_str().map_err(|_| "llvm:host-cpu")?;
    let features = features.to_str().map_err(|_| "llvm:host-cpu")?;
    // the execution engine codegens for a generic CPU unless each function
    // names the host's
    for f in module.get_functions() {
        if f.count_basic_blocks() > 0 {
            f.add_attribute(
                AttributeLoc::Function,
                ctx.create_string_attribute("target-cpu", cpu),
            );
            f.add_attribute(
                AttributeLoc::Function,
                ctx.create_string_attribute("target-features", features),
            );
        }
    }
    module.verify().map_err(|_| "llvm:verify")?;
    module
        .run_passes("default<O2>", &tm, PassBuilderOptions::create())
        .map_err(|_| "llvm:passes")?;
    finalize_with(ctx_box, module, OptimizationLevel::Default).ok_or("llvm:engine")
}

fn host_machine() -> Option<TargetMachine> {
    Target::initialize_native(&InitializationConfig::default()).ok()?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).ok()?;
    let cpu = TargetMachine::get_host_cpu_name();
    let features = TargetMachine::get_host_cpu_features();
    target.create_target_machine(
        &triple,
        cpu.to_str().ok()?,
        features.to_str().ok()?,
        OptimizationLevel::Default,
        RelocMode::Default,
        CodeModel::JITDefault,
    )
}
