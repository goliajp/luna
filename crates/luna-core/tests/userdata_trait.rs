//! v1.2 Track B — `LuaUserdata` trait sugar smoke tests.
//!
//! Covers add_method / add_method_mut / add_function / add_meta_method
//! / add_field_method_get plus wrong-self error reporting, type_name
//! exposure, gc fire-order, and __eq routing.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaUserdata, MetaMethod, UserdataMethods, Vm};

// ─────────────────────────────────────────────────────────────────────
// 1. add_method — pass-through read
// ─────────────────────────────────────────────────────────────────────

struct Counter {
    value: i64,
}
impl LuaUserdata for Counter {
    fn type_name() -> &'static str {
        "Counter"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_method("get", |_vm, this, ()| Ok::<_, _>(this.value));
        m.add_method_mut("incr", |_vm, this, (by,): (i64,)| {
            this.value += by;
            Ok::<_, _>(())
        });
    }
}

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

#[test]
fn add_method_returns_value() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 7 }).unwrap();
    let r = vm.eval("return c:get()").unwrap();
    assert!(matches!(r[0], Value::Int(7)));
}

#[test]
fn add_method_mut_mutates() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 0 }).unwrap();
    vm.eval("c:incr(10); c:incr(5)").unwrap();
    let r = vm.eval("return c:get()").unwrap();
    assert!(matches!(r[0], Value::Int(15)));
}

// ─────────────────────────────────────────────────────────────────────
// 2. Vec3 — meta-method (__add, __tostring) + add_function constructor
// ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
struct Vec3 {
    x: i64,
    y: i64,
    z: i64,
}
impl LuaUserdata for Vec3 {
    fn type_name() -> &'static str {
        "Vec3"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_function(
            "new",
            |vm, (x, y, z): (i64, i64, i64)| -> Result<Value, _> {
                Ok(vm.create_userdata(Vec3 { x, y, z }))
            },
        );
        m.add_meta_method(
            MetaMethod::Add,
            |vm, this, (rhs,): (Value,)| -> Result<Value, _> {
                let rhs = match rhs {
                    Value::Userdata(g) => unsafe { &*g.as_ptr() }
                        .downcast::<Vec3>()
                        .copied()
                        .ok_or_else(|| vm.rt_err("__add expected Vec3"))?,
                    _ => return Err(vm.rt_err("__add expected Vec3")),
                };
                Ok(vm.create_userdata(Vec3 {
                    x: this.x + rhs.x,
                    y: this.y + rhs.y,
                    z: this.z + rhs.z,
                }))
            },
        );
        m.add_meta_method(MetaMethod::ToString, |_vm, this, ()| {
            Ok::<_, _>(format!("Vec3({},{},{})", this.x, this.y, this.z))
        });
    }
}

#[test]
fn add_meta_method_arith_and_tostring() {
    let mut vm = vm();
    let mt = vm.register_userdata::<Vec3>().unwrap();
    vm.set_global("Vec3", Value::Table(mt)).unwrap();
    let r = vm
        .eval("return tostring(Vec3.new(1,2,3) + Vec3.new(10,20,30))")
        .unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "Vec3(11,22,33)"),
        _ => panic!("expected string, got {:?}", r[0]),
    }
}

#[test]
fn add_function_constructor() {
    let mut vm = vm();
    let mt = vm.register_userdata::<Vec3>().unwrap();
    vm.set_global("Vec3", Value::Table(mt)).unwrap();
    let r = vm.eval("return tostring(Vec3.new(7,8,9))").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "Vec3(7,8,9)"),
        _ => panic!("expected string"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 3. wrong-self type error
// ─────────────────────────────────────────────────────────────────────

#[test]
fn wrong_self_type_errors_cleanly() {
    // Construct a Counter, then call its `get` method on a Vec3 userdata.
    let mut vm = vm();
    let _ = vm.register_userdata::<Counter>().unwrap();
    let vec3_mt = vm.register_userdata::<Vec3>().unwrap();
    vm.set_global("Vec3", Value::Table(vec3_mt)).unwrap();
    vm.set_userdata("c", Counter { value: 1 }).unwrap();
    // Grab Counter's `get` and apply it to a Vec3 instance (manually,
    // since Lua doesn't normally let you cross-call methods like this).
    let err = vm
        .eval(
            r#"
            local get = getmetatable(c).__index.get
            local v = Vec3.new(0, 0, 0)
            return get(v)
        "#,
        )
        .unwrap_err();
    let msg = match err.0 {
        Value::Str(s) => std::str::from_utf8(s.as_bytes()).unwrap().to_string(),
        other => format!("{:?}", other),
    };
    assert!(
        msg.contains("Counter"),
        "expected type-name in error, got: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// 4. type_name appears via getmetatable().__name
// ─────────────────────────────────────────────────────────────────────

#[test]
fn type_name_in_metatable_name_field() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 0 }).unwrap();
    let r = vm.eval("return getmetatable(c).__name").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "Counter"),
        _ => panic!("expected string"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 5. add_field_method_get sugar (call-syntax in v1.2)
// ─────────────────────────────────────────────────────────────────────

struct Box2 {
    width: i64,
    height: i64,
}
impl LuaUserdata for Box2 {
    fn type_name() -> &'static str {
        "Box2"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_field_method_get("width", |_vm, this| Ok::<_, _>(this.width));
        m.add_field_method_get("height", |_vm, this| Ok::<_, _>(this.height));
    }
}

