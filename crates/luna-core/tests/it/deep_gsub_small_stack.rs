//! A host whose thread has the 1 MB stack Windows gives a main thread runs
//! gsub calls nested through their replacement functions as deep as they
//! go: luna stops them with a Lua error. PUC built for Windows overflows
//! its C stack there (each level holds a `luaL_Buffer`, 8 KB from 5.3 on,
//! before the C level limit is reached) and the process dies, so there is
//! no behavior of PUC's to match; an error the program can catch is luna's.
//! On 1 MB luna reaches the C level limit first; on a stack a little
//! larger than the room its native stack check keeps free (96 KB in an
//! optimised build, 512 KB in an unoptimised one) that check stops the
//! nesting before the stack runs out.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = r#"
    local function g() return (string.gsub("x", "x", function() return g() end)) end
    local ok, e = pcall(g)
    return ok, tostring(e)
"#;

#[test]
fn nested_gsub_on_a_1_mb_stack_raises_an_error() {
    let small = if cfg!(debug_assertions) {
        544 << 10
    } else {
        128 << 10
    };
    for stack in [1 << 20, small] {
        nested_gsub_raises(stack);
    }
}

fn nested_gsub_raises(stack: usize) {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let got = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || {
                let mut vm = Vm::new(v);
                let r = vm.eval(SCRIPT).expect("the script runs");
                match (r.first(), r.get(1)) {
                    (Some(Value::Bool(ok)), Some(Value::Str(e))) => {
                        (*ok, String::from_utf8_lossy(e.as_bytes()).into_owned())
                    }
                    other => panic!("{v:?}: got {other:?}"),
                }
            })
            .expect("a thread")
            .join()
            .expect("the thread ends without a panic");
        assert!(
            !got.0,
            "{v:?} on {stack}: the nesting ended without an error"
        );
        assert!(
            got.1.contains("stack overflow"),
            "{v:?} on {stack}: {}",
            got.1
        );
    }
}
