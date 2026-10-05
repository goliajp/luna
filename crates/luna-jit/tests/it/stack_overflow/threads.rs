//! The cases on embedder threads with small stacks.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

use super::{CASES, PRELUDE, case};

const STACKS: [usize; 2] = [256 << 10, 2 << 20];

#[derive(Clone, Copy, Debug)]
enum Mode {
    Interpreter,
    MethodJit,
    TraceJit,
    #[cfg(feature = "llvm-jit")]
    Llvm,
}

const MODES: &[Mode] = &[
    Mode::Interpreter,
    Mode::MethodJit,
    Mode::TraceJit,
    #[cfg(feature = "llvm-jit")]
    Mode::Llvm,
];

fn version(v: u8) -> LuaVersion {
    match v {
        51 => LuaVersion::Lua51,
        52 => LuaVersion::Lua52,
        53 => LuaVersion::Lua53,
        54 => LuaVersion::Lua54,
        _ => LuaVersion::Lua55,
    }
}

fn run(script: &'static str, v: u8, mode: Mode, stack: usize) -> String {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let mut vm = luna_jit::new_with_jit(version(v));
            match mode {
                Mode::Interpreter => vm.install_null_jit(),
                Mode::MethodJit => {}
                Mode::TraceJit => vm.set_jit_enabled(false),
                #[cfg(feature = "llvm-jit")]
                Mode::Llvm => luna_jit::install_llvm_backend(&mut vm),
            }
            match vm.eval(&format!("{PRELUDE}{script}")) {
                Ok(vals) => match vals.first() {
                    Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                    other => format!("not a string: {other:?}"),
                },
                Err(e) => format!("error: {}", vm.error_text(&e)),
            }
        })
        .expect("spawn")
        .join()
        .unwrap_or_else(|_| "panicked".to_string())
}

fn check(name: &str) {
    let c = case(name);
    let mut failures = Vec::new();
    for v in c.dialects.0..=c.dialects.1 {
        for &mode in MODES {
            for stack in STACKS {
                let got = run(c.script, v, mode, stack);
                let want = (c.expect)(v);
                if got != want {
                    failures.push(format!(
                        "{name} {v} {mode:?} {}K: got {got:?}, want {want:?}",
                        stack >> 10
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_case_has_a_test() {
    let src = include_str!("threads.rs");
    for c in CASES {
        assert!(
            src.contains(&format!("\n    {}\n", c.name)),
            "no test for {}",
            c.name
        );
    }
}

macro_rules! cases {
    ($($name:ident)*) => {
        $(
            #[test]
            fn $name() {
                check(stringify!($name));
            }
        )*
    };
}

cases! {
    lua_recursion
    call_metamethod
    index_metamethod
    newindex_metamethod
    eq_metamethod
    lt_le_metamethods
    arith_concat_unm_metamethods
    len_metamethod
    close_metamethod
    pairs_metamethod
    tostring_metamethod
    sort_comparator
    gsub_replacement
    load_reader
    coroutine_wrap
    coroutine_resume
    pcall_nesting
    xpcall_nesting
    handler_recursion
    parser_nesting
    self_recursion_without_end
    self_recursion_15000_deep
    self_recursion_150000_deep
}
