//! The error values the unwinder compares by identity are not roots: a
//! collection that frees one must forget it, or a later error compares
//! against freed memory (and, once the address is reused, can be taken for
//! the error it was).

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

/// Natives that raise, then message handlers that fail (LUA_ERRERR): the
/// last error object stays remembered until the next error is raised.
const ERRORS: &str = r#"
local ok1 = pcall(error, {})
local ok2 = pcall(string.rep)
local ok3 = xpcall(error, function(m) error(m) end)
return ok1, ok2, ok3
"#;

/// A message handler that recurses until the stack overflows: in 5.2 and
/// 5.3, whose Lua calls no call count stops, a million slots of calls,
/// which Miri does not get through in hours.
const OVERFLOWS: &str = r#"
local function h(m) return h(m) .. '' end
return xpcall(error, h)
"#;

fn remembered(vm: &Vm) -> usize {
    usize::from(vm.msgh_applied.is_some())
        + usize::from(vm.handler_bypass.is_some())
        + vm.errored_natives.len()
}

#[test]
fn a_collection_forgets_the_error_values_it_frees() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        let scripts: &[&str] = if cfg!(miri) && matches!(v, LuaVersion::Lua52 | LuaVersion::Lua53) {
            &[ERRORS]
        } else {
            &[OVERFLOWS, ERRORS]
        };
        for src in scripts {
            let f = vm.load(src.as_bytes(), b"=errors").expect("load");
            vm.call_value(Value::Closure(f), &[]).expect("run");
        }
        let before = remembered(&vm);
        vm.collect_garbage();
        assert_eq!(
            remembered(&vm),
            0,
            "{v:?}: {before} remembered before the collection"
        );
    }
}