#[test]
fn add_field_method_get_call_syntax() {
    // v1.3 UD2 semantic update: `add_field_method_get` now dispatches
    // through a function-`__index` trampoline that calls the getter
    // and returns its value. The v1.2 `b:width()` shape no longer
    // resolves to the field value (calling a getter via `:` would
    // evaluate to `Int(16)(b)`, which errors). The new shape is the
    // natural `b.width`. Embedders who need `b:width()` should
    // register an explicit `add_method("width", ...)` instead.
    let mut vm = vm();
    vm.set_userdata(
        "b",
        Box2 {
            width: 16,
            height: 9,
        },
    )
    .unwrap();
    let r = vm.eval("return b.width, b.height").unwrap();
    assert!(matches!(r[0], Value::Int(16)));
    assert!(matches!(r[1], Value::Int(9)));
}

// ─────────────────────────────────────────────────────────────────────
// 6. add_meta_method Eq — userdata identity vs custom equality
// ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
struct Bag {
    tag: i64,
}
impl LuaUserdata for Bag {
    fn type_name() -> &'static str {
        "Bag"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Eq, |_vm, this, (rhs,): (Value,)| {
            let rhs_tag = match rhs {
                Value::Userdata(g) => unsafe { &*g.as_ptr() }.downcast::<Bag>().map(|b| b.tag),
                _ => None,
            };
            Ok::<_, _>(rhs_tag == Some(this.tag))
        });
    }
}

#[test]
fn add_meta_method_eq_routes_via_dispatcher() {
    let mut vm = vm();
    vm.set_userdata("a", Bag { tag: 7 }).unwrap();
    vm.set_userdata("b", Bag { tag: 7 }).unwrap();
    vm.set_userdata("c", Bag { tag: 9 }).unwrap();
    let r = vm.eval("return a == b, a == c").unwrap();
    assert!(matches!(r[0], Value::Bool(true)));
    assert!(matches!(r[1], Value::Bool(false)));
}

// ─────────────────────────────────────────────────────────────────────
// 7. __gc fires — Drop on the boxed payload runs during sweep
// ─────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};

static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

struct Tracked {
    _tag: u32,
}
impl Drop for Tracked {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}
impl LuaUserdata for Tracked {}

