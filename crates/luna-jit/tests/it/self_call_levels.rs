//! Compiled self recursion counts the levels it makes as the interpreter
//! (and PUC) counts its calls: the stack each frame needs, the `CallInfo`s,
//! 5.1's array of them and its limit, and the collection checks 5.2 to 5.4
//! make on the way. The same script run with the method JIT engaged at
//! once and with it off gives the same output.

use luna_core::__private::VmJitState;
use luna_jit::version::LuaVersion;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

const HEADER: &str = r#"
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
"#;

/// `src` with the JIT off and with it engaged on the first call; with
/// the JIT, whether a chunk was compiled. Both runs hash with one seed:
/// where the collector's steps fall depends on the order tables are
/// traversed in, which the seed sets.
fn both(v: LuaVersion, src: &str) -> (String, String, usize) {
    let run = |jit: bool| {
        let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, 0x5eed);
        luna_jit::install_default_jit(&mut vm);
        vm.open_all_libs().expect("the libraries open");
        vm.set_trace_jit_enabled(false);
        if jit {
            vm.jit_mut().call_hot_threshold = 1;
        } else {
            vm.set_jit_enabled(false);
        }
        let out = match vm.eval(src) {
            Ok(vals) => vals
                .iter()
                .map(|x| match x {
                    luna_jit::runtime::Value::Str(s) => {
                        String::from_utf8_lossy(s.as_bytes()).into_owned()
                    }
                    x => format!("{x:?}"),
                })
                .collect::<Vec<_>>()
                .join(" "),
            Err(e) => format!("error: {}", vm.error_text(&e)),
        };
        (out, luna_jit::jit::cache_entry_count(&vm))
    };
    let (interp, _) = run(false);
    let (jit, compiled) = run(true);
    (interp, jit, compiled)
}

fn same(src: &str) {
    for v in ALL {
        let (interp, jit, compiled) = both(v, src);
        assert!(compiled > 0, "{v:?}: nothing compiled");
        assert_eq!(jit, interp, "{v:?}");
    }
}

#[test]
fn one_call_site_at_every_depth() {
    same(&format!(
        "{HEADER}
local function plain(n) if n == 0 then return 0 end return plain(n - 1) + 1 end
local out = {{}}
for _, n in ipairs({{1, 2, 31, 32, 33, 64, 500, 2000}}) do
  plain(n)
  out[#out + 1] = n .. ':' .. count()
end
return table.concat(out, ' ')"
    ));
}

#[test]
fn two_call_sites() {
    same(&format!(
        "{HEADER}
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local out = {{}}
for _, n in ipairs({{5, 20}}) do
  out[#out + 1] = fib(n) .. ':' .. count()
end
return table.concat(out, ' ')"
    ));
}

#[test]
fn past_the_limit() {
    same(&format!(
        "{HEADER}
local function plain(n) if n == 0 then return 0 end return plain(n - 1) + 1 end
local ok, e = pcall(plain, 300000)
return tostring(ok) .. ' ' .. tostring(e) .. ' ' .. count()"
    ));
}

#[test]
fn collections_falling_due_on_the_way() {
    // garbage first, then a recursion deep enough to grow the stack and
    // the `CallInfo`s while the collector runs
    same(
        "local junk = {}
for i = 1, 2000 do junk[i] = {} end
junk = nil
local function plain(n) if n == 0 then return 0 end return plain(n - 1) + 1 end
local out = {}
for _, n in ipairs({100, 1000, 3000}) do
  plain(n)
  out[#out + 1] = string.format('%.0f', collectgarbage('count') * 1024)
end
return table.concat(out, ' ')",
    );
}
