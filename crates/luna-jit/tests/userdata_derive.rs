//! v1.3 UD3 — `#[derive(LuaUserdata)]` + `#[lua_userdata_methods]`
//! smoke tests. Mirrors the v1.2 hand-impl trait tests in
//! `luna-core/tests/userdata_trait.rs` but uses the derive instead.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};
// The trait + derive macros are re-exported by luna-jit at the same
// `LuaUserdata` path. `lua_userdata_methods` is an attribute macro
// (not a trait), so we import it explicitly.
use luna_jit::LuaUserdata;
use luna_jit::lua_userdata_methods;

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

// ─────────────────────────────────────────────────────────────────────
// 1. Counter — get / incr / __tostring via derive
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata)]
#[lua_type_name = "Counter"]
struct Counter {
    value: i64,
}

#[lua_userdata_methods]
impl Counter {
    #[lua_method("get")]
    fn lua_get(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
        Ok(self.value)
    }

    #[lua_method_mut("incr")]
    fn lua_incr(&mut self, _vm: &mut Vm, (by,): (i64,)) -> Result<(), LuaError> {
        self.value += by;
        Ok(())
    }

    #[lua_meta_method(ToString)]
    fn lua_tostring(&self, _vm: &mut Vm, _: ()) -> Result<String, LuaError> {
        Ok(format!("Counter({})", self.value))
    }
}

#[test]
fn derive_counter_get_method() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 7 }).unwrap();
    let r = vm.eval("return c:get()").unwrap();
    assert!(matches!(r[0], Value::Int(7)));
}

#[test]
fn derive_counter_mut_method() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 0 }).unwrap();
    vm.eval("c:incr(10); c:incr(5)").unwrap();
    let r = vm.eval("return c:get()").unwrap();
    assert!(matches!(r[0], Value::Int(15)));
}

#[test]
fn derive_counter_meta_tostring() {
    let mut vm = vm();
    vm.set_userdata("c", Counter { value: 42 }).unwrap();
    let r = vm.eval("return tostring(c)").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "Counter(42)"),
        _ => panic!("expected string"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 2. Vec3 — add_function constructor + add_meta_method(Add)
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata, Copy, Clone)]
#[lua_type_name = "Vec3"]
struct Vec3 {
    x: i64,
    y: i64,
    z: i64,
}

#[lua_userdata_methods]
impl Vec3 {
    #[lua_function("new")]
    fn lua_new(vm: &mut Vm, (x, y, z): (i64, i64, i64)) -> Result<Value, LuaError> {
        Ok(vm.create_userdata(Vec3 { x, y, z }))
    }

    #[lua_meta_method(Add)]
    fn lua_add(&self, vm: &mut Vm, (rhs,): (Value,)) -> Result<Value, LuaError> {
        let rhs = match rhs {
            Value::Userdata(g) => unsafe { &*g.as_ptr() }
                .downcast::<Vec3>()
                .copied()
                .ok_or_else(|| vm.rt_err("__add expected Vec3"))?,
            _ => return Err(vm.rt_err("__add expected Vec3")),
        };
        Ok(vm.create_userdata(Vec3 {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }))
    }

    #[lua_meta_method(ToString)]
    fn lua_tostring(&self, _vm: &mut Vm, _: ()) -> Result<String, LuaError> {
        Ok(format!("Vec3({},{},{})", self.x, self.y, self.z))
    }
}

#[test]
fn derive_vec3_arith_and_constructor() {
    let mut vm = vm();
    let mt = vm.register_userdata::<Vec3>().unwrap();
    vm.set_global("Vec3", Value::Table(mt)).unwrap();
    let r = vm
        .eval("return tostring(Vec3.new(1,2,3) + Vec3.new(10,20,30))")
        .unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "Vec3(11,22,33)"),
        _ => panic!("expected string"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 3. Box2 — field-get / field-set via derive (UD1+UD2 wired through)
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata)]
#[lua_type_name = "Box2"]
struct Box2 {
    width: i64,
    height: i64,
}