#[test]
fn gc_drops_host_payload() {
    let before = DROP_COUNT.load(Ordering::SeqCst);
    {
        let mut vm = vm();
        // Allocate but never globalize → eligible for collection once
        // the temporary `Value::Userdata` goes out of scope.
        for _ in 0..3 {
            let _ = vm.create_userdata(Tracked { _tag: 1 });
        }
        vm.collect_garbage();
        // Vm drop also runs all finalizers.
    }
    let after = DROP_COUNT.load(Ordering::SeqCst);
    assert!(
        after > before,
        "expected at least one Tracked drop; before={before} after={after}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// 8. metatable is cached — repeated set_userdata reuses one Gc<Table>
// ─────────────────────────────────────────────────────────────────────

#[test]
fn metatable_cached_per_typeid() {
    let mut vm = vm();
    let mt1 = vm.register_userdata::<Counter>().unwrap();
    let mt2 = vm.register_userdata::<Counter>().unwrap();
    assert_eq!(
        mt1.as_ptr(),
        mt2.as_ptr(),
        "metatable cache returned different Gc<Table> for the same TypeId"
    );
    vm.set_userdata("a", Counter { value: 1 }).unwrap();
    vm.set_userdata("b", Counter { value: 2 }).unwrap();
    let r = vm
        .eval("return getmetatable(a) == getmetatable(b)")
        .unwrap();
    assert!(matches!(r[0], Value::Bool(true)));
}

// ─────────────────────────────────────────────────────────────────────
// 9. v1.3 UD2 — true field-style `obj.x` (no parens)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn field_style_no_parens() {
    // Same Box2 type with getters for width/height — v1.3 unlocks
    // `obj.width` syntax via the `__index` trampoline. v1.2's
    // `obj:width()` call-syntax test (above) continues to pass via
    // the dual-registration into the methods bucket.
    let mut vm = vm();
    vm.set_userdata(
        "b",
        Box2 {
            width: 16,
            height: 9,
        },
    )
    .unwrap();
    let r = vm.eval("return b.width, b.height").unwrap();
    assert!(matches!(r[0], Value::Int(16)));
    assert!(matches!(r[1], Value::Int(9)));
    // Unknown field returns nil (matches PUC __index semantics for
    // missing keys; not an error).
    let r = vm.eval("return b.nonexistent").unwrap();
    assert!(matches!(r[0], Value::Nil));
}

// ─────────────────────────────────────────────────────────────────────
// 10. v1.3 UD1 — add_field_method_set `obj.x = v`
// ─────────────────────────────────────────────────────────────────────

struct Box2Mut {
    width: i64,
    height: i64,
}
impl LuaUserdata for Box2Mut {
    fn type_name() -> &'static str {
        "Box2Mut"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_field_method_get("width", |_vm, this| Ok::<_, _>(this.width));
        m.add_field_method_get("height", |_vm, this| Ok::<_, _>(this.height));
        m.add_field_method_set("width", |_vm, this, (w,): (i64,)| {
            this.width = w;
            Ok(())
        });
        m.add_field_method_set("height", |_vm, this, (h,): (i64,)| {
            this.height = h;
            Ok(())
        });
    }
}

#[test]
fn field_style_set() {
    let mut vm = vm();
    vm.set_userdata(
        "b",
        Box2Mut {
            width: 16,
            height: 9,
        },
    )
    .unwrap();
    vm.eval("b.width = 100; b.height = 200").unwrap();
    let r = vm.eval("return b.width, b.height").unwrap();
    assert!(matches!(r[0], Value::Int(100)));
    assert!(matches!(r[1], Value::Int(200)));
    // Unknown field errors out via newindex_trampoline.
    let err = vm.eval("b.nonexistent = 1").unwrap_err();
    let msg = match err.0 {
        Value::Str(s) => std::str::from_utf8(s.as_bytes()).unwrap().to_string(),
        other => format!("{:?}", other),
    };
    assert!(
        msg.contains("nonexistent") && msg.contains("Box2Mut"),
        "expected nonexistent + Box2Mut in error, got: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// 11. v1.3 UD2 — methods win over fields on name collision
// ─────────────────────────────────────────────────────────────────────

struct CollisionType {
    _payload: i64,
}
impl LuaUserdata for CollisionType {
    fn type_name() -> &'static str {
        "CollisionType"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        // Register a method "name" returning 42 ...
        m.add_method("name", |_vm, _this, ()| Ok::<_, _>(42i64));
        // ... and a field-getter with the same name returning 99.
        m.add_field_method_get("name", |_vm, _this| Ok::<_, _>(99i64));
    }
}

#[test]
fn methods_win_on_collision() {
    // v1.3 UD2 precedence: in the `__index` trampoline, the methods
    // bucket is consulted first; only when the name is absent there
    // does the trampoline call the field-getter. Same-name collisions
    // therefore go to the method side.
    let mut vm = vm();
    vm.set_userdata("c", CollisionType { _payload: 0 }).unwrap();
    // `c:name()` resolves to the method (returns 42) because methods
    // win over fields in the trampoline.
    let r = vm.eval("return c:name()").unwrap();
    assert!(matches!(r[0], Value::Int(42)));
    // `c.name` (no parens) returns the *method closure* (because the
    // methods bucket is consulted first and returned unchanged), not
    // the field-getter's `99`. Validates documented precedence.
    let r = vm.eval("return type(c.name)").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "function"),
        _ => panic!("expected 'function' for c.name when method wins"),
    }
}
