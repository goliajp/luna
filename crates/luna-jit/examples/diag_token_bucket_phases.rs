//! Phase decomposition of `redis_lua_shape/token_bucket_1k`.
//!
//! The criterion cell times `vm.eval(src)` plus the drop of the Vm on a
//! fresh `new_minimal_with_jit(Lua54)` with base/math/string/table open.
//! This splits that time into parts that can each be measured on its own,
//! and prices the trace compile with Cranelift's own pass timings on
//! recompiles of the exact record the run produced. It uses only APIs that
//! exist in v3.0.0 as well, so the same file measures both versions.
//!
//! Run: `cargo run --release -p luna-jit --example diag_token_bucket_phases
//!       -- [iterations] [recompiles]`

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use luna_jit::jit::trace::{CompileOptions, CompiledTrace, TraceRecord};
use luna_jit::jit::{CompileResult, IntChunkCompiler, JitStorage, JitVmGuard, TraceCompiler};
use luna_jit::jit_backend::CraneliftBackend;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::{Gc, LuaClosure};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const SRC_TEMPLATE: &str = r#"
            local bucket = { tokens = 1000, last = 0, rate = 100 }
            local now = 1
            local refilled = 0
            for i = 1, ITERS do
                local elapsed = now - bucket.last
                local refill = elapsed * bucket.rate
                if refill > 0 then
                    bucket.tokens = math.min(1000, bucket.tokens + refill)
                    bucket.last = now
                    refilled = refilled + 1
                end
                if bucket.tokens >= 1 then
                    bucket.tokens = bucket.tokens - 1
                end
                now = now + 1
            end
            return bucket.tokens, refilled
        "#;

/// The cell's source with its loop count (`TB_ITERS`, default 1000): the
/// difference between two counts prices the compiled loop per iteration.
fn src() -> String {
    let iters = std::env::var("TB_ITERS").unwrap_or_else(|_| "1000".into());
    SRC_TEMPLATE.replace("ITERS", &iters)
}

#[derive(Default)]
struct Stats {
    trace_compile: Duration,
    trace_compiles: u32,
    trace_ok: u32,
    chunk_compile: Duration,
    chunk_compiles: u32,
    records: Vec<(TraceRecord, CompileOptions)>,
}

struct TimedChunk(CraneliftBackend, Rc<RefCell<Stats>>);
struct TimedTrace(CraneliftBackend, Rc<RefCell<Stats>>);

impl IntChunkCompiler for TimedChunk {
    fn try_compile(
        &self,
        storage: &mut dyn JitStorage,
        proto: Gc<Proto>,
        pre53: bool,
        float_only: bool,
    ) -> CompileResult {
        let t = Instant::now();
        let r = self.0.try_compile(storage, proto, pre53, float_only);
        let mut s = self.1.borrow_mut();
        s.chunk_compile += t.elapsed();
        s.chunk_compiles += 1;
        r
    }

    fn enter(&self, vm: *mut Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
        self.0.enter(vm, cl)
    }
}

impl TraceCompiler for TimedTrace {
    fn try_compile_trace(
        &self,
        storage: &mut dyn JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        let t = Instant::now();
        let r = self.0.try_compile_trace(storage, record, opts);
        let mut s = self.1.borrow_mut();
        s.trace_compile += t.elapsed();
        s.trace_compiles += 1;
        s.trace_ok += u32::from(r.is_some());
        s.records.push((record.clone(), opts));
        r
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        self.0.last_compile_checkpoint()
    }
}

