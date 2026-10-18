//! An embedder that opens the standard libraries with the collector
//! running (PUC `luaL_newstate` then `luaL_openlibs`, no `LUA_GCSTOP`),
//! then loads a chunk as `luaL_loadstring` does and runs it, with marks in
//! the accounting trace (`gc-trace`) where such a host's instrumented PUC
//! marks its run: for comparing the two step by step. Ignored: run by
//! hand with `LUNA_PROBE_VERSION` (51 to 55) and `LUNA_PROBE_CHUNK`.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[test]
#[ignore]
fn open_running_trace() {
    let v = match std::env::var("LUNA_PROBE_VERSION").as_deref() {
        Ok("51") => LuaVersion::Lua51,
        Ok("52") => LuaVersion::Lua52,
        Ok("53") => LuaVersion::Lua53,
        Ok("54") => LuaVersion::Lua54,
        _ => LuaVersion::Lua55,
    };
    let chunk = std::env::var("LUNA_PROBE_CHUNK").unwrap_or_default();
    let mut vm = Vm::new_minimal(v);
    vm.trace_mark("openlibs");
    vm.open_all_libs().expect("the libraries open");
    vm.trace_mark("/openlibs");
    // `LUNA_PROBE_STATE=<PUC dump>`: the collector's state beside PUC's
    #[cfg(feature = "gc-trace")]
    if let Ok(path) = std::env::var("LUNA_PROBE_STATE") {
        let dump = std::fs::read_to_string(&path).expect("the dump");
        std::fs::write(format!("{path}.luna"), vm.trace_compare_state(&dump)).expect("written");
    }
    if !chunk.is_empty() {
        // `luaL_loadstring`: `lua_load`, with its collection checks
        let Ok((f, _)) = vm.host_load_chunk(chunk.as_bytes(), chunk.as_bytes()) else {
            panic!("the chunk compiles");
        };
        vm.trace_mark("/load");
        if let Err(e) = vm.call_value(Value::Closure(f), &[]) {
            panic!("{}", vm.error_display(&e));
        }
    }
    vm.trace_mark("close");
}
