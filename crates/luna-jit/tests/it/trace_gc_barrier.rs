//! A trace's table stores take the write barrier, as the interpreter's do.
//! A store of an object the collector has not reached yet (white) into a
//! table it has already traced (black) must send the table back to be
//! traced again; without that, once the object's other references are
//! gone the table is the only way to it, and the sweep frees it while the
//! table still holds it. A `gc-verify` build of luna-core checks at every
//! atomic phase that no black table holds a dead-white object, so it
//! fails there; without the feature the test compares the results.

use luna_jit::version::LuaVersion;

/// The results as text, from a Vm on a thread of its own: `gc-verify`
/// tracks freed objects per thread, and a second Vm on the same thread
/// reuses the first one's addresses.
fn run(src: &'static str, trace: bool) -> String {
    std::thread::spawn(move || {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(trace);
        vm.jit.trace_hot_threshold = 1;
        let r = vm.eval(src).expect("runs");
        if trace {
            assert!(vm.trace_dispatched_count() > 0, "no trace ran");
        }
        format!("{r:?}")
    })
    .join()
    .expect("run panicked")
}

#[test]
fn an_object_a_trace_moves_into_a_traced_table_survives_the_cycle() {
    // Each round makes `items` between cycles (white) at the end of a
    // 2000-table chain and an empty `dst`. A few collector steps start a
    // cycle: `dst`, a register, is traced (black) at once, while the chain
    // keeps the collector from reaching `items` for many steps. The traced
    // loop then moves every item into `dst` and drops it from `items`, so
    // when the cycle ends `dst` is the only way to the items.
    let src = r#"
        collectgarbage("incremental")
        local ok = 0
        for round = 1, 120 do
          collectgarbage()
          local chain = {}
          do
            local c = chain
            for k = 1, 2000 do c.n = {}; c = c.n end
            local items = {}
            for i = 1, 16 do items[i] = {round, i} end
            c.items = items
          end
          local dst = {}
          for s = 1, round % 40 do collectgarbage("step", 1) end
          local it = chain
          for k = 1, 2000 do it = it.n end
          local items = it.items
          for i = 1, 16 do
            dst[i] = items[i]
            items[i] = 0
          end
          repeat until collectgarbage("step", 1)
          for i = 1, 16 do
            if dst[i][1] == round and dst[i][2] == i then ok = ok + 1 end
          end
        end
        return ok"#;
    let want = run(src, false);
    assert_eq!(want, "[Int(1920)]");
    assert_eq!(run(src, true), want);
}
