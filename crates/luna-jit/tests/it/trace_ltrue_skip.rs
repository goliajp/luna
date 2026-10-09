//! `LTrueSkip` (PUC 5.1–5.3 `LOADBOOL A 1 1`, which no PUC compiler emits)
//! in a trace: a chunk is dumped, one `LOADBOOL A 0 1` patched to
//! `LOADBOOL A 1 1`, loaded back, and run in a hot loop under each trace
//! tier against the interpreter.

use luna_jit::jit::trace::TraceTier;
use luna_jit::jit_backend::trace::baseline_codegen_count;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::isa::Op;

// `x` is false on every pass; with its `LOADBOOL x 0 1` patched to
// `LOADBOOL x 1 1` it is true, so `s` counts every pass
const SRC: &str = "local i, s = 0, 0 \
    while i < 300 do i = i + 1 local x = i < 0 if x then s = s + 1 end end \
    return s";

/// `src` dumped by `version` with its `LOADBOOL A 0 1` that a
/// `LOADBOOL A 1 0` follows made `LOADBOOL A 1 1`.
fn patched_chunk(version: LuaVersion) -> Vec<u8> {
    let mut vm = Vm::new(version);
    let r = vm
        .eval(&format!(
            "return string.dump((loadstring or load)({SRC:?}))"
        ))
        .expect("dumps");
    let Value::Str(s) = r[0] else {
        panic!("no dump")
    };
    let mut bytes = s.as_bytes().to_vec();
    let loadbool = if version == LuaVersion::Lua51 { 2 } else { 3 };
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let field = |w: u32| {
        (
            w & 0x3F,
            (w >> 6) & 0xFF,
            (w >> 23) & 0x1FF,
            (w >> 14) & 0x1FF,
        )
    };
    let at = (0..bytes.len() - 8)
        .find(|&at| {
            let (op, a, b, c) = field(word(&bytes, at));
            let (op2, a2, b2, c2) = field(word(&bytes, at + 4));
            (op, b, c) == (loadbool, 0, 1) && (op2, a2, b2, c2) == (loadbool, a, 1, 0)
        })
        .expect("a LOADBOOL pair");
    let w = word(&bytes, at) | 1 << 23;
    bytes[at..at + 4].copy_from_slice(&w.to_le_bytes());
    bytes
}

fn run(vm: &mut Vm, chunk: &[u8]) -> Value {
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let f = vm.load(chunk, b"=patched").expect("loads");
    assert!(
        f.proto.code.iter().any(|i| i.op() == Op::LTrueSkip),
        "the patched LOADBOOL reads as LTrueSkip"
    );
    vm.call_value(Value::Closure(f), &[]).expect("runs")[0]
}

#[test]
fn a_trace_runs_ltrue_skip_like_the_interpreter() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua53] {
        let chunk = patched_chunk(version);
        let mut interp = luna_jit::new_with_jit(version);
        interp.set_jit_enabled(false);
        interp.set_trace_jit_enabled(false);
        let want = run(&mut interp, &chunk);
        assert!(
            matches!(want, Value::Int(300) | Value::Float(300.0)),
            "{want:?}"
        );
        for tier in [TraceTier::Baseline, TraceTier::Optimizing] {
            let mut vm = luna_jit::new_with_jit(version);
            vm.set_jit_enabled(false);
            vm.jit.trace_hot_threshold = 2;
            vm.set_trace_tier(tier);
            let baseline_before = baseline_codegen_count();
            let got = run(&mut vm, &chunk);
            assert!(
                got.raw_eq(want),
                "{version:?} {tier:?}: {got:?} vs {want:?}"
            );
            assert!(
                vm.trace_compiled_count() >= 1 && vm.trace_compile_failed_count() == 0,
                "{version:?} {tier:?}: {} compiled, {} refused",
                vm.trace_compiled_count(),
                vm.trace_compile_failed_count()
            );
            assert!(
                vm.trace_dispatched_count() >= 1,
                "{version:?} {tier:?}: never entered"
            );
            let baseline = baseline_codegen_count() > baseline_before;
            assert_eq!(
                baseline,
                tier == TraceTier::Baseline,
                "{version:?} {tier:?}"
            );
        }
    }
}