#[lua_userdata_methods]
impl Box2 {
    #[lua_field_get("width")]
    fn lua_get_width(&self, _vm: &mut Vm) -> Result<i64, LuaError> {
        Ok(self.width)
    }

    #[lua_field_get("height")]
    fn lua_get_height(&self, _vm: &mut Vm) -> Result<i64, LuaError> {
        Ok(self.height)
    }

    #[lua_field_set("width")]
    fn lua_set_width(&mut self, _vm: &mut Vm, (w,): (i64,)) -> Result<(), LuaError> {
        self.width = w;
        Ok(())
    }

    #[lua_field_set("height")]
    fn lua_set_height(&mut self, _vm: &mut Vm, (h,): (i64,)) -> Result<(), LuaError> {
        self.height = h;
        Ok(())
    }
}

#[test]
fn derive_box2_field_get_set() {
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
    vm.eval("b.width = 100; b.height = 200").unwrap();
    let r = vm.eval("return b.width, b.height").unwrap();
    assert!(matches!(r[0], Value::Int(100)));
    assert!(matches!(r[1], Value::Int(200)));
}

// ─────────────────────────────────────────────────────────────────────
// 4. Skip marker — #[lua_skip] keeps a Rust-only helper
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata)]
#[lua_type_name = "Skipper"]
struct Skipper {
    payload: i64,
}

#[lua_userdata_methods]
impl Skipper {
    #[lua_method("payload")]
    fn lua_payload(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
        Ok(self.rust_helper())
    }

    // Pure-Rust helper — not exposed to Lua.
    #[lua_skip]
    fn rust_helper(&self) -> i64 {
        self.payload * 2
    }
}

#[test]
fn derive_skip_marker_keeps_rust_helper_invisible() {
    let mut vm = vm();
    vm.set_userdata("s", Skipper { payload: 5 }).unwrap();
    // The Lua-visible method calls the Rust helper internally.
    let r = vm.eval("return s:payload()").unwrap();
    assert!(matches!(r[0], Value::Int(10)));
    // `rust_helper` is NOT on the metatable. With our v1.3 trampoline,
    // looking up an unknown field with no getters falls through to
    // the methods table only, and missing keys return nil — but Box2
    // installed getters force the trampoline path; Skipper has none,
    // so it's the v1.2 table-__index fast path. `s.rust_helper` thus
    // returns nil (not an error).
    let r = vm.eval("return s.rust_helper").unwrap();
    assert!(matches!(r[0], Value::Nil));
}

// ─────────────────────────────────────────────────────────────────────
// 5. type_name override — #[lua_type_name = "X"] surfaces as __name
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata)]
#[lua_type_name = "MyCustomName"]
struct RenameMe {
    _v: i64,
}

#[lua_userdata_methods]
impl RenameMe {
    #[lua_method("get")]
    fn lua_get(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
        Ok(self._v)
    }
}

#[test]
fn derive_type_name_override() {
    let mut vm = vm();
    vm.set_userdata("r", RenameMe { _v: 1 }).unwrap();
    let r = vm.eval("return getmetatable(r).__name").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "MyCustomName"),
        _ => panic!("expected string"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 6. Default type_name — no override falls back to struct ident
// ─────────────────────────────────────────────────────────────────────

#[derive(LuaUserdata)]
struct DefaultName {
    _v: i64,
}

#[lua_userdata_methods]
impl DefaultName {
    #[lua_method("get")]
    fn lua_get(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
        Ok(self._v)
    }
}

#[test]
fn derive_default_type_name_is_struct_ident() {
    let mut vm = vm();
    vm.set_userdata("d", DefaultName { _v: 1 }).unwrap();
    let r = vm.eval("return getmetatable(d).__name").unwrap();
    match r[0] {
        Value::Str(s) => assert_eq!(std::str::from_utf8(s.as_bytes()).unwrap(), "DefaultName"),
        _ => panic!("expected string"),
    }
}
