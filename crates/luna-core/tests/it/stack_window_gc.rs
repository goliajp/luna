//! The collector's stack contract: the running stack's slots from `gc_top`
//! up are cleared when a cycle's marking ends, so a stale value a returned
//! frame left above its caller's registers is either kept alive or wiped,
//! never left pointing at a freed object. Without that, the stacks marked
//! whole (the main thread's while a coroutine runs, a suspended
//! coroutine's) reach freed objects; ASan reports a use-after-free.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run_all(src: &str) {
    for v in ALL {
        let mut vm = Vm::new(v);
        if let Err(e) = vm.eval(src) {
            panic!("{v:?}: {}", vm.error_text(&e));
        }
    }
}

// `fill` makes its closures after its last call and returns nothing, so
// they stay in the slots above the caller's registers, where the collect
// that follows does not root them (tables would not show a dangling
// pointer: freed ones are pooled, not released)
const FILL: &str = r#"
    local weak = setmetatable({}, {__mode = "k"})
    local function fill(n)
        local s = tostring(n)
        local a1 = function() return s end
        local a2 = function() return n end
        local a3 = function() return a1 end
        local a4 = function() return a2 end
        local a5 = function() return a3 end
        weak[a5] = true
        weak[a1] = true
        return
    end
    local function churn()
        local t = {}
        for i = 1, 2000 do t[i] = ("z"):rep(30) .. i end
        return #t
    end
"#;

#[test]
fn main_stack_marked_whole_holds_no_freed_value() {
    run_all(&format!(
        r#"{FILL}
        fill(1)
        collectgarbage()
        assert(next(weak) == nil)
        local co = coroutine.wrap(function()
            collectgarbage()
            churn()
            collectgarbage()
            return 1
        end)
        assert(co() == 1)
        collectgarbage()
        assert(churn() == 2000)
        "#
    ));
}

#[test]
fn suspended_coroutine_stack_holds_no_freed_value() {
    run_all(&format!(
        r#"{FILL}
        local co = coroutine.create(function()
            fill(1)
            collectgarbage()
            coroutine.yield(next(weak) == nil)
            fill(2)
            coroutine.yield(true)
            return 2
        end)
        local ok, empty = coroutine.resume(co)
        assert(ok and empty)
        churn()
        collectgarbage()
        churn()
        collectgarbage()
        assert(select(2, coroutine.resume(co)) == true)
        collectgarbage()
        assert(select(2, coroutine.resume(co)) == 2)
        "#
    ));
}

#[test]
fn missing_parameters_and_leading_locals_are_nil_after_a_dirty_frame() {
    run_all(
        r##"
        local function dirty()
            local a, b, c, d, e, f, g, h = 1, 2, 3, 4, 5, 6, 7, 8
            return
        end
        local function params(a, b, c) return a, b, c end
        local function leading() local x, y; return x, y end
        local function vararg(a, b, ...) return b, select("#", ...) end
        dirty()
        local p, q, r = params(1)
        assert(p == 1 and q == nil and r == nil)
        dirty()
        local x, y = leading()
        assert(x == nil and y == nil)
        dirty()
        local vb, vn = vararg(1)
        assert(vb == nil and vn == 0)
        "##,
    );
}

fn collecting_hook(vm: &mut Vm, _ev: luna_core::vm::exec::RustHookEvent) {
    vm.collect_garbage();
}

// `s` is written after the last safe point, above `gc_top`; a collection
// from a Rust hook must still see it (PUC roots the whole running frame
// while a hook runs)
#[test]
fn a_collecting_rust_hook_keeps_registers_above_the_last_safe_point() {
    for v in ALL {
        let mut vm = Vm::new(v);
        vm.eval("T = {}").expect("setup");
        vm.set_rust_debug_hook(
            Some(collecting_hook),
            luna_core::vm::exec::HOOK_MASK_LINE,
            0,
        );
        let r = vm.eval(
            r#"
            local t = T
            t.x = string.rep("a", 50)
            local a, b, c, d = 1, 2, 3, 4
            local s = t.x
            t.x = nil
            local e = 5
            local f = 6
            return #s + a + b + c + d + e + f
            "#,
        );
        match r {
            Ok(vals) => assert!(
                matches!(
                    vals[0],
                    luna_core::runtime::Value::Int(71) | luna_core::runtime::Value::Float(71.0)
                ),
                "{v:?}: {:?}",
                vals[0]
            ),
            Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
        }
    }
}
