//! Running out of memory everywhere, not only in compiled code.

use super::*;

/// A policy refusing every growth from the `n`th after it is armed,
/// wherever it is made.
struct After {
    armed: Arc<AtomicBool>,
    seen: usize,
    n: usize,
}

impl MemoryPolicy for After {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        if !self.armed.load(Relaxed) {
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
                let armed = Arc::new(AtomicBool::new(false));
                let policy = After {
                    armed: armed.clone(),
                    seen: 0,
                    n,
                };
                let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(policy)));
                luna_jit::install_default_jit(&mut vm);
                vm.set_trace_hot_threshold(1);
                vm.set_call_hot_threshold(1);
                armed.store(true, Relaxed);
                let r = result(&mut vm, src);
                armed.store(false, Relaxed);
                if r == want {
                    break;
                }
                assert_eq!(r, "error: not enough memory", "{v:?} after {n}: {src}");
                n += 1 + n / 16;
            }
        }
    }
}
