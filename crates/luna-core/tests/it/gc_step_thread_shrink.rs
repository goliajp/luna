//! 5.2 and 5.3 shrink a suspended coroutine's stack while the collector
//! traverses it, which a cycle run one step at a time does part way
//! through. Stepping the whole cycle keeps every object, and leaves what
//! collecting it at once leaves, give or take what PUC's own step loop
//! leaves more.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Coroutines whose stacks grew deep and then went back to a shallow
/// yield, so the collector finds most of each stack unused.
const SETUP: &str = r#"
collectgarbage()
collectgarbage("stop")
local function deep(n)
  if n == 0 then coroutine.yield() return 0 end
  return deep(n - 1) + 1
end
cos, data = {}, {}
for i = 1, 200 do
  local co = coroutine.create(function()
    local t = {i, tostring(i)}
    deep(120)
    coroutine.yield()
    return t[1], t[2]
  end)
  assert(coroutine.resume(co))
  assert(coroutine.resume(co))
  cos[i] = co
  data[i] = {i}
end
for i = 1, 200 do data[i] = nil end
"#;

/// Every coroutine runs to its end with its own values.
const CHECK: &str = r#"
for i = 1, 200 do
  local ok, a, b = coroutine.resume(cos[i])
  assert(ok and a == i and b == tostring(i), i)
  assert(coroutine.status(cos[i]) == "dead")
end
"#;

/// Runs one cycle, one step at a time when `how` is "step", else at once,
/// checking the coroutines between the steps; how many steps it took. One
/// chunk for both, so that the two runs allocate the same.
const CYCLE: &str = r#"
local how, n = ...
n = 0
repeat
  n = n + 1
  for i = 1, 200 do assert(coroutine.status(cos[i]) == "suspended", i) end
until (how == "step" and collectgarbage("step", 0)) or (how == "full" and collectgarbage("collect") and true)
return n
"#;

fn vm(v: LuaVersion) -> Vm {
    let mut vm = Vm::new(v);
    vm.eval(SETUP).expect("the coroutines are made");
    vm
}

fn cycle(vm: &mut Vm, how: &str) -> Vec<Value> {
    vm.eval(&format!(
        "return (load or loadstring)([==[{CYCLE}]==])(\"{how}\")"
    ))
    .expect("the cycle runs")
}

fn count(r: &[Value]) -> f64 {
    match r[0] {
        Value::Float(f) => f,
        Value::Int(i) => i as f64,
        ref other => panic!("a count: {other:?}"),
    }
}

#[test]
fn stepping_a_cycle_that_shrinks_coroutine_stacks_keeps_them_and_frees_as_a_full_collection() {
    for v in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let mut stepped = vm(v);
        let steps = cycle(&mut stepped, "step");
        assert!(
            count(&steps) > 1.0,
            "{v:?}: the cycle took several steps: {steps:?}"
        );
        let stepped_count = stepped
            .eval("return collectgarbage('count')")
            .expect("count");
        stepped.eval(CHECK).expect("the coroutines finish");

        let mut full = vm(v);
        assert_eq!(
            count(&cycle(&mut full, "full")),
            1.0,
            "{v:?}: one full collection"
        );
        let full_count = full.eval("return collectgarbage('count')").expect("count");
        full.eval(CHECK).expect("the coroutines finish");

        // PUC 5.2.4 and 5.3.6 run on the same script end the stepped cycle
        // this many bytes above the full one (x86_64 Linux, three runs each,
        // every one so; -m32 builds for 32-bit)
        let above = match (v, cfg!(target_pointer_width = "64")) {
            (_, true) => 16.0,
            (LuaVersion::Lua52, false) => 8.0,
            (LuaVersion::Lua53, false) => 12.0,
            _ => unreachable!("the test runs 5.2 and 5.3"),
        };
        assert_eq!(
            (count(&stepped_count) - count(&full_count)) * 1024.0,
            above,
            "{v:?}: in use after the stepped cycle against the full one"
        );
    }
}

#[test]
fn coroutines_resumed_between_the_steps_of_a_shrinking_cycle_keep_their_values() {
    for v in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let mut vm = vm(v);
        vm.eval(
            r#"
            local i = 0
            repeat
              i = i + 1
              if i <= 200 then
                local ok, a, b = coroutine.resume(cos[i])
                assert(ok and a == i and b == tostring(i), i)
              end
            until collectgarbage("step", 0)
            -- the ones the cycle did not reach, after it
            for j = i + 1, 200 do
              local ok, a = coroutine.resume(cos[j])
              assert(ok and a == j, j)
            end
            "#,
        )
        .expect("every coroutine ends with its values");
    }
}
