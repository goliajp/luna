//! Library functions keep what they hold across the checks for a
//! collection their C API calls make, as PUC keeps it on the stack. The
//! collector is set to finish a cycle at almost every check, so an object
//! held only by the function is freed before its use, and the result
//! shows it (under `gc-verify` the dangling reference panics at the sweep
//! that makes it; with `gc-stress` too every check collects in full).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Lua that makes the collector finish a cycle at each check it can.
fn eager(v: LuaVersion) -> &'static str {
    match v {
        LuaVersion::Lua54 => "collectgarbage('incremental', 1, 1000)",
        LuaVersion::Lua55 => {
            "collectgarbage('param', 'pause', 1) collectgarbage('param', 'stepmul', 1000)"
        }
        _ => "collectgarbage('setpause', 1) collectgarbage('setstepmul', 100000)",
    }
}

fn run(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    let src = format!("{} {src}", eager(v));
    match vm.eval(&src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

/// The metatable a string buffer's box gets is found in the registry under
/// its name, and names itself (5.3 `luaL_newmetatable`).
#[test]
fn the_buffer_box_metatable_keeps_its_name() {
    for (v, name) in [
        (LuaVersion::Lua53, "LUABOX"),
        (LuaVersion::Lua54, "_UBOX*"),
        (LuaVersion::Lua55, "_UBOX*"),
    ] {
        // the metatable made afresh each time, while the collector has
        // just finished a cycle and starts the next with little to do
        let src = format!(
            "local reg, got = debug.getregistry(), 'none'
             for i = 1, 60 do
               reg['{name}'] = nil
               collectgarbage()
               local s = string.rep('x', 20000 + i)
               local mt = reg['{name}']
               got = type(mt) == 'table' and tostring(mt.__name or (mt.__gc and 'gc')) or 'none'
               if got == 'none' then break end
             end
             return got"
        );
        let want = if v == LuaVersion::Lua53 { name } else { "gc" };
        assert_eq!(run(v, &src), want, "{v:?}");
    }
}

/// 5.1 `table.concat` of a number that does not fit the buffer: the
/// number's string is on the stack while the buffer is emptied.
#[test]
fn concat_keeps_a_number_string_while_the_buffer_empties() {
    let src = "local big = string.rep('a', 8190)
               local out = {}
               for i = 1, 30 do out[i] = #table.concat({big, 1.25 + i}) end
               return table.concat(out, ',', 1, 3)";
    assert_eq!(run(LuaVersion::Lua51, src), "8194,8194,8194");
}

/// 5.1–5.3 `debug.getinfo(level, ">f", x)` describes `x`, popped off the
/// stack, and returns it as the function.
#[test]
fn getinfo_keeps_the_popped_function() {
    for v in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        let src = "local n = 0
                   for i = 1, 40 do
                     if type(debug.getinfo(1, '>f', {i}).func) == 'table' then n = n + 1 end
                   end
                   return tostring(n)";
        assert_eq!(run(v, src), "40", "{v:?}");
    }
}
