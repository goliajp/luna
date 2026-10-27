//! PUC Lua 5.2 binary chunk loading.
//!
//! Smoke tests for the `dump::puc::puc_52` translator: load real
//! `luac5.2`-emitted bytecode and run it through luna's interpreter.
//!
//! The fixtures are inline byte arrays produced by:
//! ```text
//! lua-5.2.4/src/luac -o XX.luac XX.lua
//! ```
//! For each fixture, the original `.lua` source is recorded in a doc
//! comment so the test intent is clear without rerunning luac.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// `return 1 + 2`  →  PUC's compiler constant-folds to `LOADK K[0]=3; RETURN`.
const SIMPLE_LUAC: &[u8] = &[
    0x1b, 0x4c, 0x75, 0x61, 0x52, 0x00, 0x01, 0x04, 0x08, 0x04, 0x08, 0x00, 0x19, 0x93, 0x0d, 0x0a,
    0x1a, 0x0a, // header (18 bytes)
    0x00, 0x00, 0x00, 0x00, // linedefined
    0x00, 0x00, 0x00, 0x00, // lastlinedefined
    0x00, 0x01, 0x02, // numparams=0, is_vararg=1, maxstacksize=2
    0x03, 0x00, 0x00, 0x00, // sizecode = 3
    0x01, 0x00, 0x00, 0x00, // LOADK R(0) := K[0]
    0x1f, 0x00, 0x00, 0x01, // RETURN R(0) (B=2)
    0x1f, 0x00, 0x80, 0x00, // RETURN (B=1, no values)
    0x01, 0x00, 0x00, 0x00, // sizek = 1
    0x03, // LUA_TNUMBER tag
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x40, // f64 = 3.0
    0x00, 0x00, 0x00, 0x00, // sizep = 0 (no nested protos)
    0x01, 0x00, 0x00, 0x00, // sizeupvalues = 1
    0x01, 0x00, // in_stack=1, idx=0  (the _ENV upval)
    0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // source string size = 12
    0x40, 0x73, 0x69, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x6c, 0x75, 0x61, 0x00, // "@simple.lua\0"
    0x03, 0x00, 0x00, 0x00, // sizelineinfo = 3
    0x01, 0x00, 0x00, 0x00, // line 1
    0x01, 0x00, 0x00, 0x00, // line 1
    0x01, 0x00, 0x00, 0x00, // line 1
    0x00, 0x00, 0x00, 0x00, // sizelocvars = 0
    0x01, 0x00, 0x00, 0x00, // sizeupvalues (debug) = 1
    0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // upval-name size = 5
    0x5f, 0x45, 0x4e, 0x56, 0x00, // "_ENV\0"
];

/// `local x = 10; return x * x`  →  LOADK x=10; MUL r,x,x; RETURN.
const LOCAL_MUL_LUAC: &[u8] = &[
    0x1b, 0x4c, 0x75, 0x61, 0x52, 0x00, 0x01, 0x04, 0x08, 0x04, 0x08, 0x00, 0x19, 0x93, 0x0d, 0x0a,
    0x1a, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x04, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x4f, 0x00, 0x00, 0x00, 0x5f, 0x00, 0x00, 0x01, 0x1f, 0x00, 0x80,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x40, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x40, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x5f, 0x6d, 0x75, 0x6c, 0x2e, 0x6c, 0x75, 0x61, 0x00, 0x04,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x5f, 0x45, 0x4e, 0x56, 0x00,
];

/// `return "hello"`  →  LOADK K[0]="hello"; RETURN R(0).
const STR_LUAC: &[u8] = &[
    0x1b, 0x4c, 0x75, 0x61, 0x52, 0x00, 0x01, 0x04, 0x08, 0x04, 0x08, 0x00, 0x19, 0x93, 0x0d, 0x0a,
    0x1a, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x01, 0x1f, 0x00, 0x80, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x04, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x40, 0x73, 0x74, 0x72, 0x2e, 0x6c, 0x75, 0x61, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5f, 0x45, 0x4e, 0x56, 0x00,
];

