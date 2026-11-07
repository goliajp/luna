//! Allocation failure fuzz target.
//!
//! Runs a generated program (the JIT differential target's generator:
//! hot loops, closures, tables, strings, metatables, coroutines, garbage)
//! in a Vm whose memory policy refuses allocations: every one from the Nth
//! on, only the Nth (so the emergency collection's retry succeeds), only
//! the Nth that compiled code makes, or any growth past a byte limit. The
//! JIT is on for the third mode and half the other inputs; half the inputs
//! with the JIT on run only the loops whose passes write what running one
//! again would show. Asserts:
//! - no panic and no abort: a refused allocation is a Lua memory error
//!   (or the program's own error), never the end of the process;
//! - an error that is the memory error carries "not enough memory";
//! - with the JIT on and one allocation refused, in compiled code, from
//!   5.2 on: what the program prints and the error it ends with are the
//!   interpreter's without refusals (compiled code that cannot allocate
//!   leaves it to the interpreter or the dispatcher, which allocate for it
//!   and retry after an emergency collection);
//! - every byte the Vm allocated through its context is given back when
//!   the Vm is dropped;
//! - no use after free or leak on the error paths (ASan / LeakSanitizer,
//!   which `cargo fuzz` builds with).
//!
//! `$LUNA_FUZZ_DIALECT` (`5.1` … `5.5`, default `5.5`) picks the dialect;
//! `$LUNA_FUZZ_SHOW=1` prints each program, the mode and the outcome.
//!
//! Run:
//!     cd crates/luna-fuzz
//!     cargo +nightly fuzz run fuzz_alloc_fail --fuzz-dir . -- -max_total_time=300

#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use luna_core::__private::VmJitState;
use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering::Relaxed};

#[allow(dead_code)]
#[path = "program.rs"]
mod program;

#[path = "jit_program.rs"]
mod jit_program;

#[path = "jit_stmts.rs"]
mod jit_stmts;

#[path = "jit_frame.rs"]
mod jit_frame;

#[path = "jit_loops.rs"]
mod jit_loops;

#[path = "jit_hot.rs"]
mod jit_hot;

#[path = "jit_reentry.rs"]
mod jit_reentry;

#[path = "jit_gc.rs"]
mod jit_gc;

#[path = "jit_carry.rs"]
mod jit_carry;

#[path = "jit_nested.rs"]
mod jit_nested;

#[path = "jit_rerun.rs"]
mod jit_rerun;

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// refuse every allocation from the `n`th on
    From(u32),
    /// refuse only the `n`th
    Once(u32),
    /// refuse growth past this many bytes in use
    Limit(usize),
    /// refuse only the `n`th of the allocations compiled code makes
    OnceInJit(u32),
}

/// What the policy saw, shared with the test after the Vm is gone.
#[derive(Default)]
struct Seen {
    armed: AtomicBool,
    asked: AtomicU32,
    asked_in_jit: AtomicU32,
    refused: AtomicU32,
    /// a refusal was made while compiled code ran
    in_jit: AtomicBool,
    live: AtomicIsize,
}

struct Policy(Mode, Arc<Seen>);

