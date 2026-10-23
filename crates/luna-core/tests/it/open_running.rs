//! An embedder that opens the standard libraries with the collector
//! running (PUC `luaL_newstate` then `luaL_openlibs`, no `LUA_GCSTOP`),
//! then loads a chunk as `luaL_loadstring` does and runs it, with marks in
//! the accounting trace (`gc-trace`) where such a host's instrumented PUC
//! marks its run: for comparing the two step by step. Ignored: run by
//! hand with `LUNA_PROBE_VERSION` (51 to 55) and `LUNA_PROBE_CHUNK`.
//! The counts such a host sees are PUC's, as measured on its 64-bit build.

use luna_core::__private::{VmCApiLoad, VmHostInternals};
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

const CHUNK: &str = "local t, out = {}, {} for i = 1, 2000 do t[i] = {i} \
    if i % 250 == 0 then out[#out + 1] = math.floor(collectgarbage(\"count\") * 1024) end \
    end return table.concat(out, \" \")";

/// The count once the libraries are open, then what `CHUNK` (loaded as
/// `luaL_loadstring` loads it, and called) reports.
fn embedder_counts(v: LuaVersion) -> String {
    let mut vm = Vm::new(v);
    let opened = i64::from(vm.host_gc(3, [0; 3]).expect("count")) * 1024
        + i64::from(vm.host_gc(4, [0; 3]).expect("count"));
    let Ok((f, _)) = vm.host_load_chunk(CHUNK.as_bytes(), CHUNK.as_bytes()) else {
        panic!("{v:?}: the chunk compiles");
    };
    let r = vm
        .call_value(Value::Closure(f), &[])
        .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_display(&e)));
    let Some(Value::Str(s)) = r.first() else {
        panic!("{v:?}: a string");
    };
    format!("{opened} {}", String::from_utf8_lossy(s.as_bytes()))
}

#[test]
#[cfg(target_pointer_width = "64")]
fn vm_new_counts_what_an_embedder_counts() {
    // with `gc-stress` every checkpoint collects: no garbage is left
    // floating when the counts are read (and 5.1's `Vm::new` collects as
    // it opens the libraries)
    let stressed = cfg!(feature = "gc-stress");
    let want = [
        (
            LuaVersion::Lua51,
            if stressed {
                "26372 51729 75841 104049 124081 160465 180529 200529 220529"
            } else {
                "26597 53314 75841 104049 124081 160465 180529 200529 220529"
            },
        ),
        (
            LuaVersion::Lua52,
            if stressed {
                "21388 45110 67222 93430 111462 145846 163910 181910 199910"
            } else {
                "21413 45446 67558 93766 111798 146182 164246 182246 200246"
            },
        ),
        (
            LuaVersion::Lua53,
            if stressed {
                "22299 46304 68416 94624 112656 147040 165104 183104 201104"
            } else {
                "22299 47384 69496 94624 112656 147040 165104 183104 201104"
            },
        ),
        (
            LuaVersion::Lua54,
            if stressed {
                "20385 43677 65789 91997 110029 144413 162477 180477 198477"
            } else {
                "20410 43677 65789 91997 110029 144413 162477 180477 198477"
            },
        ),
        (
            LuaVersion::Lua55,
            if stressed {
                "18717 37447 55014 74881 90149 114615 129901 145151 160401"
            } else {
                "18817 37447 55014 74881 90149 114615 129901 145151 160401"
            },
        ),
    ];
    for (v, w) in want {
        assert_eq!(embedder_counts(v), w, "{v:?}");
    }
}
