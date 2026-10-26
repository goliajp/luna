//! Chains of resumes reach the C-level limit (PUC's `LUAI_MAXCCALLS`)
//! before they run out of native stack, in every build: each stops at the
//! depth PUC stops at (one more here, run without `lua.c`'s protected
//! call), and the coroutine the limit refused has PUC's status.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::Vm;

/// Through `coroutine.resume`.
const RESUMES: &str = r#"
local R
local e = 0
local function h()
  e = e + 1
  local c = coroutine.create(h)
  local ok, m = coroutine.resume(c)
  if not ok and not R then
    R = { tostring(m), coroutine.status(c) }
  end
end
h()
return R[1], R[2], e
"#;

/// Through the functions `coroutine.wrap` makes, each tail-called.
const WRAPS: &str = r#"
local d = 0
local function w() d = d + 1 return coroutine.wrap(w)() end
local ok, m = pcall(w)
return tostring(m), "", d
"#;

fn text(v: &Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        _ => panic!("a string: {v:?}"),
    }
}

/// `src` run on a thread with the stack a test thread gets by default: the
/// error, the status of the coroutine refused, and the depth reached.
fn run(v: LuaVersion, src: &'static str) -> (String, String, i64) {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let mut vm = Vm::new(v);
            let out = vm.eval(src).expect("the chain runs");
            let depth = match out[2] {
                Value::Int(n) => n,
                // 5.1 and 5.2 count in floats
                Value::Float(n) => n as i64,
                _ => panic!("a depth: {:?}", out[2]),
            };
            (text(&out[0]), text(&out[1]), depth)
        })
        .unwrap()
        .join()
        .unwrap()
}

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

#[test]
fn a_chain_of_resumes_stops_at_the_c_level_limit() {
    for v in ALL {
        let (m, status, depth) = run(v, RESUMES);
        assert!(m.ends_with("C stack overflow"), "{v:?}: {m}");
        let (want_depth, want_status) = match v {
            LuaVersion::Lua51 => (200, "dead"),
            LuaVersion::Lua52 | LuaVersion::Lua53 => (199, "suspended"),
            _ => (199, "dead"),
        };
        assert_eq!(depth, want_depth, "{v:?}");
        assert_eq!(status, want_status, "{v:?}");
    }
}

#[test]
fn a_chain_of_wrapped_calls_stops_at_the_c_level_limit() {
    for v in ALL {
        let (m, _, depth) = run(v, WRAPS);
        assert!(m.ends_with("C stack overflow"), "{v:?}: {m}");
        let want = if v == LuaVersion::Lua51 { 199 } else { 198 };
        assert_eq!(depth, want, "{v:?}");
    }
}
