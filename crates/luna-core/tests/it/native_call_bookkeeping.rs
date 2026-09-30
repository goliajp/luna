//! The native-call path: `pcall` / `xpcall` / `pairs` are recognised by a
//! kind fixed when the closure is made, whatever name they are called
//! under; the safe-point GC check is one comparison that
//! `collectgarbage("stop")` must still switch off.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn run(v: LuaVersion, src: &str) {
    let mut vm = Vm::new(v);
    if let Err(e) = vm.eval(src) {
        panic!("{v:?}: {}", vm.error_text(&e));
    }
}

#[test]
fn renamed_yieldable_natives_still_yield() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        run(
            v,
            r#"
            local p, xp, y = pcall, xpcall, coroutine.yield
            local co = coroutine.wrap(function()
                local ok, r = p(function() return y(1) + 1 end)
                assert(ok and r == 11)
                local ok2, r2 = xp(function() return y(2) * 2 end, debug.traceback)
                assert(ok2 and r2 == 40)
                return "done"
            end)
            assert(co() == 1)
            assert(co(10) == 2)
            assert(co(20) == "done")
            "#,
        );
    }
    run(
        LuaVersion::Lua54,
        r#"
        local pr = pairs
        local t = setmetatable({}, {__pairs = function()
            return function(_, k) if not k then return 1, coroutine.yield("in") end end, nil, nil
        end})
        local co = coroutine.wrap(function()
            for k, v in pr(t) do return k, v end
        end)
        assert(co() == "in")
        local k, v = co("val")
        assert(k == 1 and v == "val")
        "#,
    );
}

#[test]
fn stopped_collector_stays_stopped_until_restart() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        run(
            v,
            r#"
            collectgarbage()
            collectgarbage("stop")
            local before = collectgarbage("count")
            for i = 1, 20000 do local _ = {i, i} end
            local grown = collectgarbage("count") - before
            assert(grown > 1000, grown)
            collectgarbage("restart")
            local peak = collectgarbage("count")
            for i = 1, 200000 do local _ = {i, i} end
            assert(collectgarbage("count") < peak + 4000)
            "#,
        );
    }
}
