//! `LUNA_JIT_COUNTERS=<file>`: a test knob, left out of --help, that
//! appends this run's trace JIT counters to `<file>` as one line of
//! `key=value` words, so a harness can check that a corpus really ran
//! through compiled traces without the counters touching the output.

use luna_jit::vm::Vm;
use std::io::Write;

pub(crate) fn append(vm: &Vm) {
    let Some(path) = std::env::var_os("LUNA_JIT_COUNTERS") else {
        return;
    };
    let c = &vm.jit.counters;
    let jt = luna_jit::jit_backend::trace::trace_codegen_count();
    #[cfg(feature = "llvm-jit")]
    let llvm = luna_jit::jit_backend::trace::llvm_codegen_count();
    #[cfg(not(feature = "llvm-jit"))]
    let llvm = 0;
    let mut line = format!(
        "closed={} compiled={} failed={} dispatched={} side_compiled={} codegen={} baseline={} llvm={} tiered_up={}",
        c.closed,
        c.compiled,
        c.compile_failed,
        c.dispatched,
        c.side_trace_compiled,
        jt,
        luna_jit::jit_backend::trace::baseline_codegen_count(),
        llvm,
        c.tiered_up,
    );
    for r in &c.compile_failed_reasons {
        line.push_str(" why=");
        line.push_str(&r.replace(' ', "_"));
    }
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("LUNA_JIT_COUNTERS names a file that can be appended to");
    f.write_all(line.as_bytes())
        .expect("writing the LUNA_JIT_COUNTERS line");
}
