//! The open upvalue lists the VM hands the collector at the end of a
//! marking come back when the collection ends, whichever way it ends: an
//! upvalue left off its list would not close when its function returns.

use crate::runtime::Value;
use crate::runtime::puc::api::PucCollector;
use crate::version::LuaVersion;
use crate::vm::Vm;

/// `collect` runs while `x` is open on the main thread and, inside a
/// coroutine, while `y` is open there and the main thread waits; the
/// functions return, other calls reuse their stack slots, and the
/// closures must read what was closed.
fn closed_after(collect: &str) -> String {
    format!(
        "local function clobber(a, b, c, d, e, f) return a end
         local function mk()
           local x = 5
           local fx = function() return x end
           local co = coroutine.create(function()
             local y = 6
             local fy = function() return y end
             {collect}
             return fy
           end)
           local _, fy = coroutine.resume(co)
           {collect}
           return fx, fy
         end
         local fx, fy = mk()
         clobber(7, 7, 7, 7, 7, 7)
         return fx(), fy()"
    )
}

fn run(v: LuaVersion, collect: &str) -> (i64, i64) {
    let mut vm = Vm::new(v);
    let r = vm.eval(&closed_after(collect)).expect("the script runs");
    assert!(vm.heap.take_open_lend().is_none(), "{v:?}: lists left lent");
    // 5.1 and 5.2 have floats only
    let n = |x: Value| match x {
        Value::Int(i) => i,
        Value::Float(f) => f as i64,
        other => panic!("{v:?}: {other:?}"),
    };
    (n(r[0]), n(r[1]))
}

#[test]
fn the_open_lists_come_back_after_every_kind_of_collection() {
    // a full collection; an incremental cycle stepped through its atomic
    // step; a cycle under way dropped by a full collection
    let ways = [
        "collectgarbage()",
        "repeat until collectgarbage('step', 0)",
        "collectgarbage('step', 0) collectgarbage()",
    ];
    for v in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        for w in ways {
            assert_eq!(run(v, w), (5, 6), "{v:?} {w}");
        }
    }
}