fn fresh_vm(stats: Option<Rc<RefCell<Stats>>>) -> Vm {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    if let Some(s) = stats {
        vm.install_jit_backend(
            TimedChunk(CraneliftBackend, s.clone()),
            TimedTrace(CraneliftBackend, s),
        );
    }
    vm
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    v[v.len() / 2]
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// `eval` + drop in the criterion shape, per `mode`; returns medians (µs)
/// of eval, drop and total.
fn run_mode(mode: &str, n: usize) -> (f64, f64, f64) {
    let (mut e, mut d, mut t) = (Vec::new(), Vec::new(), Vec::new());
    let src = src();
    for _ in 0..n {
        let mut vm = fresh_vm(None);
        match mode {
            "full" => {}
            "notrace" => vm.set_trace_jit_enabled(false),
            "nojit" => {
                vm.set_trace_jit_enabled(false);
                vm.set_jit_enabled(false);
            }
            "nomethod" => vm.set_jit_enabled(false),
            _ => unreachable!("mode"),
        }
        let t0 = Instant::now();
        std::hint::black_box(vm.eval(&src).expect("run"));
        let t1 = Instant::now();
        drop(vm);
        let t2 = Instant::now();
        e.push(us(t1 - t0));
        d.push(us(t2 - t1));
        t.push(us(t2 - t0));
    }
    (median(&mut e), median(&mut d), median(&mut t))
}

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.parse().expect("number"))
        .collect();
    let n = args.first().copied().unwrap_or(400);
    let k = args.get(1).copied().unwrap_or(2000);

    // `TB_ONLY=<mode>`: run just that mode's loop (for a profiler).
    if let Ok(mode) = std::env::var("TB_ONLY") {
        let (e, d, t) = run_mode(&mode, n);
        println!("{mode} {e:.1} {d:.1} {t:.1}");
        return;
    }

    // Warm the allocator and the code paths once.
    for _ in 0..20 {
        let mut vm = fresh_vm(None);
        std::hint::black_box(vm.eval(&src()).expect("run"));
    }

    println!("== criterion shape, median of {n} (µs): eval  drop  eval+drop");
    for mode in ["full", "notrace", "nomethod", "nojit"] {
        let (e, d, t) = run_mode(mode, n);
        println!("{mode:9} {e:9.1} {d:7.1} {t:9.1}");
    }

    // Parse + bytecode compile only.
    let mut lt = Vec::new();
    let text = src();
    for _ in 0..n {
        let mut vm = fresh_vm(None);
        let t0 = Instant::now();
        std::hint::black_box(vm.load(text.as_bytes(), b"=tb").expect("load"));
        lt.push(us(t0.elapsed()));
    }
    println!("load      {:9.1}", median(&mut lt));

    // Compile time inside the full run, and the counters of one run.
    let mut tc = Vec::new();
    let mut cc = Vec::new();
    let mut last = None;
    // The records point into the protos of the Vm that recorded them, so
    // the last one is kept alive for the recompiles below.
    let mut keep: Option<Vm> = None;
    for _ in 0..n {
        let stats = Rc::new(RefCell::new(Stats::default()));
        let mut vm = fresh_vm(Some(stats.clone()));
        std::hint::black_box(vm.eval(&src()).expect("run"));
        let s = stats.borrow();
        tc.push(us(s.trace_compile));
        cc.push(us(s.chunk_compile));
        drop(keep.replace(vm));
        let vm = keep.as_ref().expect("just set");
        last = Some((
            s.trace_compiles,
            s.trace_ok,
            s.chunk_compiles,
            vm.trace_dispatched_count(),
            vm.trace_deopt_count(),
            vm.trace_side_trace_compiled_count(),
            s.records.clone(),
        ));
    }
    let (tcomp, tok, ccomp, disp, deopt, side, records) = last.expect("n > 0");
    println!(
        "trace compile (wall, all traces of one run) {:9.1}  method-JIT attempts {:7.1}",
        median(&mut tc),
        median(&mut cc)
    );
    println!(
        "traces compiled {tcomp} (ok {tok}), side traces {side}, dispatches {disp}, deopts {deopt}, method-JIT attempts {ccomp}"
    );
    for (i, (r, _)) in records.iter().enumerate() {
        println!("  record {i}: head_pc {} ops {}", r.head_pc, r.ops.len());
    }

    // Recompile each record k times: the full compile, and the lowerer plus
    // Cranelift alone into a module we built (no symbol table, no finalize).
    let mut vm = keep.expect("n > 0");
    // `TB_ABL_STORES`: for measurement builds whose lowerer drops exit
    // store-backs when `LUNA_ABL_NOSTORE` is set; only the recompiles below
    // see it, never a trace that runs.
    if std::env::var_os("TB_ABL_STORES").is_some() {
        // SAFETY: single-threaded example; no other thread reads the
        // environment concurrently.
        unsafe { std::env::set_var("LUNA_ABL_NOSTORE", "1") };
    }
    for (i, (rec, opts)) in records.iter().enumerate() {
        let _ = cranelift_codegen::timing::take_current();
        let t0 = Instant::now();
        for _ in 0..k {
            let storage = vm.jit.storage.as_mut();
            assert!(
                luna_jit::jit_backend::trace::try_compile_trace_with_options(storage, rec, *opts)
                    .is_some(),
                "recompile failed"
            );
        }
        let full = us(t0.elapsed()) / k as f64;
        let passes_full = cranelift_codegen::timing::take_current();

        let isa = cranelift_native::builder()
            .expect("host isa")
            .finish(cranelift_codegen::settings::Flags::new({
                use cranelift_codegen::settings::Configurable;
                let mut b = cranelift_codegen::settings::builder();
                b.set("use_colocated_libcalls", "false").expect("flag");
                b.set("is_pic", "false").expect("flag");
                b.set("opt_level", "speed").expect("flag");
                // `TB_NOVERIFY`: price the IR verifier by turning it off in
                // the lower-only module.
                if std::env::var_os("TB_NOVERIFY").is_some() {
                    b.set("enable_verifier", "false").expect("flag");
                }
                b
            }))
            .expect("isa");
        let t1 = Instant::now();
        for _ in 0..k {
            let builder = cranelift_jit::JITBuilder::with_isa(
                isa.clone(),
                cranelift_module::default_libcall_names(),
            );
            let mut module = cranelift_jit::JITModule::new(builder);
            assert!(
                luna_jit::jit_backend::trace::lower_trace_into(&mut module, rec, *opts).is_some(),
                "lower failed"
            );
        }
        let lower = us(t1.elapsed()) / k as f64;
        let passes_lower = cranelift_codegen::timing::take_current();
        println!(
            "== record {i}: per compile (µs): full {full:.1}  lower+cranelift into own module {lower:.1}  (cranelift passes over {k} full compiles, then {k} lower-only; totals {:?} {:?}):",
            passes_full.total(),
            passes_lower.total()
        );
        println!("{passes_full}{passes_lower}");

        // The same compile once, right after the interpreter has run the
        // workload (as in the real run): code and data caches are cold for
        // the compiler.
        let mut cold = Vec::new();
        let mut cold_lower = Vec::new();
        for _ in 0..n.min(200) {
            let mut v = fresh_vm(None);
            v.set_trace_jit_enabled(false);
            std::hint::black_box(v.eval(&src()).expect("run"));
            let t0 = Instant::now();
            let storage = v.jit.storage.as_mut();
            assert!(
                luna_jit::jit_backend::trace::try_compile_trace_with_options(storage, rec, *opts)
                    .is_some(),
                "recompile failed"
            );
            cold.push(us(t0.elapsed()));
            let mut v = fresh_vm(None);
            v.set_trace_jit_enabled(false);
            std::hint::black_box(v.eval(&src()).expect("run"));
            let t1 = Instant::now();
            let builder = cranelift_jit::JITBuilder::with_isa(
                isa.clone(),
                cranelift_module::default_libcall_names(),
            );
            let mut module = cranelift_jit::JITModule::new(builder);
            assert!(
                luna_jit::jit_backend::trace::lower_trace_into(&mut module, rec, *opts).is_some(),
                "lower failed"
            );
            cold_lower.push(us(t1.elapsed()));
        }
        println!(
            "   cold (after an interpreter run), median: full {:.1}  lower+cranelift {:.1}",
            median(&mut cold),
            median(&mut cold_lower)
        );
    }
}