/// `chunk`, written by a 64-bit `luac`, with the target's `size_t`: the
/// header's size and each string's length (`lundump.c`'s `LoadFunction`
/// order).
fn native(chunk: &[u8]) -> Vec<u8> {
    fn int(b: &[u8], p: &mut usize) -> usize {
        let v = u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap()) as usize;
        *p += 4;
        v
    }
    fn copy(b: &[u8], p: &mut usize, n: usize, out: &mut Vec<u8>) {
        out.extend_from_slice(&b[*p..*p + n]);
        *p += n;
    }
    fn string(b: &[u8], p: &mut usize, out: &mut Vec<u8>) {
        let n = u64::from_le_bytes(b[*p..*p + 8].try_into().unwrap()) as usize;
        *p += 8;
        out.extend_from_slice(&n.to_le_bytes());
        copy(b, p, n, out);
    }
    fn function(b: &[u8], p: &mut usize, out: &mut Vec<u8>) {
        copy(b, p, 11, out); // lines, parameters, vararg, stack size
        let n = int(b, &mut { *p });
        copy(b, p, 4 + 4 * n, out);
        let n = int(b, &mut { *p });
        copy(b, p, 4, out);
        for _ in 0..n {
            let t = b[*p];
            copy(b, p, 1, out);
            match t {
                1 => copy(b, p, 1, out),
                3 => copy(b, p, 8, out),
                4 => string(b, p, out),
                _ => {}
            }
        }
        let n = int(b, &mut { *p });
        copy(b, p, 4, out);
        for _ in 0..n {
            function(b, p, out);
        }
        let n = int(b, &mut { *p });
        copy(b, p, 4 + 2 * n, out);
        string(b, p, out); // source
        let n = int(b, &mut { *p });
        copy(b, p, 4 + 4 * n, out);
        let n = int(b, &mut { *p });
        copy(b, p, 4, out);
        for _ in 0..n {
            string(b, p, out);
            copy(b, p, 8, out);
        }
        let n = int(b, &mut { *p });
        copy(b, p, 4, out);
        for _ in 0..n {
            string(b, p, out);
        }
    }
    let mut out = chunk[..18].to_vec();
    out[8] = std::mem::size_of::<usize>() as u8;
    let mut p = 18;
    function(chunk, &mut p, &mut out);
    assert_eq!(p, chunk.len(), "the whole chunk");
    out
}

fn fresh_vm() -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_puc_bytecode_loading(true);
    vm
}

fn run_chunk(vm: &mut Vm, bytes: &[u8]) -> Vec<Value> {
    let cl = vm
        .load(&native(bytes), b"=puc52-test")
        .expect("PUC 5.2 chunk load failed");
    vm.call_value(Value::Closure(cl), &[]).expect("call failed")
}

#[test]
fn load_simple_return_1_plus_2() {
    let mut vm = fresh_vm();
    let r = run_chunk(&mut vm, SIMPLE_LUAC);
    // 5.2 has no integer subtype — 1+2 was folded to Float(3.0).
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Float(f) => assert!((f - 3.0).abs() < f64::EPSILON, "got {f}"),
        Value::Int(i) => assert_eq!(i, 3),
        v => panic!("expected 3, got {v:?}"),
    }
}

#[test]
fn load_local_mul() {
    let mut vm = fresh_vm();
    let r = run_chunk(&mut vm, LOCAL_MUL_LUAC);
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Float(f) => assert!((f - 100.0).abs() < f64::EPSILON, "got {f}"),
        Value::Int(i) => assert_eq!(i, 100),
        v => panic!("expected 100, got {v:?}"),
    }
}

#[test]
fn load_string_const() {
    let mut vm = fresh_vm();
    let r = run_chunk(&mut vm, STR_LUAC);
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"hello"),
        v => panic!("expected \"hello\", got {v:?}"),
    }
}

#[test]
fn reject_without_puc_gate() {
    let mut vm = Vm::new(LuaVersion::Lua54); // puc loading default off
    let err = vm
        .load(&native(SIMPLE_LUAC), b"=puc52-test")
        .expect_err("should fail with PUC loading disabled");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("PUC bytecode loading is disabled"),
        "unexpected error: {msg}"
    );
}

#[test]
fn reject_bad_signature() {
    let mut bad = native(SIMPLE_LUAC);
    bad[0] = 0x1c; // mangle the escape byte; loader can't even recognise it
    let mut vm = fresh_vm();
    assert!(vm.load(&bad, b"=puc52-test").is_err());
}

#[test]
fn reject_truncated_header() {
    let mut vm = fresh_vm();
    let err = vm
        .load(&native(SIMPLE_LUAC)[..10], b"=puc52-test")
        .expect_err("should fail on truncated header");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("truncated") || msg.contains("PUC bytecode loading is disabled"),
        "unexpected error: {msg}"
    );
}
