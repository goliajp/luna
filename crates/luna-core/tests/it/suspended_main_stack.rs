//! A collection while a coroutine runs marks the main thread's stack up to
//! the top of the native that resumed it, and clears what is above, an
//! emergency one (an allocation refused) too. A dead string left in a
//! register above that top must not stay in its slot once it is freed: the
//! coroutine's failure goes back to the wrapping native, whose error
//! message refuses its allocation as well, and that emergency collection
//! marks the main stack whole.

use luna_core::version::LuaVersion;

use super::refusing_alloc::run;

#[test]
fn a_collection_in_a_coroutine_leaves_nothing_freed_on_the_main_stack() {
    let source = "local t = {}
         for i = 1, 40 do t[i] = {k = 'v' .. i, f = function() return i end} end
         local co = coroutine.wrap(function(a) local b = coroutine.yield(a .. '!') return b .. '?' end)
         local x = co('a') .. co('b')
         return #t .. ' ' .. x .. ' ' .. t[40].f()";
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        // without the clearing, 5.2 fails refusing from 382 to 386, 5.3 from
        // 375 to 379, whatever the seed
        for (seed, n) in (360..=400).map(|n| (1, n)) {
            let got = run(v, seed, n, source);
            assert!(
                got == "40 a!b? 40" || got.starts_with("error:"),
                "{v:?} seed {seed}, refusing from {n}: {got}"
            );
        }
    }
}
