//! Compiled code whose allocations fail. A helper that cannot allocate
//! does not collect (compiled code may hold new objects only in machine
//! registers): a trace leaves at the instruction and the interpreter runs
//! it, a method-JIT call is run again by the interpreter. Either way the
//! program computes what it computes without the JIT.
//!
//! The policies here refuse only allocations made while compiled code
//! runs (an `enter_jit` window is open), so every refusal lands in a
//! helper; the interpreter's allocations go through.

use std::cell::Cell;
use std::rc::Rc;

use luna_jit::jit_backend::__jit_tls_ptrs;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::JitProtoState;
use luna_jit::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn in_compiled_code() -> bool {
    !__jit_tls_ptrs().0.is_null()
}

/// Counts the growths compiled code asks for, of `kind` (any kind when
/// `None`), and refuses the `from`th and, unless `once`, every later one.
#[derive(Clone)]
struct Refuse {
    kind: Option<BlockKind>,
    from: usize,
    once: bool,
    seen: Rc<Cell<usize>>,
    refused: Rc<Cell<usize>>,
}

impl Refuse {
    fn new(kind: Option<BlockKind>, from: usize, once: bool) -> Refuse {
        Refuse {
            kind,
            from,
            once,
            seen: Rc::new(Cell::new(0)),
            refused: Rc::new(Cell::new(0)),
        }
    }
}

impl MemoryPolicy for Refuse {
    fn allow(&mut self, _old: usize, _new: usize, kind: BlockKind, _in_use: usize) -> bool {
        if !in_compiled_code() || self.kind.is_some_and(|k| k != kind) {
            return true;
        }
        let n = self.seen.get() + 1;
        self.seen.set(n);
        let refuse = if self.once {
            n == self.from
        } else {
            n >= self.from
        };
        if refuse {
            self.refused.set(self.refused.get() + 1);
        }
        !refuse
    }
}

fn vm_with(v: LuaVersion, policy: Option<Refuse>, traces: bool) -> Vm {
    let mem = match policy {
        Some(p) => MemOwner::policy(Box::new(p)),
        None => MemOwner::system(),
    };
    let mut vm = Vm::new_with_mem(v, mem);
    luna_jit::install_default_jit(&mut vm);
    vm.set_trace_jit_enabled(traces);
    vm.jit.trace_hot_threshold = 1;
    vm.jit.call_hot_threshold = 1;
    vm
}

fn result(vm: &mut Vm, src: &str) -> String {
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Int(i)) => i.to_string(),
            Some(Value::Float(f)) => f.to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

fn interpreted(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    result(&mut vm, src)
}

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// Trace loops allocating in compiled code: new tables, a growing array
/// part, a string accumulator, closures.
const TRACE_PROGS: &[&str] = &[
    "local t = {} for i = 1, 2000 do t[i] = {i, i * 2} end \
     local s = 0 for i = 1, #t do s = s + t[i][2] end return s",
    "local t = {} for i = 1, 5000 do t[#t + 1] = i end return #t",
    "local s = '' for i = 1, 3000 do s = s .. 'x' end return #s",
    "local fs = {} for i = 1, 1000 do local j = i fs[i] = function() return j end end \
     local s = 0 for i = 1, #fs do s = s + fs[i]() end return s",
];

/// How many growths of `kind` the program's compiled code makes.
fn compiled_growths(v: LuaVersion, src: &str, kind: Option<BlockKind>) -> usize {
    let p = Refuse::new(kind, usize::MAX, true);
    let seen = p.seen.clone();
    let mut vm = vm_with(v, Some(p), true);
    result(&mut vm, src);
    seen.get()
}

/// The refused growths: a spread of up to `max` of the `n` there are.
fn spread(n: usize, max: usize) -> Vec<usize> {
    let step = (n / max).max(1);
    (1..=n).step_by(step).collect()
}

