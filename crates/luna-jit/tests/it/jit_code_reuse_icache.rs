//! A dropped `Vm` frees its JIT code and the next compile can get the same
//! memory back. On aarch64 the new code has to be synced into the
//! instruction cache before it runs, or a core that ran the old function
//! at that address can run it again.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn call(vm: &mut Vm, f: &Value) -> String {
    let r = vm.call_value(*f, &[]).expect("call");
    format!("{:?}", r.into_iter().next().unwrap_or(Value::Nil))
}

fn load(vm: &mut Vm, src: &str) -> Value {
    Value::Closure(vm.load(src.as_bytes(), b"=reuse").expect("load"))
}

#[cfg(target_os = "linux")]
mod affinity {
    unsafe extern "C" {
        fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
        fn sched_getaffinity(pid: i32, size: usize, mask: *mut u64) -> i32;
    }

    /// The CPUs this thread may run on.
    pub fn allowed() -> Vec<usize> {
        let mut mask = [0u64; 16];
        // SAFETY: mask is a cpu_set_t-sized buffer
        let rc = unsafe { sched_getaffinity(0, std::mem::size_of_val(&mask), mask.as_mut_ptr()) };
        assert_eq!(rc, 0, "sched_getaffinity");
        (0..1024)
            .filter(|c| mask[c / 64] & (1 << (c % 64)) != 0)
            .collect()
    }

    pub fn pin(cpu: usize) {
        let mut mask = [0u64; 16];
        mask[cpu / 64] = 1 << (cpu % 64);
        // SAFETY: mask is a cpu_set_t-sized buffer
        let rc = unsafe { sched_setaffinity(0, std::mem::size_of_val(&mask), mask.as_ptr()) };
        assert_eq!(rc, 0, "sched_setaffinity({cpu})");
    }
}

#[cfg(not(target_os = "linux"))]
mod affinity {
    pub fn allowed() -> Vec<usize> {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        (0..n).collect()
    }

    pub fn pin(_cpu: usize) {}
}

// core `a` runs 5.2's `return n` (float code) at some address, the Vm
// drops and frees it, core `b` compiles 5.3's `return n` (int code) into
// the same memory, then core `a` runs that code. without an instruction
// cache sync, `a` still holds the 5.2 instructions and 5.3 gets the
// float's bits tagged as Int
#[test]
fn code_recompiled_at_freed_address_runs_new_instructions() {
    let cpus = affinity::allowed();
    // pinning is per thread; with --test-threads=1 the test itself runs on
    // the main thread, which later tests share
    let wrong = std::thread::spawn(move || {
        let mut bad = Vec::new();
        for round in 0..40 {
            for (i, &a) in cpus.iter().enumerate() {
                let b = cpus[(i + 1 + round) % cpus.len()];
                let n = (round * 1000 + a) as i64;
                let src = format!("return {n}");
                affinity::pin(a);
                let mut old = luna_jit::new_with_jit(LuaVersion::Lua52);
                let f = load(&mut old, &src);
                call(&mut old, &f);
                affinity::pin(b);
                drop(old);
                let mut vm = luna_jit::new_with_jit(LuaVersion::Lua53);
                let f = load(&mut vm, &src);
                let first = call(&mut vm, &f);
                affinity::pin(a);
                let again = call(&mut vm, &f);
                for (cpu, got) in [(b, first), (a, again)] {
                    if got != format!("Int({n})") {
                        bad.push(format!("return {n} run on cpu {cpu}: {got}"));
                    }
                }
            }
        }
        bad
    })
    .join()
    .unwrap();
    assert!(
        wrong.is_empty(),
        "stale jit code ran ({} times): {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
}
