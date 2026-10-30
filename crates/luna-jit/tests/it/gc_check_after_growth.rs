//! 5.2 checks for a collection at every call of a Lua function. A trace
//! that inlines two calls with a table store between them checks again at
//! the second: the store may grow the table, and the step it makes due
//! falls there, as in the interpreter (and PUC). The loop's trace is made
//! first; finalizers then note which side of the store the steps ran on.

use std::process::Command;

const SCRIPT: &str = r#"
local st = {0}
local log = {}
local mt = {__gc = function() log[#log + 1] = st[1] end}
local function f() end
local t = {}
local function run(from, to)
  for i = from, to do
    st[1] = i
    f()
    t[-i] = i
    st[1] = -i
    f()
  end
end
run(1, 2000)
for _ = 1, 200 do setmetatable({}, mt) end
run(2001, 300000)
local after = 0
for _, s in ipairs(log) do if s < 0 then after = after + 1 end end
print(#log, after, table.concat(log, " "))
"#;

fn run(jit: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
    // 5.2 seeds its string hashes: one seed for both runs
    cmd.env("LUNA_HASH_SEED", "1").arg("--lua=5.2");
    if !jit {
        cmd.arg("--no-jit");
    }
    let out = cmd.args(["-e", SCRIPT]).output().expect("run luna");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

#[test]
#[cfg_attr(
    feature = "gc-stress",
    ignore = "collecting at every check finalizes before the loop"
)]
fn second_inlined_call_checks_after_a_store() {
    let plain = run(false);
    let fields: Vec<&str> = plain.split_whitespace().collect();
    // every finalizer ran, each in a step after the store
    assert_eq!(fields[..2], ["200", "200"], "{plain}");
    assert_eq!(run(true), plain);
}
