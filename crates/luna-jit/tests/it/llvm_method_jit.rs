//! The LLVM method JIT's self-recursive functions: the recursive calls go
//! straight to the compiled body, and still leave it when a callee deep in
//! the recursion cannot go on (a zero divisor), or when the function they
//! go through is no longer the running one.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

/// `src`'s result, as text, with LLVM's code at once, and with LLVM's code
/// after Cranelift's (the default, and with no wait), which must agree.
/// The text is made before the Vm, which owns the result's string, goes.
fn eval(src: &str) -> String {
    let run = |llvm_after| {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        luna_jit::install_llvm_backend_with(
            &mut vm,
            luna_jit::jit_backend::LlvmBackend { llvm_after },
        );
        let r = vm.eval(src).unwrap_or_else(|e| panic!("{e}"));
        text(&r)
    };
    let now = run(None);
    assert_eq!(run(Some(luna_jit::jit_backend::LLVM_AFTER)), now);
    assert_eq!(run(Some(std::time::Duration::ZERO)), now);
    now
}

fn text(r: &[Value]) -> String {
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_zero_divisor_deep_in_the_recursion_raises() {
    let r = eval(
        "local function f(n, d) if n < 1 then return 0 end return f(n - 1, d) + n % d end
         local s = 0
         for _ = 1, 300 do s = s + f(40, 7) end
         local ok, e = pcall(f, 40, 0)
         return s .. ' ' .. tostring(ok) .. ' ' .. tostring(e):gsub('^.*: ', '') .. ' ' .. f(40, 7)",
    );
    assert_eq!(r, "36000 false attempt to perform 'n%0' 120");
}

#[test]
fn recursion_through_a_reassigned_upvalue_follows_it() {
    let r = eval(
        "local f
         f = function(n) if n < 1 then return 0 end return f(n - 1) + 1 end
         local s = 0
         for _ = 1, 300 do s = s + f(30) end
         local g = f
         f = function(n) return 1000 end
         return s .. ' ' .. g(30)",
    );
    assert_eq!(r, "9000 1001");
}

/// 5.1's code generator leaves out the `LoadNil` of a register still nil
/// at entry: `return nil` reads a register nothing wrote, which the method
/// JIT must not take for the integer 0 (`load` got 0 from its reader).
#[test]
fn a_register_nothing_wrote_is_nil() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua51);
    luna_jit::install_llvm_backend(&mut vm);
    let r = vm
        .eval(
            "local function f() return nil end
             local s = ''
             for _ = 1, 200 do s = tostring(f()) end
             local g = load(function() return nil end)
             return s .. ' ' .. type(g)",
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(text(&r), "nil function");
}

/// LLVM's code for a function, compiled on the compile thread, replaces
/// Cranelift's as the function's own entry at the next call, so calls
/// reach it with nothing in between.
#[test]
fn background_code_becomes_the_entry() {
    use luna_jit::runtime::function::JitProtoState;
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend_with(
        &mut vm,
        luna_jit::jit_backend::LlvmBackend {
            llvm_after: Some(std::time::Duration::ZERO),
        },
    );
    let cl = vm
        .load(
            b"local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
              return fib, function() return fib(15) end",
            b"=t",
        )
        .expect("compile");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let (Value::Closure(fib), Value::Closure(run)) = (r[0], r[1]) else {
        panic!("{r:?}")
    };
    let entry = |vm: &mut luna_jit::vm::Vm| {
        let r = vm.call_value(Value::Closure(run), &[]).expect("run");
        assert!(matches!(r[0], Value::Int(610)), "{r:?}");
        match fib.proto.jit.get() {
            JitProtoState::Compiled { entry, .. } => entry,
            s => panic!("{s:?}"),
        }
    };
    let first = entry(&mut vm);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while entry(&mut vm) == first {
        assert!(
            std::time::Instant::now() < deadline,
            "LLVM's code never arrived"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let taken = fib.proto.jit_next.take();
    assert!(taken.is_none(), "the cell stays after its code was taken");
}

/// A function LLVM's method JIT turns down on the compile thread (5.1's
/// `return nil` from a register nothing wrote) stops being looked at for
/// LLVM's code and keeps Cranelift's.
#[test]
fn background_code_turned_down_keeps_the_entry() {
    use luna_jit::runtime::function::JitProtoState;
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua51);
    luna_jit::install_llvm_backend_with(
        &mut vm,
        luna_jit::jit_backend::LlvmBackend {
            llvm_after: Some(std::time::Duration::ZERO),
        },
    );
    let cl = vm
        .load(
            b"local function f() return nil end
              return f, function() return tostring(f()) end",
            b"=t",
        )
        .expect("compile");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let (Value::Closure(f), Value::Closure(run)) = (r[0], r[1]) else {
        panic!("{r:?}")
    };
    let call = |vm: &mut luna_jit::vm::Vm| {
        let r = vm.call_value(Value::Closure(run), &[]).expect("run");
        assert_eq!(text(&r), "nil");
        match f.proto.jit.get() {
            JitProtoState::Compiled { entry, .. } => entry,
            s => panic!("{s:?}"),
        }
    };
    let first = call(&mut vm);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        assert_eq!(call(&mut vm), first);
        let pending = f.proto.jit_next.take();
        if pending.is_none() {
            break;
        }
        f.proto.jit_next.set(pending);
        assert!(
            std::time::Instant::now() < deadline,
            "the compile thread never answered"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
