//! `--vm-churn`: create a JIT Vm, run the workload until both the method
//! JIT and the trace JIT have compiled code, drop the Vm, repeat. What a
//! dropped Vm fails to give back (machine code, GC heap, interned
//! strings) shows up as RSS growth across thousands of Vms.

use luna_jit::jit::cache_entry_count;
use luna_jit::version::LuaVersion;

/// A workload that has not compiled both kinds of code after this many
/// runs on one Vm never will; the soak fails instead of measuring Vms
/// that compiled nothing.
const MAX_RUNS_PER_VM: u32 = 100;

/// Runs `src` on a fresh JIT Vm until it holds method-JIT chunks and
/// compiled traces, then drops it. Returns the Vm's `memory_used()`
/// just before the drop.
pub fn one_vm(src: &str, mem_cap: usize) -> Result<usize, String> {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_memory_cap(Some(mem_cap));
    for _ in 0..MAX_RUNS_PER_VM {
        vm.eval(src).map_err(|e| format!("{e}"))?;
        if cache_entry_count(&vm) > 0 && vm.trace_compiled_count() > 0 {
            return Ok(vm.memory_used());
        }
    }
    Err(format!(
        "after {MAX_RUNS_PER_VM} runs on one Vm: {} method-JIT chunks, {} compiled traces",
        cache_entry_count(&vm),
        vm.trace_compiled_count()
    ))
}
