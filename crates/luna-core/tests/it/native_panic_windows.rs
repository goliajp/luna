//! A panic out of a native's call or return hook is the hook's, not the
//! native's: it reaches the host as a panic, as it did when every native
//! call had a guard of its own, also for a native the interpreter loop
//! calls without one (`for i = 1, n do f() end`) and for one that arms the
//! hook itself.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::exec::{HOOK_MASK_CALL, HOOK_MASK_RETURN, RustHookEvent};
use luna_core::vm::{LuaError, Vm};

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn hook_boom(_vm: &mut Vm, ev: RustHookEvent) {
    if matches!(ev, RustHookEvent::Return | RustHookEvent::Call) {
        panic!("hook boom");
    }
}

fn plain(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(1)]))
}

fn arm(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    vm.set_rust_debug_hook(Some(hook_boom), HOOK_MASK_RETURN, 0);
    Ok(vm.nat_return(fs, &[]))
}

/// The panic message of running `src` on a fresh Vm with the natives as
/// globals and `hook` set first, or what it returned instead.
fn outcome(v: LuaVersion, hook: u32, src: &str) -> String {
    let mut vm = Vm::new(v);
    let f = vm.native(plain);
    let a = vm.native(arm);
    vm.set_global("f", f).expect("set f");
    vm.set_global("arm", a).expect("set arm");
    if hook != 0 {
        vm.set_rust_debug_hook(Some(hook_boom), hook, 0);
    }
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| vm.eval(src)));
    match r {
        Err(p) => format!(
            "panic {}",
            p.downcast_ref::<&str>().copied().unwrap_or("<other>")
        ),
        Ok(Ok(_)) => "returned".to_string(),
        Ok(Err(e)) => format!("error {}", vm.error_text(&e)),
    }
}

#[test]
fn a_native_return_hook_panic_reaches_the_host() {
    for v in DIALECTS {
        let got = outcome(v, HOOK_MASK_RETURN, "for i = 1, 3 do f() end");
        assert_eq!(got, "panic hook boom", "{v:?}");
    }
}

#[test]
fn a_native_call_hook_panic_reaches_the_host() {
    for v in DIALECTS {
        let got = outcome(v, HOOK_MASK_CALL, "for i = 1, 3 do f() end");
        assert_eq!(got, "panic hook boom", "{v:?}");
    }
}

#[test]
fn a_return_hook_a_loop_native_arms_panics_to_the_host() {
    for v in DIALECTS {
        let got = outcome(v, 0, "for i = 1, 3 do arm() end");
        assert_eq!(got, "panic hook boom", "{v:?}");
    }
}

#[test]
fn pcall_does_not_catch_a_hook_panic() {
    for v in DIALECTS {
        let got = outcome(
            v,
            0,
            "local ok = pcall(function() for i = 1, 3 do arm() end end) return ok",
        );
        assert_eq!(got, "panic hook boom", "{v:?}");
    }
}
