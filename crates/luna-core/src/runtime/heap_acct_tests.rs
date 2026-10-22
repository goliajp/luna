//! A table's or a thread's blocks are counted as the object itself is,
//! whatever luna builds meanwhile: what its free takes off the count is
//! what was counted. Each case runs a script twice, the second time with
//! `quiet()` setting counting aside (as luna does while it builds objects
//! of its own) where the first has a call that does nothing; after a full
//! collection the counts are the same.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::{LuaError, Vm};

fn quiet(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    vm.heap.puc.silent += 1;
    Ok(vm.nat_return(fs, &[]))
}

fn loud(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    vm.heap.puc.silent -= 1;
    Ok(vm.nat_return(fs, &[]))
}

fn nothing(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[]))
}

/// The count after `script` runs and a full collection, `quiet` and
/// `loud` doing what they say or nothing.
fn count_after(v: LuaVersion, script: &str, aside: bool, reopen: bool) -> usize {
    let mut vm = Vm::new(v);
    if reopen {
        vm.open_all_libs().unwrap();
    }
    let (q, l) = if aside {
        (vm.native(quiet), vm.native(loud))
    } else {
        (vm.native(nothing), vm.native(nothing))
    };
    vm.set_global("quiet", q).unwrap();
    vm.set_global("loud", l).unwrap();
    vm.eval(script).unwrap();
    vm.set_global("quiet", Value::Nil).unwrap();
    vm.set_global("loud", Value::Nil).unwrap();
    vm.collect_garbage();
    vm.collect_garbage();
    vm.heap.bytes
}

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn same_count_in(versions: &[LuaVersion], script: &str, reopen: bool) {
    for &v in versions {
        let plain = count_after(v, script, false, reopen);
        let aside = count_after(v, script, true, reopen);
        assert_eq!(aside, plain, "{v:?}: the count after the script");
    }
}

fn same_count(script: &str) {
    same_count_in(&ALL, script, false);
}

/// A table made while counting is set aside is luna's own: neither its
/// growth afterwards nor its free is counted.
#[test]
fn a_table_made_aside_is_never_counted() {
    same_count(
        "quiet(); local t = {}; loud()
         for i = 1, 200 do t[i] = i; t['k' .. i] = i end",
    );
}

/// A counted table grown while counting is set aside: the growth is
/// counted, as its free takes it off.
#[test]
fn a_counted_table_grown_aside_is_counted() {
    same_count(
        "local t, keys = {}, {}
         for i = 1, 200 do keys[i] = 'k' .. i end
         quiet(); for i = 1, 200 do t[i] = i; t[keys[i]] = i end; loud()",
    );
}

/// A counted thread whose stack grows while counting is set aside: the
/// growth is counted, as the thread's free takes it off.
#[test]
fn a_counted_thread_grown_aside_is_counted() {
    same_count(
        "local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end
         local co = coroutine.create(function() quiet(); deep(300); loud() end)
         assert(coroutine.resume(co))",
    );
}

/// Counted objects freed by a collection while counting is set aside:
/// taken off the count as they were counted (strings, closures and their
/// upvalues, threads, tables, and what a load leaves behind).
#[test]
fn a_counted_object_freed_aside_is_taken_off() {
    same_count(
        "local keep = {}
         for i = 1, 100 do
           keep[i] = {('s'):rep(i + 50), function() return i end,
                      coroutine.create(function() end), {i}}
         end
         local f = (loadstring or load)('local a = {1, 2, {3}} return a, \"k\" .. 1')
         keep, f = nil, nil
         quiet(); collectgarbage(); loud()",
    );
}

/// Records of PUC's objects luna does not make (a binary load's extra
/// closure, a buffer's box), freed by a collection while counting is set
/// aside: taken off as they were counted.
#[test]
fn a_counted_record_freed_aside_is_taken_off() {
    same_count(
        "collectgarbage('stop')
         local f = (loadstring or load)(string.dump(function(a) return a end))
         local s = ('x'):rep(20000) .. table.concat({('y'):rep(9000), ('z'):rep(9000)})
         f, s = nil, nil
         quiet(); collectgarbage(); loud()",
    );
}

/// 5.1's io environments of a first opening of the libraries, which a
/// second one replaces, freed while counting is set aside.
#[test]
fn an_io_environment_freed_aside_is_taken_off() {
    same_count_in(
        &[LuaVersion::Lua51],
        "quiet(); collectgarbage(); collectgarbage(); loud()",
        true,
    );
}
