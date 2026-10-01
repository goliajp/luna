//! The error a `__close` handler raised stays alive while a later handler
//! in the same close chain has its coroutine suspended. (PUC runs the
//! closes after such an error without letting them yield; luna lets them.)

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// `b`'s handler raises a table; `a`'s handler receives it, drops its own
/// reference and yields. While the coroutine is suspended the error is
/// reachable only through the close chain; collecting and then allocating
/// more tables must not disturb it.
#[test]
fn a_threaded_close_error_survives_a_collection_while_suspended() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        vm.open_all_libs();
        let r = vm
            .eval(
                r#"
                local function closing(f) return setmetatable({}, {__close = f}) end
                local co = coroutine.wrap(function()
                    local a <close> = closing(function(_, e)
                        e = nil
                        coroutine.yield("paused")
                    end)
                    local b <close> = closing(function()
                        error({tag = "boom"})
                    end)
                    return "done"
                end)
                assert(co() == "paused")
                collectgarbage()
                collectgarbage()
                local junk = {}
                for i = 1, 2000 do junk[i] = {tag = "junk" .. i} end
                local ok, err = pcall(co)
                return (not ok) and type(err) == "table" and err.tag == "boom"
                "#,
            )
            .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_text(&e)));
        assert!(matches!(r.as_slice(), [Value::Bool(true)]), "{v:?}: {r:?}");
    }
}