impl MemoryPolicy for Policy {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        let s = &self.1;
        let in_jit = !luna_jit::jit_backend::__jit_tls_ptrs().0.is_null();
        let ok = !s.armed.load(Relaxed) || {
            let n = s.asked.fetch_add(1, Relaxed) + 1;
            match self.0 {
                Mode::From(k) => n < k,
                Mode::Once(k) => n != k,
                Mode::Limit(l) => in_use - old.min(in_use) + new <= l,
                Mode::OnceInJit(k) => !in_jit || s.asked_in_jit.fetch_add(1, Relaxed) + 1 != k,
            }
        };
        if ok {
            s.live.fetch_add(new as isize - old as isize, Relaxed);
        } else {
            s.refused.fetch_add(1, Relaxed);
            if in_jit {
                s.in_jit.store(true, Relaxed);
            }
        }
        ok
    }

    fn freed(&mut self, size: usize) {
        self.1.live.fetch_sub(size as isize, Relaxed);
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let mode = match u.int_in_range(0..=3).unwrap_or(0) {
        0 => Mode::From(u.int_in_range(1..=20_000).unwrap_or(1)),
        1 => Mode::Once(u.int_in_range(1..=20_000).unwrap_or(1)),
        2 => Mode::OnceInJit(u.int_in_range(1..=2_000).unwrap_or(1)),
        _ => Mode::Limit(u.int_in_range(64 << 10..=8 << 20).unwrap_or(64 << 10)),
    };
    // compiled code makes allocations only with the JIT on
    let jit = matches!(mode, Mode::OnceInJit(_)) || u.arbitrary::<bool>().unwrap_or(false);
    let only_rerun = jit && u.arbitrary::<bool>().unwrap_or(false);
    let mut g = jit_program::Gen::new(&mut u, program::dialect());
    let src = if only_rerun {
        rerun_program(&mut g)
    } else {
        g.program()
    };
    let show = std::env::var_os("LUNA_FUZZ_SHOW").is_some();
    if show {
        eprintln!("=== {mode:?}, jit {jit} ===\n{src}");
    }

    let seen = Arc::new(Seen::default());
    {
        let mem = MemOwner::policy(Box::new(Policy(mode, seen.clone())));
        // the policy is armed once the Vm and its libraries are made: a Vm
        // that cannot be made is `try_new_minimal_with_mem`'s business
        let mut vm = luna_core::vm::Vm::new_with_mem(program::dialect(), mem);
        // an instruction budget keeps every loop in the interpreter; the
        // generated programs end by themselves
        if jit {
            luna_jit::install_default_jit(&mut vm);
            vm.set_trace_hot_threshold(2);
            vm.set_call_hot_threshold(2);
        } else {
            vm.install_null_jit();
            vm.set_instr_budget(Some(50_000_000));
        }
        seen.armed.store(true, Relaxed);
        let outcome = outcome_of(&mut vm, &src);
        if show {
            eprintln!(
                "=== outcome: {outcome:?}, asked {}, refused {} ===",
                seen.asked.load(Relaxed),
                seen.refused.load(Relaxed)
            );
        }
        // a refusal surfaces as the memory error, unless the program's own
        // error came first or a pcall in it swallowed the memory error
        if let Some(e) = &outcome {
            assert!(
                seen.refused.load(Relaxed) > 0
                    || !e.contains("not enough memory")
                    || src.contains("not enough memory"),
                "a memory error with nothing refused: {e}\n{src}"
            );
        }
        // 5.1 makes no second try after a refusal: one in the interpreter a
        // helper runs (a metamethod) is the memory error there
        let retries = program::dialect() != luna_core::version::LuaVersion::Lua51;
        let once = matches!(mode, Mode::Once(_) | Mode::OnceInJit(_));
        if once && seen.in_jit.load(Relaxed) && retries {
            let mut reference = luna_core::vm::Vm::new(program::dialect());
            reference.install_null_jit();
            let want = outcome_of(&mut reference, &src).map(|e| jit_frame::mask_addresses(&e));
            let want = (want, printed(&mut reference));
            assert_eq!(
                (
                    outcome.map(|e| jit_frame::mask_addresses(&e)),
                    printed(&mut vm)
                ),
                want,
                "a refusal in compiled code changed the program's outcome\n{src}"
            );
        }
        // the collector and the finalizers run on the way out with the
        // policy still armed; they must cope with refusals too
    }
    assert_eq!(
        seen.live.load(Relaxed),
        0,
        "the Vm did not give back every byte it allocated ({mode:?}, jit {jit})\n{src}"
    );
});

/// A program of the loops writing what running a pass again would show,
/// in the fixed frame: in a whole program one is picked for few inputs.
fn rerun_program(g: &mut jit_program::Gen<'_, '_>) -> String {
    g.frame();
    for _ in 0..=g.pick(3) {
        g.rerun_kernel();
    }
    g.out.push_str(jit_frame::DUMP);
    std::mem::take(&mut g.out)
}

/// The error running `src` ends with, `None` when it ends normally.
fn outcome_of(vm: &mut luna_core::vm::Vm, src: &str) -> Option<String> {
    match vm.load(src.as_bytes(), b"=prog") {
        Ok(f) => vm
            .call_value(Value::Closure(f), &[])
            .err()
            .map(|e| vm.error_text(&e)),
        Err(e) => Some(String::from_utf8_lossy(&e.msg).into_owned()),
    }
}

/// What the program printed, addresses masked.
fn printed(vm: &mut luna_core::vm::Vm) -> String {
    match vm.eval("return table.concat(__out, '\\n')") {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => {
                jit_frame::mask_addresses(&String::from_utf8_lossy(s.as_bytes()))
            }
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}
