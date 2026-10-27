//! The fast loop's arms on their slow paths: a run of each dialect calls
//! each metamethod once. An arm that ran its instruction again after its
//! slow path would call it twice. `coverage` (unoptimised builds) also
//! checks that the same run goes through every arm.

use super::super::*;
use crate::runtime::Value;
use crate::version::LuaVersion;

/// Lua that every dialect runs: loads, tables, arithmetic, comparisons,
/// loops, calls and returns, each also on a slow path through a
/// metamethod that counts its calls.
pub(super) const COMMON: &str = r##"
local n = {}
-- an arm that keeps running its instruction again fails instead of hanging
local function count(k) n[k] = (n[k] or 0) + 1 assert(n[k] < 10, k) end
local mt = {}
for _, e in ipairs({"add", "sub", "mul", "div", "mod", "pow", "unm", "concat", "lt", "le", "eq"}) do
  mt["__" .. e] = function(a, b) count(e) if e == "lt" or e == "le" or e == "eq" then return true end return 1 end
end
mt.__len = function() count("len") return 1 end
mt.__index = function(t, k) count("index") return 1 end
mt.__newindex = function(t, k, v) count("newindex") end
mt.__call = function(self, a) count("call") return a end
local o = setmetatable({}, mt)
local p = setmetatable({}, mt)
local a, b, s, t = 1, 2.5, "x", {1, 2, 3, x = 4}
local f = false
local r = {a + b, a - b, a * b, b / a, a % 2, b % a, -a, 2 ^ 2,
  o + 1, o - 1, o * 1, o / 1, o % 1, o ^ 1, -o, #o, #t, #s, s .. "y", o .. "y",
  a < b, a <= b, b > a, a >= 1, a < 3, a <= 3, a > 0, a >= 0, a == b, s == "x", a == 1,
  o < p, o <= p, o == p, not f, t.x, t[1], t[a], o.x, o[a], o[s]}
t.y = 1; t[2] = 5; t[a] = 6; t[s] = 7; o.y = 1; o[a] = 1; o[s] = 1
G = 1; G = G + 1; local g = G
local u = 0
local function up() u = u + 1 return u end
up(); up()
for i = 1, 3 do u = u + i end
for i = 1.0, 3.0 do u = u + i end
for i = 3, 1, -1 do u = u + i end
for k, v in pairs(t) do u = u + 1 end
for i, v in ipairs(t) do u = u + 1 end
while u > 0 do u = u - 10 end
repeat u = u + 1 until u > 3
if a and b or s then u = 1 end
local x = a and b
local y = f or a
local z = (a < b) and 1 or 2
local m = {f = function(self, v) return v end}
u = m:f(1) + o(2) + select("#", 1, 2) + string.len("ab")
local function many(...) local q = {...} return q[1], ... end
many(1, 2, 3)
local function tail(k) if k > 0 then return tail(k - 1) end return k end
tail(3)
local nils
local c1, c2 = nil, nil
local function noret() end
noret()
local co = coroutine.wrap(function(v) local w = coroutine.yield(v + 1) return w end)
co(1); co(2)
local _ = pcall(error, "e")
local _ = {1, 2, 3, n = 1}
local q = 0 q = q + 1 q = q - 1 q = q * 2 q = 2 * q q = q / 2 q = q % 3
q = a ^ 2 + (a < 2.5 and 1 or 0) + (a <= 2.5 and 1 or 0) + (a < 1e10 and 1 or 0)
return n
"##;

/// Lua of 5.3 on: integer division and the bitwise operators.
pub(super) const BITS: &str = r##"
local mt = {}
local n = {}
for _, e in ipairs({"idiv", "band", "bor", "bxor", "shl", "shr", "bnot"}) do
  mt["__" .. e] = function() n[e] = (n[e] or 0) + 1 assert(n[e] < 10, e) return 1 end
