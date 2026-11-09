//! `LUNA_JIT_COUNTERS=<file>`: a test knob, left out of --help, that
//! appends this run's trace JIT counters to `<file>` as one line of
//! `key=value` words, so a harness can check that a corpus really ran
//! through compiled traces without the counters touching the output.

use luna_core::__private::VmJitState;
use luna_jit::vm::Vm;
use std::io::Write;

pub(crate) fn append(vm: &Vm) {
    let Some(path) = std::env::var_os("LUNA_JIT_COUNTERS") else {
        return;
    };
    let c = &vm.jit().counters;
    // a 32-bit build has no JIT, so no code of its
    #[cfg(target_pointer_width = "64")]
    let (jt, baseline) = (
        luna_jit::jit_backend::trace::trace_codegen_count(),
        luna_jit::jit_backend::trace::baseline_codegen_count(),
    );
    #[cfg(not(target_pointer_width = "64"))]
    let (jt, baseline) = (0, 0);
    #[cfg(feature = "llvm-jit")]
    let llvm = luna_jit::jit_backend::trace::llvm_codegen_count();
    #[cfg(not(feature = "llvm-jit"))]
    let llvm = 0;
    let mut line = format!(
        "closed={} compiled={} failed={} dispatched={} side_compiled={} codegen={} baseline={} llvm={} tiered_up={} retiered={} calls_compiled={} method={} method_replaced={} links={} links_by_vm={} links_by_stub={} links_left_by_stub={} relink={}",
        c.closed,
        c.compiled,
        c.compile_failed,
        c.dispatched,
        c.side_trace_compiled,
        jt,
        baseline,
        llvm,
        c.tiered_up,
        c.retiered,
        c.calls_compiled,
        c.method_compiled,
        c.method_replaced,
        c.links,
        c.links_by_vm.map(|n| n.to_string()).join("/"),
        c.links_by_stub,
        c.links_left_by_stub.map(|n| n.to_string()).join("/"),
        c.relink.map(|n| n.to_string()).join("/"),
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
