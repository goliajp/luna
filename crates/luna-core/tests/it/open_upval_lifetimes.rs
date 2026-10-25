//! Open upvalues outliving what made them: a thread that dies by an error
//! or is collected while closures still hold its variables, frames that an
//! error unwinds, and closures made and dropped in every phase of a cycle.
//! 5.1 and 5.2 keep an open upvalue off the collector's list until it
//! closes; each closure must keep its variable, and no freed block may be
//! read, with collections from allocations refused at every point.

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

use super::refusing_alloc::{run, run_refusing};

const BODY: &str = include_str!("open_upval_lifetimes.lua");

const WANT: &str = "suspended\t2
dies\tfalse
dead\t3
dropped\t4
unwound\t1760
orphaned\tb
orphaned\tc
loop\t30000
again\t11\t11
pairs\t15150
steps\ttrue";

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn source() -> String {
    format!(
        "local out = {{}}
         local function emit(...)
           local t = {{}}
           for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
           out[#out + 1] = table.concat(t, '\\t')
         end
         {BODY}
         return table.concat(out, '\\n')"
    )
}

#[test]
fn closures_keep_their_variables_as_puc_s_do() {
    let src = source();
    for v in ALL {
        let mut vm = Vm::new_minimal_with_hash_seed(v, 1);
        vm.open_all_libs().expect("the libraries open");
        let got = match vm.eval(&src).expect("runs").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        };
        assert_eq!(got, WANT, "{v:?}");
    }
}

#[test]
fn closures_keep_their_variables_with_allocations_refused() {
    let src = source();
    for v in ALL {
        for n in (1..6000).step_by(7) {
            let got = run(v, 1, n, &src);
            assert!(
                got == WANT || got.starts_with("error:"),
                "{v:?} refusing from {n}: {got}"
            );
        }
    }
}

#[test]
fn closures_keep_their_variables_with_one_allocation_refused() {
    // each refusal starts an emergency collection in the middle of an
    // operation, a closure's upvalues gathered before it is made among
    // them, and the allocations after it succeed
    let src = source();
    for v in ALL {
        for n in 1..3000 {
            let got = run_refusing(v, 1, n..n + 1, &src);
            assert!(
                got == WANT || got.starts_with("error:"),
                "{v:?} refusing {n}: {got}"
            );
        }
    }
}

struct Count;

impl MemoryPolicy for Count {
    fn allow(&mut self, _: usize, _: usize, _: BlockKind, _: usize) -> bool {
        true
    }
}

#[test]
fn open_upvalues_left_at_the_end_are_freed_with_the_state() {
    // suspended threads whose variables closures still hold, and the main
    // thread's own, open when the state goes
    let src = "local hold = {}
         for i = 1, 3 do
           local co = coroutine.create(function()
             local v = i
             hold[#hold + 1] = function() return v end
             coroutine.yield()
           end)
           coroutine.resume(co)
           hold[#hold + 1] = co
         end
         local w = 0
         hold[#hold + 1] = function() return w end
         return #hold";
    for v in ALL {
        let owner = MemOwner::policy(Box::new(Count));
        let mut vm = Vm::try_new_minimal_with_mem_and_seed(v, owner.clone(), 1).expect("a Vm");
        vm.open_all_libs().expect("the libraries open");
        vm.eval(src).expect("runs");
        drop(vm);
        assert_eq!(owner.ctx().in_use(), Some(0), "{v:?}");
    }
}
