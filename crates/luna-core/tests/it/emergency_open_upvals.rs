//! 5.2 frees an open upvalue whose closures are all dead when the
//! collection sweeps its thread, the variable still in scope; the collector
//! takes it off its thread's list when the marking ends. An emergency
//! collection (an allocation refused) from a pause, which marks and sweeps
//! at once, must do so too, or the list keeps the freed upvalue and closing
//! the frame writes to it.

use luna_core::version::LuaVersion;

use super::refusing_alloc::run;

#[test]
fn an_emergency_collection_takes_dead_open_upvalues_off_their_list() {
    // a long pause, so the emergency collection starts from one (5.1 has
    // no emergency collection, and 5.2 none while the collector is stopped)
    let source = "collectgarbage('setpause', 1000)
         collectgarbage()
         local function f()
           local x = 0
           local g = function() x = x + 1 end
           g()
           g = nil
           local t = {}
           for i = 1, 100 do t[i] = ('y'):rep(50) .. i end
           return x
         end
         return f()";
    for v in [LuaVersion::Lua52] {
        // without the lists lent, it fails refusing from 135
        for (seed, n) in (120..=150).map(|n| (1, n)) {
            let got = run(v, seed, n, source);
            assert!(
                got == "1" || got.starts_with("error:"),
                "{v:?} seed {seed}, refusing from {n}: {got}"
            );
        }
    }
}
