//! Vms of one engine on several threads.

use super::*;

#[test]
fn vms_on_other_threads_take_the_traces() {
    let engine = Engine::new();
    let want: Vec<Vec<String>> = PROGRAMS
        .iter()
        .map(|(_, src)| run(&mut interp(LuaVersion::Lua54), src, 2).results)
        .collect();
    for (_, src) in PROGRAMS {
        run(&mut shared(&engine, LuaVersion::Lua54), src, 2);
    }
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let engine = engine.clone();
            std::thread::spawn(move || {
                PROGRAMS
                    .iter()
                    .map(|(_, src)| run(&mut shared(&engine, LuaVersion::Lua54), src, 2))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for h in handles {
        for (k, r) in h.join().expect("thread").iter().enumerate() {
            assert_adopted_only(PROGRAMS[k].0, r, &want[k]);
        }
    }
}

/// Threads that all start at once: they compile and publish concurrently,
/// and whatever each installs runs correctly.
#[test]
fn vms_compiling_at_once_on_many_threads_agree() {
    let engine = Engine::new();
    let want: Vec<Vec<String>> = PROGRAMS
        .iter()
        .map(|(_, src)| run(&mut interp(LuaVersion::Lua54), src, 2).results)
        .collect();
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let engine = engine.clone();
            std::thread::spawn(move || {
                PROGRAMS
                    .iter()
                    .map(|(_, src)| run(&mut shared(&engine, LuaVersion::Lua54), src, 2).results)
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().expect("thread"), want);
    }
}

#[cfg(feature = "send")]
#[test]
fn a_vm_moved_to_another_thread_installs_the_traces_there() {
    let engine = Engine::new();
    run(&mut shared(&engine, LuaVersion::Lua54), TOKEN, 2);
    let want = run(&mut interp(LuaVersion::Lua54), TOKEN, 2).results;
    let vm = luna_jit::vm::SendVm::from_vm(shared(&engine, LuaVersion::Lua54));
    let got = std::thread::spawn(move || {
        let f = vm.eval(TOKEN).expect("chunk")[0];
        let results: Vec<String> = (0..2)
            .map(|_| {
                let v = vm.call_value(f, &[]).expect("call");
                v.iter().map(show).collect::<Vec<_>>().join(", ")
            })
            .collect();
        let counts = vm.with_vm(|vm| (vm.trace_compiled_count(), vm.trace_adopted_count()));
        (results, counts)
    })
    .join()
    .expect("thread");
    assert_eq!(got.0, want);
    assert_eq!(got.1.0, 0, "the moved Vm compiled");
    assert!(got.1.1 > 0, "the moved Vm installed nothing");
}

#[test]
fn engine_and_its_vms_build_on_any_thread() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Engine>();
}
