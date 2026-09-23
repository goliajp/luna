//! Protected calls made from the host: `pcall` / `xpcall` reached through
//! `call_value` (a library function calling them, as `string.gsub` does
//! with a replacement function), and `Vm::call_value_with_handler`, PUC's
//! `lua_pcall` with a message handler.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn str_of(v: Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {}", other.type_name()),
    }
}

/// `pcall` called by a library function returns `true, ...` or `false, msg`
/// like any other call of it: the continuation it pushes produces the
/// results, also when it fails to call its function. Expected output from
/// PUC 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1 (identical in all five).
#[test]
fn pcall_called_from_a_library_function() {
    let src = r##"
local out = {}
local function add(...)
  local parts = {}
  for i = 1, select("#", ...) do parts[i] = tostring((select(i, ...))) end
  out[#out + 1] = table.concat(parts, " ")
end
local function f() end
add(string.gsub("x", "x", pcall))
add(string.gsub("x", "x", function() return select("#", pcall(f)) end))
add(select("#", string.gsub("ab", "%w", pcall)))
add(pcall(pcall, f))
add(pcall(pcall, "x"))
return table.concat(out, "|")
"##;
    for v in DIALECTS {
        let mut vm = Vm::new(v);
        let r = vm
            .eval(src)
            .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_text(&e)));
        assert_eq!(
            str_of(r[0]),
            "x 1|1 1|2|true true|true false attempt to call a string value",
            "{v:?}"
        );
    }
}

fn load(vm: &mut Vm, src: &str) -> Value {
    vm.load_buffer(src.as_bytes(), b"=t", None)
        .unwrap_or_else(|e| panic!("load: {}", vm.error_text(&e)))
}

#[test]
fn call_with_handler_returns_the_results() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let f = load(&mut vm, "return ..., 'z'");
    let h = vm.globals().get(Value::Str(vm.heap.intern(b"print")));
    let r = vm
        .call_value_with_handler(f, &[Value::Int(1), Value::Int(2)], h)
        .expect("no error");
    assert_eq!(r.len(), 3);
    assert!(matches!(r[0], Value::Int(1)) && matches!(r[1], Value::Int(2)));
    assert_eq!(str_of(r[2]), "z");
    let none = load(&mut vm, "local x = 1");
    let r = vm.call_value_with_handler(none, &[], h).expect("no error");
    assert!(r.is_empty());
}

/// The handler runs where the error was raised, and the call is one C
/// level below the called function: `debug.traceback` as the handler sees
/// the raising function and ends with that level.
#[test]
fn call_with_handler_runs_the_handler_before_unwinding() {
    let src = "local function inner() error('boom') end\ninner()\n";
    let expect = [
        (
            LuaVersion::Lua51,
            "t:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\t\
             t:1: in function 'inner'\n\tt:2: in main chunk\n\t[C]: ?",
        ),
        (
            LuaVersion::Lua54,
            "t:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\t\
             t:1: in local 'inner'\n\tt:2: in main chunk\n\t[C]: in ?",
        ),
    ];
    for (v, text) in expect {
        let mut vm = Vm::new(v);
        let f = load(&mut vm, src);
        let debug = vm.globals().get(Value::Str(vm.heap.intern(b"debug")));
        let Value::Table(debug) = debug else {
            panic!("no debug library")
        };
        let tb = debug.get(Value::Str(vm.heap.intern(b"traceback")));
        let err = vm.call_value_with_handler(f, &[], tb).unwrap_err();
        assert_eq!(str_of(err.0), text, "{v:?}");
    }
}

/// `Vm::traceback` outside any call has no levels to list.
#[test]
fn traceback_with_no_stack() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    assert_eq!(vm.traceback(Some(b"m"), 1), b"m\nstack traceback:");
    assert_eq!(vm.traceback(None, 1), b"stack traceback:");
}

#[test]
fn load_errors_are_the_lauxlib_messages() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let e = vm
        .load_buffer(b"x = = 1", b"=(command line)", None)
        .unwrap_err();
    assert_eq!(str_of(e.0), "(command line):1: unexpected symbol near '='");
    let e = vm.load_buffer(b"x = 1", b"=t", Some(b"b")).unwrap_err();
    assert_eq!(str_of(e.0), "attempt to load a text chunk (mode is 'b')");
    let e = vm.load_file(Some(b"no/such/file.lua"), None).unwrap_err();
    assert!(
        str_of(e.0).starts_with("cannot open no/such/file.lua: "),
        "{}",
        str_of(e.0)
    );
}

#[test]
fn metafield_reads_the_metatable_raw() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let r = vm
        .eval(
            "local mt = setmetatable({}, {__index = function() return 'via index' end})\n\
             mt.__name = 'N'\n\
             return setmetatable({}, mt), {}",
        )
        .expect("eval");
    assert_eq!(str_of(vm.metafield(r[0], "__name")), "N");
    assert!(vm.metafield(r[0], "__tostring").is_nil());
    assert!(vm.metafield(r[1], "__name").is_nil());
    assert!(vm.metafield(Value::Int(1), "__name").is_nil());
}

/// 5.4 on raise the parser's "C stack overflow" through `luaG_errormsg`,
/// inside a protected parser that keeps the running message handler: the
/// handler of an enclosing xpcall rewrites the message `load` returns, and
/// under pcall nothing does. Expected values from PUC 5.4.9 and 5.5.1; 5.2
/// and 5.3 report a positioned syntax error instead.
#[test]
fn deep_load_runs_the_message_handler() {
    let src = r#"
local src = string.rep("(", 300) .. "1" .. string.rep(")", 300)
local _, handled = xpcall(function() local f, e = load(src, "=c"); return e end,
                          function(m) return "H:" .. m end)
local _, _, plain = pcall(load, src, "=c")
return handled, select(2, load(src, "=c")), plain
"#;
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let r = vm.eval(src).expect("eval");
        assert_eq!(str_of(r[0]), "H:C stack overflow", "{v:?}");
        assert_eq!(str_of(r[1]), "C stack overflow", "{v:?}");
        assert_eq!(str_of(r[2]), "C stack overflow", "{v:?}");
    }
}