#[test]
fn a_trace_that_cannot_allocate_leaves_and_the_interpreter_allocates() {
    for v in VERSIONS {
        for src in TRACE_PROGS {
            let want = interpreted(v, src);
            for kind in [
                Some(BlockKind::Table),
                Some(BlockKind::Str),
                Some(BlockKind::LuaFn),
                None,
            ] {
                let n = compiled_growths(v, src, kind);
                for k in spread(n, 12) {
                    let p = Refuse::new(kind, k, true);
                    let refused = p.refused.clone();
                    let mut vm = vm_with(v, Some(p), true);
                    assert_eq!(result(&mut vm, src), want, "{v:?} {kind:?} #{k}: {src}");
                    assert_eq!(refused.get(), 1, "{v:?} {kind:?} #{k}: {src}");
                    assert!(
                        vm.trace_dispatched_count() > 0,
                        "{v:?}: no trace ran: {src}"
                    );
                }
            }
        }
    }
}

#[test]
fn compiled_code_refused_every_allocation_from_some_point_still_agrees() {
    for v in VERSIONS {
        for src in TRACE_PROGS {
            let want = interpreted(v, src);
            let n = compiled_growths(v, src, None);
            for k in spread(n, 8) {
                let p = Refuse::new(None, k, false);
                let mut vm = vm_with(v, Some(p), true);
                assert_eq!(result(&mut vm, src), want, "{v:?} from #{k}: {src}");
            }
        }
    }
}

/// Functions the method JIT compiles whole: a constructor, and stores that
/// grow a table it was given.
const METHOD_PROGS: &[&str] = &[
    "local function mk(i) return {i, i + 1, i + 2} end \
     local s = 0 for i = 1, 400 do s = s + mk(i)[2] end return s",
    "local function fill(t, n) for i = 1, n do t[i] = i end return t end \
     local s = 0 for i = 1, 60 do s = s + #fill({}, i * 7) end return s",
];

#[test]
fn a_method_jit_call_that_cannot_allocate_runs_in_the_interpreter() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for src in METHOD_PROGS {
            let want = interpreted(v, src);
            let count = {
                let p = Refuse::new(None, usize::MAX, true);
                let seen = p.seen.clone();
                let mut vm = vm_with(v, Some(p), false);
                let cl = vm.load(src.as_bytes(), b"=m").expect("load");
                vm.call_value(Value::Closure(cl), &[]).expect("run");
                let inner = cl.proto.protos[0];
                assert!(
                    matches!(inner.jit.get(), JitProtoState::Compiled { .. }),
                    "{v:?}: not compiled: {src}"
                );
                seen.get()
            };
            assert!(count > 0, "{v:?}: compiled code allocated nothing: {src}");
            for k in spread(count, 12) {
                for once in [true, false] {
                    let p = Refuse::new(None, k, once);
                    let mut vm = vm_with(v, Some(p), false);
                    assert_eq!(result(&mut vm, src), want, "{v:?} #{k} once {once}: {src}");
                }
            }
        }
    }
}

/// A policy refusing every growth from the `n`th after it is armed,
/// wherever it is made.
struct After {
    armed: Rc<Cell<bool>>,
    seen: usize,
    n: usize,
}

impl MemoryPolicy for After {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        if !self.armed.get() {
            return true;
        }
        self.seen += 1;
        self.seen < self.n
    }
}

/// With the JIT on, running out of memory anywhere is the memory error,
/// never an abort, for every point it can run out at.
#[test]
#[ignore = "needs the interpreter's allocations to raise memory errors too"]
fn running_out_with_the_jit_on_is_a_memory_error() {
    for v in VERSIONS {
        for src in TRACE_PROGS.iter().chain(METHOD_PROGS) {
            let want = interpreted(v, src);
            let mut n = 1;
            loop {
                let armed = Rc::new(Cell::new(false));
                let policy = After {
                    armed: armed.clone(),
                    seen: 0,
                    n,
                };
                let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(policy)));
                luna_jit::install_default_jit(&mut vm);
                vm.jit.trace_hot_threshold = 1;
                vm.jit.call_hot_threshold = 1;
                armed.set(true);
                let r = result(&mut vm, src);
                armed.set(false);
                if r == want {
                    break;
                }
                assert_eq!(r, "error: not enough memory", "{v:?} after {n}: {src}");
                n += 1 + n / 16;
            }
        }
    }
}
