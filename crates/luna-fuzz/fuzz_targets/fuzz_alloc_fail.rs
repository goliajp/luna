//! Allocation failure fuzz target.
//!
//! Runs a generated program (the JIT differential target's generator:
//! hot loops, closures, tables, strings, metatables, coroutines, garbage)
//! in a Vm whose memory policy refuses allocations: every one from the Nth
//! on, only the Nth (so the emergency collection's retry succeeds), or any
//! growth past a byte limit. The JIT is on for half the inputs. Asserts:
//! - no panic and no abort: a refused allocation is a Lua memory error
//!   (or the program's own error), never the end of the process;
//! - an error that is the memory error carries "not enough memory";
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
use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use std::cell::Cell;
use std::rc::Rc;

#[allow(dead_code)]
#[path = "program.rs"]
mod program;

#[path = "jit_program.rs"]
mod jit_program;

#[path = "jit_stmts.rs"]
mod jit_stmts;

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

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// refuse every allocation from the `n`th on
    From(u32),
    /// refuse only the `n`th
    Once(u32),
    /// refuse growth past this many bytes in use
    Limit(usize),
}

/// What the policy saw, shared with the test after the Vm is gone.
#[derive(Default)]
struct Seen {
    armed: Cell<bool>,
    asked: Cell<u32>,
    refused: Cell<u32>,
    live: Cell<isize>,
}

struct Policy(Mode, Rc<Seen>);

impl MemoryPolicy for Policy {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        let s = &self.1;
        let ok = !s.armed.get() || {
            let n = s.asked.get() + 1;
            s.asked.set(n);
            match self.0 {
                Mode::From(k) => n < k,
                Mode::Once(k) => n != k,
                Mode::Limit(l) => in_use - old.min(in_use) + new <= l,
            }
        };
        if ok {
            s.live.set(s.live.get() + new as isize - old as isize);
        } else {
            s.refused.set(s.refused.get() + 1);
        }
        ok
    }

    fn freed(&mut self, size: usize) {
        self.1.live.set(self.1.live.get() - size as isize);
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let mode = match u.int_in_range(0..=2).unwrap_or(0) {
        0 => Mode::From(u.int_in_range(1..=20_000).unwrap_or(1)),
        1 => Mode::Once(u.int_in_range(1..=20_000).unwrap_or(1)),
        _ => Mode::Limit(u.int_in_range(64 << 10..=8 << 20).unwrap_or(64 << 10)),
    };
    let jit = u.arbitrary::<bool>().unwrap_or(false);
    let src = jit_program::Gen::new(&mut u, program::dialect()).program();
    let show = std::env::var_os("LUNA_FUZZ_SHOW").is_some();
    if show {
        eprintln!("=== {mode:?}, jit {jit} ===\n{src}");
    }

    let seen = Rc::new(Seen::default());
    {
        let mem = MemOwner::policy(Box::new(Policy(mode, seen.clone())));
        // the policy is armed once the Vm and its libraries are made: a Vm
        // that cannot be made is `try_new_minimal_with_mem`'s business
        let mut vm = luna_core::vm::Vm::new_with_mem(program::dialect(), mem);
        if jit {
            luna_jit::install_default_jit(&mut vm);
            vm.jit().trace_hot_threshold = 2;
            vm.jit().call_hot_threshold = 2;
        } else {
            vm.install_null_jit();
        }
        vm.set_instr_budget(Some(50_000_000));
        seen.armed.set(true);
        let outcome = match vm.load(src.as_bytes(), b"=prog") {
            Ok(f) => vm.call_value(Value::Closure(f), &[]).err().map(|e| vm.error_text(&e)),
            Err(e) => Some(String::from_utf8_lossy(&e.msg).into_owned()),
        };
        if show {
            eprintln!(
                "=== outcome: {outcome:?}, asked {}, refused {} ===",
                seen.asked.get(),
                seen.refused.get()
            );
        }
        // a refusal surfaces as the memory error, unless the program's own
        // error came first or a pcall in it swallowed the memory error
        if let Some(e) = &outcome {
            assert!(
                seen.refused.get() > 0
                    || !e.contains("not enough memory")
                    || src.contains("not enough memory"),
                "a memory error with nothing refused: {e}\n{src}"
            );
        }
        // the collector and the finalizers run on the way out with the
        // policy still armed; they must cope with refusals too
    }
    assert_eq!(
        seen.live.get(),
        0,
        "the Vm did not give back every byte it allocated ({mode:?}, jit {jit})\n{src}"
    );
});
