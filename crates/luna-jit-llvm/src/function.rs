//! One optimized function compiled from IR another crate builds: how
//! luna-jit's trace tier uses this backend. [`optimize`] is shared with
//! the method JIT.

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
    build(ctx, &module)?;
    optimize(ctx, &module)?;
    dump(&module);
    finalize_with(ctx_box, module, OptimizationLevel::Default).ok_or("llvm:engine")
}

/// The host's target machine and CPU, made once per thread: creating a
/// target machine is a large part of compiling one small function.
struct Host {
    tm: TargetMachine,
    cpu: String,
    features: String,
}

thread_local! {
    static HOST: Option<Host> = host();
}

fn host() -> Option<Host> {
    // registering the targets is global: once, whichever thread is first
    static NATIVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*NATIVE.get_or_init(|| Target::initialize_native(&InitializationConfig::default()).is_ok())
    {
        return None;
    }
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).ok()?;
    let cpu = TargetMachine::get_host_cpu_name().to_str().ok()?.to_owned();
    let features = TargetMachine::get_host_cpu_features()
        .to_str()
        .ok()?
        .to_owned();
    let tm = target.create_target_machine(
        &triple,
        &cpu,
        &features,
        OptimizationLevel::Default,
        RelocMode::Default,
        CodeModel::JITDefault,
    )?;
    Some(Host { tm, cpu, features })
}

/// Runs LLVM's `default<O2>` pipeline over `module` for the host CPU, and
/// marks every function defined in it for that CPU (the execution engine
/// otherwise generates code for a generic one).
pub(crate) fn optimize(ctx: &Context, module: &Module<'_>) -> Result<(), &'static str> {
    HOST.with(|h| {
        let h = h.as_ref().ok_or("llvm:target-machine")?;
        module.set_triple(&h.tm.get_triple());
        module.set_data_layout(&h.tm.get_target_data().get_data_layout());
        for f in module.get_functions() {
            if f.count_basic_blocks() > 0 {
                f.add_attribute(
                    AttributeLoc::Function,
                    ctx.create_string_attribute("target-cpu", &h.cpu),
                );
                f.add_attribute(
                    AttributeLoc::Function,
                    ctx.create_string_attribute("target-features", &h.features),
                );
            }
        }
        module.verify().map_err(|_| "llvm:verify")?;
        module
            .run_passes("default<O2>", &h.tm, PassBuilderOptions::create())
            .map_err(|_| "llvm:passes")
    })
}

/// `LUNA_TRACE_IR_DUMP=1` / `LUNA_TRACE_ASM_DUMP=1`, as for the Cranelift
/// tiers: the IR after optimization, and the machine code as assembly.
pub(crate) fn dump(module: &Module<'_>) {
    let on = |k| std::env::var_os(k).is_some_and(|v| v == "1");
    if on("LUNA_TRACE_IR_DUMP") {
        eprintln!(
            "=== LLVM IR DUMP ===\n{}\n=== END ===",
            module.print_to_string().to_string()
        );
    }
    if on("LUNA_TRACE_ASM_DUMP") {
        HOST.with(|h| {
            let asm = h.as_ref().and_then(|h| {
                h.tm.write_to_memory_buffer(module, inkwell::targets::FileType::Assembly)
                    .ok()
            });
            if let Some(asm) = asm {
                eprintln!(
                    "=== LLVM ASM DUMP ===\n{}\n=== END ===",
                    String::from_utf8_lossy(asm.as_slice())
                );
            }
        });
    }
}