end
local o = setmetatable({}, mt)
local a, b = 6, 3
local r = {a // b, a & b, a | b, a ~ b, a << 1, a >> 1, ~a, a // 2.0, a & 3, a | 1, a ~ 1,
  o // 1, o & 1, o | 1, o ~ 1, o << 1, o >> 1, ~o, a >> b, a << b}
return n
"##;

/// 5.1: a `LOADBOOL` that skips (`LTrueSkip`), patched into a dump, and
/// globals past constant 255 (`GETGLOBAL`, `SETGLOBAL`).
pub(super) const ONLY_51: &str = r##"
local function f() local x = false return x end
local b = string.dump(f)
local i = b:find("\2\0\0\0\30\0\0\1", 1, true)
local g = assert(loadstring(b:sub(1, i - 1) .. "\2\64\128\0\30\0\0\1" .. b:sub(i + 8)))
g()
local src = {"local t = {"}
for k = 1, 300 do src[#src + 1] = "'k" .. k .. "'," end
src[#src + 1] = "} G51 = 1 G51 = G51 + 1 return G51"
assert(loadstring(table.concat(src)))()
return {}
"##;

/// 5.2 and 5.3: a write to `_ENV` with a key that is a constant but no
/// string (`SetTabUpK`).
pub(super) const ENV_KEY: &str = r##"
_ENV[2.5] = 1
return {}
"##;

/// 5.5: a named vararg table read in place (`VargIdx`) and a global
/// declared with a value (`ErrNNil`).
pub(super) const ONLY_55: &str = r##"
local function v(...t) return t[1], t.n end
v(1, 2)
global G55 = 1
return {}
"##;

/// Run `src`, and read the table of counts it returns while its VM still
/// holds it: each metamethod the run called, and how often.
pub(super) fn run(v: LuaVersion, src: &str) -> Vec<(String, i64)> {
    let mut vm = Vm::new(v);
    let out = vm.eval(src).expect("the chunk runs");
    let Value::Table(t) = out[0] else {
        panic!("a table of counts")
    };
    let mut out = Vec::new();
    let mut k = Value::Nil;
    while let Some((nk, v)) = t.next(k).expect("a key of the table") {
        let (Value::Str(s), c) = (nk, v) else {
            panic!("a count under a name: {nk:?} = {v:?}")
        };
        let c = match c {
            Value::Int(c) => c,
            // 5.1 and 5.2 count in floats
            Value::Float(c) => c as i64,
            _ => panic!("a count: {c:?}"),
        };
        out.push((String::from_utf8_lossy(s.as_bytes()).into_owned(), c));
        k = nk;
    }
    out.sort();
    out
}

/// Run the corpus in every dialect, checking each metamethod's count.
pub(super) fn run_all() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let c = run(v, COMMON);
        // 5.1 takes no `__len` of a table
        let len = usize::from(v > LuaVersion::Lua51);
        assert_eq!(c.len(), 14 + len, "{v:?}: every metamethod called: {c:?}");
        for (k, n) in &c {
            let want = match k.as_str() {
                "index" => 3,
                "newindex" => 3,
                _ => 1,
            };
            assert_eq!(*n, want, "{v:?}: __{k}");
        }
        match v {
            LuaVersion::Lua51 => drop(run(v, ONLY_51)),
            LuaVersion::Lua52 | LuaVersion::Lua53 => drop(run(v, ENV_KEY)),
            LuaVersion::Lua55 => drop(run(v, ONLY_55)),
            _ => {}
        }
        if v >= LuaVersion::Lua53 {
            let c = run(v, BITS);
            assert_eq!(c.len(), 7, "{v:?}: every metamethod called: {c:?}");
            for (k, n) in c {
                assert_eq!(n, 1, "{v:?}: __{k}");
            }
        }
    }
}

/// `f` on a thread of its own, so that an arm that keeps running its
/// instruction fails the test instead of hanging it (Miri, which runs the
/// corpus some thousand times slower, is given hours).
pub(super) fn in_time<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let run = std::thread::spawn(move || tx.send(f()).unwrap());
    let limit = if cfg!(miri) { 4 * 3600 } else { 60 };
    match rx.recv_timeout(std::time::Duration::from_secs(limit)) {
        Ok(out) => out,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("the corpus never finished"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(run.join().unwrap_err())
        }
    }
}

#[test]
fn each_slow_path_calls_its_metamethod_once() {
    in_time(run_all);
}
