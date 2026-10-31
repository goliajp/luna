//! The library functions are tagged when created, and the call path and
//! compiled code find them by the tag alone.

use super::NativeKind;
use crate::runtime::{Builtin, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

fn field(vm: &mut Vm, path: &[&str]) -> Value {
    let mut v = Value::Table(vm.globals());
    for name in path {
        let Value::Table(t) = v else {
            panic!("{path:?}: not a table");
        };
        v = t.get(Value::Str(vm.heap.intern(name.as_bytes())));
    }
    v
}

fn tag(v: Value) -> (Builtin, NativeKind) {
    match v {
        Value::Native(nc) => (nc.builtin, nc.kind),
        other => panic!("not a native: {other:?}"),
    }
}

#[test]
fn library_functions_carry_their_tag() {
    for v in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let cases: [(&[&str], Builtin, NativeKind); 8] = [
            (&["pcall"], Builtin::Pcall, NativeKind::Pcall),
            (&["xpcall"], Builtin::Xpcall, NativeKind::Xpcall),
            (&["pairs"], Builtin::Pairs, NativeKind::Pairs),
            (&["error"], Builtin::Error, NativeKind::Plain),
            (&["tostring"], Builtin::Tostring, NativeKind::Plain),
            (&["math", "floor"], Builtin::MathFloor, NativeKind::Plain),
            (&["string", "sub"], Builtin::StringSub, NativeKind::Plain),
            (&["print"], Builtin::None, NativeKind::Plain),
        ];
        for (path, b, k) in cases {
            let got = tag(field(&mut vm, path));
            assert!(got == (b, k), "{v:?} {path:?}: {got:?}");
        }
        let src = b"return (ipairs({}))";
        let f = vm.load(src, b"=t").unwrap();
        let it = vm.call_value(Value::Closure(f), &[]).unwrap()[0];
        assert_eq!(tag(it).0, Builtin::IpairsIter, "{v:?}");
    }
}

#[test]
fn a_host_native_is_never_a_library_function() {
    // the very function behind `pcall`, registered by a host: plain
    let mut vm = Vm::new(LuaVersion::Lua54);
    let v = vm.native(crate::vm::builtins::nat_pcall);
    assert!(tag(v) == (Builtin::None, NativeKind::Plain));
}
