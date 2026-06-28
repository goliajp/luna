//! v2.1 Phase 1K.E.1 — probe Lua-source → bytecode for the chunks the
//! op-by-op port will need. Not a real assertion test; runs only when
//! `LUNA_LLVM_PROBE_CHUNKS=1` so it doesn't slow normal `cargo test`.

use luna_jit::LuaVersion;

#[test]
fn probe_chunks() {
    if std::env::var_os("LUNA_LLVM_PROBE_CHUNKS").is_none() {
        return;
    }
    let chunks: &[(&str, &str)] = &[
        ("return 42", "return_42"),
        ("return 1", "return_1"),
        ("local x = 2; local y = 3; return x + y", "add_xy"),
        ("local x = 7; local y = 5; return x - y", "sub_xy"),
        ("local x = 6; local y = 7; return x * y", "mul_xy"),
        ("local x = 20; local y = 4; return x // y", "idiv_xy"),
        ("local x = 17; local y = 5; return x % y", "mod_xy"),
        (
            "local x = 3; local y = 2; if x < y then return x else return y end",
            "lt_xy",
        ),
        (
            "local x = 5; if x < 10 then return 1 else return 0 end",
            "if_lt",
        ),
        ("return true", "return_true"),
        ("return false", "return_false"),
        ("return nil", "return_nil"),
        ("local x = 8; local y = 3; return x / y", "div_xy"),
        (
            "local n = 5\nlocal r = 0\nif n < 10 then r = n * 2 else r = n - 1 end\nreturn r",
            "fib_shape_branchy",
        ),
        // 1K.F probe — self-recursive function for Op::Call + GetUpval
        // + parametric-chunk emit. Outer chunk has `rec` as local;
        // inner proto has `rec` as upval[0] (5.5/5.4/5.3/5.2) or
        // upval[1] (5.1).
        (
            "local function rec(a, b) if a < b then return a end return rec(b, b) end; return rec(5, 3)",
            "self_rec_two_args",
        ),
        (
            "local function id(n) return n end; return id(42)",
            "id_one_arg",
        ),
        (
            "local function pass(a, b, c) return c end; return pass(1, 2, 3)",
            "three_args_return_last",
        ),
        (
            "local k = 10; local function f() return k end; return f()",
            "upval_read_one",
        ),
        (
            "local k = 10; local function f() return k * k end; return f()",
            "upval_read_two_mul",
        ),
        (
            "local function rec(n) if n < 1 then return n end local r = rec(n) return r end",
            "self_rec_non_tail_via_local",
        ),
    ];
    for (src, name) in chunks {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
        match vm.load(src.as_bytes(), name.as_bytes()) {
            Ok(closure) => {
                println!("=== {} ===", name);
                println!("src: {}", src);
                let proto = closure.proto;
                dump_proto("  ", &proto);
                for (idx, sub) in proto.protos.iter().enumerate() {
                    println!("  --- nested proto[{}] ---", idx);
                    dump_proto("    ", sub);
                }
            }
            Err(e) => {
                println!("=== {} === FAILED: {:?}", name, e);
            }
        }
        println!();
    }
}

fn dump_proto(indent: &str, proto: &luna_core::runtime::function::Proto) {
    println!(
        "{}num_params: {}, max_stack: {}, upvals: {}",
        indent,
        proto.num_params,
        proto.max_stack,
        proto.upvals.len(),
    );
    println!("{}consts: {:?}", indent, proto.consts);
    for (pc, inst) in proto.code.iter().enumerate() {
        println!(
            "{}{:3}: {:?} a={} b={} c={} bx={} sbx={} sj={} k={}",
            indent,
            pc,
            inst.op(),
            inst.a(),
            inst.b(),
            inst.c(),
            inst.bx(),
            inst.sbx(),
            inst.sj(),
            inst.k()
        );
    }
}
